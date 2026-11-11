use std::sync::Arc;
use std::time::Duration;

use kodact_permissions::Subject;
use serde_json::{json, Value};

use super::str_arg;
use crate::shells::{BackgroundShell, ShellChoice, ShellKind, ShellStatus, StartError};
use crate::{fit_output, truncate_middle, Tool, ToolContext, ToolOutput, INTERRUPTED};
use kodact_platform::shell::Os;

pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;
pub const MAX_TIMEOUT_MS: u64 = 600_000;

/// First words of commands that only read.
const READ_ONLY_COMMANDS: &[&str] = &[
    "ls", "cat", "head", "tail", "wc", "pwd", "echo", "grep", "rg", "find", "which", "file", "stat", "tree", "du",
    "df", "date", "whoami", "uname", "env", "printenv", "sort", "uniq", "cut", "diff", "basename", "dirname",
    "realpath", "readlink", "true", "false", "type", "less", "more", "jq",
];
const READ_ONLY_GIT: &[&str] = &[
    "status",
    "diff",
    "log",
    "show",
    "branch",
    "remote",
    "rev-parse",
    "ls-files",
    "blame",
    "describe",
    "tag",
    "config --get",
];

/// Is every part of the command line known to be read-only?
pub fn command_is_read_only(command: &str) -> bool {
    let parts = kodact_permissions::split_compound(command);
    !parts.is_empty()
        && parts.iter().all(|p| {
            if p.contains('>') || p.contains("$(") || p.contains('`') {
                return false;
            }
            let mut words = p.split_whitespace();
            match words.next() {
                Some("git") => {
                    let rest: Vec<&str> = words.collect();
                    let rest = rest.join(" ");
                    READ_ONLY_GIT.iter().any(|g| rest == *g || rest.starts_with(&format!("{g} ")))
                        && !rest.contains("--output")
                }
                Some("find") => !p.contains("-exec") && !p.contains("-delete") && !p.contains("-fprint"),
                Some("sed") => p.contains(" -n") && !p.contains(" -i"),
                Some(w) => READ_ONLY_COMMANDS.contains(&w),
                None => false,
            }
        })
}

/// The shell tool. Its name stays `Bash` whatever the shell (permission rules,
/// hooks and hosts match on it); its description and read-only check follow the shell.
pub struct Bash {
    kind: Option<ShellKind>,
    description: String,
}

impl Default for Bash {
    fn default() -> Self {
        Bash::new(kodact_platform::shell::detect())
    }
}

impl Bash {
    pub fn new(shell: &ShellChoice) -> Self {
        Bash { kind: shell.as_ref().ok().map(|s| s.kind), description: description(shell) }
    }
}

const BASE_DESCRIPTION: &str = "Run a shell command in a persistent working directory and return its output.\n\n\
     - The working directory carries over between calls while it stays inside the project's working \
       directories; otherwise it is reset to the project directory.\n\
     - Quote paths that contain spaces. Prefer absolute paths over `cd`.\n\
     - `timeout` is how long the call waits, in milliseconds: default 2 minutes, maximum 10. A command \
       still running then is not stopped: it goes on in the background, the call returns its ID and \
       output so far, and you are told when it exits. Don't start it again.\n\
     - Output beyond 30,000 characters is truncated in the middle.\n\
     - Use `run_in_background` for processes that keep running (servers, watchers) and read their \
       output later with BashOutput. You are told when one exits. Background commands are stopped when \
       the session ends; to leave a service running after that, start it detached \
       (`nohup <command> > <log> 2>&1 &`).\n\
     - Prefer the dedicated tools for reading (Read), searching (Grep, Glob) and editing (Edit, Write) \
       files instead of cat, grep, find, sed or echo redirection.\n\
     - Chain dependent commands with `&&`; independent commands can be separate calls.";

/// The tool description for the session's shell. Unix bash keeps the base text unchanged.
pub fn description(shell: &ShellChoice) -> String {
    let mut s = BASE_DESCRIPTION.to_string();
    match shell {
        Ok(sh) if sh.os == Os::Windows && sh.is_posix() => s.push_str(&format!(
            "\n\nThis machine runs Windows; commands run in {}. Use bash syntax and the Unix tools it \
             provides. Write paths with forward slashes (C:/Users/me/project or /c/Users/me/project): a \
             backslash is an escape character in bash. To run a cmd.exe built-in, use `cmd //c <command>`.",
            sh.describe()
        )),
        Ok(sh) if sh.kind == ShellKind::PowerShell => {
            s = s.replace("Chain dependent commands with `&&`", "Chain dependent commands (see below)");
            s.push_str(&format!(
                "\n\n{}Commands run in {}, whatever this tool is called. Write PowerShell, not bash: \
                 Get-ChildItem (ls), Get-Content (cat), Select-String (grep), $env:NAME for environment \
                 variables, `;` between commands.",
                if sh.os == Os::Windows { "This machine runs Windows and has no bash. " } else { "" },
                sh.describe()
            ));
            if sh.label() == "Windows PowerShell 5.1" {
                s.push_str(" This is PowerShell 5.1: there is no `&&` or `||`; write `cmd1; if ($?) { cmd2 }`.");
            } else {
                s.push_str(" `&&` and `||` work.");
            }
        }
        Ok(sh) if sh.kind == ShellKind::Posix => s.push_str(&format!(
            "\n\nCommands run in {}: POSIX sh, not bash, so avoid bash-only syntax.",
            sh.describe()
        )),
        Ok(_) => {}
        Err(m) => s.push_str(&format!("\n\nNo shell was found, so this tool can't run commands: {m}")),
    }
    s
}

/// The result text when no command can run: names the cause and tells the model to stop.
pub fn unavailable(reason: &str) -> String {
    format!(
        "Bash can't run commands in this session: {reason}\n\
         This is a problem with the user's setup, not with your command: every command fails the same way \
         until it is fixed. Don't call Bash again in this session and don't look for another way to run \
         commands. Continue with the other tools where they are enough, and tell the user the fix above."
    )
}

/// PowerShell: read-only only for a plain call of a reading cmdlet, with nothing
/// that could run another command inside it (subexpressions, script blocks,
/// pipelines, redirection, call operators, escapes).
pub fn powershell_is_read_only(command: &str) -> bool {
    const READERS: &[&str] = &[
        "get-childitem",
        "gci",
        "ls",
        "dir",
        "get-content",
        "gc",
        "cat",
        "type",
        "get-location",
        "gl",
        "pwd",
        "test-path",
        "get-item",
        "gi",
        "select-string",
        "sls",
        "resolve-path",
        "rvpa",
        "get-command",
        "gcm",
        "where.exe",
        "whoami",
        "hostname",
    ];
    let c = command.trim();
    if c.is_empty() || c.contains(['(', ')', '{', '}', '$', '@', '&', ';', '|', '>', '<', '`', '\n', '\r']) {
        return false;
    }
    let mut words = c.split_whitespace();
    match words.next().map(str::to_ascii_lowercase).as_deref() {
        Some("git") => {
            let rest: Vec<&str> = words.collect();
            let rest = rest.join(" ");
            READ_ONLY_GIT.iter().any(|g| rest == *g || rest.starts_with(&format!("{g} "))) && !rest.contains("--output")
        }
        Some(w) => READERS.contains(&w),
        None => false,
    }
}

#[async_trait::async_trait]
impl Tool for Bash {
    fn name(&self) -> &str {
        "Bash"
    }

    fn description(&self) -> String {
        self.description.clone()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "The command to execute"},
                "timeout": {"type": "number", "description": "How long to wait for the command, in milliseconds (max 600000); a command still running then goes on in the background"},
                "description": {"type": "string", "description": "A short (5-10 word) description of what the command does"},
                "run_in_background": {"type": "boolean", "description": "Run the command in the background; read output with BashOutput"},
                "dangerouslyDisableSandbox": {"type": "boolean", "description": "Run outside the sandbox (only when a sandboxed run failed for lack of access); requires the user's approval"}
            },
            "required": ["command"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, input: &Value) -> bool {
        !input.get("run_in_background").and_then(Value::as_bool).unwrap_or(false)
            && match self.kind {
                Some(ShellKind::PowerShell) => powershell_is_read_only(str_arg(input, "command")),
                _ => command_is_read_only(str_arg(input, "command")),
            }
    }

    fn permission_subject(&self, input: &Value, _ctx: &ToolContext) -> Subject {
        Subject::Command(str_arg(input, "command").to_string())
    }

    fn validate(&self, input: &Value, ctx: &ToolContext) -> Result<(), String> {
        if let Err(m) = &ctx.shell {
            return Err(unavailable(&m.to_string()));
        }
        crate::validate_required(&self.input_schema(), input)
    }

    fn sandboxed(&self, input: &Value, ctx: &ToolContext) -> bool {
        !input.get("dangerouslyDisableSandbox").and_then(Value::as_bool).unwrap_or(false) && ctx.sandbox_now().is_some()
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let command = str_arg(&input, "command").to_string();
        if command.trim().is_empty() {
            return ToolOutput::error("command must not be empty");
        }
        let cwd = ctx.shell_cwd();
        let cwd = if cwd.is_dir() { cwd } else { ctx.project_dir.clone() };
        let sandbox = if self.sandboxed(&input, ctx) { ctx.sandbox_now() } else { None };

        if input.get("run_in_background").and_then(Value::as_bool).unwrap_or(false) {
            return match ctx.shells.spawn(&ctx.shell, &command, &cwd, &ctx.env, sandbox.as_ref()) {
                Ok(sh) => ToolOutput::text(format!(
                    "Command running in background with ID: {}. Use BashOutput to read its output; you will be \
                     told when it exits.",
                    sh.id()
                ))
                .with_structured(
                    json!({"backgroundTaskId": sh.id(), "stdout": "", "stderr": "", "interrupted": false}),
                ),
                Err(e) => start_failed(&e),
            };
        }

        let ms = input.get("timeout").and_then(Value::as_u64).unwrap_or(DEFAULT_TIMEOUT_MS).clamp(1, MAX_TIMEOUT_MS);
        let sh = match crate::shells::start(&ctx.shell, &command, &cwd, &ctx.env, sandbox.as_ref(), true) {
            Ok(sh) => sh,
            Err(e) => return start_failed(&e),
        };
        let _guard = StopUnlessMoved(sh.clone());
        tokio::select! {
            _ = sh.wait() => {}
            _ = tokio::time::sleep(Duration::from_millis(ms)) => {
                // Still running: it goes on in the background instead of being killed, so a long
                // build or install isn't lost and run again.
                return moved_to_background(&sh, ms, ctx);
            }
            _ = ctx.cancel.cancelled() => {
                sh.kill();
                let _ = tokio::time::timeout(Duration::from_secs(2), sh.wait()).await;
            }
        }
        let res = sh.result(&ctx.shell);

        let mut note = String::new();
        if let Some(dir) = &res.final_cwd {
            if ctx.in_working_dirs(dir) {
                *ctx.shell_cwd.lock().unwrap() = dir.clone();
            } else {
                *ctx.shell_cwd.lock().unwrap() = ctx.project_dir.clone();
                note = format!("\nShell cwd was reset to {}", ctx.project_dir.display());
            }
        }

        let max = ctx.max_output_chars;
        let stdout = fit_output(ctx, res.stdout.trim_end(), max, "stdout");
        let stderr = fit_output(ctx, res.stderr.trim_end(), max, "stderr");
        let mut text = stdout.clone();
        if !stderr.is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&stderr);
        }
        let structured = json!({
            "stdout": stdout, "stderr": stderr, "interrupted": res.interrupted,
            "exitCode": res.code,
        });
        if res.interrupted {
            return ToolOutput::error(format!("{text}\n{INTERRUPTED}").trim_start().to_string())
                .with_structured(structured);
        }
        text.push_str(&note);
        match res.code {
            Some(0) => {
                if text.trim().is_empty() {
                    text = "(no output)".into();
                }
                ToolOutput::text(text).with_structured(structured)
            }
            code => {
                let exit = code;
                let code = code.map(|c| c.to_string()).unwrap_or_else(|| "signal".into());
                let mut msg = format!("Exit code {code}\n{text}").trim_end().to_string();
                if let Some(hint) = sandbox.as_ref().and_then(|(_, p)| p.explain_failure(&text)) {
                    msg.push_str(&format!("\n\n{hint}"));
                }
                let found = |p: &str| kodact_platform::process::find_program(p).is_some();
                if let Some(hint) = super::exit_hints::explain(exit, &text, found) {
                    msg.push_str(&format!("\n\n{hint}"));
                }
                ToolOutput::error(msg).with_structured(structured)
            }
        }
    }
}

/// Stops a Bash call's command if the call goes away while it runs (the session ends, the turn is
/// dropped), unless it moved to the background: then it belongs to the session's shells.
struct StopUnlessMoved(Arc<BackgroundShell>);

impl Drop for StopUnlessMoved {
    fn drop(&mut self) {
        if self.0.id().is_empty() {
            self.0.kill();
        }
    }
}

/// The result of a call that stopped waiting while its command still runs: the command is now a
/// background shell, and the model gets its ID and the output so far.
fn moved_to_background(sh: &Arc<BackgroundShell>, waited_ms: u64, ctx: &ToolContext) -> ToolOutput {
    let id = ctx.shells.adopt(sh);
    sh.set_awaited();
    let (out, err) = sh.take_new_output();
    let max = ctx.max_output_chars;
    let stdout = fit_output(ctx, out.trim_end(), max, "stdout");
    let stderr = fit_output(ctx, err.trim_end(), max, "stderr");
    let mut text = format!(
        "The command is still running after {}. It was not stopped: it goes on in the background as {id}, and \
         you will be told when it exits. Meanwhile do other work, read its new output with BashOutput or stop \
         it with KillShell; don't start it again. If you end your reply while it runs, the turn waits for it.",
        duration(Duration::from_millis(waited_ms))
    );
    let so_far = [stdout.as_str(), stderr.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>();
    if so_far.is_empty() {
        text.push_str("\n\nNo output so far.");
    } else {
        text.push_str(&format!("\n\nOutput so far:\n{}", so_far.join("\n")));
    }
    ToolOutput::text(text).with_structured(json!({
        "backgroundTaskId": id, "stdout": stdout, "stderr": stderr, "interrupted": false
    }))
}

/// `45s`, `4m 12s`, `1h 3m`.
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 if s.is_multiple_of(60) => format!("{}m", s / 60),
        60..=3599 => format!("{}m {}s", s / 60, s % 60),
        _ => format!("{}h {}m", s / 3600, (s % 3600) / 60),
    }
}

/// A command line for a one-line note: its first line, at most 100 characters.
fn short_command(command: &str) -> String {
    let first = command.trim().lines().next().unwrap_or_default();
    let mut s: String = first.chars().take(100).collect();
    if s.len() < first.len() || command.trim().lines().count() > 1 {
        s.push_str(" …");
    }
    s
}

/// The last `lines` lines of a text, at most `max` characters.
fn last_lines(text: &str, lines: usize, max: usize) -> String {
    let all: Vec<&str> = text.trim_end().lines().collect();
    let tail = all[all.len().saturating_sub(lines)..].join("\n");
    let skip = tail.chars().count().saturating_sub(max);
    tail.chars().skip(skip).collect()
}

/// A note for the model on background commands that exited: how they ended and their last
/// unread output. `None` when there are none.
pub fn exit_notice(exited: &[Arc<BackgroundShell>]) -> Option<String> {
    if exited.is_empty() {
        return None;
    }
    let mut parts = vec![];
    for sh in exited {
        let how = match sh.status() {
            ShellStatus::Completed(Some(code)) => format!("exited with code {code}"),
            _ => "was ended by a signal".to_string(),
        };
        let mut part = format!(
            "Background command {} {how} after {}: `{}`",
            sh.id(),
            duration(sh.runtime()),
            short_command(&sh.command)
        );
        let (out, err) = sh.unread_output();
        let unread = [out.trim_end(), err.trim_end()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>();
        if !unread.is_empty() {
            part.push_str(&format!(
                "\nIts last output (BashOutput {} returns all of it):\n{}",
                sh.id(),
                last_lines(&unread.join("\n"), 15, 2000)
            ));
        }
        parts.push(part);
    }
    Some(format!("<system-reminder>\n{}\n</system-reminder>", parts.join("\n\n")))
}

/// A note for the model on commands still running after the turn waited for them.
pub fn still_running_notice(running: &[Arc<BackgroundShell>], waited: Duration) -> Option<String> {
    if running.is_empty() {
        return None;
    }
    let list = running
        .iter()
        .map(|sh| format!("- {} (running for {}): `{}`", sh.id(), duration(sh.runtime()), short_command(&sh.command)))
        .collect::<Vec<_>>()
        .join("\n");
    Some(format!(
        "<system-reminder>\nThe turn waited {} for these commands, and they are still running:\n{list}\nRead their \
         output with BashOutput to see whether they progress. End your reply again to wait longer, or stop one \
         with KillShell if it is stuck. When the run ends, commands still running are stopped.\n</system-reminder>",
        duration(waited)
    ))
}

fn start_failed(e: &StartError) -> ToolOutput {
    if e.is_setup_problem() {
        ToolOutput::error(unavailable(&e.to_string())).with_structured(json!({
            "stdout": "", "stderr": e.to_string(), "interrupted": false, "exitCode": null, "shellUnavailable": true
        }))
    } else {
        ToolOutput::error(format!("The command did not start: {e}"))
    }
}

pub struct BashOutput;

#[async_trait::async_trait]
impl Tool for BashOutput {
    fn name(&self) -> &str {
        "BashOutput"
    }

    fn description(&self) -> String {
        "Read new output from a background shell: one started with Bash `run_in_background`, or a Bash \
         command that outlived its timeout. Returns only output produced since the last read, plus the shell's \
         status. `filter` keeps only lines matching a regex."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "bash_id": {"type": "string", "description": "The ID of the background shell"},
                "filter": {"type": "string", "description": "Optional regex; only matching lines are returned"}
            },
            "required": ["bash_id"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let id = str_arg(&input, "bash_id");
        let Some(sh) = ctx.shells.get(id) else { return ToolOutput::error(format!("No shell found with ID: {id}")) };
        let (mut out, mut err) = sh.take_new_output();
        if let Some(f) = input.get("filter").and_then(Value::as_str).filter(|f| !f.is_empty()) {
            let re = match regex::Regex::new(f) {
                Ok(r) => r,
                Err(e) => return ToolOutput::error(format!("Invalid filter regex: {e}")),
            };
            let keep = |s: &str| s.lines().filter(|l| re.is_match(l)).collect::<Vec<_>>().join("\n");
            out = keep(&out);
            err = keep(&err);
        }
        let status = sh.status();
        if status != ShellStatus::Running {
            sh.mark_reported();
        }
        let code = match status {
            ShellStatus::Completed(Some(c)) => format!("\n<exit_code>{c}</exit_code>"),
            _ => String::new(),
        };
        ToolOutput::text(format!(
            "<status>{}</status>{code}\n<stdout>\n{}\n</stdout>\n<stderr>\n{}\n</stderr>",
            status.label(),
            truncate_middle(out.trim_end(), ctx.max_output_chars),
            truncate_middle(err.trim_end(), ctx.max_output_chars)
        ))
    }
}

pub struct KillShell;

#[async_trait::async_trait]
impl Tool for KillShell {
    fn name(&self) -> &str {
        "KillShell"
    }

    fn description(&self) -> String {
        "Stop a background shell by its ID.".into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"shell_id": {"type": "string", "description": "The ID of the background shell to kill"}},
            "required": ["shell_id"],
            "additionalProperties": false
        })
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let id = str_arg(&input, "shell_id");
        match ctx.shells.get(id) {
            Some(sh) => {
                sh.kill();
                sh.mark_reported();
                ToolOutput::text(format!("Successfully killed shell: {id} ({})", sh.command))
            }
            None => ToolOutput::error(format!("No shell found with ID: {id}")),
        }
    }
}
