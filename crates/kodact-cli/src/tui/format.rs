//! Pure formatting, tool-summary, and danger-classification helpers for the TUI.

use std::time::Instant;

use kodact_permissions::Suggestion;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::layout::diff_placeholder;
use super::text::Theme;

/// Under a tool call or a command: its result.
pub(super) const RESULT_MARK: &str = "  └ ";
/// Where a result's further lines start: under the text after [`RESULT_MARK`].
pub(super) const RESULT_INDENT: &str = "    ";

/// naming the model and the endpoint) and a next step with the short technical
/// detail. `None` for anything else, which is shown as it is.
pub fn human_api_error(text: &str, model: &str, endpoint: &str) -> Option<(String, String)> {
    let rest = text.strip_prefix("API Error: ")?;
    // "... Retrying in 6s (attempt 2)." stays as a tail on the headline.
    let (rest, retry) = match rest.find(". Retrying in ") {
        Some(i) => (&rest[..i], Some(rest[i + 2..].trim_end_matches('.').to_lowercase())),
        None => (rest, None),
    };
    let host_of = |url: &str| {
        let u = url.split("://").nth(1).unwrap_or(url);
        let u = u.rsplit('@').next().unwrap_or(u);
        u.split(['/', '?', ':']).next().unwrap_or(u).to_string()
    };
    let url = rest.split_whitespace().find(|w| w.starts_with("http://") || w.starts_with("https://"));
    let host = url.map(|u| host_of(u.trim_end_matches(':'))).filter(|h| !h.is_empty()).unwrap_or_else(|| {
        if endpoint.is_empty() {
            "the endpoint".to_string()
        } else {
            endpoint.to_string()
        }
    });
    let model = if model.is_empty() { "the model" } else { model };
    // The server's own message, from a JSON body when there is one.
    let body = rest.find('{').map(|i| &rest[i..]).unwrap_or("");
    let said = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| {
            ["/detail", "/error/message", "/message", "/error"]
                .iter()
                .find_map(|p| v.pointer(p).and_then(Value::as_str).map(str::to_string))
        })
        .unwrap_or_default();
    let status: Option<u16> =
        rest.strip_prefix("HTTP ").and_then(|r| r.split_whitespace().next()).and_then(|c| c.parse().ok());
    let lower = format!("{rest} {said}").to_lowercase();
    let detail = match (status, said.is_empty()) {
        (Some(c), false) => format!("HTTP {c}: {said}"),
        (Some(c), true) => format!("HTTP {c}"),
        (None, _) => rest.split(':').next().unwrap_or(rest).trim().to_string(),
    };
    let names_model = lower.contains("model not found")
        || lower.contains("does not exist")
        || lower.contains("unknown model")
        || (status == Some(404) && lower.contains("model"));
    let (head, next) = if names_model {
        (format!("{model} isn't available on {host}"), format!("/model to choose another · {detail}"))
    } else if status == Some(404) {
        // A 404 that says nothing of a model is as likely the address: a base URL with a
        // wrong path answers this way, and "/model" would send the person the wrong way.
        (
            format!("{host} has nothing at that address, or no model {model}"),
            format!("check KODACT_OPENAI_BASE_URL (it usually ends in /v1), or /model · {detail}"),
        )
    } else if matches!(status, Some(401) | Some(403)) {
        (format!("{host} didn't accept the key"), format!("check KODACT_OPENAI_API_KEY or KODACT_API_KEY · {detail}"))
    } else if status == Some(429) {
        // Only a failure still being retried says so; the last one says it is over.
        let next = if retry.is_some() {
            format!("KodAct waits and retries · {detail}")
        } else {
            format!("KodAct retried and it still refused: try again in a moment · {detail}")
        };
        (format!("{host} is limiting requests"), next)
    } else if status.is_some_and(|c| c >= 500) {
        (format!("{host} had an error"), format!("try again in a moment · {detail}"))
    } else if lower.contains("could not connect")
        || lower.contains("unreachable")
        || lower.contains("lookup address")
        || lower.contains("tls")
        || lower.contains("timed out")
        || lower.contains("error decoding")
    {
        (format!("Can't reach {host}"), format!("check the network and KODACT_OPENAI_BASE_URL · {detail}"))
    } else {
        let c = status?;
        let why = if said.is_empty() { format!("HTTP {c}") } else { said.clone() };
        (format!("{host} refused the request: {why}"), "/model or /status to check the setup".to_string())
    };
    let head = match retry {
        Some(r) => format!("{head} · {r}"),
        None => head,
    };
    Some((head, next))
}

/// `spans` with paths under `cwd` written from it (`src/calc.py`) and the home
/// folder as `~`, so a step's row shows what matters in the columns it has.
pub fn relative_spans(spans: Vec<Span<'static>>, cwd: &str) -> Vec<Span<'static>> {
    let cwd = cwd.trim_end_matches('/');
    let home = std::env::var("HOME").ok().filter(|h| h.len() > 1).map(|h| h.trim_end_matches('/').to_string());
    spans
        .into_iter()
        .map(|mut s| {
            let mut c = s.content.to_string();
            if cwd.len() > 1 && c.contains(&format!("{cwd}/")) {
                c = c.replace(&format!("{cwd}/"), "");
            }
            if let Some(h) = &home {
                if c.contains(&format!("{h}/")) {
                    c = c.replace(&format!("{h}/"), "~/");
                }
            }
            s.content = c.into();
            s
        })
        .collect()
}

/// A count of tokens in a few characters: `950`, `3.1k`, `1.2M`.
pub(super) fn short_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3).replace(".0k", "k"),
        _ => format!("{:.1}M", n as f64 / 1e6).replace(".0M", "M"),
    }
}

/// `s` in at most `max` characters, cut with `…`.
pub(super) fn fit_words(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Lines a failed result shows from its end.
pub(super) const FAILURE_LINES: usize = 5;

/// A line of test-runner progress only (`....F.s`), which says nothing on its own.
pub(super) fn progress_only(l: &str) -> bool {
    let l = l.trim();
    !l.is_empty() && l.chars().all(|c| matches!(c, '.' | 'F' | 'E' | 's' | 'x' | 'X'))
}

/// A line that names an error, drawn in red in a failure's output.
pub(super) fn names_an_error(l: &str) -> bool {
    let lower = l.to_lowercase();
    ["error", "failed", "fail:", "panicked", "exception", "not found", "denied", "exit code", "traceback"]
        .iter()
        .any(|w| lower.contains(w))
}

/// How a tool call ended, for its row in the transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolEnd {
    Done,
    Failed,
    /// The turn ended before it did (interrupted).
    Stopped,
}

/// What a destructive command would do, in a few words, for the permission
/// question: removing files, rewriting history, wiping a disk or a table.
/// Files outside `cwd` (the project) are named as such.
pub fn danger_of(tool: &str, input: &Value, cwd: &str) -> Option<String> {
    if tool != "Bash" {
        return None;
    }
    let cmd = input["command"].as_str()?;
    let words: Vec<&str> = cmd.split_whitespace().collect();
    let has = |w: &str| words.contains(&w);
    for (i, w) in words.iter().enumerate() {
        if *w == "rm" {
            let args: Vec<&str> =
                words[i + 1..].iter().take_while(|a| !matches!(**a, "&&" | ";" | "||" | "|")).copied().collect();
            let forced =
                args.iter().any(|a| a.starts_with('-') && (a.contains('r') || a.contains('R') || a.contains('f')));
            let paths: Vec<&str> = args.iter().filter(|a| !a.starts_with('-')).copied().collect();
            if forced || !paths.is_empty() {
                let cwd = cwd.trim_end_matches('/');
                let outside = |p: &str| {
                    let rooted = p.starts_with('~') || p.starts_with('/');
                    rooted && !p.starts_with(&format!("{cwd}/")) && p != cwd || p.starts_with("..")
                };
                let out: Vec<&str> = paths.iter().copied().filter(|p| outside(p)).collect();
                if !out.is_empty() && !cwd.is_empty() {
                    return Some(format!("deletes files outside the project: {}", out.join(" ")));
                }
                return Some(format!("deletes files: {}", paths.join(" ")).trim_end_matches(": ").to_string());
            }
        }
    }
    let lower = cmd.to_ascii_lowercase();
    let what = if has("git")
        && (lower.contains("reset --hard") || lower.contains("clean -f") || lower.contains("checkout -- "))
    {
        "discards uncommitted changes"
    } else if has("git") && lower.contains("push") && (lower.contains("--force") || has("-f")) {
        "rewrites the remote branch"
    } else if lower.contains("drop table") || lower.contains("drop database") || lower.contains("truncate table") {
        "deletes data"
    } else if has("mkfs") || words.iter().any(|w| w.starts_with("of=/dev/")) {
        "overwrites a disk"
    } else {
        return None;
    };
    Some(what.to_string())
}

/// Most diff lines shown under an edit; the rest are counted.
pub(super) const DIFF_LINES: usize = 16;

/// A line with nothing but spaces.
pub(super) fn blank(l: &Line) -> bool {
    l.spans.iter().all(|s| s.content.trim().is_empty())
}

/// A checklist row's status word (`ok`, `note`, `FAIL`) and its colour.
pub(super) fn check_mark(t: &Theme, line: &str) -> Option<(&'static str, Style)> {
    [("ok  ", t.success()), ("note", t.warning()), ("FAIL", t.error())]
        .into_iter()
        .find(|(m, _)| line.strip_prefix(m).is_some_and(|r| r.starts_with(' ')))
}

/// How a tool result is shown: a one-line summary that says what happened
/// under its header (`120 lines`, `2 additions, 1 removal`), then, for edits,
/// the changed lines with their numbers; for commands, the first lines of output.
pub fn result_lines(
    t: &Theme,
    tool: &str,
    text: &str,
    is_error: bool,
    structured: Option<&Value>,
) -> Vec<Line<'static>> {
    let mark = || Span::styled(RESULT_MARK, t.hairline());
    let one = |s: String, st: Style| vec![Line::from(vec![mark(), Span::styled(s, st)])];
    let first_line = |text: &str| -> String {
        text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").chars().take(160).collect()
    };
    if is_error {
        // A failure shows its end, where the error is: the last lines, the ones that name
        // the error in red, the rest quiet; what came before is counted.
        let body: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty() && !progress_only(l)).collect();
        if body.is_empty() {
            return one("(no output)".into(), t.error());
        }
        let skip = body.len().saturating_sub(FAILURE_LINES);
        let mut out = vec![];
        if skip > 0 {
            out.push(Line::from(vec![mark(), Span::styled(format!("… {skip} earlier lines"), t.dim())]));
        }
        for l in &body[skip..] {
            let lead = if out.is_empty() { mark() } else { Span::raw(RESULT_INDENT) };
            let st = if names_an_error(l) { t.error() } else { t.dim() };
            out.push(Line::from(vec![lead, Span::styled(l.chars().take(200).collect::<String>(), st)]));
        }
        return out;
    }
    match (tool, structured) {
        ("Read", Some(v)) if v["type"] == "text" => {
            let f = &v["file"];
            let n = f["numLines"].as_u64().unwrap_or(0);
            let total = f["totalLines"].as_u64().unwrap_or(n);
            let msg = if text.contains("is unchanged since you last read") {
                "Unchanged since the last read".to_string()
            } else if n < total {
                format!("{n} of {total} lines")
            } else {
                format!("{n} {}", if n == 1 { "line" } else { "lines" })
            };
            one(msg, t.dim())
        }
        ("Read", Some(v)) => one(format!("{} file", v["type"].as_str().unwrap_or("a")), t.dim()),
        ("TodoWrite", Some(v)) if v["newTodos"].is_array() => todo_checklist(t, &v["newTodos"]),
        ("Edit" | "MultiEdit" | "Write", Some(v)) if v.get("structuredPatch").is_some() => {
            let hunks = v["structuredPatch"].as_array().cloned().unwrap_or_default();
            let lines: Vec<String> = hunks
                .iter()
                .flat_map(|h| h["lines"].as_array().cloned().unwrap_or_default())
                .filter_map(|l| l.as_str().map(str::to_string))
                .collect();
            let added = lines.iter().filter(|l| l.starts_with('+')).count();
            let removed = lines.iter().filter(|l| l.starts_with('-')).count();
            if tool == "Write" && hunks.is_empty() {
                let n = v["content"].as_str().map(|c| c.lines().count()).unwrap_or(0);
                return one(format!("{n} lines, a new file"), t.dim());
            }
            let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
            let mut out = one(format!("{}, {}", plural(added, "addition"), plural(removed, "removal")), t.dim());
            // The changes themselves are laid out when the transcript is, at its width:
            // side by side on a wide terminal, one column on a narrow one.
            out.push(diff_placeholder(t, &hunks));
            out
        }
        ("Bash", _) => {
            // A command that went well: its last lines, where the summary is, without
            // progress dots (`....`); what came before is counted.
            let all: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
            let body: Vec<&str> = all.iter().copied().filter(|l| !progress_only(l)).collect();
            let body = if body.is_empty() { all } else { body };
            if body.is_empty() {
                return one("(no output)".into(), t.dim());
            }
            let skip = body.len().saturating_sub(3);
            let mut out = vec![];
            if skip > 0 {
                out.push(Line::from(vec![mark(), Span::styled(format!("… {skip} earlier lines"), t.dim())]));
            }
            for l in &body[skip..] {
                let lead = if out.is_empty() { mark() } else { Span::raw(RESULT_INDENT) };
                let l: String = l.chars().take(200).collect();
                out.push(Line::from(vec![lead, Span::styled(l, t.dim())]));
            }
            out
        }
        _ => {
            let more = text.lines().count().saturating_sub(1);
            let more = if more > 0 { format!(" (+{more} lines)") } else { String::new() };
            one(format!("{}{more}", first_line(text)), t.dim())
        }
    }
}

/// The mark of the plan's task in progress in the transcript: a circle with its centre filled
/// (`◉`), still, as scrollback is, and one column wide, as the other marks are.
const TODO_ACTIVE_MARK: &str = "◉ ";

/// A TodoWrite call's plan, as a checklist under its header, every task in the plan's order:
/// - done ones with a green `✓` and their text struck through, as a Markdown task list shows
///   them;
/// - the one in progress with a still circle (`◉`), in lime and bold;
/// - the rest open `○`.
pub(super) fn todo_checklist(t: &Theme, todos: &Value) -> Vec<Line<'static>> {
    let items: Vec<&Value> = todos.as_array().map(|a| a.iter().collect()).unwrap_or_default();
    if items.is_empty() {
        return vec![Line::from(vec![Span::styled(RESULT_MARK, t.hairline()), Span::styled("No tasks", t.dim())])];
    }
    items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let lead = if i == 0 { Span::styled(RESULT_MARK, t.hairline()) } else { Span::raw(RESULT_INDENT) };
            let content: String = item["content"].as_str().unwrap_or("").chars().take(200).collect();
            let (mark, mark_style, text_style) = match item["status"].as_str() {
                Some("completed") => ("✓ ", t.success(), t.dim().add_modifier(Modifier::CROSSED_OUT)),
                Some("in_progress") => (TODO_ACTIVE_MARK, t.accent(), t.accent().add_modifier(Modifier::BOLD)),
                _ => ("○ ", t.subtle(), Style::default()),
            };
            Line::from(vec![lead, Span::styled(mark, mark_style), Span::styled(content, text_style)])
        })
        .collect()
}

/// A tool call that has not answered yet.
#[derive(Debug, Clone)]
pub struct Running {
    pub id: String,
    /// The header without its dot, while it runs: `Running cargo test`.
    pub header: Vec<Span<'static>>,
    /// The call, for the header once it is done: `Ran cargo test`.
    pub name: String,
    pub input: Value,
    pub since: Instant,
    /// A sub-agent's latest step (`Reading src/lib.rs`) and how many it has taken.
    pub step: Option<String>,
    pub steps: usize,
    /// Every step a sub-agent has taken, in words, for its tree.
    pub trail: Vec<String>,
}

/// What a tool call does, as a verb: past tense once it is done (`Ran`,
/// `Edited`), present while it runs (`Running`, `Editing`).
pub fn tool_verb(name: &str, done: bool) -> &'static str {
    let (past, now) = match name {
        "Bash" | "BashOutput" => ("Ran", "Running"),
        "Read" | "NotebookRead" => ("Read", "Reading"),
        "Edit" | "MultiEdit" | "NotebookEdit" => ("Edited", "Editing"),
        "Write" => ("Wrote", "Writing"),
        "Grep" => ("Searched", "Searching"),
        "Glob" | "LS" => ("Listed", "Listing"),
        "WebFetch" => ("Fetched", "Fetching"),
        "WebSearch" => ("Searched the web", "Searching the web"),
        "Task" => ("Delegated", "Delegating"),
        "TodoWrite" => ("Updated the plan", "Updating the plan"),
        "Skill" => ("Used skill", "Using skill"),
        _ => ("Called", "Calling"),
    };
    if done {
        past
    } else {
        now
    }
}

/// A tool's name as people read it: `mcp__jira__add_comment` is `jira.add_comment`.
pub fn tool_display_name(name: &str) -> String {
    match name.strip_prefix("mcp__").and_then(|r| r.split_once("__")) {
        Some((server, tool)) => format!("{server}.{tool}"),
        None => name.to_string(),
    }
}

/// A tool call's header: a verb, then what it works on: a path in link
/// colour, a command as written, an outside tool by its name.
pub fn tool_header(t: &Theme, name: &str, input: &Value, done: bool) -> Vec<Span<'static>> {
    // A command with a description reads as its purpose; the command itself follows
    // on its own line (`$ cargo test`), as a terminal would show it.
    if let Some(d) = shell_description(name, input) {
        return vec![Span::styled(d, t.bold())];
    }
    let arg = summarize(name, input);
    let mut spans = vec![Span::styled(tool_verb(name, done).to_string(), t.bold())];
    let builtin = tool_verb(name, true) != "Called";
    if !builtin {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(tool_display_name(name), t.info()));
    }
    if arg.is_empty() {
        return spans;
    }
    spans.push(Span::raw(" "));
    match name {
        "Bash" => spans.push(Span::raw(arg)),
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "LS" | "Glob" | "Grep" | "WebFetch" => {
            spans.push(Span::styled(arg, t.info()));
        }
        _ => spans.push(Span::styled(arg, t.dim())),
    }
    spans
}

/// The width from which the full screen can show the sidebar.
pub const SIDEBAR_MIN_COLS: u16 = 140;

/// A TodoWrite call's items: (status, content).
pub fn todo_items(input: &Value) -> Vec<(String, String)> {
    input["todos"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|t| {
            (t["status"].as_str().unwrap_or("pending").to_string(), t["content"].as_str().unwrap_or("").to_string())
        })
        .filter(|(_, c)| !c.trim().is_empty())
        .collect()
}

/// A TodoWrite call's task in progress, as the spinner says it: its `activeForm`, else its
/// content; `None` when no task is in progress.
pub fn active_task(input: &Value) -> Option<String> {
    let task = input["todos"].as_array()?.iter().find(|t| t["status"] == "in_progress")?;
    [&task["activeForm"], &task["content"]]
        .into_iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

/// The skill a tool call uses, if it is a `Skill` call: `{"skill": "simplify"}` → `simplify`.
pub fn skill_of(name: &str, input: &Value) -> Option<String> {
    let skill = input.get("skill").and_then(Value::as_str).filter(|_| name == "Skill")?;
    Some(skill.trim_start_matches('/').to_string())
}

/// A sub-agent tool: the call that runs another agent.
pub fn is_agent_tool(name: &str) -> bool {
    matches!(name, "Task" | "Agent")
}

/// A sub-agent's steps as a small tree under its row. While it runs, the last three;
/// once done, the first, how many more, and the last two.
pub fn agent_tree(t: &Theme, trail: &[String], running: bool) -> Vec<Line<'static>> {
    let rows: Vec<String> = if running {
        trail[trail.len().saturating_sub(3)..].to_vec()
    } else if trail.len() <= 4 {
        trail.to_vec()
    } else {
        let more = trail.len() - 3;
        vec![trail[0].clone(), format!("… {more} more"), trail[trail.len() - 2].clone(), trail[trail.len() - 1].clone()]
    };
    let n = rows.len();
    rows.into_iter()
        .enumerate()
        .map(|(i, r)| {
            let branch = if i + 1 == n { "└ " } else { "├ " };
            let st = if r.starts_with('…') { t.subtle() } else { t.dim() };
            Line::from(vec![Span::raw("  "), Span::styled(branch, t.hairline()), Span::styled(fit_words(&r, 120), st)])
        })
        .collect()
}

/// The command a shell step runs, when this style draws it in a box (every style but
/// the plain ones).
pub fn shell_box(look: super::look::Look, name: &str, input: &Value) -> Option<String> {
    if look.plain_steps() || !matches!(name, "Bash" | "PowerShell") {
        return None;
    }
    input["command"].as_str().filter(|c| !c.trim().is_empty()).map(str::to_string)
}

/// A duration as a step shows it: `0.4s`, `6.2s`, `1m 05s`.
pub fn seconds(d: std::time::Duration) -> String {
    let s = d.as_secs_f64();
    if s < 60.0 {
        format!("{s:.1}s")
    } else {
        format!("{}m {:02}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

/// A shell command's description, when the model gave one: what the command is for.
pub fn shell_description(name: &str, input: &Value) -> Option<String> {
    if !matches!(name, "Bash" | "PowerShell") {
        return None;
    }
    let d = input["description"].as_str()?.lines().next()?.trim();
    (!d.is_empty() && input["command"].as_str().is_some_and(|c| !c.trim().is_empty())).then(|| fit_words(d, 100))
}

/// The line under a described command's header: `$ cargo test -q`, in the code colour.
pub fn shell_command_line(t: &Theme, input: &Value) -> Line<'static> {
    let cmd = input["command"].as_str().unwrap_or("");
    let first = cmd.lines().next().unwrap_or("").trim_end();
    let more = if cmd.lines().nth(1).is_some() { " …" } else { "" };
    let shown: String = first.chars().take(160).collect();
    let cut = if first.chars().count() > 160 { "…" } else { "" };
    Line::from(vec![
        Span::raw("  "),
        Span::styled("$ ", t.subtle()),
        Span::styled(format!("{shown}{cut}{more}"), t.role(super::palette::CODE_TEXT)),
    ])
}

/// The main argument of a tool call, for one line.
pub fn summarize(tool: &str, input: &Value) -> String {
    let key = match tool {
        "Bash" => "command",
        "Read" | "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => "file_path",
        "Glob" | "Grep" => "pattern",
        "LS" => "path",
        "WebFetch" => "url",
        "WebSearch" => "query",
        "Task" => "description",
        "Skill" => "skill",
        _ => "",
    };
    let v = input.get(key).and_then(Value::as_str).map(str::to_string).or_else(|| match tool {
        "TodoWrite" => input["todos"].as_array().map(|a| {
            let done = a.iter().filter(|t| t["status"] == "completed").count();
            format!("{done} of {} done", a.len())
        }),
        _ => input.as_object().and_then(|o| o.values().find_map(Value::as_str).map(str::to_string)),
    });
    let v = v.unwrap_or_default();
    let one = v.lines().next().unwrap_or("");
    if one.chars().count() > 100 {
        format!("{}…", one.chars().take(100).collect::<String>())
    } else {
        one.to_string()
    }
}

/// The label of the "always" option for a permission suggestion.
pub(super) fn rule_text(s: &Suggestion) -> Option<String> {
    match s {
        Suggestion::AddRules { rules, .. } => {
            Some(format!("Always allow {}", rules.iter().map(|r| r.to_rule_string()).collect::<Vec<_>>().join(", ")))
        }
        Suggestion::AddDirectories { directories, .. } => {
            Some(format!("Allow {} for this session", directories.join(", ")))
        }
        Suggestion::SetMode { mode, .. } if mode == "acceptEdits" => {
            Some("Allow all edits this session (shift+tab)".into())
        }
        Suggestion::SetMode { mode, .. } => Some(format!("Allow, and switch to {mode} mode")),
    }
}

/// What an edit would change, for the permission dialog: removed lines as
/// `  - `, added lines as `  + ` (the dialog colours them), a few of each.
pub(super) fn edit_preview(tool: &str, input: &Value) -> Vec<String> {
    const MAX: usize = 200;
    let side = |text: &str, sign: char| -> Vec<String> {
        let lines: Vec<&str> = text.lines().collect();
        let mut out: Vec<String> = lines.iter().take(MAX).map(|l| format!("  {sign} {l}")).collect();
        if lines.len() > MAX {
            out.push(format!("    … {} more lines", lines.len() - MAX));
        }
        out
    };
    match tool {
        "Edit" => {
            let mut v = side(input["old_string"].as_str().unwrap_or(""), '-');
            v.extend(side(input["new_string"].as_str().unwrap_or(""), '+'));
            v
        }
        "MultiEdit" => input["edits"]
            .as_array()
            .into_iter()
            .flatten()
            .take(20)
            .flat_map(|e| {
                let mut v = side(e["old_string"].as_str().unwrap_or(""), '-');
                v.extend(side(e["new_string"].as_str().unwrap_or(""), '+'));
                v
            })
            .collect(),
        "Write" => side(input["content"].as_str().unwrap_or(""), '+'),
        _ => vec![],
    }
}
