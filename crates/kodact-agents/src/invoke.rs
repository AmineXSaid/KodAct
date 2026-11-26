//! Invoking a skill, as the reference CLI does, from `/name args` or the
//! Skill tool: its text filled in (`substitute.rs`), its ``!`command` `` lines
//! and ` ```! ` blocks run and replaced by their output, and its frontmatter
//! applied for the turn it runs in: `allowed-tools` allowed without asking,
//! `disallowed-tools` taken away, `model` and `effort` switched (all of it
//! undone when the turn ends), and `hooks` kept for the session. A
//! `context: fork` skill comes back as a task for a sub-agent instead.

use std::path::Path;
use std::sync::LazyLock;
use std::time::Duration;

use kodact_engine::EngineHandle;
use kodact_hooks::HookRunner;
use kodact_permissions::{Decision, Request, Rule, Subject};
use kodact_platform::shell::{ShellChoice, ShellConfig};
use kodact_tools::ToolContext;
use regex::Regex;
use serde_json::Value;

use crate::skills::SkillDef;
use crate::substitute::{fill, fill_vars, SkillVars};

/// What a skill's command may take before it is cut, and how long it may run.
const OUTPUT_MAX: usize = 30_000;
const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

/// What invoking a skill gives.
#[derive(Debug, Clone, PartialEq)]
pub enum Invoked {
    /// The text that goes in the conversation.
    Text(String),
    /// Run `prompt` as a sub-agent of type `agent`; its report is the result.
    Fork { agent: String, prompt: String },
}

/// The session a skill runs in: absent in tests and where there is no engine.
#[derive(Clone, Copy, Default)]
pub struct Session<'a> {
    pub handle: Option<&'a EngineHandle>,
    pub hooks: Option<&'a HookRunner>,
}

fn vars<'a>(skill: &'a SkillDef, ctx: &'a ToolContext) -> SkillVars<'a> {
    SkillVars {
        skill_dir: Some(skill.dir.as_path()).filter(|d| !d.as_os_str().is_empty()),
        project_dir: Some(ctx.project_dir.as_path()),
        session_id: Some(ctx.session_id.as_str()).filter(|s| !s.is_empty()),
        effort: ctx.env.get("KODACT_EFFORT").map(String::as_str),
        plugin: skill.plugin_dir.as_deref().zip(skill.name.split_once(':').map(|(p, _)| p)),
    }
}

/// Invoke `skill` with `args`: see the module docs.
pub async fn invoke(skill: &SkillDef, args: &str, ctx: &ToolContext, session: Session<'_>) -> Result<Invoked, String> {
    let v = vars(skill, ctx);
    let body = fill(skill.body.trim(), args, &skill.arguments, &v);
    let allowed: Vec<String> = skill.allowed_tools.iter().map(|r| fill_vars(r, &v)).collect();
    let body = run_commands(skill, &body, &allowed, ctx, session.handle).await?;
    let mut kept_model = None;

    if let Some(h) = session.handle {
        let rules = allowed.iter().filter_map(|r| Rule::parse(r).ok()).collect();
        h.permissions.write().unwrap().grant_turn(rules, skill.disallowed_tools.clone());
        if let Some(why) = h.override_for_turn(skill.model.as_deref(), skill.effort.as_deref()) {
            tracing::warn!(skill = %skill.name, %why, "skill model");
            kept_model = Some(why);
        }
    }
    if let (Some(hooks), Some(mut value)) = (session.hooks, skill.hooks.clone()) {
        fill_value(&mut value, &v);
        let errors = hooks.add_session_hooks(&value);
        if !errors.is_empty() {
            tracing::warn!(skill = %skill.name, ?errors, "skill hooks");
        }
    }
    let mut text = if skill.dir.as_os_str().is_empty() {
        body
    } else {
        format!("Base directory for this skill: {}\n\n{body}", skill.dir.display())
    };
    // The model it asked for can't be served here: said with the skill, so the turn's model
    // isn't taken for the one the skill names.
    if let Some(why) = kept_model {
        text.push_str(&format!("\n\n(This skill's model: {why}.)"));
    }
    Ok(if skill.fork {
        Invoked::Fork { agent: skill.agent.clone().unwrap_or_else(|| "general-purpose".into()), prompt: text }
    } else {
        if let Some(h) = session.handle {
            h.record_skill(&skill.name, &text);
        }
        Invoked::Text(text)
    })
}

fn fill_value(v: &mut Value, vars: &SkillVars) {
    match v {
        Value::String(s) => *s = fill_vars(s, vars),
        Value::Array(a) => a.iter_mut().for_each(|x| fill_value(x, vars)),
        Value::Object(o) => o.values_mut().for_each(|x| fill_value(x, vars)),
        _ => {}
    }
}

/// `` !`cmd` `` at the start of a line or after whitespace.
static INLINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)(^|[ \t])!`([^`\n]+)`").expect("regex"));
/// A fenced block opened with ```` ```! ````.
static FENCED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?ms)^```!\s*\n(.*?)\n```[ \t]*$").expect("regex"));

/// The skill's commands found in `body`: (byte range to replace, command, kept prefix).
fn commands_in(body: &str) -> Vec<(std::ops::Range<usize>, String, String)> {
    let mut found: Vec<(std::ops::Range<usize>, String, String)> =
        FENCED.captures_iter(body).map(|c| (c.get(0).unwrap().range(), c[1].to_string(), String::new())).collect();
    for c in INLINE.captures_iter(body) {
        let whole = c.get(0).unwrap().range();
        if !found.iter().any(|(r, _, _)| r.start <= whole.start && whole.end <= r.end) {
            found.push((whole, c[2].trim().to_string(), c[1].to_string()));
        }
    }
    found.sort_by_key(|(r, _, _)| r.start);
    found
}

/// Run the skill's commands and put their output in their place. A command the
/// rules deny, or that is neither in `allowed-tools` nor allowed by the
/// session's rules, stops the invocation; so does one that fails.
async fn run_commands(
    skill: &SkillDef,
    body: &str,
    allowed: &[String],
    ctx: &ToolContext,
    handle: Option<&EngineHandle>,
) -> Result<String, String> {
    let found = commands_in(body);
    if found.is_empty() {
        return Ok(body.to_string());
    }
    if skill.no_shell {
        let mut out = String::new();
        let mut last = 0;
        for (range, _, prefix) in &found {
            out.push_str(&body[last..range.start]);
            out.push_str(prefix);
            out.push_str("[shell command execution disabled by policy]");
            last = range.end;
        }
        out.push_str(&body[last..]);
        return Ok(out);
    }
    let shell = skill_shell(skill)?;
    let mut out = String::new();
    let mut last = 0;
    for (range, command, prefix) in found {
        permit(skill, &command, allowed, ctx, handle)?;
        let output = run_one(&shell, &command, &ctx.project_dir).await.map_err(|e| format!("/{}: {e}", skill.name))?;
        out.push_str(&body[last..range.start]);
        out.push_str(&prefix);
        out.push_str(&output);
        last = range.end;
    }
    out.push_str(&body[last..]);
    Ok(out)
}

/// Whether the skill may run `command` before the model sees it.
fn permit(
    skill: &SkillDef,
    command: &str,
    allowed: &[String],
    ctx: &ToolContext,
    handle: Option<&EngineHandle>,
) -> Result<(), String> {
    let req = Request::new("Bash", Subject::Command(command.to_string()), false);
    let decision = handle.map(|h| h.permissions.read().unwrap().decide(&req));
    if let Some(Decision::Deny { reason }) = &decision {
        return Err(format!("/{} runs `{command}`, which is denied ({reason}).", skill.name));
    }
    if crate::commands::bang_allowed(allowed, command, &ctx.project_dir) {
        return Ok(());
    }
    match decision {
        Some(Decision::Allow { .. }) => Ok(()),
        _ => Err(format!(
            "/{} runs `{command}` before it starts, which needs permission: allow it in the skill's \
             allowed-tools (e.g. \"Bash({} *)\") or in your permission rules.",
            skill.name,
            command.split_whitespace().next().unwrap_or(command)
        )),
    }
}

/// The shell a skill's commands run in: bash unless it asks for PowerShell
/// (`shell: powershell`); without that shell, the invocation fails.
fn skill_shell(skill: &SkillDef) -> Result<kodact_platform::shell::Shell, String> {
    let choice: ShellChoice = match skill.shell.as_deref() {
        Some("powershell") => {
            let pwsh = ShellConfig { env_override: Some("pwsh".into()), ..Default::default() };
            kodact_platform::shell::resolve(&pwsh).or_else(|_| {
                kodact_platform::shell::resolve(&ShellConfig {
                    env_override: Some("powershell".into()),
                    ..Default::default()
                })
            })
        }
        _ => kodact_platform::shell::resolve(&ShellConfig { posix_only: true, ..ShellConfig::from_env() }),
    };
    choice.map_err(|e| format!("/{} runs commands, but {e}", skill.name))
}

/// Commands whose exit code 1 means "nothing found" or "different", not failure.
fn one_is_ok(command: &str) -> bool {
    let first = command.split_whitespace().next().unwrap_or("");
    let first = first.rsplit(['/', '\\']).next().unwrap_or(first);
    matches!(first, "grep" | "egrep" | "fgrep" | "rg" | "diff" | "cmp" | "test" | "[" | "find" | "git")
        && !command.contains("&&")
}

async fn run_one(shell: &kodact_platform::shell::Shell, command: &str, cwd: &Path) -> Result<String, String> {
    let (std_cmd, _script) = shell.command(&shell.script(command, None)).map_err(|e| e.to_string())?;
    let mut std_cmd = std_cmd;
    kodact_platform::process::no_window(&mut std_cmd);
    let run = tokio::process::Command::from(std_cmd)
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let o = match tokio::time::timeout(COMMAND_TIMEOUT, run).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(format!("could not run `{command}`: {e}")),
        Err(_) => return Err(format!("`{command}` did not finish within {}s", COMMAND_TIMEOUT.as_secs())),
    };
    let mut text = String::from_utf8_lossy(&o.stdout).trim_end().to_string();
    let err = String::from_utf8_lossy(&o.stderr);
    if !err.trim().is_empty() {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(err.trim_end());
    }
    let code = o.status.code();
    if code != Some(0) && !(code == Some(1) && one_is_ok(command)) {
        let code = code.map_or("a signal".to_string(), |c| format!("exit code {c}"));
        return Err(format!("`{command}` failed ({code}):\n{}", kodact_tools::truncate_middle(&text, 4_000)));
    }
    Ok(kodact_tools::truncate_middle(&text, OUTPUT_MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(body: &str, allowed: &[&str]) -> SkillDef {
        SkillDef {
            name: "t".into(),
            description: "t".into(),
            body: body.into(),
            allowed_tools: allowed.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn finds_commands_where_the_reference_cli_does() {
        let body = "Branch: !`git branch --show-current`\nKEY=!`date`\n!`ls`\n```!\nexport A=1\necho \"$A\"\n```\nend";
        let found: Vec<(String, String)> = commands_in(body).into_iter().map(|(_, c, p)| (c, p)).collect();
        assert_eq!(
            found,
            [
                ("git branch --show-current".into(), " ".into()),
                ("ls".into(), String::new()),
                ("export A=1\necho \"$A\"".into(), String::new())
            ]
        );
    }

    #[tokio::test]
    async fn runs_allowed_commands_and_stops_on_the_rest() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.txt"), "x").unwrap();
        let ctx = ToolContext::new(d.path());
        let s = skill(
            "Files:\n!`ls`\n```!\necho one\necho two >&2\n```\nCount: $0",
            &["Bash(ls *)", "Bash(ls)", "Bash(echo *)"],
        );
        let Invoked::Text(t) = invoke(&s, "3", &ctx, Session::default()).await.unwrap() else { panic!() };
        assert_eq!(t, "Files:\na.txt\none\ntwo\nCount: 3");
        // Not allowed: the invocation stops and says how to allow it.
        let s = skill("!`cat a.txt`", &[]);
        let e = invoke(&s, "", &ctx, Session::default()).await.unwrap_err();
        assert!(e.contains("allowed-tools") && e.contains("Bash(cat *)"), "{e}");
        // A failing command stops it too, with its output; grep finding nothing does not.
        let s = skill("!`ls missing-file`", &["Bash(ls *)"]);
        assert!(invoke(&s, "", &ctx, Session::default()).await.unwrap_err().contains("failed"));
        let s = skill("[ !`grep nothing a.txt`]", &["Bash(grep *)"]);
        assert_eq!(invoke(&s, "", &ctx, Session::default()).await.unwrap(), Invoked::Text("[ ]".into()));
        // Turned off by policy: a placeholder, nothing runs.
        let mut s = skill("!`ls`", &["Bash(ls)"]);
        s.no_shell = true;
        assert_eq!(
            invoke(&s, "", &ctx, Session::default()).await.unwrap(),
            Invoked::Text("[shell command execution disabled by policy]".into())
        );
    }

    #[tokio::test]
    async fn a_fork_skill_becomes_a_task() {
        let ctx = ToolContext::new(Path::new("/p"));
        let mut s = skill("Survey $ARGUMENTS", &[]);
        s.fork = true;
        s.agent = Some("Explore".into());
        assert_eq!(
            invoke(&s, "the API", &ctx, Session::default()).await.unwrap(),
            Invoked::Fork { agent: "Explore".into(), prompt: "Survey the API".into() }
        );
    }
}
