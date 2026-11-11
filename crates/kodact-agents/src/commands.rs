//! Custom slash commands: Markdown prompts in `commands/` (user, the
//! project's `.agents/` and `.kodact/`, and plugins).
//!
//! `/name args` expands the file's body:
//! - `$ARGUMENTS` becomes the whole argument string, and `$1`..`$9` the
//!   shell-split arguments. Without either, the arguments are appended.
//! - ``!`command` `` runs the command and puts its output in place. It runs
//!   only when the command's `allowed-tools` frontmatter allows that Bash
//!   command.
//! - `@path` attaches the file's contents (see [`crate::attach`]).
//!
//! A file in a subdirectory is named `dir:name`. A plugin's commands are
//! named `plugin:name`.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use kodact_permissions::{MatchMode, Request, Rule, Subject};
use regex::Regex;

use crate::frontmatter;

#[derive(Debug, Clone, PartialEq)]
pub struct CommandDef {
    pub name: String,
    pub description: String,
    pub argument_hint: Option<String>,
    /// `allowed-tools`: here, which ``!`command` `` expansions may run.
    pub allowed_tools: Vec<String>,
    pub model: Option<String>,
    pub body: String,
    pub path: PathBuf,
    /// `user`, `project`, or the plugin's name.
    pub source: String,
}

fn md_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            md_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "md") {
            out.push(p);
        }
    }
}

fn first_line(body: &str) -> String {
    body.lines()
        .map(|l| l.trim().trim_start_matches('#').trim())
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(100).collect())
        .unwrap_or_default()
}

pub fn parse_command(name: &str, text: &str, path: &Path, source: &str) -> CommandDef {
    let (fm, body) = frontmatter::parse(text);
    CommandDef {
        name: name.to_string(),
        description: fm.text("description").map(str::to_string).unwrap_or_else(|| first_line(&body)),
        argument_hint: fm.text("argument-hint").map(str::to_string),
        allowed_tools: fm.list("allowed-tools").unwrap_or_default(),
        model: fm.text("model").map(str::to_string).filter(|m| !m.is_empty()),
        body,
        path: path.to_path_buf(),
        source: source.to_string(),
    }
}

/// Every command, later sources replacing earlier ones by name: user, project
/// (`.agents/`, then `.kodact/`), then plugins.
pub fn load_commands(
    project: &Path,
    plugins: &[crate::plugins::Plugin],
    warnings: &mut Vec<String>,
) -> Vec<CommandDef> {
    // (folder, source, the plugin's name as a prefix, the plugin's own folder)
    let mut dirs: Vec<(PathBuf, String, Option<String>, Option<PathBuf>)> =
        kodact_config::resource_dirs("commands", project)
            .into_iter()
            .map(|(scope, d)| {
                let s = match scope {
                    kodact_config::Scope::User => "user",
                    kodact_config::Scope::Project => "project",
                };
                (d, s.to_string(), None, None)
            })
            .collect();
    for p in plugins {
        for d in p.component_dirs("commands") {
            dirs.push((d, p.name.clone(), Some(p.name.clone()), Some(p.dir.clone())));
        }
    }
    let mut out: Vec<CommandDef> = vec![];
    for (dir, source, prefix, plugin_dir) in dirs {
        let mut files = vec![];
        md_files(&dir, &mut files);
        for f in files {
            let rel = f.strip_prefix(&dir).unwrap_or(&f).with_extension("");
            let local =
                rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect::<Vec<_>>().join(":");
            let name = match &prefix {
                Some(p) => format!("{p}:{local}"),
                None => local,
            };
            match std::fs::read_to_string(&f) {
                Ok(text) => {
                    out.retain(|c| c.name != name);
                    let mut cmd = parse_command(&name, &text, &f, &source);
                    // A plugin's command names its own folder as a plugin's skill does
                    // (`${KODACT_PLUGIN_ROOT}`, `${CLAUDE_PLUGIN_ROOT}`): it was sent to the
                    // model unfilled. The project folder is filled for every command.
                    let vars = crate::substitute::SkillVars {
                        project_dir: Some(project),
                        plugin: plugin_dir.as_deref().zip(prefix.as_deref()),
                        ..Default::default()
                    };
                    cmd.body = crate::substitute::fill_vars(&cmd.body, &vars);
                    cmd.allowed_tools =
                        cmd.allowed_tools.iter().map(|t| crate::substitute::fill_vars(t, &vars)).collect();
                    out.push(cmd);
                }
                Err(e) => warnings.push(format!("command {}: {e}", f.display())),
            }
        }
    }
    out
}

static BANG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!`([^`\n]+)`").expect("regex"));
static POSITIONAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$([1-9])").expect("regex"));

/// Does `allowed-tools` let a ``!`command` `` expansion run `cmd`?
pub fn bang_allowed(allowed_tools: &[String], cmd: &str, cwd: &Path) -> bool {
    let req = Request::new("Bash", Subject::Command(cmd.to_string()), false);
    allowed_tools.iter().filter_map(|r| Rule::parse(r).ok()).any(|rule| rule.matches(&req, cwd, MatchMode::All))
}

/// The prompt `/name args` sends.
pub async fn expand(
    cmd: &CommandDef,
    args: &str,
    cwd: &Path,
    shell: &kodact_platform::shell::ShellChoice,
) -> Result<String, String> {
    let args = args.trim();
    let mut text = cmd.body.clone();
    let positional = shlex::split(args).unwrap_or_else(|| args.split_whitespace().map(str::to_string).collect());
    let uses_args = text.contains("$ARGUMENTS") || POSITIONAL.is_match(&text);
    text = text.replace("$ARGUMENTS", args);
    text = POSITIONAL
        .replace_all(&text, |c: &regex::Captures| {
            let i: usize = c[1].parse().unwrap_or(1);
            positional.get(i - 1).cloned().unwrap_or_default()
        })
        .into_owned();
    if !uses_args && !args.is_empty() {
        text.push_str(&format!("\n\nARGUMENTS: {args}"));
    }

    // !`command` expansions.
    let mut out = String::new();
    let mut last = 0;
    for m in BANG.captures_iter(&text) {
        let whole = m.get(0).expect("match");
        let command = m[1].trim();
        out.push_str(&text[last..whole.start()]);
        last = whole.end();
        if !bang_allowed(&cmd.allowed_tools, command, cwd) {
            return Err(format!(
                "/{} runs `{command}`, which its allowed-tools frontmatter does not permit (add e.g. \"Bash({})\").",
                cmd.name,
                command.split_whitespace().next().unwrap_or(command)
            ));
        }
        let prepared = match shell {
            Ok(sh) => sh.command(&sh.script(command, None)).map_err(|e| e.to_string()),
            Err(m) => Err(m.to_string()),
        };
        let (std_cmd, _script) = match prepared {
            Ok(c) => c,
            Err(why) => {
                out.push_str(&format!("(could not run: {why})"));
                continue;
            }
        };
        let mut std_cmd = std_cmd;
        kodact_platform::process::no_window(&mut std_cmd);
        let run = tokio::process::Command::from(std_cmd)
            .current_dir(cwd)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output();
        let output = match tokio::time::timeout(Duration::from_secs(60), run).await {
            Ok(Ok(o)) => {
                let mut s = String::from_utf8_lossy(&o.stdout).trim_end().to_string();
                let err = String::from_utf8_lossy(&o.stderr);
                if !err.trim().is_empty() {
                    s.push_str(&format!("\n{}", err.trim_end()));
                }
                s
            }
            Ok(Err(e)) => format!("(could not run: {e})"),
            Err(_) => "(timed out after 60s)".into(),
        };
        out.push_str(&kodact_tools::truncate_middle(&output, 20_000));
    }
    out.push_str(&text[last..]);

    // @file attachments.
    let attached = crate::attach::at_mentions(&out, cwd, &|_| Ok(()));
    if !attached.is_empty() {
        out.push_str("\n\n");
        out.push_str(&crate::attach::render(&attached));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(body: &str, allowed: &[&str]) -> CommandDef {
        CommandDef {
            name: "t".into(),
            description: String::new(),
            argument_hint: None,
            allowed_tools: allowed.iter().map(|s| s.to_string()).collect(),
            model: None,
            body: body.into(),
            path: PathBuf::new(),
            source: "project".into(),
        }
    }

    #[tokio::test]
    async fn expands_arguments_commands_and_files() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("notes.md"), "remember this").unwrap();
        let c = cmd("Fix issue #$1 with priority $2. All: $ARGUMENTS", &[]);
        assert_eq!(
            expand(&c, "123 \"very high\"", d.path(), kodact_platform::shell::detect()).await.unwrap(),
            "Fix issue #123 with priority very high. All: 123 \"very high\""
        );
        let c = cmd("Review the code.", &[]);
        assert_eq!(
            expand(&c, "src/main.rs", d.path(), kodact_platform::shell::detect()).await.unwrap(),
            "Review the code.\n\nARGUMENTS: src/main.rs"
        );
        let c = cmd("Status:\n!`echo clean`\nDone", &["Bash(echo:*)"]);
        assert_eq!(expand(&c, "", d.path(), kodact_platform::shell::detect()).await.unwrap(), "Status:\nclean\nDone");
        let c = cmd("!`rm -rf x`", &["Bash(echo:*)"]);
        assert!(expand(&c, "", d.path(), kodact_platform::shell::detect())
            .await
            .unwrap_err()
            .contains("allowed-tools"));
        let c = cmd("Summarize @notes.md please, not @missing.md", &[]);
        let out = expand(&c, "", d.path(), kodact_platform::shell::detect()).await.unwrap();
        assert!(
            out.ends_with(&format!("<file path=\"{}\">\nremember this\n</file>", d.path().join("notes.md").display())),
            "{out}"
        );
    }

    #[test]
    fn loads_namespaced_commands_with_precedence() {
        let d = tempfile::tempdir().unwrap();
        let proj = d.path();
        for (dir, file, text) in [
            (".agents/commands", "review.md", "---\ndescription: shared review\n---\nReview it."),
            (
                ".kodact/commands",
                "review.md",
                "---\ndescription: kodact review\nargument-hint: [file]\n---\nReview $ARGUMENTS.",
            ),
            (".kodact/commands/frontend", "component.md", "# Make a component\nBuild $1."),
        ] {
            std::fs::create_dir_all(proj.join(dir)).unwrap();
            std::fs::write(proj.join(dir).join(file), text).unwrap();
        }
        let mut w = vec![];
        let cmds = load_commands(proj, &[], &mut w);
        let review = cmds.iter().find(|c| c.name == "review").unwrap();
        assert_eq!((review.description.as_str(), review.argument_hint.as_deref()), ("kodact review", Some("[file]")));
        let comp = cmds.iter().find(|c| c.name == "frontend:component").unwrap();
        assert_eq!(comp.description, "Make a component");
        assert!(w.is_empty());
    }

    /// A plugin's command said `${CLAUDE_PLUGIN_ROOT}` to the model as it was written.
    #[test]
    fn a_plugins_command_knows_its_own_folder() {
        let d = tempfile::tempdir().unwrap();
        let (proj, plug) = (d.path().join("proj"), d.path().join("plug"));
        std::fs::create_dir_all(plug.join("commands")).unwrap();
        std::fs::create_dir_all(proj.join(".kodact/commands")).unwrap();
        std::fs::write(
            plug.join("commands/lint.md"),
            "---\nallowed-tools: Bash(${KODACT_PLUGIN_ROOT}/lint.sh *)\n---\nRun ${CLAUDE_PLUGIN_ROOT}/lint.sh in ${CLAUDE_PROJECT_DIR} on $1; keep \\${HOME} and ${OTHER}.",
        )
        .unwrap();
        std::fs::write(proj.join(".kodact/commands/own.md"), "From ${KODACT_PROJECT_DIR}, not ${KODACT_PLUGIN_ROOT}.")
            .unwrap();
        let plugin = crate::plugins::Plugin {
            name: "plug".into(),
            description: String::new(),
            version: None,
            dir: plug.clone(),
            manifest: serde_json::Value::Null,
        };
        let mut w = vec![];
        let cmds = load_commands(&proj, &[plugin], &mut w);
        let lint = cmds.iter().find(|c| c.name == "plug:lint").unwrap();
        let (root, project) = (plug.display(), proj.display());
        assert!(lint.body.starts_with(&format!("Run {root}/lint.sh in {project} on $1;")), "{}", lint.body);
        assert!(lint.body.contains("${OTHER}"), "what it does not know is left alone: {}", lint.body);
        assert_eq!(lint.allowed_tools, [format!("Bash({root}/lint.sh *)")]);
        // A command of the project's own has no plugin folder to name.
        let own = cmds.iter().find(|c| c.name == "own").unwrap();
        assert_eq!(own.body.trim(), format!("From {project}, not ${{KODACT_PLUGIN_ROOT}}."));
    }
}
