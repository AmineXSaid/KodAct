//! System prompt assembly.

use std::path::{Path, PathBuf};

use kodact_types::{CacheControl, SystemBlock};

const BUILTIN: &[(&str, &str)] = &[
    ("01-identity", include_str!("../prompts/01-identity.md")),
    ("02-working", include_str!("../prompts/02-working.md")),
    ("03-tools", include_str!("../prompts/03-tools.md")),
    ("04-style", include_str!("../prompts/04-style.md")),
];

/// The section added for unattended runs (`--autonomous`).
pub const AUTONOMOUS: &str = include_str!("../prompts/05-autonomous.md");

/// How the system prompt is chosen (`--system-prompt`, `--append-system-prompt`).
#[derive(Debug, Clone, Default)]
pub struct SystemPromptOptions {
    /// Replace the default prompt entirely.
    pub replace: Option<String>,
    /// Appended after the default (or replacement) prompt.
    pub append: Option<String>,
    /// Directory of `system/*.md` and `tools/*.md` overrides (`KODACT_PROMPTS_DIR`).
    pub prompts_dir: Option<PathBuf>,
    /// Move per-machine sections into the first user message.
    pub exclude_dynamic: bool,
    /// Output style instructions (`outputStyle`), appended as their own section.
    pub output_style: Option<String>,
    /// `--autonomous`: add the unattended-run section.
    pub autonomous: bool,
}

/// Facts about the machine and project for the environment section.
#[derive(Debug, Clone)]
pub struct EnvInfo {
    pub cwd: PathBuf,
    pub additional_dirs: Vec<PathBuf>,
    pub model: String,
    pub model_name: String,
    pub date: String,
    pub git_status: Option<String>,
    /// The project's check commands (verification loop), shown to the model.
    pub checks: Vec<String>,
    /// The shell commands run in, when it isn't plain bash on Unix (Git Bash,
    /// PowerShell, sh) or when there is none; `None` keeps the usual prompt.
    pub shell: Option<String>,
}

impl EnvInfo {
    pub fn collect(cwd: &Path, additional_dirs: &[PathBuf], model: &str) -> Self {
        let model_name = kodact_api::models::model_info(model)
            .map(|m| m.display_name.to_string())
            .unwrap_or_else(|| model.to_string());
        EnvInfo {
            cwd: cwd.to_path_buf(),
            additional_dirs: additional_dirs.to_vec(),
            model: model.to_string(),
            model_name,
            date: chrono::Local::now().format("%Y-%m-%d").to_string(),
            git_status: kodact_git::status_snapshot(cwd),
            checks: vec![],
            shell: None,
        }
    }

    pub fn render(&self) -> String {
        let mut s = format!(
            "<env>\nWorking directory: {}\nIs directory a git repo: {}\nPlatform: {}\nToday's date: {}\n",
            self.cwd.display(),
            if self.git_status.is_some() { "Yes" } else { "No" },
            std::env::consts::OS,
            self.date
        );
        if let Some(sh) = &self.shell {
            s.push_str(&format!("Shell: {sh}\n"));
        }
        if !self.additional_dirs.is_empty() {
            let dirs: Vec<String> = self.additional_dirs.iter().map(|d| d.display().to_string()).collect();
            s.push_str(&format!("Additional working directories: {}\n", dirs.join(", ")));
        }
        if !self.checks.is_empty() {
            let checks: Vec<String> = self.checks.iter().map(|c| format!("`{c}`")).collect();
            s.push_str(&format!("Project checks (run them after changing code): {}\n", checks.join(", ")));
        }
        s.push_str(&format!("</env>\nYou are powered by the model {}.\n", self.model_name));
        if let Some(g) = &self.git_status {
            s.push_str(&format!("\ngitStatus: This is the git status at the start of the conversation. It is a snapshot and will not update.\n{g}\n"));
        }
        s
    }
}

fn read_dir_md(dir: &Path) -> Vec<(String, String)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut files: Vec<(String, String)> = rd
        .flatten()
        .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("md"))
        .filter_map(|e| {
            let name = e.path().file_stem()?.to_string_lossy().into_owned();
            Some((name, std::fs::read_to_string(e.path()).ok()?))
        })
        .collect();
    files.sort();
    files
}

/// Replace `{{key}}` placeholders in an external prompt set.
fn fill(text: &str, env: &EnvInfo) -> String {
    text.replace("{{cwd}}", &env.cwd.display().to_string())
        .replace("{{date}}", &env.date)
        .replace("{{model}}", &env.model)
        .replace("{{model_name}}", &env.model_name)
        .replace("{{platform}}", std::env::consts::OS)
        .replace("{{is_git}}", if env.git_status.is_some() { "Yes" } else { "No" })
        .replace("{{shell}}", env.shell.as_deref().unwrap_or("bash"))
}

/// Tool description overrides from `<prompts_dir>/tools/<Name>.md`.
pub fn tool_description_overrides(opts: &SystemPromptOptions) -> Vec<(String, String)> {
    opts.prompts_dir.as_deref().map(|d| read_dir_md(&d.join("tools"))).unwrap_or_default()
}

/// The system prompt blocks, plus text for the first user message when
/// dynamic sections are excluded from the system prompt.
pub fn build_system(opts: &SystemPromptOptions, env: &EnvInfo) -> (Vec<SystemBlock>, Option<String>) {
    let mut base = if let Some(r) = &opts.replace {
        r.clone()
    } else if let Some(sections) =
        opts.prompts_dir.as_deref().map(|d| read_dir_md(&d.join("system"))).filter(|s| !s.is_empty())
    {
        sections.iter().map(|(_, t)| fill(t, env)).collect::<Vec<_>>().join("\n\n")
    } else {
        BUILTIN.iter().map(|(_, t)| t.trim().to_string()).collect::<Vec<_>>().join("\n\n")
    };
    if let Some(style) = &opts.output_style {
        base.push_str(&format!("\n\n# Output style\n\n{style}"));
    }
    if opts.autonomous {
        base.push_str("\n\n");
        base.push_str(AUTONOMOUS.trim());
    }
    let mut deferred = None;
    let using_default = opts.replace.is_none();
    if using_default {
        if opts.exclude_dynamic {
            deferred = Some(env.render());
        } else {
            base.push_str("\n\n");
            base.push_str(&env.render());
        }
    }
    if let Some(a) = &opts.append {
        base.push_str("\n\n");
        base.push_str(a);
    }
    let mut block = SystemBlock::text(base.trim().to_string());
    block.cache_control = Some(CacheControl::ephemeral());
    (vec![block], deferred)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env() -> EnvInfo {
        EnvInfo {
            cwd: "/w".into(),
            additional_dirs: vec![],
            model: "claude-opus-5-5".into(),
            model_name: "Opus 5.5".into(),
            date: "2026-10-06".into(),
            git_status: None,
            checks: vec![],
            shell: None,
        }
    }

    #[test]
    fn default_prompt_has_sections_and_env() {
        let (blocks, deferred) = build_system(&SystemPromptOptions::default(), &env());
        let t = &blocks[0].text;
        assert!(t.starts_with("You are KodAct"));
        assert!(t.contains("<env>") && t.contains("Working directory: /w"));
        assert!(deferred.is_none());
        assert!(blocks[0].cache_control.is_some());
        assert!(!t.contains("Shell:"), "plain bash on Unix keeps the prompt unchanged");
        let mut e = env();
        e.shell = Some(r"Git Bash (C:\Program Files\Git\bin\bash.exe)".into());
        let (blocks, _) = build_system(&SystemPromptOptions::default(), &e);
        assert!(blocks[0].text.contains("\nShell: Git Bash (C:\\Program Files\\Git\\bin\\bash.exe)\n"));
    }

    /// A run (22f2cd7): every threshold passed at reply 19 of 35, and
    /// the rest of the run widened margins on deterministic metrics that couldn't change the
    /// grade; the rework broke the solver script, restored with about 5 minutes left. The rule is
    /// in the prompt for every mode, and it keeps the remaining work (06eba3b still had to write
    /// the files the task grades after its metrics passed).
    #[test]
    fn the_prompt_says_to_finish_once_the_target_is_met() {
        for autonomous in [false, true] {
            let (b, _) = build_system(&SystemPromptOptions { autonomous, ..Default::default() }, &env());
            let t = &b[0].text;
            assert!(t.contains("Finish when the work is done"), "autonomous: {autonomous}");
            assert!(t.contains("complete what is still asked (output files, cleanup)"));
            assert!(t.contains("already meets its stated target") && t.contains("unless the task asks for the best"));
            assert!(t.contains("run it again to confirm it holds, and mention the margin"));
        }
    }

    /// Two rules for every model and mode (090446d). Drafts go into files as the work goes:
    /// about a sixth of the run's output was reasoning cut at the reply limit, much of it whole
    /// programs drafted there and lost. A given output format is all that output holds:
    /// modernize-scientific-stack printed an extra line per station before the asked one, and
    /// the program reading the output took the wrong number.
    #[test]
    fn the_prompt_says_to_work_in_files_and_keep_output_formats_exact() {
        for autonomous in [false, true] {
            let (b, _) = build_system(&SystemPromptOptions { autonomous, ..Default::default() }, &env());
            let t = &b[0].text;
            assert!(t.contains("Work in files as you go"), "autonomous: {autonomous}");
            assert!(t.contains("build a large file in parts, rather than composing all of it in your reasoning"));
            assert!(t.contains("that output holds exactly it: no extra lines"));
        }
    }

    /// A run (231fd6b): one approach lost every game in seven tries (replies 44 to 79),
    /// including after the model wrote that it was useless at reply 58, while the parameter
    /// sweeps that moved another from 59 to 75 wins got less of the time. The score is in free-form
    /// output, so the switch is the model's call, made on its own measured results.
    #[test]
    fn autonomous_section_is_added_only_when_asked() {
        let (b, _) = build_system(&SystemPromptOptions::default(), &env());
        assert!(!b[0].text.contains("Running unattended"));
        assert!(!b[0].text.contains("three attempts in a row at one idea"), "unattended runs only");
        let (b, _) = build_system(&SystemPromptOptions { autonomous: true, ..Default::default() }, &env());
        assert!(b[0].text.contains("# Running unattended"));
        assert!(b[0].text.contains("Make a real attempt"));
        assert!(b[0].text.contains("keep the best version you have tested"));
        assert!(b[0].text.contains("Note each attempt's result"));
        assert!(b[0].text.contains("three attempts in a row at one idea haven't beaten your best"));
        assert!(b[0].text.contains("substantially different one (going back to an idea you set aside counts"));
    }

    #[test]
    fn replace_append_and_exclude_dynamic() {
        let opts = SystemPromptOptions {
            replace: Some("Custom.".into()),
            append: Some("Extra.".into()),
            ..Default::default()
        };
        let (b, _) = build_system(&opts, &env());
        assert_eq!(b[0].text, "Custom.\n\nExtra.");
        let opts = SystemPromptOptions { exclude_dynamic: true, ..Default::default() };
        let (b, d) = build_system(&opts, &env());
        assert!(!b[0].text.contains("<env>"));
        assert!(d.unwrap().contains("<env>"));
    }

    #[test]
    fn prompts_dir_overrides() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("system")).unwrap();
        std::fs::create_dir_all(d.path().join("tools")).unwrap();
        std::fs::write(d.path().join("system/b.md"), "Second {{model}}").unwrap();
        std::fs::write(d.path().join("system/a.md"), "First in {{cwd}}").unwrap();
        std::fs::write(d.path().join("tools/Bash.md"), "Custom bash").unwrap();
        let opts = SystemPromptOptions { prompts_dir: Some(d.path().into()), ..Default::default() };
        let (b, _) = build_system(&opts, &env());
        assert!(b[0].text.starts_with("First in /w\n\nSecond claude-opus-5-5"));
        assert_eq!(tool_description_overrides(&opts), vec![("Bash".to_string(), "Custom bash".to_string())]);
    }
}
