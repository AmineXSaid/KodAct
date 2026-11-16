//! Commands about this session: `/rename`, `/export`, `/diff`, `/context`,
//! `/debug`, `/permissions` and `/add-dir`.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use kodact_types::{ContentBlock, Message, MessageContent, MessagesRequest, Role, SystemBlock};
use serde_json::{json, Value};

use super::run::{err, ok};
use super::settings::{save, Scope};
use super::Exec;
use crate::driver::Driver;

/// The model for side requests: `smallFastModel`, else the default small
/// model; an OpenAI-compatible endpoint (which may not serve that model) uses
/// the session's model unless `smallFastModel` names one.
fn small_model(d: &Driver) -> String {
    match d.info.settings.str("/smallFastModel") {
        Some(m) => kodact_api::resolve_model(m),
        None if d.engine.provider_name() == "openai" => d.engine.handle().model(),
        None => kodact_api::models::SMALL_FAST_MODEL.to_string(),
    }
}

/// The model that judges `/goal`: `goalCheckModel`, else the session's model.
/// A small model is too easily convinced by the agent's own summary.
pub(crate) fn goal_model(d: &Driver) -> String {
    d.info.settings.str("/goalCheckModel").map(kodact_api::resolve_model).unwrap_or_else(|| d.engine.handle().model())
}

/// A one-off request with no tools, outside the conversation. Its cost counts toward the session.
pub(crate) async fn side_request(
    d: &mut Driver,
    system: &str,
    user: String,
    max_tokens: u32,
) -> Result<String, String> {
    side_request_with(d, system, user, max_tokens, &tokio_util::sync::CancellationToken::new()).await
}

/// [`side_request`] that stops when `cancel` is cancelled.
pub(crate) async fn side_request_with(
    d: &mut Driver,
    system: &str,
    user: String,
    max_tokens: u32,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<String, String> {
    let model = small_model(d);
    side_request_on(d, &model, system, user, max_tokens, cancel).await
}

/// [`side_request_with`] on a given model.
pub(crate) async fn side_request_on(
    d: &mut Driver,
    model: &str,
    system: &str,
    user: String,
    max_tokens: u32,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<String, String> {
    let model = model.to_string();
    let req = MessagesRequest {
        model: model.clone(),
        max_tokens,
        messages: vec![Message::user(vec![ContentBlock::text(user)])],
        system: vec![SystemBlock::text(system)],
        tools: vec![],
        tool_choice: None,
        thinking: None,
        temperature: None,
        metadata: None,
        output_config: None,
        speed: None,
        stream: true,
        betas: vec![],
        carry_reasoning: false,
    };
    let provider = d.engine.provider();
    let msg = kodact_api::complete(provider.as_ref(), req, cancel).await.map_err(|e| e.to_string())?;
    d.engine.record_side_usage(&model, &msg.usage);
    Ok(msg.content.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("").trim().to_string())
}

fn is_reminder(text: &str) -> bool {
    text.trim_start().starts_with("<system-reminder>")
}

/// One line per tool call: `Name(first argument)`.
fn tool_call_line(name: &str, input: &Value) -> String {
    let arg = input
        .as_object()
        .and_then(|o| o.values().find_map(Value::as_str))
        .map(|s| {
            let one = s.lines().next().unwrap_or_default();
            if one.chars().count() > 80 {
                format!("{}…", one.chars().take(80).collect::<String>())
            } else {
                one.to_string()
            }
        })
        .unwrap_or_default();
    format!("{} {name}({arg})", crate::glyphs::TOOL)
}

/// The conversation as plain text, as `/export` writes it.
pub fn render_conversation(messages: &[Message]) -> String {
    let mut out = String::new();
    for m in messages {
        for b in &m.content {
            match (m.role, b) {
                (Role::User, ContentBlock::Text { text, .. }) if !is_reminder(text) => {
                    let mut lines = text.trim().lines();
                    let _ = writeln!(out, "> {}", lines.next().unwrap_or_default());
                    for l in lines {
                        let _ = writeln!(out, "  {l}");
                    }
                    out.push('\n');
                }
                (Role::User, ContentBlock::ToolResult { content, is_error, .. }) => {
                    let text = content.to_text();
                    let n = text.lines().count();
                    let first = text.lines().next().unwrap_or_default().chars().take(100).collect::<String>();
                    let tag = if *is_error == Some(true) { "error: " } else { "" };
                    let more = if n > 1 { format!(" (+{} lines)", n - 1) } else { String::new() };
                    let _ = writeln!(out, "  {}  {tag}{first}{more}\n", crate::glyphs::RESULT);
                }
                (Role::Assistant, ContentBlock::Text { text, .. }) if !text.trim().is_empty() => {
                    let _ = writeln!(out, "{}\n", text.trim());
                }
                (Role::Assistant, ContentBlock::ToolUse { name, input, .. }) => {
                    let _ = writeln!(out, "{}", tool_call_line(name, input));
                }
                _ => {}
            }
        }
    }
    out.trim_end().to_string()
}

/// A session name: no control characters, single spaces, at most 200 characters.
pub fn clean_title(raw: &str) -> String {
    let s: String = raw.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_matches(|c: char| c == '"' || c == '\'' || c == '`').trim().trim_end_matches('.').trim();
    s.chars().take(200).collect()
}

const TITLE_PROMPT: &str = "You name coding sessions. Reply with only a short title (three to seven words) for \
                            the conversation you are given: sentence case, no quotes, no closing punctuation.";

pub(super) async fn rename(d: &mut Driver, args: &str) -> Exec {
    let name = if args.is_empty() {
        let text = render_conversation(&d.engine.state.messages);
        if text.is_empty() {
            return err("Nothing to name yet. Send a message first, or give a name: /rename <name>");
        }
        let tail: String = text.chars().rev().take(6000).collect::<Vec<_>>().into_iter().rev().collect();
        match side_request(d, TITLE_PROMPT, tail, 60).await {
            Ok(t) => t,
            Err(e) => return err(format!("Could not generate a name: {e}. Give one instead: /rename <name>")),
        }
    } else {
        args.to_string()
    };
    let name = clean_title(&name);
    if name.is_empty() {
        return err("The name is empty. Usage: /rename [name]");
    }
    d.engine.transcript().set_title(&name);
    ok(format!("Session renamed to: {name}"))
}

pub(super) fn export(d: &Driver, args: &str) -> Exec {
    let body = render_conversation(&d.engine.state.messages);
    if body.is_empty() {
        return err("Nothing to export yet.");
    }
    let title = d.engine.transcript().title().map(|t| format!(" ({t})")).unwrap_or_default();
    let text = format!(
        "KodAct conversation {}{title}\nExported {}\n\n{body}\n",
        d.info.session_id,
        chrono::Local::now().format("%Y-%m-%d %H:%M")
    );
    if args.is_empty() {
        return ok(text.trim_end());
    }
    let path = kodact_tools::expand_path(args, &d.info.cwd);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match std::fs::write(&path, text) {
        Ok(()) => ok(format!("Conversation exported to: {}", path.display())),
        Err(e) => err(format!("Could not write {}: {e}", path.display())),
    }
}

/// A prompt's first line, shortened, for listings.
fn excerpt(text: &str) -> String {
    let one = text.trim().lines().next().unwrap_or_default();
    if one.chars().count() > 60 {
        format!("{}…", one.chars().take(60).collect::<String>())
    } else {
        one.to_string()
    }
}

fn rel(p: &Path, cwd: &Path) -> String {
    p.strip_prefix(cwd).unwrap_or(p).display().to_string()
}

/// What `/diff` shows, for the text and the screen alike.
struct DiffData {
    /// `Some(diff)` in a git repository (`git diff HEAD`); `None` outside git.
    git: Option<String>,
    untracked: Vec<String>,
    /// Outside git: unified diffs of the files KodAct changed, against how they were.
    own: String,
    /// "1. \"prompt\": a.rs, b.rs", one per prompt that changed files.
    by_prompt: Vec<String>,
}

fn diff_data(d: &Driver) -> DiffData {
    let cwd = &d.info.cwd;
    let history = d.engine.history();
    let (git, untracked) = match kodact_git::uncommitted(cwd) {
        Some((diff, untracked)) => (Some(diff), untracked),
        None => (None, vec![]),
    };
    let mut own = String::new();
    if git.is_none() {
        // No git: diff the files KodAct changed against how they were before.
        for (path, before) in history.originals() {
            let before = before.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            let after = std::fs::read_to_string(&path).unwrap_or_default();
            if before == after {
                continue;
            }
            let name = rel(&path, cwd);
            let udiff = similar::TextDiff::from_lines(&before, &after)
                .unified_diff()
                .header(&format!("a/{name}"), &format!("b/{name}"))
                .to_string();
            own.push_str(&udiff);
        }
    }
    let st = &d.engine.state;
    let by_prompt = history
        .turns()
        .iter()
        .enumerate()
        .map(|(i, (turn, files))| {
            let prompt = st
                .uuids
                .iter()
                .position(|u| u == turn)
                .and_then(|j| st.messages.get(j))
                .map(|m| {
                    m.content
                        .iter()
                        .filter_map(|b| b.as_text())
                        .find(|t| !is_reminder(t))
                        .map(excerpt)
                        .unwrap_or_default()
                })
                .unwrap_or_else(|| "(an earlier prompt)".into());
            let names: Vec<String> = files.iter().map(|f| rel(f, cwd)).collect();
            format!("{}. \"{prompt}\": {}", i + 1, names.join(", "))
        })
        .collect();
    DiffData { git, untracked, own, by_prompt }
}

pub(super) fn diff(d: &Driver) -> Exec {
    let data = diff_data(d);
    let mut out = String::new();
    match &data.git {
        Some(diff) => {
            if diff.is_empty() && data.untracked.is_empty() {
                out.push_str("No uncommitted changes.\n");
            } else {
                if !diff.is_empty() {
                    let _ = writeln!(out, "Uncommitted changes (git diff HEAD):\n{diff}");
                }
                if !data.untracked.is_empty() {
                    let _ = writeln!(out, "\nUntracked files: {}", data.untracked.join(", "));
                }
            }
        }
        None => {
            out.push_str(&data.own);
            if out.is_empty() {
                out.push_str("Not a git repository, and KodAct hasn't changed any files in this session.\n");
            }
        }
    }
    if !data.by_prompt.is_empty() {
        out.push_str("\nFiles KodAct changed, by prompt:\n");
        for line in &data.by_prompt {
            let _ = writeln!(out, "  {line}");
        }
    }
    ok(out.trim_end())
}

/// `/diff` as a screen: the changed files, then each file's hunks.
pub(super) fn diff_screen(d: &Driver) -> super::screens::Screen {
    let data = diff_data(d);
    let (title, unified) = match &data.git {
        Some(diff) => ("Uncommitted changes (git diff HEAD)", diff.as_str()),
        None => ("Files KodAct changed (not a git repository)", data.own.as_str()),
    };
    super::screens::Screen {
        title: title.into(),
        rows: super::screens::diff_rows(unified, &data.untracked, &data.by_prompt),
    }
}

const DEBUG_PROMPT: &str = "Debug logging is on for this session and writes to {log}. The user reports this \
problem:\n\n{issue}\n\nRead the end of that log (it can be long: start with the last few hundred lines, and search \
it for errors and warnings), together with the conversation so far. Explain what went wrong and why, quoting the log \
lines that show it, and suggest a fix or a workaround. If the log doesn't cover the problem, say what to reproduce so \
that it does.";

pub(super) fn debug(d: &mut Driver, args: &str) -> Exec {
    let path = crate::debug::log_path(&d.info.session_id);
    let log = match crate::debug::active() {
        // Logging to the terminal: there is no file to read.
        Some(to) if to == "stderr" => {
            return err("Debug logging goes to stderr (--debug), so KodAct can't read it. Restart with \
                        --debug-file <path>, or without --debug and run /debug again.");
        }
        Some(to) => to,
        None => {
            if let Err(e) = crate::debug::enable(&path) {
                return err(format!("Could not turn on debug logging: {e}"));
            }
            path.display().to_string()
        }
    };
    // Reading this log needs no prompt; other sessions' logs still do.
    d.engine.handle().permissions.write().unwrap().add_read_dir(Path::new(&log));
    if args.is_empty() {
        return ok(format!(
            "Debug logging is on, writing to {log}. Reproduce the problem, then run /debug <what went wrong> and \
             KodAct will read the log."
        ));
    }
    Exec::Submit(MessageContent::Text(DEBUG_PROMPT.replace("{log}", &log).replace("{issue}", args)))
}

const BEHAVIORS: [&str; 3] = ["allow", "ask", "deny"];

/// The settings layer a rule comes from, by its normalized text.
fn rule_source(d: &Driver, behavior: &str, rule: &str) -> String {
    d.info
        .settings
        .layers
        .iter()
        .rev()
        .find(|l| {
            l.value
                .pointer(&format!("/permissions/{behavior}"))
                .and_then(Value::as_array)
                .is_some_and(|a| a.iter().filter_map(Value::as_str).any(|s| normalize_rule(s).as_deref() == Some(rule)))
        })
        .map(|l| l.source.as_str().to_string())
        .unwrap_or_else(|| "session".into())
}

/// Where a rule comes from when it isn't the person's to remove: managed
/// policy, `--settings`, or `--allowedTools` / `--disallowedTools`.
fn fixed_rule_source(d: &Driver, rule: &str) -> Option<&'static str> {
    let in_list = |list: Option<&Value>| {
        list.and_then(Value::as_array)
            .is_some_and(|a| a.iter().any(|v| v.as_str().and_then(normalize_rule).as_deref() == Some(rule)))
    };
    for layer in &d.info.settings.layers {
        let source = match layer.source {
            kodact_config::SettingSource::Managed => "managed policy settings",
            kodact_config::SettingSource::Flag => "--settings",
            _ => continue,
        };
        if BEHAVIORS.iter().any(|b| in_list(layer.value.pointer(&format!("/permissions/{b}")))) {
            return Some(source);
        }
    }
    let flags = d.launch_rules();
    flags
        .iter()
        .flat_map(|s| kodact_permissions::split_rule_list(s))
        .any(|r| normalize_rule(&r).as_deref() == Some(rule))
        .then_some("--allowedTools or --disallowedTools")
}

fn normalize_rule(s: &str) -> Option<String> {
    kodact_permissions::Rule::parse(s.trim()).ok().map(|r| r.to_string())
}

fn permissions_list(d: &Driver) -> String {
    let h = d.engine.handle();
    let p = h.permissions.read().unwrap();
    let mut s = format!("Permission mode: {}\n", p.mode.as_str());
    for behavior in BEHAVIORS {
        let rules = match behavior {
            "allow" => &p.rules.allow,
            "ask" => &p.rules.ask,
            _ => &p.rules.deny,
        };
        let title = format!("{}{}", behavior[..1].to_uppercase(), &behavior[1..]);
        if rules.is_empty() {
            let _ = writeln!(s, "\n{title}: none");
            continue;
        }
        let _ = writeln!(s, "\n{title}:");
        for r in rules {
            let text = r.to_string();
            let _ = writeln!(s, "  {text}  [{}]", rule_source(d, behavior, &text));
        }
    }
    let dirs = d.engine.tool_ctx().working_dirs.read().unwrap().clone();
    let _ = writeln!(s, "\nWorking directories:");
    for dir in dirs {
        let _ = writeln!(s, "  {}", dir.display());
    }
    s.push_str(
        "\nAdd a rule: /permissions add <allow|ask|deny> <rule> [--scope local|project|user|session]\n\
         Remove one: /permissions remove <rule>",
    );
    s
}

/// Split a trailing `--scope <name>` off `args`.
fn take_scope(args: &str) -> Result<(String, Option<String>), String> {
    match args.rsplit_once("--scope") {
        Some((rest, scope)) => {
            let scope = scope.trim();
            if scope.is_empty() || scope.contains(char::is_whitespace) {
                return Err("--scope takes one of local, project, user or session.".into());
            }
            Ok((rest.trim().to_string(), Some(scope.to_string())))
        }
        None => Ok((args.trim().to_string(), None)),
    }
}

fn rules_in(d: &Driver, scope: Scope, behavior: &str) -> Vec<Value> {
    list_in(d, scope, &format!("/permissions/{behavior}"))
}

/// A list in a settings file as it is on disk now (other sessions may have changed it).
fn list_in(d: &Driver, scope: Scope, pointer: &str) -> Vec<Value> {
    let path = scope.path(&d.info);
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.pointer(pointer).and_then(Value::as_array).cloned())
        .unwrap_or_default()
}

pub(super) fn permissions(d: &mut Driver, args: &str) -> Exec {
    let (verb, rest) = args.split_once(char::is_whitespace).unwrap_or((args, ""));
    match verb {
        "" | "list" => ok(permissions_list(d)),
        "add" => {
            let (behavior, rest) = rest.trim().split_once(char::is_whitespace).unwrap_or((rest.trim(), ""));
            if !BEHAVIORS.contains(&behavior) {
                return err("Usage: /permissions add <allow|ask|deny> <rule> [--scope local|project|user|session]");
            }
            let (rule, scope) = match take_scope(rest) {
                Ok(x) => x,
                Err(e) => return err(e),
            };
            let parsed = match kodact_permissions::Rule::parse(&rule) {
                Ok(r) => r,
                Err(e) => return err(format!("Invalid rule {rule:?}: {}", e.why)),
            };
            let text = parsed.to_string();
            let b = match behavior {
                "allow" => kodact_permissions::Behavior::Allow,
                "ask" => kodact_permissions::Behavior::Ask,
                _ => kodact_permissions::Behavior::Deny,
            };
            let scope = match scope.as_deref() {
                None | Some("local") => Some(Scope::Local),
                Some("session") => None,
                Some(s) => match Scope::parse(s) {
                    Some(sc) => Some(sc),
                    None => return err("--scope takes one of local, project, user or session."),
                },
            };
            // Saved first: a rule that couldn't be saved doesn't quietly apply anyway.
            let saved = match scope {
                None => "(This session only.)".to_string(),
                Some(sc) => {
                    let mut list = rules_in(d, sc, behavior);
                    if !list.iter().any(|v| v.as_str().and_then(normalize_rule).as_deref() == Some(text.as_str())) {
                        list.push(json!(text));
                    }
                    match save(d, sc, &["permissions", behavior], Some(Value::Array(list))) {
                        Ok(note) => note,
                        Err(e) => return err(format!("{e}. The rule was not added.")),
                    }
                }
            };
            d.engine.handle().permissions.write().unwrap().add_rule(b, parsed);
            ok(format!("Added {behavior} rule {text}. {saved}"))
        }
        "remove" | "rm" => {
            let rule = rest.trim();
            let Some(text) = normalize_rule(rule) else {
                return err("Usage: /permissions remove <rule>");
            };
            if let Some(source) = fixed_rule_source(d, &text) {
                return err(format!("{text} is set by {source}, which /permissions can't change."));
            }
            let mut removed = vec![];
            {
                let h = d.engine.handle();
                let mut guard = h.permissions.write().unwrap();
                let rules = &mut guard.rules;
                for (name, list) in [("allow", &mut rules.allow), ("ask", &mut rules.ask), ("deny", &mut rules.deny)] {
                    let before = list.len();
                    list.retain(|r| r.to_string() != text);
                    if list.len() != before {
                        removed.push(format!("{name} (session)"));
                    }
                }
                let added = &mut guard.added;
                for list in [&mut added.allow, &mut added.ask, &mut added.deny] {
                    list.retain(|r| r.to_string() != text);
                }
            }
            for sc in [Scope::User, Scope::Project, Scope::Local] {
                for behavior in BEHAVIORS {
                    let list = rules_in(d, sc, behavior);
                    let kept: Vec<Value> = list
                        .iter()
                        .filter(|v| v.as_str().and_then(normalize_rule).as_deref() != Some(text.as_str()))
                        .cloned()
                        .collect();
                    if kept.len() != list.len() {
                        if let Err(e) = save(d, sc, &["permissions", behavior], Some(Value::Array(kept))) {
                            return err(e);
                        }
                        removed.push(format!("{behavior} ({})", sc.path(&d.info).display()));
                    }
                }
            }
            if removed.is_empty() {
                return err(format!("No rule {text} is set. /permissions lists the rules."));
            }
            ok(format!("Removed {text} from: {}.", removed.join(", ")))
        }
        _ => err("Usage: /permissions [list | add <allow|ask|deny> <rule> [--scope ...] | remove <rule>]"),
    }
}

pub(super) fn add_dir(d: &mut Driver, args: &str) -> Exec {
    let (raw, save_it) = match args.strip_suffix("--save") {
        Some(r) => (r.trim(), true),
        None => (args, false),
    };
    if raw.is_empty() {
        return err("Usage: /add-dir <path> [--save]");
    }
    let path = kodact_tools::expand_path(raw, &d.info.cwd);
    let dir: PathBuf = match kodact_platform::path::canonicalize(&path) {
        Ok(p) if p.is_dir() => p,
        Ok(_) => return err(format!("{} is not a directory.", path.display())),
        Err(e) => return err(format!("{}: {e}", path.display())),
    };
    let ctx = d.engine.tool_ctx().clone();
    if ctx.working_dirs.read().unwrap().iter().any(|w| dir.starts_with(w)) {
        return ok(format!("{} is already inside a working directory.", dir.display()));
    }
    ctx.working_dirs.write().unwrap().push(dir.clone());
    d.engine.handle().permissions.write().unwrap().add_directory(&dir);
    d.prompt.env.additional_dirs.push(dir.clone());
    let dirs: Vec<PathBuf> = ctx.working_dirs.read().unwrap().iter().skip(1).cloned().collect();
    d.engine.transcript().append_meta(json!({"additionalDirectories": dirs}));
    d.engine.remind(format!(
        "The user added a working directory: {}. You can read and edit files there too.",
        dir.display()
    ));
    let saved = if save_it {
        let mut list = list_in(d, Scope::Local, "/permissions/additionalDirectories");
        let entry = json!(dir.display().to_string());
        if !list.contains(&entry) {
            list.push(entry);
        }
        save(d, Scope::Local, &["permissions", "additionalDirectories"], Some(Value::Array(list)))
            .unwrap_or_else(|e| format!("Not saved: {e}."))
    } else {
        "(This session only; add --save to keep it.)".into()
    };
    ok(format!("Added working directory {}. {saved}", dir.display()))
}

pub(super) fn goal(d: &mut Driver, args: &str) -> Exec {
    use crate::goal::{is_clear_word, Goal, Status, MAX_CONDITION};
    if args.is_empty() {
        return ok(match &d.goal {
            None => "No goal set. Set one with /goal <condition>.".to_string(),
            Some(g) => {
                let spent = d.engine.state.total_cost_usd - g.cost_at_start;
                let mut s = format!("Goal: {}\nStatus: {}", g.condition, g.status.as_str());
                if let Some(p) = &g.paused {
                    let _ = write!(s, " (paused: {p})");
                }
                let _ = write!(
                    s,
                    "\nChecked {} time(s) · {} · ${spent:.4} spent",
                    g.checks,
                    super::run::duration(g.started.elapsed())
                );
                match (&g.status, &g.last_reason) {
                    (Status::Failed(r), _) | (_, Some(r)) if !r.is_empty() => {
                        let _ = write!(s, "\nLast check: {r}");
                    }
                    _ => {}
                }
                s
            }
        });
    }
    if is_clear_word(args) {
        return match d.goal.take() {
            Some(mut g) if g.is_active() => {
                let c = g.condition.clone();
                g.status = Status::Cleared;
                d.goal = Some(g);
                d.goal_changed();
                d.goal = None;
                ok(format!("Goal cleared: {c}"))
            }
            _ => ok("No goal set."),
        };
    }
    if d.info.settings.bool("/disableAllHooks") == Some(true) {
        return err("/goal is unavailable while disableAllHooks is set: goals run as a check at the end of each turn.");
    }
    if args.chars().count() > MAX_CONDITION {
        return err(format!(
            "The goal is too long ({} characters; the limit is {MAX_CONDITION}).",
            args.chars().count()
        ));
    }
    d.goal = Some(Goal::new(args, d.engine.state.total_cost_usd));
    d.goal_changed();
    Exec::Submit(MessageContent::Text(args.to_string()))
}

const RECAP_PROMPT: &str = "You summarize coding sessions. Given a conversation between a user and a coding agent, \
reply with one line (at most 30 words): what was asked, what has been done, and what is still open. No preamble.";

pub(super) async fn recap(d: &mut Driver) -> Exec {
    let text = crate::goal::evaluator_transcript(&d.engine.state.messages);
    if text.is_empty() {
        return err("Nothing to recap yet.");
    }
    match side_request(d, RECAP_PROMPT, text, 120).await {
        Ok(line) => ok(line.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")),
        Err(e) => err(format!("Could not summarize: {e}")),
    }
}

pub(super) fn plan(d: &mut Driver, args: &str) -> Exec {
    d.engine.handle().set_permission_mode(kodact_permissions::PermissionMode::Plan);
    d.info.init.permission_mode = "plan".into();
    if args.is_empty() {
        return ok("Plan mode on: KodAct will look around and propose a plan before changing anything. Approving the plan ends plan mode.");
    }
    Exec::Submit(MessageContent::Text(args.to_string()))
}
