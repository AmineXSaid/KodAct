//! The UI's state: what is being typed, what streams in, the dialog that is
//! open, and the lines waiting to go into the terminal's scrollback. Keys and
//! session events change it; the screen is drawn from it.

use kodact_core::glyphs;
use std::collections::VecDeque;
use std::path::Path;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use kodact_core::commands::picker::{Choice, Pick, Picker};
use kodact_core::commands::screens::{FieldKind, Form, RowAction, Screen};
use kodact_engine::{EngineEvent, NoticeLevel, PermissionAnswer, PermissionPrompt};
use kodact_permissions::{PermissionMode, Suggestion};
use kodact_types::{ApiMessage, ContentBlock, Delta, Role, StreamEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::editor::Editor;
use super::text::{Markdown, Theme};

/// Under a tool call or a command: its result.
const RESULT_MARK: &str = "  └ ";
/// Where a result's further lines start: under the text after [`RESULT_MARK`].
const RESULT_INDENT: &str = "    ";

/// A slash command for the `/` menu.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandInfo {
    pub name: String,
    pub args: String,
    pub description: String,
}

/// What the status line shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusView {
    pub model: String,
    pub mode: String,
    pub cwd: String,
    pub cost: f64,
    /// Some usage came from a model without a known price: `cost` is a lower bound.
    pub cost_unknown: bool,
    /// Context used, in percent of the model's window.
    pub context_pct: Option<u8>,
    /// Some context is in use (so 0% means "under 1%").
    pub context_used: bool,
    /// The model's context window, so the meter can follow a turn's requests.
    pub window: u64,
    /// Tokens in context now, and where automatic compaction runs: the gauge shows the
    /// room left between them.
    pub context_tokens: u64,
    pub compact_at: u64,
    /// The window is KodAct's guess (an unknown model the endpoint gave no size for).
    pub window_guessed: bool,
    /// Tokens this session has used so far (every request's input and output), for
    /// a model without a price.
    pub session_tokens: u64,
}

/// From the session task to the UI.
pub enum UiEvent {
    Engine(EngineEvent),
    /// A question for the person (permission, AskUserQuestion, plan approval).
    Ask {
        prompt: PermissionPrompt,
        reply: oneshot::Sender<PermissionAnswer>,
    },
    /// A local answer (a slash command) or an error to show.
    Reply {
        text: String,
        is_error: bool,
    },
    Status(StatusView),
    Commands(Vec<CommandInfo>),
    /// Every skill's name (at the start, and after `/reload-skills` or a plugin change).
    Skills(Vec<String>),
    /// The project's files, for `@` completion.
    Files(Vec<String>),
    /// The `theme` setting changed (`/theme`).
    Theme(String),
    /// The UI's style (`tuiStyle`): how its pages and lines are laid out.
    Look(String),
    /// Put this text on the clipboard (`/copy`).
    Copy(String),
    /// The `statusLine` command's output, or `None` when there is none.
    StatusLine(Option<String>),
    /// Choices for a command typed without its argument (`/model`, `/resume`, ...).
    Picker(Picker),
    /// A screen for a command typed without arguments (`/diff`, `/context`, `/hooks`, `/agents`).
    Screen(Screen),
    /// The conversation on screen was replaced (`/clear`, `/resume`, `/rewind`, or one
    /// loaded at start): the screen starts over and shows `replay`, the conversation
    /// as it is now. `recent` is the newest other conversations here, for the front
    /// page; `echo` is the command that did it, written again after the replay.
    Reset {
        replay: Vec<kodact_types::Message>,
        recent: Vec<(String, String)>,
        echo: Option<String>,
    },
    /// The session started work by itself (a scheduled task).
    Busy,
    /// The session is ready for the next input.
    Idle,
    /// The session ended (`/exit`).
    Exit,
}

/// An `API Error: ...` notice as a person would say it: a headline (what went wrong,
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
    let (head, next) = if lower.contains("model not found")
        || lower.contains("does not exist")
        || lower.contains("unknown model")
        || status == Some(404)
    {
        (format!("{model} isn't available on {host}"), format!("/model to choose another · {detail}"))
    } else if matches!(status, Some(401) | Some(403)) {
        (format!("{host} didn't accept the key"), format!("check KODACT_OPENAI_API_KEY or KODACT_API_KEY · {detail}"))
    } else if status == Some(429) {
        (format!("{host} is limiting requests"), format!("KodAct waits and retries · {detail}"))
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
fn short_count(n: u64) -> String {
    match n {
        0..=999 => n.to_string(),
        1_000..=999_999 => format!("{:.1}k", n as f64 / 1e3).replace(".0k", "k"),
        _ => format!("{:.1}M", n as f64 / 1e6).replace(".0M", "M"),
    }
}

/// `s` in at most `max` characters, cut with `…`.
fn fit_words(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Lines a failed result shows from its end.
const FAILURE_LINES: usize = 5;

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

/// What the UI loop does for the app.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Send(String),
    /// Ask the session for the picker of this command text (a second step).
    Picker(String),
    Interrupt,
    SetMode(PermissionMode),
    Exit,
}

/// One AskUserQuestion question.
#[derive(Debug, Clone)]
struct Question {
    question: String,
    options: Vec<(String, String)>,
    multi: bool,
}

#[derive(Debug)]
enum DialogKind {
    Permission {
        always: Option<String>,
        /// What a destructive command would do (`deletes files: build/`): the answer
        /// starts on Deny, and only `y` allows it.
        danger: Option<String>,
    },
    Plan,
    Questions {
        questions: Vec<Question>,
        at: usize,
        answers: serde_json::Map<String, Value>,
        picked: Vec<bool>,
    },
    /// Typing filters the rows.
    Picker {
        picker: Picker,
        filter: String,
    },
    /// A screen: scrolling rows, some with an action (docs/TUI.md, "Screens").
    Viewer {
        screen: Screen,
        /// The highlighted row.
        cursor: usize,
        /// The first row shown.
        top: usize,
        /// Rows jumped from, for Esc.
        back: Vec<usize>,
    },
    /// Inputs that fill in a command.
    Form {
        form: Form,
        /// The field being edited.
        at: usize,
    },
}

/// An open question for the person.
pub struct Dialog {
    /// The question being answered (none for a picker).
    prompt: Option<PermissionPrompt>,
    reply: Option<oneshot::Sender<PermissionAnswer>>,
    kind: DialogKind,
    selected: usize,
    /// "Type an answer" in a question.
    typing: Option<Editor>,
}

/// Ctrl+R: a search back through prompt history.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Search {
    pub query: String,
    /// The history entry shown.
    pub hit: Option<usize>,
    /// What was typed before the search, for Esc.
    draft: String,
}

/// The keyboard shortcuts (`?` on an empty prompt, `/keybindings`).
pub const KEYS: &[(&str, &str, Option<&str>)] = &[
    ("Enter", "Send (queued while a turn runs)", Some("submit")),
    ("Shift+Enter, Alt+Enter, Ctrl+J, \\ Enter", "New line", Some("newline")),
    ("Esc", "Interrupt the turn; close a menu; cancel a dialog", Some("cancel")),
    ("Esc Esc", "On an empty prompt: rewind (/rewind)", None),
    ("Ctrl+C", "Clear the input; interrupt; twice on an empty prompt: exit", Some("interrupt")),
    ("Ctrl+D", "Exit (empty prompt)", Some("exit")),
    ("Shift+Tab", "Cycle the permission mode: default, accept edits, plan", Some("cycleMode")),
    ("Up / Down", "Move between lines; earlier prompts", None),
    ("Ctrl+R", "Search earlier prompts", Some("historySearch")),
    ("Tab", "Complete a / command or an @ path", Some("complete")),
    ("@", "Mention a file (a menu of project paths)", None),
    ("!", "At the start: run a shell command", None),
    ("/", "At the start: a command (the menu lists them)", None),
    ("Ctrl+A / Ctrl+E, Home / End", "Start / end of line", None),
    ("Ctrl+W, Alt+Backspace", "Delete the word before the cursor", Some("deleteWord")),
    ("Ctrl+U / Ctrl+K", "Delete to the start / end of the line", None),
    ("Alt+B / Alt+F, Ctrl+Left / Ctrl+Right", "Word left / right", None),
    ("Ctrl+L", "Redraw the screen", Some("redraw")),
    ("Ctrl+B", "Show or hide the sidebar (full screen, 140+ columns)", Some("toggleSidebar")),
];

/// The shortcuts in effect, one per line: the defaults, or with `keybindings.json`
/// the keys each action has now, and the user's own bindings.
pub fn keys_text(keymap: &super::keys::Keymap) -> String {
    let rows: Vec<(String, String)> = KEYS
        .iter()
        .map(|(k, what, action)| {
            let keys = match action {
                Some(a) if !keymap.is_default() => keymap.keys_for(a).join(", "),
                _ => k.to_string(),
            };
            (keys, what.to_string())
        })
        .collect();
    let w = rows.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0);
    let mut lines: Vec<String> = rows.iter().map(|(k, what)| format!("{k:<w$}  {what}")).collect();
    if !keymap.is_default() {
        lines.push(String::new());
        lines.push(format!("Custom bindings from {}.", kodact_config::config_dir().join("keybindings.json").display()));
        let unbound = keymap.unbound();
        if !unbound.is_empty() {
            lines.push(format!("Unbound: {}.", unbound.join(", ")));
        }
    }
    lines.join("\n")
}

const TERMINAL_SETUP: &str = "Shift+Enter starts a new line when the terminal reports it as its own key. KodAct asks \
for this through the keyboard protocol that kitty, WezTerm, foot, Ghostty, Alacritty and iTerm2 (with \
\"Report keys using CSI u\" on) support.\n\nWhere it isn't available, these always start a new line: Alt+Enter \
(Option+Enter on macOS, with \"Use Option as Meta key\" on), Ctrl+J, or \\ then Enter.\n\nInside tmux, add \
`set -s extended-keys on` and `set -as terminal-features 'xterm*:extkeys'` to ~/.tmux.conf.";

/// `/terminal-setup`: whether Shift+Enter works here, and the ways around it.
pub fn terminal_setup_text(keyboard_protocol: bool) -> String {
    let now = if keyboard_protocol {
        "This terminal has the keyboard protocol on: Shift+Enter works."
    } else {
        "This terminal didn't turn the keyboard protocol on, so Shift+Enter may arrive as Enter."
    };
    format!("{now}\n\n{TERMINAL_SETUP}")
}

/// Rows the `@` menu offers at most.
const FILE_MATCHES: usize = 50;

/// Two presses of Esc or Ctrl-C within this make the second one count.
const DOUBLE_PRESS: Duration = Duration::from_millis(800);

pub struct App {
    /// Tool calls of this turn by id (name, input), so each result is shown the way its tool needs.
    calls: std::collections::HashMap<String, (String, Value)>,
    /// Tool calls still running, in call order: their header waits in the live
    /// region with a turning dot, and goes to scrollback with its result.
    pub running: Vec<Running>,
    pub theme: Theme,
    pub editor: Editor,
    pub status: StatusView,
    pub busy: bool,
    pub busy_since: Option<Instant>,
    /// What the session is doing, for the spinner: "Thinking", "Running Bash", ...
    pub activity: String,
    pub queued: VecDeque<String>,
    /// Lines waiting to be written into scrollback.
    pub pending: Vec<Line<'static>>,
    /// The current answer's unfinished last line.
    pub live: String,
    md: Markdown,
    /// The next answer line is the first of its block (it gets the marker).
    first_line: bool,
    /// Text streamed for the current message (its complete copy isn't written again).
    streamed: bool,
    /// The usage the current request reported so far (its start, then its deltas).
    request_usage: kodact_types::Usage,
    /// The current request's reply has thinking in it: its output isn't carried into the next
    /// request (over chat completions), so the live meter leaves it out.
    request_thought: bool,
    /// The model's reasoning as it streams, shown under the prompt while it thinks,
    /// then folded into the transcript when its answer or a step begins.
    pub thought: String,
    /// When the current reasoning began, for "Thought for 4s".
    pub thought_since: Option<Instant>,
    /// When the open question was asked: the wait shown is the person's, not the turn's.
    pub asked_at: Option<Instant>,
    /// The last line already written to the terminal is blank.
    printed_blank: bool,
    pub dialog: Option<Dialog>,
    commands: Vec<CommandInfo>,
    files: Vec<String>,
    pub search: Option<Search>,
    pub menu_selected: usize,
    menu_dismissed: bool,
    last_esc: Option<Instant>,
    last_ctrl_c: Option<Instant>,
    /// A short hint in the status line ("Press Ctrl-C again to exit").
    pub hint: Option<(String, Instant)>,
    pub exit: bool,
    pub dirty: bool,
    /// Colour is allowed at all (not `NO_COLOR` or `--color never`).
    color_ok: bool,
    /// Text for the loop to put on the clipboard.
    pub clipboard: Option<String>,
    /// The `statusLine` command's output.
    pub status_text: Option<String>,
    /// Rows a screen showed last time it was drawn (PageUp/PageDown move by this).
    pub viewer_page: std::cell::Cell<usize>,
    /// Key bindings in effect (`keybindings.json`).
    pub keymap: super::keys::Keymap,
    /// The UI's style.
    pub look: super::look::Look,
    /// Sub-agents that finished this turn, for the line that waits on the rest.
    pub agents_done: usize,
    /// The endpoint said the model isn't there: the session row marks it until a
    /// request gets through or the model changes.
    pub model_down: bool,
    /// The turn so far: model requests, and the spend and tokens when it began.
    turn_requests: usize,
    turn_tokens_from: u64,
    /// The tool call the model is writing now: its name and its input so far.
    preparing: Option<(String, String)>,
    /// Something has been sent this session (the placeholder then asks for a reply).
    pub talked: bool,
    /// Columns the full screen sets the transcript in (0 inline): the live rows follow it.
    pub transcript_indent: usize,
    /// The front screen is showing: nothing sent yet. The input is a centred box under the wordmark.
    pub front: bool,
    /// No earlier conversation in this folder: the welcome shows an example request.
    pub first_run: bool,
    /// The sidebar is open (Ctrl+B, full screen from [`SIDEBAR_MIN_COLS`] columns).
    pub sidebar: bool,
    /// The session's title for the sidebar: its first prompt, on one line.
    pub title: String,
    /// MCP servers and their state: (name, status).
    pub mcp: Vec<(String, String)>,
    /// How many tools each MCP server brings, by server name, for the sidebar.
    pub mcp_tools: std::collections::HashMap<String, usize>,
    /// Every skill's name: the sidebar counts them, and a typed `/name` that is one counts as used.
    pub skills: Vec<String>,
    /// Skills used this session, most recent first, with how many times.
    pub skills_used: Vec<(String, usize)>,
    /// The model's latest plan (TodoWrite): (status, content).
    pub todos: Vec<(String, String)>,
    /// Where requests go (`127.0.0.1:11434`), for the line under the front screen's input.
    pub endpoint: String,
    /// The git branch, for the status style's bar (empty outside a repository).
    pub branch: String,
    /// `/focus`: tool calls and their results stay out of the scrollback.
    pub focus: bool,
    /// A screen or picker waiting for an open question to be answered.
    held: Option<Dialog>,
    /// The conversation on screen was replaced: the loop starts the screen over before it
    /// writes [`App::pending`]. Holds the newest other conversations, for the front page.
    pub reset: Option<Vec<(String, String)>>,
}

/// Most diff lines shown under an edit; the rest are counted.
const DIFF_LINES: usize = 16;

/// A line with nothing but spaces.
fn blank(l: &Line) -> bool {
    l.spans.iter().all(|s| s.content.trim().is_empty())
}

/// A checklist row's status word (`ok`, `note`, `FAIL`) and its colour.
fn check_mark(t: &Theme, line: &str) -> Option<(&'static str, Style)> {
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

/// The start of a placeholder line that stands for a diff until it is laid out.
const DIFF_MARK: &str = "\u{e000}kodact-diff";
/// Width from which a diff is drawn side by side.
pub const SPLIT_DIFF_COLS: u16 = 140;

/// A line that stands for an edit's changes: the styles they are drawn in, and
/// the hunks. [`lay_out`] turns it into rows at the width of the moment.
fn diff_placeholder(t: &Theme, hunks: &[Value]) -> Line<'static> {
    Line::from(vec![
        Span::styled(DIFF_MARK, t.removed()),
        Span::styled("", t.added()),
        Span::styled("", t.dim()),
        Span::styled("", t.hairline()),
        Span::raw(Value::Array(hunks.to_vec()).to_string()),
    ])
}

/// Lines ready to wrap at `width`: each diff placeholder becomes its rows (side by
/// side from [`SPLIT_DIFF_COLS`], else one column), every other line stays as it is.
pub fn lay_out(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let mut out = Vec::with_capacity(lines.len());
    for l in lines {
        match l.spans.first() {
            Some(_) if super::shellbox::is_placeholder(&l) => out.extend(super::shellbox::lay_out(&l, width)),
            Some(s) if s.content == DIFF_MARK && l.spans.len() == 5 => {
                let styles = [l.spans[0].style, l.spans[1].style, l.spans[2].style, l.spans[3].style];
                let hunks: Vec<Value> = serde_json::from_str(&l.spans[4].content).unwrap_or_default();
                out.extend(diff_rows(&hunks, styles, width));
            }
            // A table, anywhere on the line: what comes before it (the answer's mark) leads it.
            _ => match l.spans.iter().position(|s| s.content == super::text::TABLE_MARK) {
                Some(at) => {
                    let lead = l.spans[..at].to_vec();
                    out.extend(super::text::lay_out_table(lead, &l.spans[at..], width));
                }
                None => out.push(l),
            },
        }
    }
    out
}

/// One changed line of a diff: its number in the old or new file, and its text.
struct Changed {
    num: u64,
    body: String,
}

/// An edit's changed lines, as rows: removed and added runs paired side by side when
/// `width` allows, else one column with `-` and `+`. At most [`DIFF_LINES`] rows.
fn diff_rows(hunks: &[Value], [removed, added, quiet, rule]: [Style; 4], width: u16) -> Vec<Line<'static>> {
    // Runs of removals followed by additions, in order.
    let mut runs: Vec<(Vec<Changed>, Vec<Changed>)> = vec![];
    let mut cur: (Vec<Changed>, Vec<Changed>) = (vec![], vec![]);
    for h in hunks {
        let (mut old, mut new) = (h["oldStart"].as_u64().unwrap_or(1), h["newStart"].as_u64().unwrap_or(1));
        for l in h["lines"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            let (sign, body) = l.split_at(l.chars().next().map(char::len_utf8).unwrap_or(0));
            match sign {
                "-" => {
                    if !cur.1.is_empty() {
                        runs.push(std::mem::take(&mut cur));
                    }
                    cur.0.push(Changed { num: old, body: body.to_string() });
                    old += 1;
                }
                "+" => {
                    cur.1.push(Changed { num: new, body: body.to_string() });
                    new += 1;
                }
                _ => {
                    if !cur.0.is_empty() || !cur.1.is_empty() {
                        runs.push(std::mem::take(&mut cur));
                    }
                    old += 1;
                    new += 1;
                }
            }
        }
        if !cur.0.is_empty() || !cur.1.is_empty() {
            runs.push(std::mem::take(&mut cur));
        }
    }
    let changed: usize = runs.iter().map(|(r, a)| r.len() + a.len()).sum();
    let mut out = vec![];
    let mut shown = 0;
    if width >= SPLIT_DIFF_COLS {
        // Side by side: the old line on the left, the new one on the right.
        let half = (width as usize).saturating_sub(RESULT_INDENT.len() + 3) / 2;
        let cell = |c: Option<&Changed>, st: Style| -> Span<'static> {
            let text = match c {
                Some(c) => format!("{:>4}  {}", c.num, c.body),
                None => String::new(),
            };
            let text: String = text.chars().take(half).collect();
            let pad = half.saturating_sub(super::text::width(&text));
            Span::styled(format!("{text}{}", " ".repeat(pad)), st)
        };
        'runs: for (rem, add) in &runs {
            for i in 0..rem.len().max(add.len()) {
                if out.len() == DIFF_LINES {
                    break 'runs;
                }
                shown += usize::from(rem.get(i).is_some()) + usize::from(add.get(i).is_some());
                out.push(Line::from(vec![
                    Span::raw(RESULT_INDENT),
                    cell(rem.get(i), removed),
                    Span::styled(" │ ", rule),
                    cell(add.get(i), added),
                ]));
            }
        }
    } else {
        'one: for (rem, add) in &runs {
            for (sign, c, st) in rem.iter().map(|c| ("-", c, removed)).chain(add.iter().map(|c| ("+", c, added))) {
                if out.len() == DIFF_LINES {
                    break 'one;
                }
                shown += 1;
                out.push(Line::from(vec![
                    Span::raw(RESULT_INDENT),
                    Span::styled(format!("{:>4} {sign} {}", c.num, c.body), st),
                ]));
            }
        }
    }
    if changed > shown {
        out.push(Line::from(Span::styled(format!("{RESULT_INDENT}… {} more changed lines", changed - shown), quiet)));
    }
    out
}

/// The width from which the full screen can show the sidebar.
pub const SIDEBAR_MIN_COLS: u16 = 140;

/// How much of a finished thought the transcript keeps, in characters.
pub const THOUGHT_CHARS: usize = 600;

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
        "TodoWrite" => input["todos"].as_array().map(|a| format!("{} todos", a.len())),
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
fn rule_text(s: &Suggestion) -> Option<String> {
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
fn edit_preview(tool: &str, input: &Value) -> Vec<String> {
    const MAX: usize = 8;
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
            .take(3)
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

const MODES: [PermissionMode; 3] = [PermissionMode::Default, PermissionMode::AcceptEdits, PermissionMode::Plan];

impl App {
    pub fn new(theme: Theme, history: Vec<String>) -> Self {
        App {
            calls: Default::default(),
            running: vec![],
            theme,
            editor: Editor::with_history(history),
            status: StatusView::default(),
            busy: false,
            busy_since: None,
            activity: String::new(),
            queued: VecDeque::new(),
            pending: vec![],
            live: String::new(),
            md: Markdown::default(),
            first_line: true,
            streamed: false,
            request_usage: Default::default(),
            request_thought: false,
            thought: String::new(),
            thought_since: None,
            asked_at: None,
            printed_blank: false,
            dialog: None,
            commands: vec![],
            files: vec![],
            search: None,
            menu_selected: 0,
            menu_dismissed: false,
            last_esc: None,
            last_ctrl_c: None,
            hint: None,
            exit: false,
            dirty: true,
            color_ok: theme.color,
            clipboard: None,
            status_text: None,
            viewer_page: std::cell::Cell::new(10),
            keymap: Default::default(),
            look: Default::default(),
            transcript_indent: 0,
            agents_done: 0,
            model_down: false,
            turn_requests: 0,
            turn_tokens_from: 0,
            preparing: None,
            talked: false,
            front: false,
            first_run: false,
            sidebar: false,
            title: String::new(),
            mcp: vec![],
            mcp_tools: Default::default(),
            skills: vec![],
            skills_used: vec![],
            todos: vec![],
            endpoint: String::new(),
            branch: String::new(),
            focus: false,
            held: None,
            reset: None,
        }
    }

    // ---- scrollback ----

    /// A blank line between items, unless one is already there.
    fn gap(&mut self) {
        let last_blank = match self.pending.last() {
            Some(l) => blank(l),
            None => self.printed_blank,
        };
        if !last_blank {
            self.pending.push(Line::default());
        }
    }

    pub fn take_pending(&mut self) -> Vec<Line<'static>> {
        if let Some(l) = self.pending.last() {
            self.printed_blank = blank(l);
        }
        std::mem::take(&mut self.pending)
    }

    fn commit_text_line(&mut self, raw: &str) {
        let t = self.theme;
        let lines = self.md.push(raw, &t);
        self.commit_answer_lines(lines);
    }

    /// The mark before an answer's first line: KodAct's dot, or two spaces in styles that head
    /// the answer with a time (Ledger) or keep it plain (Glow, Bare).
    fn answer_mark(&self) -> Span<'static> {
        use super::look::Look;
        match self.look {
            Look::Frame | Look::Status | Look::Card => Span::styled(format!("{} ", glyphs::ANSWER), self.theme.dim()),
            _ => Span::raw("  "),
        }
    }

    /// The local time, for the Ledger style's column.
    fn clock() -> String {
        chrono::Local::now().format("%H:%M").to_string()
    }

    /// Answer lines into scrollback: the first of a block gets the answer mark.
    fn commit_answer_lines(&mut self, lines: Vec<Line<'static>>) {
        use super::look::Look;
        let t = self.theme;
        for line in lines {
            // Blank lines before the answer's first words (models often open with "\n\n")
            // would get the answer mark alone on its row.
            if self.first_line && blank(&line) {
                continue;
            }
            let marker = if self.first_line { self.answer_mark() } else { Span::raw("  ") };
            if self.first_line {
                self.gap();
                // Card and Ledger head KodAct's turn.
                match self.look {
                    Look::Card => self.pending.push(Line::from(Span::styled("  kodact", t.accent()))),
                    Look::Ledger => self.pending.push(Line::from(vec![
                        Span::styled(format!("  {}  ", Self::clock()), t.subtle()),
                        Span::styled("kodact", t.accent()),
                    ])),
                    _ => {}
                }
            }
            self.first_line = false;
            let mut line = t.answer(line);
            line.spans.insert(0, marker);
            self.pending.push(line);
        }
    }

    /// The unfinished answer line as it will look in scrollback.
    pub fn live_line(&self) -> Option<Line<'static>> {
        if self.live.is_empty() {
            return None;
        }
        let t = self.theme;
        let mut line = t.answer(self.md.clone().line(&self.live, &t)?);
        let marker = if self.first_line { self.answer_mark() } else { Span::raw("  ") };
        line.spans.insert(0, marker);
        Some(line)
    }

    fn flush_live(&mut self) {
        self.flush_thought();
        if !self.live.is_empty() {
            let rest = std::mem::take(&mut self.live);
            self.commit_text_line(&rest);
        }
        // A table that ended the block is drawn now.
        let t = self.theme;
        let held = self.md.finish(&t);
        self.commit_answer_lines(held);
    }

    /// The reasoning that just ended goes into the transcript, folded: how long it took,
    /// then its words, quiet and in italics, at most [`THOUGHT_CHARS`] of them.
    fn flush_thought(&mut self) {
        let since = self.thought_since.take();
        let text = std::mem::take(&mut self.thought);
        if self.activity == "Thinking" {
            self.activity = "Working".into();
        }
        let text = text.trim();
        if text.is_empty() {
            return;
        }
        let t = self.theme;
        let secs = since.map(|s| s.elapsed().as_secs()).unwrap_or(0).max(1);
        self.gap();
        self.pending.push(Line::from(vec![
            Span::styled("∴ ", t.subtle()),
            Span::styled(format!("Thought for {secs}s"), t.subtle().add_modifier(Modifier::ITALIC)),
        ]));
        let words = t.subtle().add_modifier(Modifier::ITALIC);
        let mut left = THOUGHT_CHARS;
        for para in text.lines().map(str::trim).filter(|l| !l.is_empty()) {
            if left == 0 {
                break;
            }
            let n = para.chars().count();
            let shown: String = if n > left {
                format!("{}…", para.chars().take(left).collect::<String>().trim_end())
            } else {
                para.to_string()
            };
            left = left.saturating_sub(n);
            self.pending.push(Line::from(vec![Span::raw("  "), Span::styled(shown, words)]));
        }
        self.first_line = true;
    }

    /// A skill was used (by the model, a sub-agent, or typed as `/name`): it goes to the
    /// top of the sidebar's list, with how many times it has been used.
    pub fn used_skill(&mut self, name: &str) {
        let name = name.trim().trim_start_matches('/');
        if name.is_empty() {
            return;
        }
        let times = match self.skills_used.iter().position(|(s, _)| s == name) {
            Some(i) => self.skills_used.remove(i).1 + 1,
            None => 1,
        };
        self.skills_used.insert(0, (name.to_string(), times));
    }

    /// Write the person's prompt into scrollback.
    pub fn echo_prompt(&mut self, text: &str) {
        if self.title.is_empty() {
            self.title = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim().to_string();
        }
        // A skill typed as a command (`/simplify the parser`).
        if let Some(word) = text.trim_start().strip_prefix('/').and_then(|r| r.split_whitespace().next()) {
            if self.skills.iter().any(|s| s == word) {
                self.used_skill(word);
            }
        }
        use super::look::Look;
        self.gap();
        let t = self.theme;
        match self.look {
            // A warm band, no mark.
            Look::Glow => {
                let st = if t.color {
                    Style::default().bg(super::palette::WARM_BG.color(t.depth, t.light))
                } else {
                    t.user()
                };
                for l in text.lines() {
                    self.pending.push(Line::from(Span::styled(format!("  {l}  "), st)));
                }
            }
            // A header line, then the words.
            Look::Card => {
                self.pending.push(Line::from(Span::styled(format!("  you · {}", Self::clock()), t.subtle())));
                for l in text.lines() {
                    self.pending.push(Line::from(format!("  {l}")));
                }
            }
            // The time and the voice in a column.
            Look::Ledger => {
                for (i, l) in text.lines().enumerate() {
                    let lead = if i == 0 {
                        vec![
                            Span::styled(format!("  {}  ", Self::clock()), t.subtle()),
                            Span::styled("you    ", t.dim()),
                        ]
                    } else {
                        vec![Span::raw(" ".repeat(16))]
                    };
                    let mut spans = lead;
                    spans.push(Span::raw(l.to_string()));
                    self.pending.push(Line::from(spans));
                }
            }
            // Bold, after a plain mark.
            Look::Bare => {
                for (i, l) in text.lines().enumerate() {
                    let mark = if i == 0 { "› " } else { "  " };
                    self.pending
                        .push(Line::from(vec![Span::styled(mark, t.subtle()), Span::styled(l.to_string(), t.bold())]));
                }
            }
            // The input's own mark, in the colour of the mode the words were sent in, then
            // the words, bright and bold: what was typed reads as typed, with no surface.
            Look::Frame => {
                let mark = if t.color { t.mode(&self.status.mode) } else { t.bold() };
                let words = if t.color {
                    Style::default()
                        .fg(super::palette::PROMPT_FG.color(t.depth, t.light))
                        .add_modifier(ratatui::style::Modifier::BOLD)
                } else {
                    t.bold()
                };
                for (i, l) in text.lines().enumerate() {
                    let lead =
                        if i == 0 { Span::styled(format!("{} ", glyphs::PROMPT), mark) } else { Span::raw("  ") };
                    self.pending.push(Line::from(vec![lead, Span::styled(l.to_string(), words)]));
                }
            }
            Look::Status => {
                let st = t.user();
                // The mark in brand colour on the prompt's own surface.
                let mark_st = if t.color { st.patch(t.accent()) } else { st };
                for (i, l) in text.lines().enumerate() {
                    let marker = if i == 0 { format!("{} ", glyphs::PROMPT) } else { "  ".into() };
                    self.pending
                        .push(Line::from(vec![Span::styled(marker, mark_st), Span::styled(format!("{l} "), st)]));
                }
            }
        }
    }

    fn reply_lines(&mut self, text: &str, is_error: bool) {
        // A checklist (`/doctor`): only its failed rows are errors.
        let checklist = text.lines().next().is_some_and(|l| check_mark(&self.theme, l).is_some());
        let st = if is_error && !checklist { self.theme.error() } else { Style::default() };
        self.gap();
        for (i, l) in text.lines().enumerate() {
            let marker = Span::styled(if i == 0 { RESULT_MARK } else { RESULT_INDENT }, self.theme.hairline());
            let line = match check_mark(&self.theme, l) {
                Some((mark, ms)) => {
                    Line::from(vec![marker, Span::styled(mark, ms), Span::raw(l[mark.len()..].to_string())])
                }
                None => Line::from(vec![marker, Span::styled(l.to_string(), st)]),
            };
            self.pending.push(line);
        }
    }

    /// A tool call's header into scrollback, its dot in the colour of how it ended.
    fn commit_tool(&mut self, r: Running, end: ToolEnd) {
        let failed = end == ToolEnd::Failed;
        use super::look::Look;
        self.gap();
        let t = self.theme;
        let line = match self.look {
            // Plain, quiet lines: `ran cargo test`.
            l if l.plain_steps() => {
                let mut text = tool_verb(&r.name, true).to_lowercase();
                if tool_verb(&r.name, true) == "Called" {
                    text.push(' ');
                    text.push_str(&tool_display_name(&r.name));
                }
                let arg = summarize(&r.name, &r.input);
                if !arg.is_empty() {
                    text.push(' ');
                    text.push_str(&arg);
                }
                let lead = if failed { Span::styled("✗ ", t.error()) } else { Span::raw("  ") };
                Line::from(vec![lead, Span::styled(text, if failed { t.error() } else { t.subtle() })])
            }
            // A ✓ in the brand colour, a ✗ when it failed.
            Look::Glow => {
                let mut spans =
                    vec![if failed { Span::styled("✗ ", t.error()) } else { Span::styled("✓ ", t.accent()) }];
                spans.extend(self.header(&r.name, &r.input, true));
                Line::from(spans)
            }
            // The state as a glyph and, unless it went well, a word: never colour alone.
            _ => {
                let (mark, st) = match end {
                    ToolEnd::Done => ("✓", t.success()),
                    ToolEnd::Failed => ("✗", t.error()),
                    ToolEnd::Stopped => ("■", t.dim()),
                };
                let mut spans = vec![Span::styled(format!("{mark} "), st)];
                spans.extend(self.header(&r.name, &r.input, true));
                match end {
                    ToolEnd::Failed => spans.push(Span::styled(" · failed", t.error())),
                    ToolEnd::Stopped => spans.push(Span::styled(" · stopped", t.dim())),
                    ToolEnd::Done => {}
                }
                Line::from(spans)
            }
        };
        let mut line = line;
        // A command keeps how long it ran; its box below holds the command itself.
        let boxed = shell_box(self.look, &r.name, &r.input).is_some();
        if boxed {
            // Before the word for how it ended: `✗ Run the tests  6.2s · failed`.
            let at = line.spans.iter().rposition(|s| s.content.starts_with(" · ")).unwrap_or(line.spans.len());
            line.spans.insert(at, Span::styled(format!("  {}", seconds(r.since.elapsed())), t.subtle()));
        }
        self.pending.push(line);
        // A sub-agent's work, kept small: its first step, how many more, its last two.
        if !self.look.plain_steps() && is_agent_tool(&r.name) && !r.trail.is_empty() {
            self.pending.extend(agent_tree(&t, &r.trail, false));
        }
        // A described command: the command itself under its purpose, as a terminal shows it.
        if !boxed && !self.look.plain_steps() && shell_description(&r.name, &r.input).is_some() {
            self.pending.push(shell_command_line(&t, &r.input));
        }
    }

    fn notice(&mut self, level: NoticeLevel, text: &str) {
        let t = self.theme;
        // A mark in the level's colour; the words stay readable.
        let (mark, mark_st, st) = match level {
            NoticeLevel::Info => ("·", t.dim(), t.dim()),
            NoticeLevel::Warning => ("▲", t.warning(), Style::default()),
            NoticeLevel::Error => ("✗", t.error(), t.error()),
        };
        self.gap();
        // An API error, rewritten for a person: what went wrong in a line, then what to do.
        let human = human_api_error(text, &self.status.model, &self.endpoint);
        if human.as_ref().is_some_and(|(head, _)| head.contains("isn't available on")) {
            self.model_down = true;
        }
        let text = human.as_ref().map(|(head, _)| head.as_str()).unwrap_or(text);
        // The mark in the column of every other mark (❯, ●, ✓), the words in the text's.
        for (i, l) in text.lines().enumerate() {
            let lead = if i == 0 { Span::styled(format!("{mark} "), mark_st) } else { Span::raw("  ") };
            self.pending.push(Line::from(vec![lead, Span::styled(l.to_string(), st)]));
        }
        if let Some((_, next)) = human {
            self.pending.push(Line::from(vec![Span::raw("  "), Span::styled(next, t.dim())]));
        }
    }

    // ---- session events ----

    pub fn on_event(&mut self, ev: UiEvent) {
        self.dirty = true;
        match ev {
            UiEvent::Engine(e) => self.on_engine(e),
            UiEvent::Ask { prompt, reply } => self.open_dialog(prompt, reply),
            UiEvent::Reply { text, is_error } => {
                if !text.trim().is_empty() {
                    self.reply_lines(&text, is_error);
                }
            }
            UiEvent::Status(s) => {
                if s.model != self.status.model {
                    self.model_down = false;
                }
                self.status = s;
            }
            UiEvent::Commands(c) => self.commands = c,
            UiEvent::Skills(names) => self.skills = names,
            UiEvent::Files(f) => self.files = f,
            UiEvent::Theme(name) => {
                // The session's /color stays.
                let accent = self.theme.accent;
                self.theme = Theme::named(&name, self.color_ok, self.theme.depth);
                self.theme.accent = accent;
            }
            UiEvent::Look(name) => self.look = super::look::Look::named(&name).unwrap_or_default(),
            UiEvent::Copy(text) => self.clipboard = Some(text),
            UiEvent::StatusLine(text) => self.status_text = text,
            UiEvent::Picker(picker) => self.open_picker(picker),
            UiEvent::Screen(screen) => self.open_screen(screen),
            UiEvent::Reset { replay, recent, echo } => self.reset_to(replay, recent, echo),
            UiEvent::Busy => {
                if !self.busy {
                    self.busy = true;
                    self.busy_since = Some(Instant::now());
                    self.activity = "Running a scheduled task".into();
                    self.first_line = true;
                }
            }
            UiEvent::Idle => {
                self.flush_live();
                for r in std::mem::take(&mut self.running) {
                    self.commit_tool(r, ToolEnd::Stopped);
                }
                self.turn_summary();
                self.busy = false;
                self.busy_since = None;
                self.activity.clear();
            }
            UiEvent::Exit => self.exit = true,
        }
    }

    /// Start the screen over with `replay`, the conversation as it is now: nothing
    /// written before stays, and an empty conversation goes back to the front page.
    fn reset_to(&mut self, replay: Vec<kodact_types::Message>, recent: Vec<(String, String)>, echo: Option<String>) {
        self.pending.clear();
        self.printed_blank = true;
        self.running.clear();
        self.calls.clear();
        self.live.clear();
        self.thought.clear();
        self.thought_since = None;
        self.md.reset();
        self.first_line = true;
        self.streamed = false;
        self.todos.clear();
        self.skills_used.clear();
        self.agents_done = 0;
        self.title.clear();
        self.talked = !replay.is_empty();
        self.front = replay.is_empty();
        self.first_run = false;
        self.reset = Some(recent);
        // Tool calls show in a replay even in focus mode, which is for the work under way.
        let focus = std::mem::take(&mut self.focus);
        for m in replay {
            if m.role == Role::User {
                let said: Vec<&str> = m.content.iter().filter_map(ContentBlock::as_text).collect();
                if !said.is_empty() {
                    self.flush_live();
                    self.echo_prompt(&said.join("\n"));
                    self.first_line = true;
                }
                if m.content.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. })) {
                    self.on_engine(EngineEvent::User {
                        message: m,
                        uuid: String::new(),
                        tool_use_result: None,
                        is_meta: false,
                        parent_tool_use_id: None,
                    });
                }
            } else {
                self.on_engine(EngineEvent::Assistant {
                    message: ApiMessage {
                        id: String::new(),
                        kind: "message".into(),
                        role: Role::Assistant,
                        model: String::new(),
                        content: m.content,
                        stop_reason: None,
                        stop_sequence: None,
                        usage: Default::default(),
                    },
                    uuid: String::new(),
                    parent_tool_use_id: None,
                });
            }
        }
        self.flush_live();
        // A call the conversation never got a result for (it was interrupted).
        for r in std::mem::take(&mut self.running) {
            self.commit_tool(r, ToolEnd::Stopped);
        }
        self.focus = focus;
        self.activity.clear();
        if let Some(text) = echo {
            self.echo_prompt(&text);
        }
    }

    /// The next queued message, once the session is idle.
    pub fn next_queued(&mut self) -> Option<String> {
        if self.busy {
            return None;
        }
        let text = self.queued.pop_front()?;
        Some(self.start(text))
    }

    /// A message goes to the session: echo it and mark the session busy.
    fn start(&mut self, text: String) -> String {
        self.echo_prompt(&text);
        self.busy = true;
        self.busy_since = Some(Instant::now());
        self.activity = "Working".into();
        self.first_line = true;
        self.agents_done = 0;
        self.turn_requests = 0;
        self.turn_tokens_from = self.status.session_tokens;
        self.talked = true;
        text
    }

    /// The context meter follows each request of a turn, not only its end.
    fn meter(&mut self) {
        // What the next request carries: the input, and the output unless it was thinking.
        // The engine's own measure replaces this when the turn ends.
        let u = &self.request_usage;
        let input = u.input_tokens + u.cache_read_input_tokens + u.cache_creation_input_tokens;
        let shown = u.output_tokens.saturating_sub(u.reasoning_output_tokens);
        let used = if self.request_thought { input } else { input + shown };
        if used > 0 {
            self.status.context_tokens = used;
        }
        if used > 0 && self.status.window > 0 {
            self.status.context_pct = Some((used * 100 / self.status.window).min(100) as u8);
            self.status.context_used = true;
        }
    }

    fn on_engine(&mut self, e: EngineEvent) {
        match e {
            // Sub-agents' own messages stay in their transcripts; their steps show on the
            // row of the call that runs them, so delegated work is never a silent wait.
            EngineEvent::Assistant { message, parent_tool_use_id: Some(parent), .. } => {
                if let Some(r) = self.running.iter_mut().find(|r| r.id == parent) {
                    for b in &message.content {
                        if let ContentBlock::ToolUse { name, input, .. } = b {
                            let header = relative_spans(tool_header(&self.theme, name, input, false), &self.status.cwd);
                            let words: String = header.iter().map(|s| s.content.as_ref()).collect();
                            r.step = Some(words.clone());
                            r.steps += 1;
                            r.trail.push(words);
                        }
                    }
                }
                // A skill a sub-agent used is used in this session too.
                for b in &message.content {
                    if let ContentBlock::ToolUse { name, input, .. } = b {
                        if let Some(skill) = skill_of(name, input) {
                            self.used_skill(&skill);
                        }
                    }
                }
            }
            EngineEvent::Stream { parent_tool_use_id: Some(_), .. }
            | EngineEvent::User { parent_tool_use_id: Some(_), .. } => {}
            EngineEvent::Stream { event, .. } => match event {
                StreamEvent::MessageStart { message } => {
                    self.streamed = false;
                    self.request_thought = false;
                    self.model_down = false;
                    self.turn_requests += 1;
                    self.request_usage = message.usage;
                    self.meter();
                }
                StreamEvent::MessageDelta { usage, .. } => {
                    // Deltas carry the output so far, and the input when the endpoint sends it late.
                    let u = &mut self.request_usage;
                    if usage.input_tokens + usage.cache_read_input_tokens + usage.cache_creation_input_tokens > 0 {
                        u.input_tokens = usage.input_tokens;
                        u.cache_read_input_tokens = usage.cache_read_input_tokens;
                        u.cache_creation_input_tokens = usage.cache_creation_input_tokens;
                    }
                    u.output_tokens = u.output_tokens.max(usage.output_tokens);
                    u.reasoning_output_tokens = u.reasoning_output_tokens.max(usage.reasoning_output_tokens);
                    self.meter();
                }
                StreamEvent::ContentBlockStart { content_block: ContentBlock::Text { .. }, .. } => {
                    self.flush_live();
                    self.first_line = true;
                    self.md.reset();
                    self.activity = "Writing".into();
                }
                StreamEvent::ContentBlockDelta { delta: Delta::TextDelta { text }, .. } => {
                    self.streamed = true;
                    self.live.push_str(&text);
                    // The answer's blank line goes above its first words, before its first line ends.
                    if self.first_line && !self.live.trim().is_empty() {
                        self.gap();
                    }
                    while let Some(i) = self.live.find('\n') {
                        let line: String = self.live[..i].to_string();
                        self.live.drain(..=i);
                        self.commit_text_line(&line);
                    }
                }
                // A tool call the model is still writing: say what it is preparing, and
                // for which file once its path has arrived, so a long Write or a big edit
                // reads as work in progress, not as a stall.
                StreamEvent::ContentBlockStart { content_block: ContentBlock::ToolUse { name, .. }, .. } => {
                    self.flush_live();
                    self.activity = Self::preparing(&name, None);
                    self.preparing = Some((name, String::new()));
                }
                StreamEvent::ContentBlockDelta { delta: Delta::InputJsonDelta { partial_json }, .. } => {
                    if let Some((name, json)) = self.preparing.as_mut() {
                        json.push_str(&partial_json);
                        let target = ["file_path", "notebook_path", "path"]
                            .iter()
                            .find_map(|k| Self::json_string_value(json, k));
                        self.activity = Self::preparing(name, target.as_deref());
                    }
                }
                StreamEvent::ContentBlockDelta { delta: Delta::ThinkingDelta { thinking }, .. } => {
                    self.thought_since.get_or_insert_with(Instant::now);
                    self.thought.push_str(&thinking);
                    self.activity = "Thinking".into();
                    self.request_thought = true;
                }
                StreamEvent::ContentBlockStop { .. } | StreamEvent::MessageStop => {
                    self.preparing = None;
                    self.flush_live()
                }
                _ => {}
            },
            EngineEvent::Assistant { message, .. } => {
                self.flush_live();
                for b in &message.content {
                    // The model's plan and the skills it uses, for the sidebar.
                    if let ContentBlock::ToolUse { name, input, .. } = b {
                        if name == "TodoWrite" {
                            self.todos = todo_items(input);
                        }
                        if let Some(skill) = skill_of(name, input) {
                            self.used_skill(&skill);
                        }
                    }
                    match b {
                        ContentBlock::Text { text, .. } if !self.streamed && !text.trim().is_empty() => {
                            self.first_line = true;
                            self.md.reset();
                            for l in text.lines() {
                                self.commit_text_line(l);
                            }
                        }
                        ContentBlock::ToolUse { name, .. } if self.focus => self.activity = format!("Running {name}"),
                        ContentBlock::ToolUse { id, name, input, .. } => {
                            self.calls.insert(id.clone(), (name.clone(), input.clone()));
                            let header = self.header(name, input, false);
                            self.running.push(Running {
                                id: id.clone(),
                                header,
                                name: name.clone(),
                                input: input.clone(),
                                since: Instant::now(),
                                step: None,
                                steps: 0,
                                trail: Vec::new(),
                            });
                            // In words: "Running pytest -q", "Reading src/lib.rs".
                            let words: String =
                                self.header(name, input, false).iter().map(|s| s.content.as_ref()).collect();
                            self.activity = fit_words(&words, 48);
                        }
                        _ => {}
                    }
                }
                self.streamed = false;
            }
            EngineEvent::User { message, is_meta: false, tool_use_result, .. } => {
                if self.focus {
                    self.activity = "Working".into();
                    return;
                }
                for b in &message.content {
                    if let ContentBlock::ToolResult { tool_use_id, content, is_error, .. } = b {
                        let (tool, input) = self.calls.remove(tool_use_id).unwrap_or_default();
                        let failed = *is_error == Some(true);
                        if matches!(tool.as_str(), "Task" | "Agent") {
                            self.agents_done += 1;
                        }
                        if let Some(i) = self.running.iter().position(|r| r.id == *tool_use_id) {
                            let r = self.running.remove(i);
                            self.commit_tool(r, if failed { ToolEnd::Failed } else { ToolEnd::Done });
                        }
                        // A shell command: its command and output in one box, laid out at the
                        // width of the moment.
                        let shell = shell_box(self.look, &tool, &input);
                        let lines = if let Some(cmd) = shell {
                            let (code, output) = super::shellbox::split_exit(&content.to_text());
                            let state = super::shellbox::State::Ended { code, failed };
                            vec![super::shellbox::placeholder(&self.theme, &cmd, &output, state)]
                        } else {
                            result_lines(
                                &self.theme,
                                &tool,
                                &content.to_text(),
                                *is_error == Some(true),
                                tool_use_result.as_ref(),
                            )
                        };
                        // Plain styles drop the result's `└` and keep it quiet, under the step.
                        let lines = if self.look.plain_steps() {
                            lines
                                .into_iter()
                                .map(|mut l| {
                                    if let Some(first) = l.spans.first_mut() {
                                        if first.content == RESULT_MARK {
                                            *first = Span::raw(RESULT_INDENT);
                                        }
                                    }
                                    l
                                })
                                .collect()
                        } else {
                            lines
                        };
                        self.pending.extend(lines);
                        self.activity = "Working".into();
                    }
                }
            }
            EngineEvent::PromptAccepted { message, .. } => {
                let names: Vec<String> = message
                    .content
                    .iter()
                    .filter_map(ContentBlock::as_text)
                    .flat_map(kodact_agents::attach::attached_paths)
                    .map(|p| Path::new(&p).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or(p))
                    .collect();
                if !names.is_empty() {
                    let line = format!("  (attached: {})", names.join(", "));
                    self.pending.push(Line::from(Span::styled(line, self.theme.dim())));
                }
            }
            EngineEvent::Notice { level, text } => self.notice(level, &text),
            EngineEvent::System { subtype, data } => match subtype.as_str() {
                "compact_boundary" => self.notice(NoticeLevel::Info, "Conversation compacted."),
                "model_fallback" => self.notice(
                    NoticeLevel::Warning,
                    &format!(
                        "Switched to {} for this turn.",
                        data.get("to").and_then(Value::as_str).unwrap_or("another model")
                    ),
                ),
                _ => {}
            },
            EngineEvent::ToolProgress { tool_name, elapsed_secs, .. } => {
                self.activity = format!("Running {tool_name} ({}s)", elapsed_secs as u64);
            }
            _ => {}
        }
    }

    // ---- dialogs ----

    fn open_dialog(&mut self, prompt: PermissionPrompt, reply: oneshot::Sender<PermissionAnswer>) {
        self.flush_live();
        let kind = match prompt.tool_name.as_str() {
            "AskUserQuestion" => {
                let questions: Vec<Question> = prompt.input["questions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|q| Question {
                        question: q["question"].as_str().unwrap_or("").to_string(),
                        options: q["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .map(|o| {
                                (
                                    o["label"].as_str().unwrap_or("").to_string(),
                                    o["description"].as_str().unwrap_or("").to_string(),
                                )
                            })
                            .collect(),
                        multi: q["multiSelect"].as_bool().unwrap_or(false),
                    })
                    .collect();
                let picked = vec![false; questions.first().map(|q| q.options.len()).unwrap_or(0)];
                DialogKind::Questions { questions, at: 0, answers: serde_json::Map::new(), picked }
            }
            "ExitPlanMode" => {
                // The plan goes into scrollback, so the dialog stays small.
                self.gap();
                self.pending.push(Line::from(Span::styled("Here is the plan:", self.theme.bold())));
                let plan = prompt.input["plan"].as_str().unwrap_or("").to_string();
                let mut md = Markdown::default();
                let mut lines: Vec<Line<'static>> = plan.lines().flat_map(|l| md.push(l, &self.theme)).collect();
                lines.extend(md.finish(&self.theme));
                for mut line in lines {
                    line.spans.insert(0, Span::raw("  "));
                    self.pending.push(line);
                }
                DialogKind::Plan
            }
            _ => DialogKind::Permission {
                always: prompt.suggestions.first().and_then(rule_text),
                danger: danger_of(&prompt.tool_name, &prompt.input, &self.status.cwd),
            },
        };
        // A destructive command starts on Deny, so a stray Enter never runs it.
        let selected = match &kind {
            DialogKind::Permission { always, danger: Some(_) } => 1 + usize::from(always.is_some()),
            _ => 0,
        };
        self.dialog = Some(Dialog { prompt: Some(prompt), reply: Some(reply), kind, selected, typing: None });
        self.asked_at = Some(Instant::now());
    }

    fn answer(&mut self, a: PermissionAnswer) {
        self.asked_at = None;
        // The step's time counts from its answer: the wait for you isn't its run.
        let asked = self.asking_for().map(str::to_string);
        for r in self.running.iter_mut().filter(|r| Some(&r.id) == asked.as_ref()) {
            r.since = Instant::now();
        }
        if let Some(mut d) = self.dialog.take() {
            if let Some(tx) = d.reply.take() {
                let _ = tx.send(a);
            }
        }
        // A screen or picker that arrived while the question was open shows now.
        self.dialog = self.held.take();
    }

    /// Show a screen or picker, unless a question is waiting for its answer: replacing that
    /// dialog would drop its reply, which the engine reads as "the UI closed" (deny and
    /// interrupt). It is held until the question is answered.
    fn show(&mut self, d: Dialog) {
        if self.dialog.as_ref().is_some_and(|open| open.reply.is_some()) {
            self.held = Some(d);
        } else {
            self.dialog = Some(d);
        }
    }

    /// The rows of the open dialog: (label, description).
    pub fn dialog_options(&self) -> Vec<(String, String)> {
        let Some(d) = &self.dialog else { return vec![] };
        match &d.kind {
            DialogKind::Permission { always, .. } => {
                let mut v = vec![("Allow once".to_string(), "y".to_string())];
                if let Some(r) = always {
                    v.push((r.clone(), "a".to_string()));
                }
                v.push(("Deny, and tell KodAct what to do".to_string(), "n".to_string()));
                v
            }
            DialogKind::Plan => vec![
                ("Yes, and accept edits without asking".into(), String::new()),
                ("Yes, and ask before each edit".into(), String::new()),
                ("No, keep planning".into(), "esc".into()),
            ],
            DialogKind::Questions { questions, at, picked, .. } => {
                let q = &questions[*at];
                let mut v: Vec<(String, String)> = q
                    .options
                    .iter()
                    .enumerate()
                    .map(|(i, (l, desc))| {
                        let label =
                            if q.multi { format!("[{}] {l}", if picked[i] { "x" } else { " " }) } else { l.clone() };
                        (label, desc.clone())
                    })
                    .collect();
                v.push(("Type an answer".into(), String::new()));
                v
            }
            DialogKind::Viewer { .. } | DialogKind::Form { .. } => vec![],
            DialogKind::Picker { .. } => self
                .picker_rows()
                .into_iter()
                .map(|c| (if c.current { format!("{} ✔", c.label) } else { c.label.clone() }, c.detail.clone()))
                .collect(),
        }
    }

    /// A question to the person is open (permission, plan approval, AskUserQuestion):
    /// the turn waits on them, not on the model.
    /// After a turn that reached the model, quietly under it: the mode it ran in
    /// (its square and name in the mode's colour), the model, how long it took and
    /// the tokens it used: `■ Default · opus · 12s · 1.2k tokens`. No money: `/cost`
    /// gives that when asked. In plan mode, how to go on.
    fn turn_summary(&mut self) {
        let (Some(since), true) = (self.busy_since, self.turn_requests > 0) else { return };
        let t = self.theme;
        let secs = since.elapsed().as_secs();
        let mode = t.mode(&self.status.mode);
        let sep = || Span::styled(" · ", t.subtle());
        let mut spans = vec![
            Span::raw("  "),
            Span::styled(if t.color { "■ " } else { "" }, mode),
            Span::styled(super::render::mode_name(&self.status.mode), mode),
        ];
        if !self.status.model.is_empty() {
            spans.extend([sep(), Span::styled(self.status.model.clone(), t.dim())]);
        }
        spans.extend([sep(), Span::styled(format!("{secs}s"), t.subtle())]);
        let n = self.status.session_tokens.saturating_sub(self.turn_tokens_from);
        if n > 0 {
            spans.extend([sep(), Span::styled(format!("{} tokens", short_count(n)), t.subtle())]);
        }
        self.pending.push(Line::from(spans));
        if self.status.mode == "plan" && self.dialog.is_none() {
            self.gap();
            self.pending.push(Line::from(vec![
                Span::styled("◆ ", t.teal()),
                Span::styled("Plan ready", t.bold()),
                Span::styled(" · shift+tab to leave plan mode and run it · type to revise", t.dim()),
            ]));
        }
    }

    /// What the model is preparing while it writes a tool call, in words:
    /// `Preparing an edit to cart.py`, `Writing notes.md`, `Preparing a command`.
    pub fn preparing(name: &str, target: Option<&str>) -> String {
        let file = target.map(|p| p.rsplit(['/', '\\']).next().unwrap_or(p).to_string()).filter(|f| !f.is_empty());
        match (name, file) {
            ("Edit" | "MultiEdit", Some(f)) => format!("Preparing an edit to {f}"),
            ("Edit" | "MultiEdit", None) => "Preparing an edit".into(),
            ("Write", Some(f)) => format!("Writing {f}"),
            ("Write", None) => "Writing a file".into(),
            ("NotebookEdit", Some(f)) => format!("Preparing an edit to {f}"),
            ("Bash" | "PowerShell", _) => "Preparing a command".into(),
            ("Task" | "Agent", _) => "Briefing an agent".into(),
            ("TodoWrite", _) => "Updating the plan".into(),
            _ => "Preparing the next step".into(),
        }
    }

    /// The value of string field `key` in a JSON object still being streamed, once
    /// its closing quote has arrived (escapes kept simple: `\"` and `\\`).
    pub fn json_string_value(json: &str, key: &str) -> Option<String> {
        let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
        let rest = json[at..].trim_start().strip_prefix(':')?.trim_start().strip_prefix('"')?;
        let mut out = String::new();
        let mut chars = rest.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => out.push(chars.next()?),
                c => out.push(c),
            }
        }
        None
    }

    /// A tool call's header with its paths relative to the session's folder.
    fn header(&self, name: &str, input: &Value, done: bool) -> Vec<Span<'static>> {
        // A boxed command without a description: the box shows the command, so the
        // header names only the program it runs (`Ran cargo`).
        if let Some(cmd) = shell_box(self.look, name, input).filter(|_| shell_description(name, input).is_none()) {
            let program = cmd.split_whitespace().find(|w| !w.contains('=')).unwrap_or("").to_string();
            return vec![
                Span::styled(tool_verb(name, done).to_string(), self.theme.bold()),
                Span::raw(" "),
                Span::styled(program, self.theme.role(super::palette::SUCCESS)),
            ];
        }
        relative_spans(tool_header(&self.theme, name, input, done), &self.status.cwd)
    }

    /// The destructive effect of the command the open question is about, if any.
    pub fn dialog_danger(&self) -> Option<&str> {
        match &self.dialog.as_ref()?.kind {
            DialogKind::Permission { danger, .. } => danger.as_deref(),
            _ => None,
        }
    }

    /// The tool call the open question is about.
    pub fn asking_for(&self) -> Option<&str> {
        let d = self.dialog.as_ref()?;
        matches!(d.kind, DialogKind::Permission { .. }).then(|| d.prompt.as_ref().map(|p| p.tool_use_id.as_str()))?
    }

    pub fn waiting_for_answer(&self) -> bool {
        self.dialog.as_ref().is_some_and(|d| {
            matches!(d.kind, DialogKind::Permission { .. } | DialogKind::Plan | DialogKind::Questions { .. })
        })
    }

    /// The open picker's rows that match its filter.
    fn picker_rows(&self) -> Vec<&kodact_core::commands::picker::Choice> {
        let Some(Dialog { kind: DialogKind::Picker { picker, filter }, .. }) = &self.dialog else { return vec![] };
        let f = filter.to_lowercase();
        picker
            .choices
            .iter()
            .filter(|c| f.is_empty() || c.label.to_lowercase().contains(&f) || c.detail.to_lowercase().contains(&f))
            .collect()
    }

    /// The dialog's title and body lines.
    pub fn dialog_text(&self) -> (String, Vec<String>) {
        let Some(d) = &self.dialog else { return (String::new(), vec![]) };
        match &d.kind {
            DialogKind::Picker { picker, filter } => {
                let body = if filter.is_empty() {
                    vec!["❯ type to filter".to_string()]
                } else {
                    vec![format!("❯ {filter}▏")]
                };
                (picker.title.clone(), body)
            }
            DialogKind::Permission { .. } => {
                let Some(p) = &d.prompt else { return (String::new(), vec![]) };
                let arg = summarize(&p.tool_name, &p.input);
                let mut body = vec![];
                if p.tool_name == "Bash" {
                    body.extend(p.input["command"].as_str().unwrap_or("").lines().take(6).map(|l| format!("  {l}")));
                    // Where it runs: a relative path means nothing without it.
                    if !self.status.cwd.is_empty() {
                        body.push(format!("  in {}", super::short_path(&self.status.cwd)));
                    }
                    if let Some(d) = p.input["description"].as_str().filter(|d| !d.trim().is_empty()) {
                        body.push(format!("  {d}"));
                    }
                } else if tool_verb(&p.tool_name, true) == "Called" {
                    // An outside tool: everything it will be sent, so nothing is approved unseen.
                    let fields = p.input.as_object().cloned().unwrap_or_default();
                    let pad = fields.keys().map(|k| k.chars().count()).max().unwrap_or(0).min(16);
                    for (k, v) in fields.iter().take(16) {
                        let v = match v {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        for (i, l) in v.lines().enumerate().take(12) {
                            let key = if i == 0 { k.as_str() } else { "" };
                            body.push(format!("  {key:<pad$}  {l}"));
                        }
                    }
                } else if !arg.is_empty() {
                    body.push(format!("  {arg}"));
                }
                body.extend(edit_preview(&p.tool_name, &p.input));
                // The generic reason says nothing the title doesn't.
                if !p.reason.is_empty() && p.reason != "this tool requires permission" {
                    body.push(p.reason.clone());
                }
                let title = if p.tool_name == "Bash" {
                    "Run this command?".to_string()
                } else {
                    format!("Approve {}?", tool_display_name(&p.tool_name))
                };
                (title, body)
            }
            DialogKind::Plan => ("Go ahead with this plan?".into(), vec![]),
            DialogKind::Viewer { screen, .. } => (screen.title.clone(), vec![]),
            DialogKind::Form { form, .. } => (form.title.clone(), vec![]),
            DialogKind::Questions { questions, at, .. } => {
                let q = &questions[*at];
                let n = questions.len();
                let title = if n > 1 { format!("{} ({}/{n})", q.question, at + 1) } else { q.question.clone() };
                let mut body = vec![];
                if let Some(e) = &d.typing {
                    body.push(format!("> {}", e.text()));
                }
                (title, body)
            }
        }
    }

    pub fn dialog_selected(&self) -> usize {
        self.dialog.as_ref().map(|d| d.selected).unwrap_or(0)
    }

    fn dialog_key(&mut self, key: KeyEvent) -> Vec<Action> {
        if matches!(self.dialog.as_ref().map(|d| &d.kind), Some(DialogKind::Viewer { .. })) {
            return self.viewer_key(key);
        }
        if matches!(self.dialog.as_ref().map(|d| &d.kind), Some(DialogKind::Form { .. })) {
            return self.form_key(key);
        }
        let n = self.dialog_options().len();
        let Some(d) = self.dialog.as_mut() else { return vec![] };
        if let DialogKind::Picker { filter, .. } = &mut d.kind {
            match key.code {
                KeyCode::Up => d.selected = d.selected.checked_sub(1).unwrap_or(n.saturating_sub(1)),
                KeyCode::Down | KeyCode::Tab => d.selected = (d.selected + 1) % n.max(1),
                KeyCode::Enter if n > 0 => {
                    let i = d.selected.min(n - 1);
                    return self.choose(i);
                }
                KeyCode::Esc => self.dialog = None,
                KeyCode::Backspace => {
                    filter.pop();
                    d.selected = 0;
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    filter.push(c);
                    d.selected = 0;
                }
                _ => {}
            }
            return vec![];
        }
        // Typing a free-form answer.
        if let Some(e) = d.typing.as_mut() {
            match key.code {
                KeyCode::Enter => {
                    let text = e.text();
                    d.typing = None;
                    if !text.trim().is_empty() {
                        self.answer_question(text);
                    }
                }
                KeyCode::Esc => d.typing = None,
                KeyCode::Backspace => e.backspace(),
                KeyCode::Left => e.left(),
                KeyCode::Right => e.right(),
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => e.insert(&c.to_string()),
                _ => {}
            }
            return vec![];
        }
        if let DialogKind::Permission { always, danger } = &d.kind {
            // A destructive command is allowed with `y` only: Enter on Allow just says so.
            if danger.is_some() && key.code == KeyCode::Enter && d.selected + 1 < n {
                self.hint = Some(("Press y to allow it; Enter or n denies".into(), Instant::now()));
                return vec![];
            }
            let pick = match key.code {
                KeyCode::Char('y') => Some(0),
                KeyCode::Char('a') if always.is_some() => Some(1),
                KeyCode::Char('n') => Some(n.saturating_sub(1)),
                _ => None,
            };
            if let Some(i) = pick.filter(|_| !key.modifiers.contains(KeyModifiers::CONTROL)) {
                d.selected = i;
                return self.choose(i);
            }
        }
        match key.code {
            KeyCode::Up => d.selected = d.selected.checked_sub(1).unwrap_or(n.saturating_sub(1)),
            KeyCode::Down | KeyCode::Tab => d.selected = (d.selected + 1) % n.max(1),
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                let i = c as usize - '1' as usize;
                if i < n {
                    d.selected = i;
                    return self.choose(i);
                }
            }
            KeyCode::Char(' ') => {
                if let DialogKind::Questions { questions, at, picked, .. } = &mut d.kind {
                    if questions[*at].multi && d.selected < picked.len() {
                        picked[d.selected] = !picked[d.selected];
                    }
                }
            }
            KeyCode::Enter => {
                let i = d.selected;
                return self.choose(i);
            }
            KeyCode::Esc => return self.choose(n.saturating_sub(1)),
            _ => {}
        }
        vec![]
    }

    fn choose(&mut self, i: usize) -> Vec<Action> {
        let n = self.dialog_options().len();
        let Some(d) = self.dialog.as_mut() else { return vec![] };
        let allow = |updated_permissions| PermissionAnswer::Allow { updated_input: None, updated_permissions };
        let stop = |message: &str| PermissionAnswer::Deny { message: message.to_string(), interrupt: true };
        match &mut d.kind {
            DialogKind::Picker { .. } => {
                let Some(pick) = self.picker_rows().get(i).map(|c| c.pick.clone()) else { return vec![] };
                self.dialog = None;
                return match pick {
                    Pick::Run(text) => self.submit(text),
                    Pick::RunThenEdit { command, edit } => {
                        let a = self.submit(command);
                        self.editor.set(&edit);
                        a
                    }
                    Pick::Step(text) => vec![Action::Picker(text)],
                    Pick::Edit(text) => {
                        self.editor.set(&text);
                        vec![]
                    }
                };
            }
            DialogKind::Viewer { .. } | DialogKind::Form { .. } => {}
            DialogKind::Permission { always, .. } => {
                let always_row = always.is_some();
                let suggestions = d.prompt.as_ref().map(|p| p.suggestions.clone()).unwrap_or_default();
                let a = match (i, always_row) {
                    (0, _) => allow(vec![]),
                    (1, true) => allow(suggestions.iter().filter_map(|s| serde_json::to_value(s).ok()).collect()),
                    _ => stop(
                        "The user said no; nothing was run. Don't run it again or reach the same result another way \
                         (another command, shell or tool). Wait for them to say what to do instead.",
                    ),
                };
                self.answer(a);
            }
            DialogKind::Plan => {
                let a = match i {
                    0 => allow(vec![json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"})]),
                    1 => allow(vec![]),
                    _ => stop("The user wants to keep planning. Wait for their feedback on the plan."),
                };
                self.answer(a);
            }
            DialogKind::Questions { questions, at, picked, .. } => {
                if i + 1 == n {
                    // "Type an answer"; Esc on it declines.
                    if d.selected + 1 == n && d.typing.is_none() {
                        d.typing = Some(Editor::default());
                    }
                    return vec![];
                }
                let q = &questions[*at];
                let answer = if q.multi {
                    let mut chosen: Vec<String> =
                        q.options.iter().zip(picked.iter()).filter(|(_, p)| **p).map(|(o, _)| o.0.clone()).collect();
                    if chosen.is_empty() {
                        chosen.push(q.options[i].0.clone());
                    }
                    chosen.join(", ")
                } else {
                    q.options[i].0.clone()
                };
                self.answer_question(answer);
            }
        }
        vec![]
    }

    fn answer_question(&mut self, answer: String) {
        let Some(d) = self.dialog.as_mut() else { return };
        let DialogKind::Questions { questions, at, answers, picked } = &mut d.kind else { return };
        answers.insert(questions[*at].question.clone(), json!(answer));
        if *at + 1 < questions.len() {
            *at += 1;
            *picked = vec![false; questions[*at].options.len()];
            d.selected = 0;
            return;
        }
        let mut input = d.prompt.as_ref().map(|p| p.input.clone()).unwrap_or_default();
        input["answers"] = Value::Object(std::mem::take(answers));
        self.answer(PermissionAnswer::Allow { updated_input: Some(input), updated_permissions: vec![] });
    }

    fn open_picker(&mut self, picker: Picker) {
        self.flush_live();
        let selected = picker.choices.iter().position(|c| c.current).unwrap_or(0);
        self.show(Dialog {
            prompt: None,
            reply: None,
            kind: DialogKind::Picker { picker, filter: String::new() },
            selected,
            typing: None,
        });
    }

    // ---- screens ----

    /// Show a screen (`/diff`, `/context`, `/hooks`, `/agents`).
    pub fn open_screen(&mut self, screen: Screen) {
        self.flush_live();
        let kind = DialogKind::Viewer { screen, cursor: 0, top: 0, back: vec![] };
        self.show(Dialog { prompt: None, reply: None, kind, selected: 0, typing: None });
    }

    /// The open screen: its rows, the highlighted row and the first row shown.
    pub fn viewer(&self) -> Option<(&Screen, usize, usize)> {
        match &self.dialog.as_ref()?.kind {
            DialogKind::Viewer { screen, cursor, top, .. } => Some((screen, *cursor, *top)),
            _ => None,
        }
    }

    /// The open form and the field being edited.
    pub fn form(&self) -> Option<(&Form, usize)> {
        match &self.dialog.as_ref()?.kind {
            DialogKind::Form { form, at } => Some((form, *at)),
            _ => None,
        }
    }

    fn viewer_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let page = self.viewer_page.get().max(1);
        let Some(Dialog { kind: DialogKind::Viewer { screen, cursor, top, back }, .. }) = self.dialog.as_mut() else {
            return vec![];
        };
        let last = screen.rows.len().saturating_sub(1);
        // Nothing to choose: the keys scroll the page, and no row is highlighted.
        if screen.rows.iter().all(|r| r.action.is_none()) {
            let bottom = screen.rows.len().saturating_sub(page);
            *top = match key.code {
                KeyCode::Up => top.saturating_sub(1),
                KeyCode::Down => (*top + 1).min(bottom),
                KeyCode::PageUp => top.saturating_sub(page),
                KeyCode::PageDown | KeyCode::Char(' ') => (*top + page).min(bottom),
                KeyCode::Home => 0,
                KeyCode::End => bottom,
                KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') | KeyCode::Enter => {
                    self.dialog = None;
                    return vec![];
                }
                _ => *top,
            };
            *cursor = *top;
            return vec![];
        }
        match key.code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(last),
            KeyCode::PageUp => *cursor = cursor.saturating_sub(page),
            KeyCode::PageDown | KeyCode::Char(' ') => *cursor = (*cursor + page).min(last),
            KeyCode::Home => *cursor = 0,
            KeyCode::End => *cursor = last,
            KeyCode::Esc | KeyCode::Left | KeyCode::Char('q') => match back.pop() {
                Some(from) if key.code != KeyCode::Char('q') => *cursor = from,
                _ => {
                    self.dialog = None;
                    return vec![];
                }
            },
            KeyCode::Enter | KeyCode::Right => match screen.rows.get(*cursor).and_then(|r| r.action.clone()) {
                Some(RowAction::Jump(to)) => {
                    back.push(*cursor);
                    *cursor = to.min(last);
                    // The target goes to the top, so what follows it shows.
                    *top = *cursor;
                }
                Some(RowAction::Run(text)) if key.code == KeyCode::Enter => {
                    self.dialog = None;
                    return self.submit(text);
                }
                Some(RowAction::Confirm { question, command }) if key.code == KeyCode::Enter => {
                    let choices = vec![
                        Choice {
                            label: "Yes".into(),
                            detail: command.clone(),
                            pick: Pick::Run(command),
                            current: false,
                        },
                        Choice {
                            label: "No".into(),
                            detail: String::new(),
                            pick: Pick::Edit(String::new()),
                            current: false,
                        },
                    ];
                    self.dialog = None;
                    self.open_picker(Picker { title: question, choices });
                    return vec![];
                }
                Some(RowAction::Form(form)) if key.code == KeyCode::Enter => {
                    self.dialog = Some(Dialog {
                        prompt: None,
                        reply: None,
                        kind: DialogKind::Form { form, at: 0 },
                        selected: 0,
                        typing: None,
                    });
                    return vec![];
                }
                _ => {}
            },
            _ => {}
        }
        // Keep the highlighted row in view.
        if *cursor < *top {
            *top = *cursor;
        } else if *cursor >= *top + page {
            *top = *cursor + 1 - page;
        }
        vec![]
    }

    fn form_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(Dialog { kind: DialogKind::Form { form, at }, .. }) = self.dialog.as_mut() else { return vec![] };
        let n = form.fields.len();
        match key.code {
            KeyCode::Esc => self.dialog = None,
            KeyCode::Enter => {
                let text = form.command();
                self.dialog = None;
                return self.submit(text);
            }
            KeyCode::Tab | KeyCode::Down => *at = (*at + 1) % n.max(1),
            KeyCode::BackTab | KeyCode::Up => *at = at.checked_sub(1).unwrap_or(n.saturating_sub(1)),
            code => match form.fields.get_mut(*at).map(|f| &mut f.kind) {
                Some(FieldKind::Text(t)) => match code {
                    KeyCode::Backspace => {
                        t.pop();
                    }
                    KeyCode::Char('u') if ctrl => t.clear(),
                    KeyCode::Char(c) if !ctrl => t.push(c),
                    _ => {}
                },
                Some(FieldKind::Choice { options, at: i }) => match code {
                    KeyCode::Left => *i = i.checked_sub(1).unwrap_or(options.len().saturating_sub(1)),
                    KeyCode::Right | KeyCode::Char(' ') => *i = (*i + 1) % options.len().max(1),
                    _ => {}
                },
                Some(FieldKind::Multi { options, picked, at: i }) => match code {
                    KeyCode::Left => *i = i.checked_sub(1).unwrap_or(options.len().saturating_sub(1)),
                    KeyCode::Right => *i = (*i + 1) % options.len().max(1),
                    KeyCode::Char(' ') if *i < picked.len() => picked[*i] = !picked[*i],
                    _ => {}
                },
                None => {}
            },
        }
        vec![]
    }

    /// Esc on a dialog that the reply can't reach any more (the turn was interrupted).
    pub fn close_dialog_if_stale(&mut self) {
        if self.dialog.as_ref().is_some_and(|d| d.reply.as_ref().is_some_and(|r| r.is_closed())) {
            self.dialog = self.held.take();
            self.dirty = true;
        }
    }

    // ---- the `/` menu ----

    /// Commands matching what is typed, while it is a bare `/word`.
    pub fn menu(&self) -> Vec<&CommandInfo> {
        if self.menu_dismissed || self.dialog.is_some() {
            return vec![];
        }
        let text = self.editor.text();
        let Some(word) = text.strip_prefix('/') else { return vec![] };
        if word.contains(char::is_whitespace) {
            return vec![];
        }
        let word = word.to_lowercase();
        let mut starts: Vec<&CommandInfo> = self.commands.iter().filter(|c| c.name.starts_with(&word)).collect();
        let contains = self.commands.iter().filter(|c| !c.name.starts_with(&word) && c.name.contains(&word));
        starts.extend(contains);
        starts
    }

    /// Files matching the `@word` before the cursor: name matches first, then shorter paths.
    pub fn file_menu(&self) -> Vec<&str> {
        if self.menu_dismissed || self.dialog.is_some() || self.search.is_some() {
            return vec![];
        }
        let (_, word) = self.editor.word_before_cursor();
        let Some(q) = word.strip_prefix('@') else { return vec![] };
        let q = q.to_lowercase();
        let mut hits: Vec<(bool, usize, &str)> = self
            .files
            .iter()
            // What is typed already (a completed directory) isn't offered again.
            .filter(|f| f.to_lowercase().contains(&q) && f.to_lowercase() != q)
            .map(|f| {
                let name = f.trim_end_matches('/').rsplit('/').next().unwrap_or(f).to_lowercase();
                (!name.starts_with(&q), f.len(), f.as_str())
            })
            .collect();
        hits.sort();
        hits.into_iter().take(FILE_MATCHES).map(|(_, _, f)| f).collect()
    }

    fn complete_file(&mut self, path: &str) {
        let (_, word) = self.editor.word_before_cursor();
        // A path with spaces is quoted, the way `@` mentions read it. A directory
        // gets no trailing space, so Tab can go on into it.
        let end = if path.ends_with('/') { "" } else { " " };
        let mention =
            if path.contains(char::is_whitespace) { format!("@\"{path}\"{end}") } else { format!("@{path}{end}") };
        self.editor.replace_back(word.chars().count(), &mention);
    }

    // ---- history search (Ctrl+R) ----

    /// The newest history entry at or before `from` that contains `query`.
    fn find_history(&self, query: &str, from: Option<usize>) -> Option<usize> {
        let h = self.editor.history();
        let end = from.map(|f| f + 1).unwrap_or(h.len()).min(h.len());
        (0..end).rev().find(|&i| h[i].contains(query))
    }

    fn search_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let mut s = self.search.take()?;
        match key.code {
            KeyCode::Char('r') if ctrl => {
                let older = s.hit.and_then(|h| h.checked_sub(1)).and_then(|o| self.find_history(&s.query, Some(o)));
                s.hit = older.or(s.hit);
            }
            // Esc or Ctrl+G: back to what was typed.
            KeyCode::Esc => {
                self.editor.set(&s.draft);
                return Some(vec![]);
            }
            KeyCode::Char('g') if ctrl => {
                self.editor.set(&s.draft);
                return Some(vec![]);
            }
            KeyCode::Backspace => {
                s.query.pop();
                s.hit = self.find_history(&s.query, None);
            }
            KeyCode::Char(c) if !ctrl => {
                s.query.push(c);
                s.hit = self.find_history(&s.query, None);
            }
            // Enter takes the match into the input box; any other key takes it and then acts.
            KeyCode::Enter => return Some(vec![]),
            _ => return None,
        }
        match s.hit {
            Some(h) => {
                let text = self.editor.history()[h].clone();
                self.editor.set(&text);
            }
            None => self.editor.set(&s.draft),
        }
        self.search = Some(s);
        Some(vec![])
    }

    // ---- keys ----

    pub fn on_term_event(&mut self, ev: Event) -> Vec<Action> {
        self.dirty = true;
        match ev {
            // Through the user's key bindings first (keybindings.json).
            Event::Key(k) if k.kind != KeyEventKind::Release => match self.keymap.translate(k) {
                Some(k) => self.on_key(k),
                None => vec![],
            },
            Event::Paste(text) => {
                if let Some(e) = self.dialog.as_mut().and_then(|d| d.typing.as_mut()) {
                    e.insert(&text);
                } else if self.dialog.is_none() {
                    self.editor.insert(&text);
                }
                vec![]
            }
            _ => vec![],
        }
    }

    fn set_hint(&mut self, text: &str) {
        self.hint = Some((text.to_string(), Instant::now()));
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let now = Instant::now();
        // Ctrl-C: clear, interrupt, or (twice) exit; it works in dialogs too.
        if ctrl && key.code == KeyCode::Char('c') {
            self.search = None;
            if self.dialog.is_some() {
                self.answer(PermissionAnswer::Deny { message: "The user interrupted.".into(), interrupt: true });
                return vec![Action::Interrupt];
            }
            if !self.editor.is_empty() {
                self.editor.clear();
                return vec![];
            }
            if self.busy {
                return vec![Action::Interrupt];
            }
            if self.last_ctrl_c.is_some_and(|t| now.duration_since(t) < DOUBLE_PRESS) {
                return vec![Action::Exit];
            }
            self.last_ctrl_c = Some(now);
            self.set_hint("Press Ctrl-C again to exit");
            return vec![];
        }
        if self.dialog.is_some() {
            return self.dialog_key(key);
        }
        if self.search.is_some() {
            if let Some(a) = self.search_key(key) {
                return a;
            }
        }
        if ctrl && key.code == KeyCode::Char('b') {
            self.sidebar = !self.sidebar;
            return vec![];
        }
        if ctrl && key.code == KeyCode::Char('r') {
            let draft = self.editor.text();
            self.search = Some(Search { query: String::new(), hit: None, draft });
            return vec![];
        }
        if ctrl && key.code == KeyCode::Char('d') && self.editor.is_empty() {
            // A running turn stops first, so the session can end cleanly.
            return if self.busy { vec![Action::Interrupt, Action::Exit] } else { vec![Action::Exit] };
        }
        let files = self.file_menu().len();
        if files > 0 {
            match key.code {
                KeyCode::Up => {
                    self.menu_selected = self.menu_selected.checked_sub(1).unwrap_or(files - 1);
                    return vec![];
                }
                KeyCode::Down => {
                    self.menu_selected = (self.menu_selected + 1) % files;
                    return vec![];
                }
                KeyCode::Tab | KeyCode::Enter if !key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
                    let path = self.file_menu()[self.menu_selected.min(files - 1)].to_string();
                    self.complete_file(&path);
                    self.menu_selected = 0;
                    return vec![];
                }
                KeyCode::Esc => {
                    self.menu_dismissed = true;
                    return vec![];
                }
                _ => {}
            }
        }
        if key.code == KeyCode::BackTab {
            let cur = MODES.iter().position(|m| m.as_str() == self.status.mode).unwrap_or(0);
            let next = MODES[(cur + 1) % MODES.len()];
            self.status.mode = next.as_str().to_string();
            return vec![Action::SetMode(next)];
        }
        if key.code == KeyCode::Char('?') && self.editor.is_empty() && !ctrl && !alt {
            self.reply_lines(&keys_text(&self.keymap), false);
            return vec![];
        }
        let menu_len = self.menu().len();
        match key.code {
            KeyCode::Esc => {
                if menu_len > 0 {
                    self.menu_dismissed = true;
                    return vec![];
                }
                if self.busy {
                    return vec![Action::Interrupt];
                }
                if self.editor.is_empty() && self.last_esc.is_some_and(|t| now.duration_since(t) < DOUBLE_PRESS) {
                    self.last_esc = None;
                    return self.submit("/rewind".into());
                }
                self.last_esc = Some(now);
                if !self.editor.is_empty() {
                    self.set_hint("Esc again on an empty prompt opens /rewind");
                }
                return vec![];
            }
            KeyCode::Enter if key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) => {
                self.editor.insert("\n")
            }
            KeyCode::Char('j') if ctrl => self.editor.insert("\n"),
            KeyCode::Enter => {
                if menu_len > 0 {
                    let c = self.menu()[self.menu_selected.min(menu_len - 1)].clone();
                    // A command that takes arguments waits for them; one that doesn't runs now.
                    let typed = self.editor.text();
                    if c.args.is_empty() || typed == format!("/{}", c.name) && c.args.starts_with('[') {
                        self.editor.set(&format!("/{}", c.name));
                    } else {
                        self.editor.set(&format!("/{} ", c.name));
                        return vec![];
                    }
                }
                let text = self.editor.text();
                if let Some(stripped) = text.strip_suffix('\\') {
                    self.editor.set(&format!("{stripped}\n"));
                    return vec![];
                }
                if text.trim().is_empty() {
                    return vec![];
                }
                let text = self.editor.take();
                return self.submit(text);
            }
            KeyCode::Tab if menu_len > 0 => {
                let c = self.menu()[self.menu_selected.min(menu_len - 1)].clone();
                self.editor.set(&format!("/{} ", c.name));
            }
            KeyCode::Up if menu_len > 0 => {
                self.menu_selected = self.menu_selected.checked_sub(1).unwrap_or(menu_len - 1);
                return vec![];
            }
            KeyCode::Down if menu_len > 0 => {
                self.menu_selected = (self.menu_selected + 1) % menu_len;
                return vec![];
            }
            KeyCode::Up => {
                self.editor.up();
            }
            KeyCode::Down => {
                self.editor.down();
            }
            KeyCode::Left if ctrl => self.editor.word_left(),
            KeyCode::Right if ctrl => self.editor.word_right(),
            KeyCode::Left => self.editor.left(),
            KeyCode::Right => self.editor.right(),
            KeyCode::Home => self.editor.home(),
            KeyCode::End => self.editor.end(),
            KeyCode::Backspace if alt || ctrl => self.editor.delete_word(),
            KeyCode::Backspace => self.editor.backspace(),
            KeyCode::Delete => self.editor.delete(),
            KeyCode::Char('a') if ctrl => self.editor.home(),
            KeyCode::Char('e') if ctrl => self.editor.end(),
            KeyCode::Char('w') if ctrl => self.editor.delete_word(),
            KeyCode::Char('u') if ctrl => self.editor.kill_to_start(),
            KeyCode::Char('k') if ctrl => self.editor.kill_to_end(),
            KeyCode::Char('b') if alt => self.editor.word_left(),
            KeyCode::Char('f') if alt => self.editor.word_right(),
            KeyCode::Char(c) if !ctrl => self.editor.insert(&c.to_string()),
            _ => {}
        }
        // Typing changes the menu: show it again from the top.
        self.menu_dismissed = false;
        if self.menu().len() != menu_len || self.file_menu().len() != files {
            self.menu_selected = 0;
        }
        vec![]
    }

    /// `/color [name|default]`: the accent colour for this session.
    fn color_command(&mut self, arg: &str) -> (String, bool) {
        let names: Vec<&str> = super::text::ACCENTS.iter().map(|(n, _)| *n).collect();
        let list = format!("Colours: {}, default.", names.join(", "));
        match arg {
            "" => (format!("The accent colour marks prompts, answers and selections. {list} /color <name> sets it for this session."), false),
            "default" | "reset" => {
                self.theme.accent = None;
                ("Accent colour: KodAct's own.".into(), false)
            }
            name => match super::text::ACCENTS.iter().find(|(n, _)| *n == name) {
                Some((n, c)) => {
                    self.theme.accent = Some(*c);
                    let note = if self.theme.color { "" } else { " (Colour is off, so it shows once colour is on.)" };
                    (format!("Accent colour: {n}, for this session.{note}"), false)
                }
                None => (format!("Unknown colour {name:?}. {list}"), true),
            },
        }
    }

    /// `/focus [on|off]`: keep tool calls out of the scrollback.
    fn focus_command(&mut self, arg: &str) -> (String, bool) {
        let on = match arg {
            "" => !self.focus,
            "on" => true,
            "off" => false,
            _ => return ("Usage: /focus [on|off]".into(), true),
        };
        self.focus = on;
        let text = if on {
            "Focus on: tool calls and their results stay out of the scrollback (the spinner still names the tool). /focus again turns it off."
        } else {
            "Focus off: tool calls show again."
        };
        (text.into(), false)
    }

    /// Send `text` as if typed (queued while a turn runs).
    pub fn submit(&mut self, text: String) -> Vec<Action> {
        self.front = false;
        self.menu_selected = 0;
        self.menu_dismissed = false;
        // The UI's own immediate commands answer at once, even while a turn runs.
        let local = match text.trim() {
            "/keybindings" => Some((keys_text(&self.keymap), false)),
            "/terminal-setup" => Some((terminal_setup_text(super::keyboard_protocol()), false)),
            t if t == "/color" || t.starts_with("/color ") => Some(self.color_command(t["/color".len()..].trim())),
            t if t == "/focus" || t.starts_with("/focus ") => Some(self.focus_command(t["/focus".len()..].trim())),
            _ => None,
        };
        if let Some((answer, is_error)) = local {
            self.echo_prompt(&text);
            self.reply_lines(&answer, is_error);
            return vec![];
        }
        if self.busy {
            // Immediate commands (/status, /usage, /btw ...) are answered from the session view
            // while the turn runs (C17); anything else waits for the turn to end.
            if kodact_core::commands::immediate(&text, &kodact_core::commands::Catalog::default()) {
                self.echo_prompt(&text);
                return vec![Action::Send(text)];
            }
            self.queued.push_back(text);
            return vec![];
        }
        vec![Action::Send(self.start(text))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::text::plain;
    use kodact_types::{MessageContent, StopReason, Usage};

    fn app() -> App {
        let mut a = App::new(Theme { color: false, light: false, accent: None, depth: Default::default() }, vec![]);
        a.commands = vec![
            CommandInfo { name: "clear".into(), args: "[name]".into(), description: "Start a new conversation".into() },
            CommandInfo { name: "compact".into(), args: "[instructions]".into(), description: "Summarize".into() },
            CommandInfo { name: "model".into(), args: "[model]".into(), description: "Switch models".into() },
            CommandInfo { name: "status".into(), args: String::new(), description: "Show status".into() },
        ];
        a
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn typed(a: &mut App, s: &str) {
        for c in s.chars() {
            a.on_key(key(KeyCode::Char(c)));
        }
    }

    fn texts(lines: &[Line]) -> Vec<String> {
        lines.iter().map(plain).collect()
    }

    #[test]
    fn a_replaced_conversation_starts_the_screen_over() {
        use kodact_types::Message;
        let mut a = app();
        a.submit("old question".into());
        a.on_event(UiEvent::Engine(EngineEvent::Notice { level: NoticeLevel::Info, text: "old notice".into() }));
        assert!(!a.front && a.reset.is_none());

        // /clear: nothing of the old conversation is left to write, and the front page is back.
        let recent = vec![("old question".to_string(), "just now".to_string())];
        a.on_event(UiEvent::Reset { replay: vec![], recent: recent.clone(), echo: Some("/clear".into()) });
        a.on_event(UiEvent::Reply { text: "Conversation cleared.".into(), is_error: false });
        assert_eq!(a.reset.take(), Some(recent), "the loop empties the screen and lists what can be resumed");
        assert!(a.front, "an empty conversation shows the front page");
        let lines = texts(&a.take_pending());
        assert!(!lines.iter().any(|l| l.contains("old")), "{lines:?}");
        let at =
            |want: &str| lines.iter().position(|l| l.contains(want)).unwrap_or_else(|| panic!("{want}: {lines:?}"));
        assert!(at("/clear") < at("Conversation cleared."), "the command, then its answer");

        // /resume: the conversation it moved to is written out, a tool call with its result.
        let call = ContentBlock::ToolUse {
            id: "t".into(),
            name: "Read".into(),
            input: json!({"file_path": "src/cart.rs"}),
            cache_control: None,
            provider: None,
        };
        let replay = vec![
            Message::user_text("why is the total off?"),
            Message { role: Role::Assistant, content: vec![ContentBlock::text("Looking at the cart."), call] },
            Message::user(vec![ContentBlock::tool_result("t".to_string(), "fn total() {}", false)]),
            Message { role: Role::Assistant, content: vec![ContentBlock::text("It rounds **each line**.")] },
        ];
        a.on_event(UiEvent::Reset { replay, recent: vec![], echo: Some("/resume 1".into()) });
        assert!(!a.front && a.reset.is_some());
        assert!(a.running.is_empty(), "nothing is left running");
        let lines = texts(&a.take_pending());
        let at =
            |want: &str| lines.iter().position(|l| l.contains(want)).unwrap_or_else(|| panic!("{want}: {lines:?}"));
        let order = [
            at("why is the total off?"),
            at("Looking at the cart."),
            at("src/cart.rs"),
            at("It rounds each line."),
            at("/resume 1"),
        ];
        assert!(order.windows(2).all(|w| w[0] < w[1]), "in the conversation's order: {lines:?}");
        assert!(!lines.iter().any(|l| l.contains("Conversation cleared")), "{lines:?}");
    }

    fn delta(text: &str) -> UiEvent {
        UiEvent::Engine(EngineEvent::Stream {
            event: StreamEvent::ContentBlockDelta { index: 0, delta: Delta::TextDelta { text: text.into() } },
            parent_tool_use_id: None,
        })
    }

    #[test]
    fn enter_sends_and_queues_while_busy() {
        let mut a = app();
        typed(&mut a, "hello");
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Send("hello".into())]);
        assert!(a.busy);
        assert_eq!(texts(&a.take_pending()), ["", "❯ hello"]);
        typed(&mut a, "next");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty(), "queued while a turn runs");
        assert_eq!(a.queued.len(), 1);
        assert_eq!(a.next_queued(), None, "still busy");
        a.on_event(UiEvent::Idle);
        assert_eq!(a.next_queued().as_deref(), Some("next"));
        // A trailing backslash, Shift+Enter or Ctrl+J start a new line instead.
        a.on_event(UiEvent::Idle);
        typed(&mut a, "one\\");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
        typed(&mut a, "two");
        a.on_key(ctrl('j'));
        assert_eq!(a.editor.text(), "one\ntwo\n");
    }

    #[test]
    fn a_failed_checklist_paints_only_its_failed_rows() {
        let mut a = App::new(Theme { color: true, light: false, accent: None, depth: Default::default() }, vec![]);
        a.on_event(UiEvent::Reply { text: "ok   git          found\nFAIL shell        none".into(), is_error: true });
        let lines = a.take_pending();
        let (ok, fail) = (&lines[1], &lines[2]);
        assert_eq!(ok.spans[1].style, a.theme.success());
        assert_ne!(ok.spans[2].style, a.theme.error(), "the ok row's detail is plain");
        assert_eq!(fail.spans[1].content, "FAIL");
        assert_eq!(fail.spans[1].style, a.theme.error());
        // Any other failed reply is an error throughout.
        a.on_event(UiEvent::Reply { text: "Unknown command: /x".into(), is_error: true });
        assert_eq!(a.take_pending().last().unwrap().spans[1].style, a.theme.error());
    }

    #[test]
    fn notices_lead_with_a_mark_in_their_colour() {
        let mut a = App::new(Theme { color: true, light: false, accent: None, depth: Default::default() }, vec![]);
        a.on_event(UiEvent::Engine(EngineEvent::Notice {
            level: NoticeLevel::Warning,
            text: "careful\nsecond".into(),
        }));
        let lines = a.take_pending();
        assert_eq!(texts(&lines), ["", "▲ careful", "  second"], "in the column of the other marks");
        assert_eq!(lines[1].spans[0].style, a.theme.warning());
        assert_eq!(lines[1].spans[1].style, Style::default(), "the words stay readable");
        a.on_event(UiEvent::Engine(EngineEvent::Notice { level: NoticeLevel::Error, text: "broke".into() }));
        assert_eq!(texts(&a.take_pending()), ["", "✗ broke"]);
    }

    #[test]
    fn key_bindings_apply_and_show_in_the_key_table() {
        let mut a = app();
        let (k, w) = crate::tui::keys::Keymap::parse(r#"{"ctrl+s": "submit", "ctrl+r": "none"}"#);
        assert!(w.is_empty(), "{w:?}");
        a.keymap = k;
        let term = |a: &mut App, code, m| a.on_term_event(Event::Key(KeyEvent::new(code, m)));
        typed(&mut a, "hello");
        assert_eq!(term(&mut a, KeyCode::Char('s'), KeyModifiers::CONTROL), vec![Action::Send("hello".into())]);
        a.on_event(UiEvent::Idle);
        term(&mut a, KeyCode::Char('r'), KeyModifiers::CONTROL);
        assert!(a.search.is_none(), "Ctrl+R is unbound");
        let table = keys_text(&a.keymap);
        assert!(table.contains("enter, ctrl+s") && table.contains("Unbound: ctrl+r."), "{table}");
        assert!(keys_text(&Default::default()).starts_with("Enter "), "the defaults read as before");
    }

    #[test]
    fn color_and_focus_change_this_session_only() {
        let mut a = App::new(Theme { color: true, light: false, accent: None, depth: Default::default() }, vec![]);
        assert!(a.submit("/color green".into()).is_empty(), "answered by the UI");
        assert_eq!(a.theme.accent, Some(ratatui::style::Color::Green));
        assert_eq!(a.theme.accent().fg, Some(ratatui::style::Color::Green));
        a.on_event(UiEvent::Theme("light".into()));
        assert_eq!(a.theme.accent, Some(ratatui::style::Color::Green), "a theme change keeps it");
        a.take_pending();
        a.submit("/color mauve".into());
        assert!(texts(&a.take_pending()).iter().any(|l| l.contains("Unknown colour \"mauve\"")));
        a.submit("/color default".into());
        assert_eq!(a.theme.accent, None);

        // Focus: tool calls and results stay out of the scrollback; text still shows.
        a.submit("/focus".into());
        assert!(a.focus);
        a.take_pending();
        a.on_event(UiEvent::Engine(EngineEvent::Assistant {
            message: ApiMessage {
                id: "m".into(),
                kind: "message".into(),
                role: Role::Assistant,
                model: "m".into(),
                content: vec![
                    ContentBlock::text("Looking."),
                    ContentBlock::ToolUse {
                        id: "t".into(),
                        name: "Bash".into(),
                        input: json!({"command": "ls"}),
                        cache_control: None,
                        provider: None,
                    },
                ],
                stop_reason: Some(StopReason::ToolUse),
                stop_sequence: None,
                usage: Usage::default(),
            },
            uuid: "u".into(),
            parent_tool_use_id: None,
        }));
        a.on_event(UiEvent::Engine(EngineEvent::User {
            message: kodact_types::Message::user(vec![ContentBlock::tool_result("t".to_string(), "file.txt", false)]),
            uuid: "u2".into(),
            tool_use_result: None,
            is_meta: false,
            parent_tool_use_id: None,
        }));
        let lines = texts(&a.take_pending());
        assert!(lines.iter().any(|l| l.contains("Looking.")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("Bash") || l.contains("file.txt")), "{lines:?}");
        assert_eq!(a.activity, "Working");
        a.submit("/focus off".into());
        assert!(!a.focus);
    }

    #[test]
    fn a_screen_arriving_during_a_question_waits_for_its_answer() {
        let mut a = app();
        let (tx, mut rx) = oneshot::channel();
        a.on_event(UiEvent::Ask {
            prompt: PermissionPrompt {
                tool_name: "Bash".into(),
                tool_use_id: "t".into(),
                input: json!({"command": "ls"}),
                reason: String::new(),
                suggestions: vec![],
                blocked_path: None,
            },
            reply: tx,
        });
        // /context typed mid-turn comes back as a screen while the question is open.
        let screen = kodact_core::commands::screens::Screen { title: "Context".into(), rows: vec![] };
        a.on_event(UiEvent::Screen(screen));
        assert_eq!(a.dialog_text().0, "Run this command?", "the question stays");
        assert!(rx.try_recv().is_err(), "and isn't answered");
        a.on_key(key(KeyCode::Char('1')));
        assert!(matches!(rx.try_recv(), Ok(PermissionAnswer::Allow { .. })));
        assert_eq!(a.viewer().map(|v| v.0.title.as_str()), Some("Context"), "then the screen shows");
    }

    #[test]
    fn immediate_commands_are_sent_while_busy() {
        let mut a = app();
        typed(&mut a, "hello");
        a.on_key(key(KeyCode::Enter));
        a.take_pending();
        typed(&mut a, "/usage");
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Send("/usage".into())], "sent, not queued");
        assert!(a.queued.is_empty() && a.busy, "the turn goes on");
        assert_eq!(texts(&a.take_pending()), ["", "❯ /usage"], "echoed");
        typed(&mut a, "/compact");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty(), "not immediate: queued");
        assert_eq!(a.queued.len(), 1);
    }

    #[test]
    fn the_context_meter_follows_each_request() {
        let mut a = app();
        a.status.window = 1000;
        let ev = |event| UiEvent::Engine(EngineEvent::Stream { event, parent_tool_use_id: None });
        let start = ApiMessage {
            id: "m".into(),
            kind: "message".into(),
            role: Role::Assistant,
            model: "x".into(),
            content: vec![],
            stop_reason: None,
            stop_sequence: None,
            usage: Usage { input_tokens: 100, ..Default::default() },
        };
        a.on_event(ev(StreamEvent::MessageStart { message: start }));
        assert_eq!(a.status.context_pct, Some(10));
        let body: kodact_types::MessageDeltaBody = serde_json::from_str("{}").unwrap();
        a.on_event(ev(StreamEvent::MessageDelta {
            delta: body,
            usage: Usage { output_tokens: 50, ..Default::default() },
        }));
        assert_eq!(a.status.context_pct, Some(15), "output counts too");
        // An endpoint that reports usage only at the end (OpenAI-compatible).
        let mut b = app();
        b.status.window = 1000;
        let body: kodact_types::MessageDeltaBody = serde_json::from_str("{}").unwrap();
        b.on_event(ev(StreamEvent::MessageDelta {
            delta: body,
            usage: Usage { input_tokens: 300, output_tokens: 20, ..Default::default() },
        }));
        assert_eq!(b.status.context_pct, Some(32));
        // A reply that thought: its output is reasoning the next request won't carry.
        let mut c = app();
        c.status.window = 1000;
        c.on_event(ev(StreamEvent::MessageStart {
            message: ApiMessage { usage: Usage::default(), ..start_message() },
        }));
        c.on_event(ev(StreamEvent::ContentBlockDelta {
            index: 0,
            delta: Delta::ThinkingDelta { thinking: "let me think".into() },
        }));
        let body: kodact_types::MessageDeltaBody = serde_json::from_str("{}").unwrap();
        c.on_event(ev(StreamEvent::MessageDelta {
            delta: body,
            usage: Usage { input_tokens: 300, output_tokens: 400, ..Default::default() },
        }));
        assert_eq!(c.status.context_pct, Some(30), "the thinking isn't counted: 30%, not 70%");
    }

    fn start_message() -> ApiMessage {
        ApiMessage {
            id: "m".into(),
            kind: "message".into(),
            role: Role::Assistant,
            model: "x".into(),
            content: vec![],
            stop_reason: None,
            stop_sequence: None,
            usage: Usage::default(),
        }
    }

    #[test]
    fn a_streaming_answer_is_set_apart_from_the_prompt_at_once() {
        let mut a = app();
        a.echo_prompt("hi");
        a.take_pending();
        a.on_event(UiEvent::Engine(EngineEvent::Stream {
            event: StreamEvent::ContentBlockStart { index: 0, content_block: ContentBlock::text("") },
            parent_tool_use_id: None,
        }));
        a.on_event(delta("I will walk"));
        assert_eq!(texts(&a.take_pending()), [""], "the blank line goes out with the first words");
        a.on_event(delta(" through it.\n"));
        assert_eq!(texts(&a.take_pending()), ["• I will walk through it."], "and isn't added twice");
    }

    #[test]
    fn api_errors_read_as_a_person_would_say_them() {
        let h = |t: &str| human_api_error(t, "quick-thinking", "gpt.example.net");
        // The texts KodAct's engine sends, from a real gateway run.
        let (head, next) = h(r#"API Error: HTTP 400 (error) from https://gpt.technica-engineering.net/api/v1/chat/completions: {"detail":"Model not found"}"#).unwrap();
        assert_eq!(head, "quick-thinking isn't available on gpt.technica-engineering.net");
        assert_eq!(next, "/model to choose another · HTTP 400: Model not found");
        let (head, _) = h("API Error: network error: could not connect to https://gw.example.com/v1/chat/completions: Host is unreachable (os error 113). Retrying in 6s (attempt 1).").unwrap();
        assert_eq!(head, "Can't reach gw.example.com · retrying in 6s (attempt 1)");
        let (head, next) =
            h(r#"API Error: HTTP 401 from https://u:p@gw.example.com/v1: {"error":{"message":"bad key"}}"#).unwrap();
        assert_eq!(head, "gw.example.com didn't accept the key", "no credentials from the URL");
        assert!(next.contains("HTTP 401: bad key"));
        assert!(h("API Error: HTTP 503 from https://gw.example.com/v1").unwrap().0.ends_with("had an error"));
        assert!(h("Something else entirely").is_none(), "other notices stay as they are");
        // On screen: the mark in the marks' column, the headline, then the next step.
        let mut a = app();
        a.status.model = "quick-thinking".into();
        a.on_event(UiEvent::Engine(EngineEvent::Notice {
            level: NoticeLevel::Error,
            text: r#"API Error: HTTP 400 (error) from https://gw.example.com/v1/chat/completions: {"detail":"Model not found"}"#.into(),
        }));
        assert_eq!(
            texts(&a.take_pending()),
            [
                "",
                "✗ quick-thinking isn't available on gw.example.com",
                "  /model to choose another · HTTP 400: Model not found"
            ]
        );
    }

    #[test]
    fn the_persons_message_takes_the_input_mark_in_its_modes_colour() {
        let mut a = App::new(Theme { color: true, light: false, accent: None, depth: Default::default() }, vec![]);
        a.look = crate::tui::look::Look::Frame;
        a.echo_prompt("fix the cart\nand the tests");
        let lines = a.take_pending();
        assert_eq!(texts(&lines), ["", "❯ fix the cart", "  and the tests"], "later lines line up under the words");
        let default_mark = lines[1].spans[0].style.fg;
        assert_eq!(default_mark, a.theme.mode("default").fg);
        assert!(lines[1].spans[1].style.add_modifier.contains(ratatui::style::Modifier::BOLD));
        a.status.mode = "plan".into();
        a.echo_prompt("plan it");
        let lines = a.take_pending();
        let plan = lines.iter().find(|l| plain(l).contains("plan it")).unwrap();
        assert_eq!(plan.spans[0].style.fg, a.theme.mode("plan").fg);
        assert_ne!(plan.spans[0].style.fg, default_mark, "the mark shows the mode it was sent in");
    }

    #[test]
    fn an_answer_that_opens_with_blank_lines_keeps_its_mark_on_the_words() {
        // Reasoning models behind OpenAI-compatible gateways often start the text with "\n\n".
        let mut a = app();
        a.echo_prompt("hi");
        a.take_pending();
        a.on_event(UiEvent::Engine(EngineEvent::Stream {
            event: StreamEvent::ContentBlockStart { index: 0, content_block: ContentBlock::text("") },
            parent_tool_use_id: None,
        }));
        a.on_event(delta("\n\n"));
        a.on_event(delta("Hi there.\nSecond line.\n"));
        assert_eq!(texts(&a.take_pending()), ["", "• Hi there.", "  Second line."]);
    }

    #[test]
    fn streamed_text_moves_to_scrollback_line_by_line() {
        let mut a = app();
        a.on_event(UiEvent::Engine(EngineEvent::Stream {
            event: StreamEvent::ContentBlockStart { index: 0, content_block: ContentBlock::text("") },
            parent_tool_use_id: None,
        }));
        a.on_event(delta("Here is **the** plan:\n- step"));
        assert_eq!(texts(&a.pending), ["", "• Here is the plan:"]);
        assert_eq!(a.live, "- step");
        a.on_event(delta(" one\n```sh\nls\n"));
        a.on_event(UiEvent::Engine(EngineEvent::Stream {
            event: StreamEvent::ContentBlockStop { index: 0 },
            parent_tool_use_id: None,
        }));
        assert_eq!(
            texts(&a.take_pending())[2..],
            ["  • step one", "  ╭─ sh", "  │ ls"],
            "lists get bullets; code gets a gutter"
        );
        // The complete message adds its tool call, not its text again.
        let msg = ApiMessage {
            id: "m".into(),
            kind: "message".into(),
            role: Role::Assistant,
            model: "x".into(),
            content: vec![
                ContentBlock::text("Here is the plan"),
                ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "Bash".into(),
                    input: json!({"command": "cargo test"}),
                    cache_control: None,
                    provider: None,
                },
            ],
            stop_reason: Some(StopReason::ToolUse),
            stop_sequence: None,
            usage: Usage::default(),
        };
        a.on_event(UiEvent::Engine(EngineEvent::Assistant {
            message: msg,
            uuid: "u".into(),
            parent_tool_use_id: None,
        }));
        // The call waits in the live region with a turning dot until it answers.
        assert!(texts(&a.take_pending()).is_empty());
        assert_eq!(a.running.len(), 1);
        // The box under it shows the command; the header names the program.
        assert_eq!(texts(&[Line::from(a.running[0].header.clone())]), ["Running cargo"]);
        a.on_event(UiEvent::Engine(EngineEvent::User {
            message: kodact_types::Message::user(vec![ContentBlock::tool_result("t1", "ok\n2 passed", false)]),
            uuid: "u2".into(),
            tool_use_result: None,
            is_meta: false,
            parent_tool_use_id: None,
        }));
        // The header goes out with its result: the command and its output in a box.
        let out = texts(&lay_out(a.take_pending(), 40));
        assert!(out[1].starts_with("✓ Ran cargo  0."), "{out:#?}");
        assert_eq!(
            out[2..].iter().map(|r| r.trim_end()).collect::<Vec<_>>(),
            [
                "  ╭────────────────────────────────────╮",
                "  │ $ cargo test                       │",
                "  ├────────────────────────────────────┤",
                "  │ ok                                 │",
                "  │ 2 passed                           │",
                "  ├────────────────────────────────────┤",
                "  │  exit 0   2 lines                  │",
                "  ╰────────────────────────────────────╯",
            ],
            "{out:#?}"
        );
        assert!(a.running.is_empty());
        let _ = MessageContent::Text(String::new());
    }

    #[test]
    fn each_style_writes_the_conversation_its_own_way() {
        use crate::tui::look::Look;
        let prompt_for = |look: Look| {
            let mut a = app();
            a.look = look;
            a.echo_prompt("fix the cart");
            texts(&a.take_pending())
        };
        assert_eq!(prompt_for(Look::Frame).last().unwrap(), "❯ fix the cart", "the input's mark, then the words");
        assert_eq!(prompt_for(Look::Bare).last().unwrap(), "› fix the cart");
        let card = prompt_for(Look::Card);
        assert!(card[card.len() - 2].starts_with("  you · "), "{card:?}");
        let ledger = prompt_for(Look::Ledger);
        assert!(ledger.last().unwrap().ends_with("you    fix the cart"), "{ledger:?}");
        assert_eq!(prompt_for(Look::Glow).last().unwrap(), "  fix the cart  ");
        // Steps: plain quiet lines in Bare and Ledger, a ✓ in Glow, the dot elsewhere.
        let step_for = |look: Look| {
            let mut a = app();
            a.look = look;
            let r = Running {
                id: "t".into(),
                header: vec![],
                name: "Bash".into(),
                input: json!({"command": "cargo test"}),
                since: Instant::now(),
                step: None,
                steps: 0,
                trail: Vec::new(),
            };
            a.commit_tool(r, ToolEnd::Done);
            texts(&a.take_pending()).last().unwrap().clone()
        };
        assert_eq!(step_for(Look::Bare), "  ran cargo test");
        assert_eq!(step_for(Look::Ledger), "  ran cargo test");
        // Boxed styles: the program and how long it ran; the box holds the command.
        assert!(step_for(Look::Glow).starts_with("✓ Ran cargo  0."), "{}", step_for(Look::Glow));
        assert!(step_for(Look::Frame).starts_with("✓ Ran cargo  0."), "{}", step_for(Look::Frame));
        // A finished sub-agent keeps a short tree: its first step, how many more, its last two.
        let mut a = app();
        a.look = Look::Frame;
        let r = Running {
            id: "t".into(),
            header: vec![],
            name: "Task".into(),
            input: json!({"description": "find the parser"}),
            since: Instant::now(),
            step: None,
            steps: 6,
            trail: (1..=6).map(|i| format!("Read f{i}.rs")).collect(),
        };
        a.commit_tool(r, ToolEnd::Done);
        let rows = texts(&a.take_pending());
        assert_eq!(
            rows[rows.len() - 4..],
            ["  ├ Read f1.rs", "  ├ … 3 more", "  ├ Read f5.rs", "  └ Read f6.rs"],
            "{rows:?}"
        );
        assert_eq!(agent_tree(&a.theme, &["a".into(), "b".into()], true).len(), 2, "a short trail is shown whole");
        // A described command: its purpose and how long it ran; the box holds the command.
        let mut a = app();
        a.look = Look::Frame;
        let r = Running {
            id: "t".into(),
            header: vec![],
            name: "Bash".into(),
            input: json!({"command": "python3 -m pytest -q\necho done", "description": "Run the cart tests"}),
            since: Instant::now(),
            step: None,
            steps: 0,
            trail: Vec::new(),
        };
        a.commit_tool(r, ToolEnd::Failed);
        let rows = texts(&a.take_pending());
        let last = rows.last().unwrap();
        assert!(last.starts_with("✗ Run the cart tests  0.") && last.ends_with("s · failed"), "{rows:?}");
        // The plain styles keep the command on its own line under the purpose.
        let mut a = app();
        a.look = Look::Bare;
        a.commit_tool(
            Running {
                id: "t".into(),
                header: vec![],
                name: "Bash".into(),
                input: json!({"command": "python3 -m pytest -q", "description": "Run the cart tests"}),
                since: Instant::now(),
                step: None,
                steps: 0,
                trail: Vec::new(),
            },
            ToolEnd::Done,
        );
        assert!(texts(&a.take_pending()).iter().all(|r| !r.contains('\u{e000}')));
        assert_eq!(
            texts(&[Line::from(tool_header(&a.theme, "Bash", &json!({"command": "ls", "description": " "}), true))]),
            ["Ran ls"]
        );
        // A style arrives from the session by name.
        let mut a = app();
        a.on_event(UiEvent::Look("ledger".into()));
        assert_eq!(a.look, Look::Ledger);
        a.on_event(UiEvent::Look("neon".into()));
        assert_eq!(a.look, Look::Frame, "an unknown name falls back to the first style");
    }

    #[test]
    fn results_say_what_happened() {
        let t = Theme { color: false, light: false, accent: None, depth: Default::default() };
        let read =
            json!({"type": "text", "file": {"filePath": "/p/a.rs", "numLines": 40, "startLine": 1, "totalLines": 120}});
        assert_eq!(texts(&result_lines(&t, "Read", "     1\tfn main", false, Some(&read))), ["  └ 40 of 120 lines"]);
        let edit = json!({"filePath": "/p/src/calc.py", "structuredPatch": [{
            "oldStart": 1, "oldLines": 3, "newStart": 1, "newLines": 3,
            "lines": [" def add(a, b):", "-    return a - b", "+    return a + b", ""]
        }]});
        // The changes are laid out at the width of the moment: one column when narrow...
        let lines = result_lines(&t, "Edit", "The file was updated", false, Some(&edit));
        assert_eq!(
            texts(&lay_out(lines.clone(), 100)),
            ["  └ 1 addition, 1 removal", "       2 -     return a - b", "       2 +     return a + b"]
        );
        // ...side by side from 140 columns: the old line on the left, the new on the right.
        let wide = texts(&lay_out(lines, 160));
        assert_eq!(wide.len(), 2, "{wide:?}");
        let (old, new) = wide[1].split_once(" │ ").expect("two halves");
        assert_eq!(old.trim(), "2      return a - b");
        assert_eq!(new.trim(), "2      return a + b");
        assert!(crate::tui::text::width(&wide[1]) <= 160);
        // A long edit: at most DIFF_LINES rows, the rest counted.
        let many: Vec<Value> = (0..40).map(|i| json!(format!("+line {i}"))).collect();
        let big = json!({"structuredPatch": [{"oldStart": 1, "newStart": 1, "lines": many}]});
        let rows = texts(&lay_out(result_lines(&t, "Edit", "", false, Some(&big)), 100));
        assert_eq!(rows.last().unwrap(), &format!("    … {} more changed lines", 40 - DIFF_LINES));
        let write = json!({"type": "create", "filePath": "/p/new.txt", "content": "a\nb\n", "structuredPatch": []});
        assert_eq!(texts(&result_lines(&t, "Write", "x", false, Some(&write))), ["  └ 2 lines, a new file"]);
        assert_eq!(texts(&result_lines(&t, "Bash", "", false, None)), ["  └ (no output)"]);
        let long = (1..=6).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        // A command that went well: its last lines, where the summary is.
        assert_eq!(
            texts(&result_lines(&t, "Bash", &long, false, None)),
            ["  └ … 3 earlier lines", "    line 4", "    line 5", "    line 6"]
        );
        assert_eq!(
            texts(&result_lines(&t, "Bash", ".\n1 passed in 0.03s", false, None)),
            ["  └ 1 passed in 0.03s"],
            "no progress dots"
        );
        // A failure shows the error, in red, with what led to it.
        let pytest =
            "Exit code 1\n....F\nFAILED tests/test_calc.py::test_div - ZeroDivisionError\n1 failed, 4 passed in 0.12s";
        let failed = result_lines(&t, "Bash", pytest, true, None);
        assert_eq!(
            texts(&failed),
            [
                "  └ Exit code 1",
                "    FAILED tests/test_calc.py::test_div - ZeroDivisionError",
                "    1 failed, 4 passed in 0.12s"
            ]
        );
        assert_eq!(failed[1].spans[1].style, t.error(), "the error line in red");
        let many = (1..=9).map(|i| format!("step {i}")).collect::<Vec<_>>().join("\n") + "\nerror: boom";
        let lines = texts(&result_lines(&t, "Bash", &many, true, None));
        assert_eq!(lines[0], "  └ … 5 earlier lines");
        assert_eq!(lines.last().unwrap(), "    error: boom");
    }

    #[test]
    fn edit_permission_shows_the_change() {
        let mut a = app();
        let (tx, rx) = oneshot::channel();
        std::mem::forget(rx);
        a.on_event(UiEvent::Ask {
            prompt: PermissionPrompt {
                tool_name: "Edit".into(),
                tool_use_id: "t".into(),
                input: json!({"file_path": "src/calc.py", "old_string": "return a - b", "new_string": "return a + b"}),
                reason: "this tool requires permission".into(),
                suggestions: vec![],
                blocked_path: None,
            },
            reply: tx,
        });
        let (title, body) = a.dialog_text();
        assert_eq!(title, "Approve Edit?");
        assert_eq!(body, ["  src/calc.py", "  - return a - b", "  + return a + b"], "no generic reason line");
        assert!(a.waiting_for_answer());
    }

    #[test]
    fn slash_menu_filters_completes_and_runs() {
        let mut a = app();
        typed(&mut a, "/c");
        let names: Vec<&str> = a.menu().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["clear", "compact"]);
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.editor.text(), "/compact ");
        assert!(a.menu().is_empty(), "the menu closes after the name");
        a.editor.clear();
        typed(&mut a, "/stat");
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Send("/status".into())]);
        a.on_event(UiEvent::Idle);
        typed(&mut a, "/mo");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty(), "a command with arguments waits for them");
        assert_eq!(a.editor.text(), "/model ");
        a.editor.clear();
        typed(&mut a, "/zz");
        assert!(a.menu().is_empty());
        typed(&mut a, "\u{8}");
        a.editor.clear();
        typed(&mut a, "/cl");
        a.on_key(key(KeyCode::Esc));
        assert!(a.menu().is_empty(), "Esc hides the menu");
    }

    #[test]
    fn esc_ctrl_c_and_shift_tab() {
        let mut a = app();
        a.status.mode = "default".into();
        assert_eq!(
            a.on_key(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
            vec![Action::SetMode(PermissionMode::AcceptEdits)]
        );
        assert_eq!(a.status.mode, "acceptEdits");
        typed(&mut a, "x");
        a.on_key(ctrl('c'));
        assert!(a.editor.is_empty(), "Ctrl-C clears first");
        assert!(a.on_key(ctrl('c')).is_empty());
        assert_eq!(a.on_key(ctrl('c')), vec![Action::Exit], "twice exits");
        a.busy = true;
        assert_eq!(a.on_key(key(KeyCode::Esc)), vec![Action::Interrupt]);
        a.busy = false;
        a.on_key(key(KeyCode::Esc));
        assert_eq!(a.on_key(key(KeyCode::Esc)), vec![Action::Send("/rewind".into())]);
        // Ctrl-D during a turn stops it before leaving.
        assert_eq!(a.on_key(ctrl('d')), vec![Action::Interrupt, Action::Exit]);
    }

    fn prompt(tool: &str, input: Value) -> PermissionPrompt {
        PermissionPrompt {
            tool_name: tool.into(),
            tool_use_id: "t".into(),
            input,
            reason: String::new(),
            suggestions: vec![],
            blocked_path: None,
        }
    }

    #[test]
    fn dialogs_answer_permissions_questions_and_plans() {
        let mut a = app();
        let (tx, mut rx) = oneshot::channel();
        let mut p = prompt("Bash", json!({"command": "npm test"}));
        p.suggestions = vec![Suggestion::AddRules {
            rules: vec![serde_json::from_value(json!({"toolName": "Bash", "ruleContent": "npm test"})).unwrap()],
            behavior: kodact_permissions::Behavior::Allow,
            destination: "localSettings".into(),
        }];
        a.on_event(UiEvent::Ask { prompt: p, reply: tx });
        assert_eq!(a.dialog_text().0, "Run this command?");
        assert_eq!(a.dialog_options()[1].0, "Always allow Bash(npm test)");
        a.on_key(key(KeyCode::Char('2')));
        match rx.try_recv().unwrap() {
            PermissionAnswer::Allow { updated_permissions, .. } => assert_eq!(updated_permissions.len(), 1),
            other => panic!("{other:?}"),
        }
        assert!(a.dialog.is_none());

        let (tx, mut rx) = oneshot::channel();
        a.on_event(UiEvent::Ask { prompt: prompt("Write", json!({"file_path": "a.txt"})), reply: tx });
        a.on_key(key(KeyCode::Esc));
        assert!(matches!(rx.try_recv().unwrap(), PermissionAnswer::Deny { interrupt: true, .. }));

        // An outside tool shows everything it will be sent; y, a and n answer.
        let (tx, mut rx) = oneshot::channel();
        let input = json!({"issue": "ACME-103", "body": "Fixed.\nTests pass."});
        a.on_event(UiEvent::Ask { prompt: prompt("mcp__jira__add_comment", input.clone()), reply: tx });
        let (title, body) = a.dialog_text();
        assert_eq!(title, "Approve jira.add_comment?");
        assert!(body.iter().any(|l| l.trim_start().starts_with("issue") && l.ends_with("ACME-103")), "{body:?}");
        assert!(body.iter().any(|l| l.trim() == "Tests pass."), "every line of every argument: {body:?}");
        a.on_key(key(KeyCode::Char('n')));
        assert!(matches!(rx.try_recv().unwrap(), PermissionAnswer::Deny { .. }));
        let (tx, mut rx) = oneshot::channel();
        a.on_event(UiEvent::Ask { prompt: prompt("mcp__jira__add_comment", input.clone()), reply: tx });
        a.on_key(key(KeyCode::Char('y')));
        assert!(matches!(rx.try_recv().unwrap(), PermissionAnswer::Allow { .. }));
        let header = |done| texts(&[Line::from(tool_header(&a.theme, "mcp__jira__add_comment", &input, done))]);
        assert!(header(true)[0].starts_with("Called jira.add_comment "), "{:?}", header(true));
        assert!(header(false)[0].starts_with("Calling jira.add_comment "));

        // Two questions: a pick, then a typed answer.
        let (tx, mut rx) = oneshot::channel();
        let q = json!({"questions": [
            {"question": "Which db?", "options": [{"label": "sqlite"}, {"label": "postgres"}]},
            {"question": "Name?", "options": [{"label": "app"}]}
        ]});
        a.on_event(UiEvent::Ask { prompt: prompt("AskUserQuestion", q), reply: tx });
        a.on_key(key(KeyCode::Down));
        a.on_key(key(KeyCode::Enter));
        assert_eq!(a.dialog_text().0, "Name? (2/2)");
        a.on_key(key(KeyCode::Char('2')));
        typed(&mut a, "kodact");
        a.on_key(key(KeyCode::Enter));
        match rx.try_recv().unwrap() {
            PermissionAnswer::Allow { updated_input: Some(v), .. } => {
                assert_eq!(v["answers"], json!({"Which db?": "postgres", "Name?": "kodact"}))
            }
            other => panic!("{other:?}"),
        }

        // A plan: written to scrollback; "Yes, and accept edits" switches the mode.
        let (tx, mut rx) = oneshot::channel();
        a.take_pending();
        a.on_event(UiEvent::Ask { prompt: prompt("ExitPlanMode", json!({"plan": "1. Edit a\n2. Test"})), reply: tx });
        assert_eq!(texts(&a.take_pending())[1..], ["Here is the plan:", "  1. Edit a", "  2. Test"]);
        a.on_key(key(KeyCode::Enter));
        match rx.try_recv().unwrap() {
            PermissionAnswer::Allow { updated_permissions, .. } => {
                assert_eq!(updated_permissions[0]["mode"], "acceptEdits")
            }
            other => panic!("{other:?}"),
        }
    }

    fn picker() -> Picker {
        use kodact_core::commands::picker::Choice;
        let c = |label: &str, pick: Pick, current: bool| Choice {
            label: label.into(),
            detail: String::new(),
            pick,
            current,
        };
        Picker {
            title: "Pick".into(),
            choices: vec![
                c("Opus", Pick::Run("/model opus".into()), false),
                c("Haiku", Pick::Run("/model haiku".into()), true),
                c("Step", Pick::Step("/rewind 1".into()), false),
                c("Back", Pick::RunThenEdit { command: "/rewind 1 both".into(), edit: "old prompt".into() }, false),
                c("Add", Pick::Edit("/permissions add allow ".into()), false),
            ],
        }
    }

    #[test]
    fn pickers_filter_and_run_command_text() {
        let mut a = app();
        a.on_event(UiEvent::Picker(picker()));
        assert_eq!(a.dialog_selected(), 1, "the current choice is selected");
        assert_eq!(a.dialog_options()[1].0, "Haiku ✔");
        typed(&mut a, "op");
        assert_eq!(a.dialog_options().len(), 1);
        assert_eq!(a.dialog_text().1, ["❯ op▏"]);
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Send("/model opus".into())]);
        assert!(a.dialog.is_none());
        a.on_event(UiEvent::Idle);

        a.on_event(UiEvent::Picker(picker()));
        typed(&mut a, "step");
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Picker("/rewind 1".into())]);
        a.on_event(UiEvent::Picker(picker()));
        typed(&mut a, "back");
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Send("/rewind 1 both".into())]);
        assert_eq!(a.editor.text(), "old prompt", "the rewound prompt is back in the input box");
        a.on_event(UiEvent::Idle);
        a.editor.clear();
        a.on_event(UiEvent::Picker(picker()));
        typed(&mut a, "add");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
        assert_eq!(a.editor.text(), "/permissions add allow ");
        a.on_event(UiEvent::Picker(picker()));
        a.on_key(key(KeyCode::Esc));
        assert!(a.dialog.is_none(), "Esc closes a picker");
    }

    #[test]
    fn ctrl_r_searches_history() {
        let mut a = App::new(
            Theme { color: false, light: false, accent: None, depth: Default::default() },
            vec!["cargo test".into(), "git status".into(), "cargo build".into()],
        );
        typed(&mut a, "draft");
        a.on_key(ctrl('r'));
        typed(&mut a, "cargo");
        assert_eq!(a.editor.text(), "cargo build", "the newest match first");
        a.on_key(ctrl('r'));
        assert_eq!(a.editor.text(), "cargo test", "Ctrl+R again: older");
        a.on_key(ctrl('r'));
        assert_eq!(a.editor.text(), "cargo test", "no older match: it stays");
        a.on_key(key(KeyCode::Esc));
        assert_eq!(a.editor.text(), "draft", "Esc goes back to what was typed");
        assert!(a.search.is_none());
        a.on_key(ctrl('r'));
        typed(&mut a, "stat");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty(), "Enter takes the match, it doesn't send");
        assert_eq!(a.editor.text(), "git status");
        assert!(a.search.is_none());
        // Another key takes the match and acts.
        a.editor.clear();
        a.on_key(ctrl('r'));
        typed(&mut a, "build");
        a.on_key(key(KeyCode::End));
        typed(&mut a, " --release");
        assert_eq!(a.editor.text(), "cargo build --release");
    }

    #[test]
    fn at_completes_paths() {
        let mut a = app();
        a.on_event(UiEvent::Files(vec![
            "src/".into(),
            "src/main.rs".into(),
            "docs/main-notes.md".into(),
            "Cargo.toml".into(),
            "my notes.txt".into(),
        ]));
        typed(&mut a, "explain @mai");
        assert_eq!(a.file_menu(), ["src/main.rs", "docs/main-notes.md"], "name matches, shorter first");
        a.on_key(key(KeyCode::Down));
        assert!(a.on_key(key(KeyCode::Enter)).is_empty(), "Enter completes instead of sending");
        assert_eq!(a.editor.text(), "explain @docs/main-notes.md ");
        assert!(a.file_menu().is_empty());
        typed(&mut a, "and @CARGO");
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.editor.text(), "explain @docs/main-notes.md and @Cargo.toml ");
        typed(&mut a, "@src");
        a.on_key(key(KeyCode::Tab));
        assert!(a.editor.text().ends_with("@src/"), "a directory keeps the cursor in its path: {}", a.editor.text());
        assert_eq!(a.file_menu(), ["src/main.rs"], "and its files are offered next");
        a.on_key(key(KeyCode::Esc));
        assert!(a.file_menu().is_empty(), "Esc hides it");
        assert_eq!(
            a.on_key(key(KeyCode::Enter)),
            vec![Action::Send("explain @docs/main-notes.md and @Cargo.toml @src/".into())]
        );
        a.busy = false;
        typed(&mut a, "@my");
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.editor.text(), "@\"my notes.txt\" ", "spaces are quoted");
    }

    #[test]
    fn attached_files_show_under_the_prompt() {
        let mut a = app();
        let reminder = kodact_agents::attach::reminder(&[kodact_agents::attach::Attachment {
            path: "/w/src/a.rs".into(),
            kind: kodact_agents::attach::Kind::File,
            text: "fn a() {}".into(),
        }])
        .unwrap();
        a.on_event(UiEvent::Engine(EngineEvent::PromptAccepted {
            message: kodact_types::Message::user(vec![
                ContentBlock::text("explain @src/a.rs"),
                ContentBlock::text(reminder),
            ]),
            uuid: "u".into(),
        }));
        assert_eq!(texts(&a.pending), ["  (attached: a.rs)"]);
    }

    #[test]
    fn question_mark_themes_and_clipboard() {
        let mut a = App::new(Theme { color: true, light: false, accent: None, depth: Default::default() }, vec![]);
        a.on_key(key(KeyCode::Char('?')));
        let shown = texts(&a.take_pending()).join("\n");
        assert!(shown.contains("Shift+Tab") && shown.contains("Ctrl+R"), "{shown}");
        assert!(a.editor.is_empty(), "? on an empty prompt isn't typed");
        typed(&mut a, "why?");
        assert_eq!(a.editor.text(), "why?");

        a.on_event(UiEvent::Theme("light".into()));
        assert_eq!(a.theme, Theme { color: true, light: true, accent: None, depth: Default::default() });
        a.on_event(UiEvent::Theme("none".into()));
        assert!(!a.theme.color);
        a.on_event(UiEvent::Theme("dark".into()));
        assert_eq!(a.theme, Theme { color: true, light: false, accent: None, depth: Default::default() });
        // Without colour allowed, no theme brings it back.
        let mut b = App::new(Theme { color: false, light: false, accent: None, depth: Default::default() }, vec![]);
        b.on_event(UiEvent::Theme("light".into()));
        assert!(!b.theme.color);

        a.on_event(UiEvent::Copy("text".into()));
        assert_eq!(a.clipboard.as_deref(), Some("text"));
        a.on_event(UiEvent::StatusLine(Some("custom".into())));
        assert_eq!(a.status_text.as_deref(), Some("custom"));
    }

    #[test]
    fn ui_commands_answer_at_once_while_busy() {
        let mut a = app();
        typed(&mut a, "long task");
        a.on_key(key(KeyCode::Enter));
        a.take_pending();
        typed(&mut a, "/keybindings");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
        assert!(a.queued.is_empty(), "not queued behind the turn");
        assert!(texts(&a.take_pending()).iter().any(|l| l.contains("Shift+Tab")));
        assert!(terminal_setup_text(false).contains("Alt+Enter") && terminal_setup_text(true).contains("works"));
    }

    #[test]
    fn a_tool_call_being_written_says_what_is_being_prepared() {
        use kodact_types::{Delta, StreamEvent};
        let mut a = app();
        let ev = |event| UiEvent::Engine(EngineEvent::Stream { event, parent_tool_use_id: None });
        let start = ContentBlock::ToolUse {
            id: "t1".into(),
            name: "Write".into(),
            input: json!({}),
            cache_control: None,
            provider: None,
        };
        a.on_event(ev(StreamEvent::ContentBlockStart { index: 0, content_block: start }));
        assert_eq!(a.activity, "Writing a file");
        let delta = |j: &str| StreamEvent::ContentBlockDelta {
            index: 0,
            delta: Delta::InputJsonDelta { partial_json: j.into() },
        };
        a.on_event(ev(delta(r#"{"file_path": "/repo/src/no"#)));
        assert_eq!(a.activity, "Writing a file", "a path only counts once its quote closes");
        a.on_event(ev(delta(r##"tes.md", "content": "# Notes"##)));
        assert_eq!(a.activity, "Writing notes.md");
        a.on_event(ev(StreamEvent::ContentBlockStop { index: 0 }));
        assert!(a.preparing.is_none());
        assert_eq!(App::preparing("Edit", Some("a/b/cart.py")), "Preparing an edit to cart.py");
        assert_eq!(App::preparing("Bash", None), "Preparing a command");
        assert_eq!(App::json_string_value(r#"{"path": "a\"b"}"#, "path").as_deref(), Some("a\"b"));
    }
}
