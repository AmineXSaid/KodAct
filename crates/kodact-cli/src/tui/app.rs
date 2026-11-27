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
use kodact_permissions::PermissionMode;
#[cfg(test)]
use kodact_permissions::Suggestion;
use kodact_types::{ApiMessage, ContentBlock, Delta, Role, StreamEvent};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::{json, Value};
use tokio::sync::oneshot;

use super::editor::Editor;
use super::text::{Markdown, Theme};

pub use super::format::*;
pub use super::layout::*;

/// A slash command for the `/` menu.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandInfo {
    pub name: String,
    pub args: String,
    pub description: String,
}

/// Options a dialog's selection moves by on PageUp and PageDown.
const DIALOG_PAGE: usize = 10;

/// What the status line shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StatusView {
    pub model: String,
    pub mode: String,
    pub cwd: String,
    /// The git branch checked out in `cwd` now (empty outside a repository). Read with
    /// each snapshot: the session changes directory (`/cd`) and branches change under it.
    pub branch: String,
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
    /// The answer of an action command (`/status`, `/plugin`, `/reload-plugins`): shown as a
    /// toast or a panel, never written into the conversation.
    Action {
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
#[derive(Debug, PartialEq)]
pub enum Action {
    Send(String),
    /// Send a prompt with attached images (`--image` or clipboard paste).
    SendWithImages(String, Vec<super::clipboard::AttachedImage>),
    /// Read an image from the system clipboard (`Ctrl+V` / `Alt+V`).
    PasteImage,
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
    /// Preview scroll offset (used by permission dialogs).
    scroll: usize,
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

/// Ctrl+F: search through the discussion transcript.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Find {
    pub query: String,
    /// 0-based index into matching wrapped lines (newest-first).
    pub current: usize,
    /// Total matching wrapped lines (updated by `Full`).
    pub total: usize,
}

/// The keyboard shortcuts (`?` on an empty prompt, `/keybindings`).
/// Each row: its default keys, what they do, and the actions it covers (a row may
/// cover two, as "start / end of line" does).
pub const KEYS: &[(&str, &str, &[&str])] = &[
    ("Enter", "Send (queued while a turn runs)", &["submit"]),
    ("Shift+Enter, Alt+Enter, Ctrl+J, \\ Enter", "New line", &["newline"]),
    ("Esc", "Interrupt the turn; close a menu; cancel a dialog", &["cancel"]),
    ("Esc Esc", "On an empty prompt: rewind (/rewind)", &[]),
    ("Ctrl+C", "Clear the input; interrupt the turn; twice on an empty prompt: exit", &["interrupt"]),
    ("Ctrl+D", "Exit (empty prompt)", &["exit"]),
    ("Shift+Tab", "Cycle the permission mode: default, accept edits, plan", &["cycleMode"]),
    ("Up / Down", "Move between lines; earlier prompts", &[]),
    ("Ctrl+R", "Search earlier prompts", &["historySearch"]),
    ("Ctrl+F", "Search in the discussion", &[]),
    ("Tab", "Complete a / command or an @ path", &["complete"]),
    ("@", "Mention a file (a menu of project paths)", &[]),
    ("!", "At the start: run a shell command", &[]),
    ("/", "At the start: a command (the menu lists them)", &[]),
    ("Ctrl+A / Ctrl+E, Home / End", "Start / end of line", &["lineStart", "lineEnd"]),
    ("Ctrl+W, Alt+Backspace", "Delete the word before the cursor", &["deleteWord"]),
    ("Ctrl+U / Ctrl+K", "Delete to the start / end of the line", &["killToStart", "killToEnd"]),
    ("Alt+B / Alt+F, Ctrl+Left / Ctrl+Right", "Word left / right", &["wordLeft", "wordRight"]),
    ("Ctrl+L", "Redraw the screen", &["redraw"]),
    ("Ctrl+B", "Show or hide the sidebar (full screen, 140+ columns)", &["toggleSidebar"]),
    ("Ctrl+O", "Show or hide thoughts and full tool output", &["toggleDetails"]),
    ("Alt+C", "Copy the latest fenced code block (/copy code picks when multiple)", &[]),
];

/// The shortcuts in effect, one per line: the defaults, or with `keybindings.json`
/// the keys each action has now, and the user's own bindings.
pub fn keys_text(keymap: &super::keys::Keymap) -> String {
    let rows: Vec<(String, String)> = KEYS
        .iter()
        .map(|(k, what, actions)| {
            // With bindings of the user's, every action of the row shows the keys it has
            // now. A row of two actions used to keep its default text, so a key bound to
            // "lineStart" or "wordLeft" was missing from the table that lists the keys.
            let keys = if actions.is_empty() || keymap.is_default() {
                k.to_string()
            } else {
                actions.iter().map(|a| keymap.keys_for(a).join(", ")).collect::<Vec<_>>().join(" / ")
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
    /// What each finished sub-agent ran on, by its call's id, for its row's badge.
    ran_on: std::collections::HashMap<String, RanOn>,
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
    pub find: Option<Find>,
    pub menu_selected: usize,
    menu_dismissed: bool,
    last_esc: Option<Instant>,
    last_ctrl_c: Option<Instant>,
    /// A short hint in the status line ("Press Ctrl-C again to exit").
    pub hint: Option<(String, Instant)>,
    /// An action command's one-line answer, in the status line for `TOAST_FOR`: its text,
    /// whether it is an error, and when it came.
    pub toast: Option<(String, bool, Instant)>,
    /// A built-in command sent but not shown: it is written as a prompt only if it turns into
    /// one (the engine accepts a prompt for it), and stays out of the conversation otherwise.
    unechoed: Option<String>,
    /// The action command whose answer is awaited, for the panel's title.
    action: Option<String>,
    /// A mode change not yet noted in the transcript: the mode it left, and whether a turn
    /// was running. Shift+Tab cycles through several modes; only where it lands is written,
    /// once, when the next prompt goes out or the running turn prints again.
    mode_note: Option<(PermissionMode, Instant, bool)>,
    /// Shift+Tab reached Dangerous and waits for `y` before turning it on.
    pub danger_armed: bool,
    /// Dangerous mode was confirmed this session (or the session was launched in it, or
    /// with `--allow-dangerously-skip-permissions`): Shift+Tab goes to it without asking.
    pub danger_confirmed: bool,
    /// Managed settings disable Dangerous mode (`disableBypassPermissionsMode`): the cycle skips it.
    pub danger_blocked: bool,
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
    /// The welcome page's suggestion card that has the keyboard (Tab or ↓ from an empty prompt).
    pub card: Option<usize>,
    /// The last full-screen draw showed the suggestion cards (a short window, an inline
    /// screen or a long note leave them out, and then they must not take keys).
    pub cards_drawn: std::cell::Cell<bool>,
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
    /// The plan's task in progress, as the spinner says it while the model works: its
    /// `activeForm` ("Running the tests"), or its content when it has none.
    pub todo_active: Option<String>,
    /// Where requests go (`127.0.0.1:11434`), for the line under the front screen's input.
    pub endpoint: String,
    /// The git branch, for the status style's bar (empty outside a repository).
    pub branch: String,
    /// `/focus`: tool calls and their results stay out of the scrollback.
    pub focus: bool,
    /// Show expanded thoughts, uncropped diffs, and longer shell outputs (`Ctrl+O`).
    pub verbose: bool,
    /// Incremented whenever detail expansion toggles so full-screen views re-wrap in place.
    pub layout_version: u64,
    /// Whether any collapsible block (thought, truncated diff, long shell tail) was emitted.
    pub has_collapsible: bool,
    /// Whether any fenced code block was emitted in the current transcript.
    pub has_code: bool,
    /// A screen or picker waiting for an open question to be answered.
    held: Option<Dialog>,
    /// Images attached to the next prompt (`--image` or clipboard paste).
    pub pending_images: Vec<super::clipboard::AttachedImage>,
    /// True after the user forced a queued message while waiting for `TurnEnd`.
    pub forcing_queue: bool,
    /// Last rendered inner text width of the input box (0 = unbounded).
    pub input_width: usize,
    /// The conversation on screen was replaced: the loop starts the screen over before it
    /// writes [`App::pending`]. Holds the newest other conversations, for the front page.
    pub reset: Option<Vec<(String, String)>>,
}

/// What the status line says while Dangerous waits for its confirmation (fits 80 cols).
pub const DANGER_PROMPT: &str = "! Dangerous (no prompts): press y to enable, or any other key to skip";

impl App {
    pub fn new(theme: Theme, history: Vec<String>) -> Self {
        App {
            calls: Default::default(),
            ran_on: Default::default(),
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
            find: None,
            menu_selected: 0,
            menu_dismissed: false,
            last_esc: None,
            last_ctrl_c: None,
            hint: None,
            toast: None,
            unechoed: None,
            action: None,
            mode_note: None,
            danger_armed: false,
            danger_confirmed: false,
            danger_blocked: false,
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
            card: None,
            cards_drawn: std::cell::Cell::new(false),
            first_run: false,
            sidebar: false,
            title: String::new(),
            mcp: vec![],
            mcp_tools: Default::default(),
            skills: vec![],
            skills_used: vec![],
            todos: vec![],
            todo_active: None,
            endpoint: String::new(),
            branch: String::new(),
            focus: false,
            verbose: false,
            layout_version: 0,
            has_collapsible: false,
            has_code: false,
            held: None,
            pending_images: Vec::new(),
            forcing_queue: false,
            input_width: 0,
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
            if line.spans.first().is_some_and(|s| s.content == "╭─") {
                self.has_code = true;
            }
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

    /// The reasoning that just ended goes into the transcript as a collapsible
    /// placeholder line, collapsed to a 1-line summary until toggled via `Ctrl+O`.
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
        let lines = text.lines().map(str::trim).filter(|l| !l.is_empty()).count().max(1);
        self.has_collapsible = true;
        self.gap();
        self.pending.push(Line::from(vec![
            Span::styled(THOUGHT_MARK, t.subtle()),
            Span::styled("", t.subtle().add_modifier(Modifier::ITALIC)),
            Span::raw(serde_json::json!({ "secs": secs, "lines": lines, "body": text }).to_string()),
        ]));
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
        // Skills typed anywhere in the prompt (`/simplify the parser` or `check /simplify`).
        for (_, token) in kodact_core::commands::inline_tokens(text) {
            let trimmed = token.trim_end_matches(['.', ',', ':', ';', ')', ']', '!', '?', '"', '\'']);
            if let Some(skill) = self
                .skills
                .iter()
                .find(|s| s.eq_ignore_ascii_case(token) || (!trimmed.is_empty() && s.eq_ignore_ascii_case(trimmed)))
                .cloned()
            {
                self.used_skill(&skill);
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
            // The input's own mark, in the colour of the mode the words were sent in, on a
            // subtle raised surface band so user turns separate cleanly from replies.
            Look::Frame => {
                let mark = if t.color { t.mode(&self.status.mode) } else { t.bold() };
                let band = (t.color && t.depth != super::palette::Depth::Ansi16)
                    .then(|| Style::default().bg(super::palette::PROMPT_BG.color(t.depth, t.light)));
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
                    let mut line = Line::from(vec![lead, Span::styled(l.to_string(), words)]);
                    if let Some(st) = band {
                        line = line.style(st);
                    }
                    self.pending.push(line);
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
        // A sub-agent on another model says which, and where: before how it ended.
        if let Some(ran) = self.ran_on.remove(&r.id) {
            let at = line.spans.iter().rposition(|s| s.content.starts_with(" · ")).unwrap_or(line.spans.len());
            for (i, s) in model_badge(&t, &ran).into_iter().enumerate() {
                line.spans.insert(at + i, s);
            }
        }
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
        // A running turn prints again: note the mode it now runs under, once Shift+Tab rests.
        if self.mode_note.is_some_and(|(_, at, _)| at.elapsed() >= super::render::HINT_FOR) {
            self.note_mode();
        }
        match ev {
            UiEvent::Engine(e) => self.on_engine(e),
            UiEvent::Ask { prompt, reply } => self.open_dialog(prompt, reply),
            UiEvent::Reply { text, is_error } => {
                if !text.trim().is_empty() {
                    self.reply_lines(&text, is_error);
                }
            }
            UiEvent::Action { text, is_error } => {
                self.unechoed = None;
                self.show_action(&text, is_error);
            }
            UiEvent::Status(s) => {
                if s.model != self.status.model {
                    self.model_down = false;
                }
                self.branch = s.branch.clone();
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
                // A command that answered nothing leaves nothing behind.
                self.unechoed = None;
                self.action = None;
                self.flush_live();
                for r in std::mem::take(&mut self.running) {
                    self.commit_tool(r, ToolEnd::Stopped);
                }
                self.turn_summary();
                self.busy = false;
                self.forcing_queue = false;
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
        self.todo_active = None;
        self.skills_used.clear();
        self.agents_done = 0;
        self.forcing_queue = false;
        self.title.clear();
        self.talked = !replay.is_empty();
        self.front = replay.is_empty();
        self.first_run = false;
        self.has_collapsible = false;
        self.has_code = false;
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
                            self.todo_active = active_task(input);
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
                            // The model it really ran on, from its result.
                            if let Some(ran) =
                                tool_use_result.as_ref().and_then(|v| RanOn::from_result(&input, v, &self.status.model))
                            {
                                self.ran_on.insert(tool_use_id.clone(), ran);
                            }
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
                            if super::shellbox::is_collapsible(&output) {
                                self.has_collapsible = true;
                            }
                            let state = super::shellbox::State::Ended { code, failed };
                            vec![super::shellbox::placeholder(&self.theme, &cmd, &output, state)]
                        } else {
                            let is_collapsible_diff = matches!(tool.as_str(), "Edit" | "MultiEdit" | "Write")
                                && tool_use_result
                                    .as_ref()
                                    .and_then(|v| v.get("structuredPatch"))
                                    .and_then(Value::as_array)
                                    .is_some_and(|hunks| {
                                        hunks
                                            .iter()
                                            .flat_map(|h| h["lines"].as_array().into_iter().flatten())
                                            .filter_map(Value::as_str)
                                            .filter(|l| l.starts_with('+') || l.starts_with('-'))
                                            .count()
                                            > DIFF_LINES
                                    });
                            if is_collapsible_diff {
                                self.has_collapsible = true;
                            }
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
                // A built-in that sent a prompt (`/plan <description>`) is part of the conversation.
                if let Some(text) = self.unechoed.take() {
                    self.action = None;
                    self.echo_prompt(&text);
                    self.activity = "Working".into();
                    self.first_line = true;
                    self.talked = true;
                }
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
        self.danger_armed = false;
        self.flush_live();
        let mut typing = None;
        let kind = match prompt.tool_name.as_str() {
            "AskUserQuestion" => {
                let questions: Vec<Question> = kodact_tools::builtin::parse_questions(&prompt.input)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|q| Question { question: q.question, options: q.options, multi: q.multi })
                    .collect();
                let Some(first) = questions.first() else {
                    let _ = reply.send(PermissionAnswer::Deny {
                        message: "No questions were provided.".into(),
                        interrupt: false,
                    });
                    return;
                };
                if first.options.is_empty() {
                    typing = Some(Editor::default());
                }
                let picked = vec![false; first.options.len()];
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
        self.dialog = Some(Dialog { prompt: Some(prompt), reply: Some(reply), kind, selected, scroll: 0, typing });
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
                let Some(q) = questions.get(*at) else { return vec![] };
                if q.options.is_empty() {
                    return vec![];
                }
                let mut v: Vec<(String, String)> = q
                    .options
                    .iter()
                    .enumerate()
                    .map(|(i, (l, desc))| {
                        let checked = picked.get(i).copied().unwrap_or(false);
                        let label =
                            if q.multi { format!("[{}] {l}", if checked { "x" } else { " " }) } else { l.clone() };
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
                .map(|c| (if c.current { format!("{} ✓", c.label) } else { c.label.clone() }, c.detail.clone()))
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
            ("AskUserQuestion", _) => "Preparing a question".into(),
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

    /// Custom footer hint for question dialogs (typing mode, multi-select).
    pub fn dialog_question_hint(&self) -> Option<&'static str> {
        let d = self.dialog.as_ref()?;
        let DialogKind::Questions { questions, at, .. } = &d.kind else { return None };
        let q = questions.get(*at)?;
        if d.typing.is_some() {
            Some(if q.options.is_empty() {
                "Enter to submit · Esc to cancel"
            } else {
                "Enter to submit · Esc to go back"
            })
        } else if q.multi {
            Some("Space to toggle · Enter to confirm · Esc to cancel")
        } else {
            Some("Enter to select · Esc to cancel")
        }
    }

    /// The tool call the open question is about.
    pub fn asking_for(&self) -> Option<&str> {
        let d = self.dialog.as_ref()?;
        matches!(d.kind, DialogKind::Permission { .. } | DialogKind::Plan | DialogKind::Questions { .. })
            .then(|| d.prompt.as_ref().map(|p| p.tool_use_id.as_str()))?
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
                    body.extend(p.input["command"].as_str().unwrap_or("").lines().take(100).map(|l| format!("  {l}")));
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
                let Some(q) = questions.get(*at) else { return (String::new(), vec![]) };
                let n = questions.len();
                let title = if n > 1 { format!("{} ({}/{n})", q.question, at + 1) } else { q.question.clone() };
                let mut body = vec![];
                if let Some(e) = &d.typing {
                    body.push(format!("❯ {}", e.text_with_cursor()));
                }
                (title, body)
            }
        }
    }

    /// Number of leading command lines in `dialog_text().1` when prompting for `Bash`.
    pub fn dialog_bash_cmd_lines(&self) -> usize {
        let Some(d) = &self.dialog else { return 0 };
        let DialogKind::Permission { .. } = d.kind else { return 0 };
        let Some(p) = &d.prompt else { return 0 };
        if p.tool_name != "Bash" {
            return 0;
        }
        p.input["command"].as_str().unwrap_or("").lines().take(100).count()
    }

    pub fn dialog_selected(&self) -> usize {
        self.dialog.as_ref().map(|d| d.selected).unwrap_or(0)
    }

    pub fn dialog_scroll(&self) -> usize {
        self.dialog.as_ref().map(|d| d.scroll).unwrap_or(0)
    }

    pub fn scroll_dialog(&mut self, delta: isize) -> bool {
        let Some(d) = self.dialog.as_ref() else { return false };
        if !matches!(d.kind, DialogKind::Permission { .. }) {
            return false;
        }
        let max_scroll = self.dialog_text().1.len().saturating_sub(1);
        if let Some(d) = self.dialog.as_mut() {
            d.scroll = d.scroll.saturating_add_signed(delta).min(max_scroll);
        }
        self.dirty = true;
        true
    }

    fn cancel_question_dialog(&mut self) {
        self.answer(PermissionAnswer::Deny {
            message: "The user cancelled the question. Wait for their instructions on what to do instead.".into(),
            interrupt: true,
        });
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
                KeyCode::PageUp => d.selected = d.selected.saturating_sub(DIALOG_PAGE),
                KeyCode::PageDown => d.selected = (d.selected + DIALOG_PAGE).min(n.saturating_sub(1)),
                KeyCode::Home => d.selected = 0,
                KeyCode::End => d.selected = n.saturating_sub(1),
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
        if d.typing.is_some() {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let alt = key.modifiers.contains(KeyModifiers::ALT);
            let no_options = match &d.kind {
                DialogKind::Questions { questions, at, .. } => questions.get(*at).is_none_or(|q| q.options.is_empty()),
                _ => false,
            };
            match key.code {
                KeyCode::Enter => {
                    let text = d.typing.as_ref().map(Editor::text).unwrap_or_default();
                    let trimmed = text.trim().to_string();
                    if !trimmed.is_empty() {
                        d.typing = None;
                        self.answer_question(trimmed);
                    } else {
                        self.set_hint("Type an answer first, or press Esc");
                    }
                }
                KeyCode::Esc => {
                    if no_options {
                        self.cancel_question_dialog();
                    } else {
                        d.typing = None;
                    }
                }
                KeyCode::Backspace if alt || ctrl => {
                    if let Some(e) = d.typing.as_mut() {
                        e.delete_word();
                    }
                }
                KeyCode::Backspace => {
                    if let Some(e) = d.typing.as_mut() {
                        e.backspace();
                    }
                }
                KeyCode::Delete => {
                    if let Some(e) = d.typing.as_mut() {
                        e.delete();
                    }
                }
                KeyCode::Left if ctrl => {
                    if let Some(e) = d.typing.as_mut() {
                        e.word_left();
                    }
                }
                KeyCode::Right if ctrl => {
                    if let Some(e) = d.typing.as_mut() {
                        e.word_right();
                    }
                }
                KeyCode::Left => {
                    if let Some(e) = d.typing.as_mut() {
                        e.left();
                    }
                }
                KeyCode::Right => {
                    if let Some(e) = d.typing.as_mut() {
                        e.right();
                    }
                }
                KeyCode::Home => {
                    if let Some(e) = d.typing.as_mut() {
                        e.home();
                    }
                }
                KeyCode::End => {
                    if let Some(e) = d.typing.as_mut() {
                        e.end();
                    }
                }
                KeyCode::Char('a') if ctrl => {
                    if let Some(e) = d.typing.as_mut() {
                        e.home();
                    }
                }
                KeyCode::Char('e') if ctrl => {
                    if let Some(e) = d.typing.as_mut() {
                        e.end();
                    }
                }
                KeyCode::Char('w') if ctrl => {
                    if let Some(e) = d.typing.as_mut() {
                        e.delete_word();
                    }
                }
                KeyCode::Char('u') if ctrl => {
                    if let Some(e) = d.typing.as_mut() {
                        e.kill_to_start();
                    }
                }
                KeyCode::Char('k') if ctrl => {
                    if let Some(e) = d.typing.as_mut() {
                        e.kill_to_end();
                    }
                }
                KeyCode::Char('z') if ctrl => {
                    if let Some(e) = d.typing.as_mut() {
                        e.undo();
                    }
                }
                KeyCode::Char('y') if ctrl => {
                    if let Some(e) = d.typing.as_mut() {
                        if !e.redo() {
                            e.yank();
                        }
                    }
                }
                KeyCode::Char('b') if alt => {
                    if let Some(e) = d.typing.as_mut() {
                        e.word_left();
                    }
                }
                KeyCode::Char('f') if alt => {
                    if let Some(e) = d.typing.as_mut() {
                        e.word_right();
                    }
                }
                KeyCode::Char(c) if !ctrl && c != '\n' && c != '\r' => {
                    if let Some(e) = d.typing.as_mut() {
                        e.insert(&c.to_string());
                    }
                }
                _ => {}
            }
            return vec![];
        }
        if let DialogKind::Permission { always, danger } = &d.kind {
            if key.code == KeyCode::BackTab {
                return self.cycle_mode();
            }
            if matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
                let delta = if key.code == KeyCode::PageUp { -(DIALOG_PAGE as isize) } else { DIALOG_PAGE as isize };
                self.scroll_dialog(delta);
                return vec![];
            }
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
            // A long list (an endpoint's models, a folder's conversations) by the page.
            KeyCode::PageUp => d.selected = d.selected.saturating_sub(DIALOG_PAGE),
            KeyCode::PageDown => d.selected = (d.selected + DIALOG_PAGE).min(n.saturating_sub(1)),
            KeyCode::Home => d.selected = 0,
            KeyCode::End => d.selected = n.saturating_sub(1),
            KeyCode::Char(c) if c.is_ascii_digit() && c != '0' && !key.modifiers.contains(KeyModifiers::CONTROL) => {
                let i = c as usize - '1' as usize;
                if i < n {
                    if let DialogKind::Questions { questions, at, picked, .. } = &mut d.kind {
                        if questions.get(*at).is_some_and(|q| q.multi) && i < picked.len() {
                            d.selected = i;
                            picked[i] = !picked[i];
                            return vec![];
                        }
                    }
                    d.selected = i;
                    return self.choose(i);
                }
            }
            KeyCode::Char(' ') => {
                if let DialogKind::Questions { questions, at, picked, .. } = &mut d.kind {
                    if questions.get(*at).is_some_and(|q| q.multi) && d.selected < picked.len() {
                        picked[d.selected] = !picked[d.selected];
                    }
                }
            }
            KeyCode::Enter => {
                let i = d.selected;
                return self.choose(i);
            }
            KeyCode::Esc => {
                if matches!(d.kind, DialogKind::Questions { .. }) {
                    self.cancel_question_dialog();
                    return vec![];
                }
                return self.choose(n.saturating_sub(1));
            }
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
                let Some(q) = questions.get(*at) else { return vec![] };
                if i >= q.options.len() || i + 1 == n {
                    if d.typing.is_none() {
                        d.typing = Some(Editor::default());
                    }
                    return vec![];
                }
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
        let Some(cur) = questions.get(*at) else { return };
        answers.insert(cur.question.clone(), json!(answer));
        if *at + 1 < questions.len() {
            *at += 1;
            let next_opts = questions[*at].options.len();
            *picked = vec![false; next_opts];
            d.selected = 0;
            d.typing = (next_opts == 0).then(Editor::default);
            return;
        }
        let mut input = d.prompt.as_ref().map(|p| p.input.clone()).unwrap_or_else(|| json!({}));
        if !input.is_object() {
            input = json!({});
        }
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
            scroll: 0,
            typing: None,
        });
    }

    // ---- screens ----

    /// Show a screen (`/diff`, `/context`, `/hooks`, `/agents`).
    pub fn open_screen(&mut self, screen: Screen) {
        self.flush_live();
        let kind = DialogKind::Viewer { screen, cursor: 0, top: 0, back: vec![] };
        self.show(Dialog { prompt: None, reply: None, kind, selected: 0, scroll: 0, typing: None });
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
                        scroll: 0,
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

    /// Commands matching the `/word` before the cursor. At the start of the prompt, all
    /// commands are offered; mid-prompt, only skills, custom commands, and MCP prompts.
    pub fn menu(&self) -> Vec<&CommandInfo> {
        if self.menu_dismissed || self.dialog.is_some() || self.search.is_some() || self.find.is_some() {
            return vec![];
        }
        let (_, word) = self.editor.word_before_cursor();
        let Some(q) = word.strip_prefix('/') else { return vec![] };
        if q.contains('/') || q.contains('\\') {
            return vec![];
        }
        let q = q.to_lowercase();
        let sole = self.editor.word_is_sole_prompt();
        let is_skill = |name: &str| self.skills.iter().any(|s| s.eq_ignore_ascii_case(name));
        let eligible = |c: &&CommandInfo| sole || is_skill(&c.name) || kodact_core::commands::lookup(&c.name).is_none();
        let skill_kw = q.len() >= 2 && ("skill".starts_with(&q) || q == "skills");
        let mut starts: Vec<&CommandInfo> =
            self.commands.iter().filter(eligible).filter(|c| c.name.to_lowercase().starts_with(&q)).collect();
        let contains = self
            .commands
            .iter()
            .filter(eligible)
            .filter(|c| !c.name.to_lowercase().starts_with(&q) && c.name.to_lowercase().contains(&q));
        starts.extend(contains);
        if skill_kw {
            let by_skill = self.commands.iter().filter(eligible).filter(|c| {
                is_skill(&c.name) && !c.name.to_lowercase().starts_with(&q) && !c.name.to_lowercase().contains(&q)
            });
            starts.extend(by_skill);
        }
        starts
    }

    fn complete_command(&mut self, name: &str, trailing_space: bool) {
        let (_, word) = self.editor.word_before_cursor();
        let has_h_space = matches!(self.editor.char_after_cursor(), Some(' ' | '\t'));
        let replacement = if trailing_space && !has_h_space { format!("/{name} ") } else { format!("/{name}") };
        self.editor.replace_back(word.chars().count(), &replacement);
        if trailing_space && has_h_space {
            self.editor.right();
        }
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
        let q = query.to_lowercase();
        (0..end).rev().find(|&i| h[i].to_lowercase().contains(&q))
    }

    /// All history entries matching the active `Ctrl+R` query, newest first.
    pub fn search_matches(&self) -> Vec<(usize, &str)> {
        let Some(s) = &self.search else { return vec![] };
        let q = s.query.to_lowercase();
        self.editor
            .history()
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, entry)| entry.to_lowercase().contains(&q))
            .map(|(i, entry)| (i, entry.as_str()))
            .collect()
    }

    fn search_key(&mut self, key: KeyEvent) -> Option<Vec<Action>> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let mut s = self.search.take()?;
        match key.code {
            KeyCode::Char('r') if ctrl => {
                let older = s.hit.and_then(|h| h.checked_sub(1)).and_then(|o| self.find_history(&s.query, Some(o)));
                s.hit = older.or(s.hit).or_else(|| self.find_history(&s.query, None));
            }
            KeyCode::Up => {
                let older = s.hit.and_then(|h| h.checked_sub(1)).and_then(|o| self.find_history(&s.query, Some(o)));
                s.hit = older.or(s.hit);
            }
            KeyCode::Down => {
                let h = self.editor.history();
                let q = s.query.to_lowercase();
                if let Some(cur) = s.hit {
                    let newer = ((cur + 1)..h.len()).find(|&i| h[i].to_lowercase().contains(&q));
                    if let Some(n) = newer {
                        s.hit = Some(n);
                    }
                }
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

    fn find_key(&mut self, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let Some(mut f) = self.find.take() else { return false };
        match key.code {
            KeyCode::Esc => return true,
            KeyCode::Char('g') | KeyCode::Char('c') if ctrl => return true,
            KeyCode::Char('f') | KeyCode::Char('n') if ctrl => {
                if f.total > 0 {
                    f.current = (f.current + 1) % f.total;
                }
            }
            KeyCode::Char('p') if ctrl => {
                if f.total > 0 {
                    f.current = f.current.checked_sub(1).unwrap_or(f.total - 1);
                }
            }
            KeyCode::Enter | KeyCode::Up => {
                if f.total > 0 {
                    f.current = (f.current + 1) % f.total;
                }
            }
            KeyCode::Down => {
                if f.total > 0 {
                    f.current = f.current.checked_sub(1).unwrap_or(f.total - 1);
                }
            }
            KeyCode::Backspace => {
                f.query.pop();
                f.current = 0;
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                f.query.push(c);
                f.current = 0;
            }
            _ => {
                self.find = Some(f);
                return false;
            }
        }
        self.find = Some(f);
        true
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
                self.danger_armed = false;
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

    fn cycle_mode(&mut self) -> Vec<Action> {
        let cur = PermissionMode::parse(&self.status.mode).unwrap_or(PermissionMode::Default);
        let next = cur.next(!self.danger_blocked);
        if next == PermissionMode::BypassPermissions && !self.danger_confirmed {
            self.danger_armed = true;
            return vec![];
        }
        self.set_mode(next)
    }

    fn set_mode(&mut self, mode: PermissionMode) -> Vec<Action> {
        let from = PermissionMode::parse(&self.status.mode).unwrap_or_default();
        self.status.mode = mode.as_str().to_string();
        // The status line names the mode at once; the transcript waits until the cycling stops.
        let left = self.mode_note.map_or(from, |(left, _, _)| left);
        let busy = self.busy || self.mode_note.is_some_and(|(_, _, b)| b);
        self.mode_note = Some((left, Instant::now(), busy));
        self.set_hint(&format!("{} mode · {}", mode.label(), mode.definition()));

        let auto_allow = match &self.dialog {
            Some(Dialog { kind: DialogKind::Permission { .. }, prompt: Some(p), .. }) => match mode {
                PermissionMode::BypassPermissions => true,
                PermissionMode::AcceptEdits | PermissionMode::Auto => {
                    matches!(p.tool_name.as_str(), "Edit" | "MultiEdit" | "Write" | "NotebookEdit")
                }
                _ => false,
            },
            _ => false,
        };
        if auto_allow {
            self.answer(PermissionAnswer::Allow { updated_input: None, updated_permissions: vec![] });
        }
        vec![Action::SetMode(mode)]
    }

    /// Write the pending mode change into the transcript, once: nothing when Shift+Tab came
    /// back to the mode it started from.
    fn note_mode(&mut self) {
        let Some((left, _, busy)) = self.mode_note.take() else { return };
        let mode = PermissionMode::parse(&self.status.mode).unwrap_or_default();
        if mode == left {
            return;
        }
        let m_str = mode.as_str();
        self.gap();
        let t = self.theme;
        let mut spans = vec![
            Span::styled("── ", t.hairline()),
            Span::styled("◆ ", t.mode(m_str)),
            Span::styled(format!("{} mode", mode.label()), t.mode(m_str).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" · {}", mode.short_rule()), t.subtle()),
            Span::styled(format!(" - {}", mode.definition()), t.dim()),
        ];
        if busy {
            spans.push(Span::styled(" (active immediately)", t.subtle()));
        }
        spans.push(Span::styled(" ──", t.hairline()));
        self.pending.push(Line::from(spans));
    }

    /// The welcome page's suggestion cards. From an empty prompt Tab or ↓ gives the
    /// keyboard to the first card; ←/→ (or Tab) move along; Enter sends the
    /// card's prompt; Esc or ↑ hands the keyboard back to the prompt, and so does typing,
    /// with the key typed. `None` when the cards don't take the key.
    fn card_key(&mut self, key: KeyEvent, modified: bool) -> Option<Vec<Action>> {
        let cards_shown = self.front && self.look == super::look::Look::Frame && !self.busy && self.cards_drawn.get();
        if !cards_shown || !self.editor.is_empty() {
            self.card = None;
            return None;
        }
        let n = super::front::CARDS.len();
        let Some(i) = self.card else {
            if !modified && matches!(key.code, KeyCode::Tab | KeyCode::Down) && key.modifiers.is_empty() {
                self.card = Some(0);
                return Some(vec![]);
            }
            return None;
        };
        match key.code {
            KeyCode::Right | KeyCode::Tab => self.card = Some((i + 1) % n),
            KeyCode::Left => self.card = Some((i + n - 1) % n),
            KeyCode::Esc | KeyCode::Up => self.card = None,
            KeyCode::Enter => {
                self.card = None;
                return Some(self.submit(super::front::CARDS[i].3.to_string()));
            }
            // Anything else goes to the prompt, which has the keyboard again.
            _ => {
                self.card = None;
                return None;
            }
        }
        Some(vec![])
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Vec<Action> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let now = Instant::now();
        // Dangerous mode waits for `y` the first time in a session: `y` turns it on,
        // and any other key skips past Dangerous to Manual (`default`).
        if self.danger_armed && (!ctrl || key.code != KeyCode::Char('c')) {
            self.danger_armed = false;
            return if key.code == KeyCode::Char('y') && !ctrl && !alt {
                self.danger_confirmed = true;
                self.set_mode(PermissionMode::BypassPermissions)
            } else {
                self.set_mode(PermissionMode::Default)
            };
        }
        // Ctrl-C: clear, interrupt, or (twice) exit; it works in dialogs too.
        if ctrl && key.code == KeyCode::Char('c') {
            self.danger_armed = false;
            self.search = None;
            self.find = None;
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
        if self.find.is_some() && self.find_key(key) {
            return vec![];
        }
        if self.search.is_some() {
            if let Some(a) = self.search_key(key) {
                return a;
            }
        }
        if let Some(a) = self.card_key(key, ctrl || alt) {
            return a;
        }
        if ctrl && key.code == KeyCode::Char('b') {
            self.sidebar = !self.sidebar;
            return vec![];
        }
        if ctrl && key.code == KeyCode::Char('o') {
            self.verbose = !self.verbose;
            self.layout_version = self.layout_version.wrapping_add(1);
            self.set_hint(if self.verbose {
                "Expanded thoughts and outputs (ctrl+o to collapse)"
            } else {
                "Collapsed thoughts and outputs"
            });
            return vec![];
        }
        if ctrl && key.code == KeyCode::Char('f') {
            self.search = None;
            self.find = Some(Find::default());
            return vec![];
        }
        if ctrl && key.code == KeyCode::Char('r') {
            self.find = None;
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
            return self.cycle_mode();
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
            KeyCode::Enter | KeyCode::Char('\n') | KeyCode::Char('\r')
                if key.modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::ALT) =>
            {
                self.editor.insert("\n")
            }
            KeyCode::Char('j') if ctrl => self.editor.insert("\n"),
            KeyCode::Char('c') if alt && !ctrl => return vec![Action::Send("/copy code last".into())],
            KeyCode::Enter if ctrl && self.busy && menu_len == 0 => {
                let text = self.editor.text();
                if !text.trim().is_empty() {
                    let text = self.editor.take();
                    let actions = self.submit(text);
                    if !actions.is_empty() {
                        return actions;
                    }
                }
                if !self.queued.is_empty() && !self.forcing_queue {
                    self.forcing_queue = true;
                    self.flash_hint("Stopping current turn to send queued message");
                    return vec![Action::Interrupt];
                }
                return vec![];
            }
            KeyCode::Enter => {
                if menu_len > 0 {
                    let c = self.menu()[self.menu_selected.min(menu_len - 1)].clone();
                    let (_, word) = self.editor.word_before_cursor();
                    let typed = word.strip_prefix('/').unwrap_or(&word);
                    if self.editor.word_is_sole_prompt() {
                        // A command that takes arguments waits for them; one that doesn't runs now.
                        if c.args.is_empty() || (typed == c.name && c.args.starts_with('[')) {
                            self.complete_command(&c.name, false);
                        } else {
                            self.complete_command(&c.name, true);
                            return vec![];
                        }
                    } else if !typed.eq_ignore_ascii_case(&c.name) {
                        self.complete_command(&c.name, true);
                        self.menu_selected = 0;
                        return vec![];
                    }
                }
                let text = self.editor.text();
                if let Some(stripped) = text.strip_suffix('\\') {
                    self.editor.set(&format!("{stripped}\n"));
                    return vec![];
                }
                if text.trim().is_empty() {
                    if self.busy && !self.queued.is_empty() && !self.forcing_queue {
                        self.forcing_queue = true;
                        self.flash_hint("Stopping current turn to send queued message");
                        return vec![Action::Interrupt];
                    }
                    return vec![];
                }
                let text = self.editor.take();
                return self.submit(text);
            }
            KeyCode::Tab if menu_len > 0 => {
                let c = self.menu()[self.menu_selected.min(menu_len - 1)].clone();
                self.complete_command(&c.name, true);
            }
            KeyCode::Up if menu_len > 0 => {
                self.menu_selected = self.menu_selected.checked_sub(1).unwrap_or(menu_len - 1);
                return vec![];
            }
            KeyCode::Down if menu_len > 0 => {
                self.menu_selected = (self.menu_selected + 1) % menu_len;
                return vec![];
            }
            // A message waiting behind the turn can be taken back: Up on an empty prompt
            // puts the newest one in the input, to change or to drop (Ctrl+U). Before,
            // a queued message could only be sent.
            KeyCode::Up if self.editor.is_empty() && !self.queued.is_empty() => {
                if let Some(text) = self.queued.pop_back() {
                    self.editor.set(&text);
                    if self.queued.is_empty() {
                        self.forcing_queue = false;
                    }
                }
            }
            KeyCode::Up => {
                let w = if self.input_width > 0 { self.input_width } else { usize::MAX };
                self.editor.up_wrapped(w);
            }
            KeyCode::Down => {
                let w = if self.input_width > 0 { self.input_width } else { usize::MAX };
                self.editor.down_wrapped(w);
            }
            KeyCode::Char('p') if ctrl => {
                let w = if self.input_width > 0 { self.input_width } else { usize::MAX };
                self.editor.up_wrapped(w);
            }
            KeyCode::Char('n') if ctrl => {
                let w = if self.input_width > 0 { self.input_width } else { usize::MAX };
                self.editor.down_wrapped(w);
            }
            KeyCode::Left if ctrl => self.editor.word_left(),
            KeyCode::Right if ctrl => self.editor.word_right(),
            KeyCode::Left => self.editor.left(),
            KeyCode::Right => self.editor.right(),
            KeyCode::Home => self.editor.home(),
            KeyCode::End => self.editor.end(),
            KeyCode::Char('v') if alt || ctrl => return vec![Action::PasteImage],
            KeyCode::Backspace if self.editor.is_empty() && !self.pending_images.is_empty() => {
                self.pending_images.pop();
                self.flash_hint("Removed attached image");
            }
            KeyCode::Backspace if alt || ctrl => self.editor.delete_word(),
            KeyCode::Backspace => self.editor.backspace(),
            KeyCode::Delete => self.editor.delete(),
            KeyCode::Char('a') if ctrl => self.editor.home(),
            KeyCode::Char('e') if ctrl => self.editor.end(),
            KeyCode::Char('w') if ctrl => self.editor.delete_word(),
            KeyCode::Char('u') if ctrl => self.editor.kill_to_start(),
            KeyCode::Char('k') if ctrl => self.editor.kill_to_end(),
            KeyCode::Char('z') if ctrl && key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.editor.redo();
            }
            KeyCode::Char('Z') if ctrl => {
                self.editor.redo();
            }
            KeyCode::Char('z') if ctrl => {
                self.editor.undo();
            }
            KeyCode::Char('y') if ctrl => {
                if !self.editor.redo() {
                    self.editor.yank();
                }
            }
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
    /// Show an action command's answer without writing it into the conversation: one short
    /// line is a toast in the status line, anything longer a panel that closes with Esc.
    fn show_action(&mut self, text: &str, is_error: bool) {
        let title = self.action.take().unwrap_or_default();
        let text = text.trim_end();
        if text.trim().is_empty() {
            return;
        }
        if !text.contains('\n') && text.chars().count() <= TOAST_MAX_CHARS {
            self.toast = Some((text.trim().to_string(), is_error, Instant::now()));
            return;
        }
        self.open_screen(action_panel(&title, text, is_error));
    }

    /// Show a transient status hint for `HINT_FOR` seconds.
    pub fn flash_hint(&mut self, msg: impl Into<String>) {
        self.hint = Some((msg.into(), Instant::now()));
    }

    /// Attach an image to the next prompt (enforcing `MAX_IMAGES`).
    pub fn attach_image(&mut self, img: super::clipboard::AttachedImage) -> Result<(), String> {
        if self.pending_images.len() >= super::clipboard::MAX_IMAGES {
            return Err(format!("at most {} images can be attached per prompt", super::clipboard::MAX_IMAGES));
        }
        let label = img.pill_label(self.pending_images.len() + 1);
        self.pending_images.push(img);
        self.flash_hint(format!("Attached {label}"));
        Ok(())
    }

    pub fn submit(&mut self, text: String) -> Vec<Action> {
        self.note_mode();
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
            self.action = Some(text.trim().to_string());
            self.show_action(&answer, is_error);
            return vec![];
        }
        if self.busy {
            // Immediate commands (/status, /usage, /btw ...) are answered from the session view
            // while the turn runs (C17), as a toast or a panel; anything else waits for the
            // turn to end.
            if kodact_core::commands::immediate(&text, &kodact_core::commands::Catalog::default()) {
                self.action = Some(text.trim().to_string());
                return vec![Action::Send(text)];
            }
            self.queued.push_back(text);
            return vec![];
        }
        if is_action_candidate(&text) && self.pending_images.is_empty() {
            // A built-in: shown only if it turns into a prompt (see `unechoed`).
            self.action = Some(text.trim().to_string());
            self.unechoed = Some(text.clone());
            self.busy = true;
            self.busy_since = Some(Instant::now());
            self.activity = format!("Running {}", text.split_whitespace().next().unwrap_or(""));
            return vec![Action::Send(text)];
        }
        if !self.pending_images.is_empty() {
            let imgs = std::mem::take(&mut self.pending_images);
            let badges: Vec<String> =
                imgs.iter().enumerate().map(|(i, img)| format!("[{}]", img.pill_label(i + 1))).collect();
            let sent = self.start(text);
            self.pending.push(Line::from(Span::styled(format!("  {}", badges.join(" ")), self.theme.dim())));
            return vec![Action::SendWithImages(sent, imgs)];
        }
        vec![Action::Send(self.start(text))]
    }
}

/// The longest answer shown as a toast; a longer one opens a panel.
pub const TOAST_MAX_CHARS: usize = 100;

/// `text` runs a built-in command that answers in the UI rather than in the conversation
/// (`/status`, `/plugin`, `/reload-plugins`). Commands that put another conversation on screen
/// (`/clear`, `/resume`, `/rewind`) and `/exit` keep their own handling; custom commands,
/// skills and MCP prompts are prompts and always stay in the conversation.
pub fn is_action_candidate(text: &str) -> bool {
    use kodact_core::commands::Builtin;
    match kodact_core::commands::builtin_of(text) {
        Some(Builtin::Clear | Builtin::Resume | Builtin::Rewind | Builtin::Exit) | None => false,
        Some(_) => true,
    }
}

/// A panel for an action command's answer: its lines, titled with the command, each
/// wrapped at words to the panel's width (a screen cuts rows to fit, and a sentence cut
/// off mid-word is lost).
fn action_panel(title: &str, text: &str, is_error: bool) -> Screen {
    use kodact_core::commands::screens::{Row, Tone};
    let cols = crossterm::terminal::size().map(|(c, _)| c as usize).unwrap_or(80);
    // The viewer's frame and row marker take 8 columns.
    let width = cols.saturating_sub(8).max(20);
    let rows = text
        .lines()
        .flat_map(|l| wrap_words(l, width))
        .map(|l| {
            let l = l.as_str();
            let tone = if is_error || l.starts_with("FAIL") {
                Tone::Removed
            } else if l.starts_with("ok ") {
                Tone::Added
            } else {
                Tone::Plain
            };
            Row::text(l, tone)
        })
        .collect();
    Screen { title: if title.is_empty() { "KodAct".into() } else { title.to_string() }, rows }
}

/// `line` broken at spaces into lines at most `width` columns wide; a word wider than that
/// gets a line of its own. A wrapped line keeps the original's indent.
fn wrap_words(line: &str, width: usize) -> Vec<String> {
    use unicode_width::UnicodeWidthStr;
    if line.width() <= width {
        return vec![line.to_string()];
    }
    let indent: String = line.chars().take_while(|c| *c == ' ').collect();
    let mut out = vec![];
    let mut cur = String::new();
    for word in line.split(' ').filter(|w| !w.is_empty()) {
        let lead = if cur.is_empty() { indent.as_str() } else { " " };
        if !cur.is_empty() && cur.width() + lead.width() + word.width() > width {
            out.push(std::mem::take(&mut cur));
            cur.push_str(&indent);
        } else {
            cur.push_str(lead);
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn panel_lines_wrap_at_words_and_keep_their_indent() {
        let lines = super::wrap_words("  Set model to quick-thinking. KodAct doesn't know it", 20);
        assert_eq!(lines, ["  Set model to", "  quick-thinking.", "  KodAct doesn't", "  know it"]);
        assert_eq!(super::wrap_words("short", 20), ["short"]);
    }

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
        // Queued messages can be taken back, newest first, and are then not sent.
        a.busy = true;
        for m in ["first queued", "second queued"] {
            typed(&mut a, m);
            a.on_key(key(KeyCode::Enter));
        }
        assert_eq!(a.queued.len(), 2);
        a.on_key(key(KeyCode::Up));
        assert_eq!((a.editor.text(), a.queued.len()), ("second queued".to_string(), 1), "Up takes the newest back");
        a.on_key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        a.busy = false;
        assert_eq!(a.next_queued().as_deref(), Some("first queued"));
        a.busy = false;
        assert_eq!(a.next_queued(), None, "the withdrawn one is never sent");

        // Pressing Enter on an empty prompt with a queued message forces it by interrupting.
        a.busy = true;
        typed(&mut a, "urgent");
        assert!(a.on_key(key(KeyCode::Enter)).is_empty());
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Interrupt]);
        assert!(a.forcing_queue);
        assert!(a.on_key(key(KeyCode::Enter)).is_empty(), "idempotent while stopping");
        a.on_event(UiEvent::Idle);
        assert!(!a.forcing_queue);
        assert_eq!(a.next_queued().as_deref(), Some("urgent"));

        // Ctrl+Enter while busy with typed text queues it and immediately interrupts.
        typed(&mut a, "right now");
        assert_eq!(a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL)), vec![Action::Interrupt]);
        assert!(a.forcing_queue);
        assert_eq!(a.queued.front().map(String::as_str), Some("right now"));
        a.queued.clear();
        a.forcing_queue = false;
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
        // A row that covers two actions lists each one's keys, the user's among them.
        let (k, w) = crate::tui::keys::Keymap::parse(r#"{"f9": "lineStart", "alt+w": "wordLeft", "f12": "killToEnd"}"#);
        assert!(w.is_empty(), "{w:?}");
        let table = keys_text(&k);
        let row = |what: &str| {
            table.lines().find(|l| l.ends_with(what)).unwrap_or_else(|| panic!("{what}: {table}")).to_string()
        };
        assert!(row("Start / end of line").starts_with("ctrl+a, home, f9 / ctrl+e, end"), "{table}");
        assert!(row("Word left / right").starts_with("alt+b, ctrl+left, alt+w / alt+f, ctrl+right"), "{table}");
        assert!(row("Delete to the start / end of the line").starts_with("ctrl+u / ctrl+k, f12"), "{table}");
        assert!(KEYS
            .iter()
            .flat_map(|(_, _, a)| a.iter())
            .all(|a| crate::tui::keys::ACTIONS.iter().any(|(n, _, _)| n == a)));
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
        assert!(a.take_pending().is_empty(), "an action leaves the conversation as it was");
        assert!(a.toast.as_ref().is_some_and(|(t, err, _)| *err && t.contains("Unknown colour \"mauve\"")));
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

    /// Action commands answer in a toast or a panel and leave the conversation (and its title)
    /// as it was; prompt commands are messages in it.
    #[test]
    fn action_commands_stay_out_of_the_conversation_and_prompt_commands_stay_in_it() {
        let mut a = app();
        a.take_pending();
        // /status: not echoed, not the title, and its answer is a panel titled with it.
        assert_eq!(a.submit("/status".into()), vec![Action::Send("/status".into())]);
        assert!(a.take_pending().is_empty() && a.title.is_empty());
        a.on_event(UiEvent::Action { text: "Version: 1\nModel: m".into(), is_error: false });
        a.on_event(UiEvent::Idle);
        assert!(a.take_pending().is_empty(), "nothing written into the conversation");
        let (screen, _, _) = a.viewer().expect("a panel");
        assert_eq!(screen.title, "/status");
        assert_eq!(screen.rows.len(), 2);
        a.on_key(key(KeyCode::Esc));
        assert!(a.viewer().is_none() && a.take_pending().is_empty(), "closing it leaves nothing behind");

        // /reload-plugins: a one-line answer is a toast; an error is a red one.
        a.submit("/reload-plugins".into());
        a.on_event(UiEvent::Action { text: "Reloaded plugins: 1 added.".into(), is_error: false });
        a.on_event(UiEvent::Idle);
        assert!(a.take_pending().is_empty() && a.viewer().is_none());
        assert!(a.toast.as_ref().is_some_and(|(t, err, _)| !err && t == "Reloaded plugins: 1 added."));
        a.submit("/plugins".into());
        a.on_event(UiEvent::Action { text: "No plugins loaded.".into(), is_error: true });
        assert!(a.toast.as_ref().is_some_and(|(_, err, _)| *err));
        a.on_event(UiEvent::Idle);

        // A built-in that turns into a prompt is written once its prompt is accepted.
        a.submit("/plan add a cache".into());
        assert!(a.take_pending().is_empty(), "not yet: it may answer locally");
        a.on_event(UiEvent::Engine(EngineEvent::PromptAccepted {
            message: kodact_types::Message::user_text("plan: add a cache"),
            uuid: "u".into(),
        }));
        assert!(texts(&a.take_pending()).iter().any(|l| l.contains("❯ /plan add a cache")));
        a.on_event(UiEvent::Idle);

        // A skill or custom command is a prompt: written at once, as any message.
        a.submit("/simplify the parser".into());
        assert!(texts(&a.take_pending()).iter().any(|l| l.contains("❯ /simplify the parser")));
    }

    /// A long one-line answer opens a panel wrapped to the width, never cut.
    #[test]
    fn a_long_action_answer_is_wrapped_in_its_panel() {
        let mut a = app();
        a.submit("/model x".into());
        let long = "Model: x · window unknown: set it in modelLimits. Your endpoint doesn't list x; if requests fail, /model shows the ones it does.";
        a.on_event(UiEvent::Action { text: long.into(), is_error: false });
        let (screen, _, _) = a.viewer().expect("a panel");
        let rows: Vec<String> = screen.rows.iter().map(|r| r.spans.iter().map(|s| s.text.as_str()).collect()).collect();
        let width = crossterm::terminal::size().map(|(c, _)| c as usize).unwrap_or(80).saturating_sub(8).max(20);
        assert!(rows.iter().all(|r| r.chars().count() <= width), "{rows:?}");
        assert_eq!(rows.join(" "), long, "nothing lost");
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
        assert!(a.take_pending().is_empty(), "an action command isn't written into the conversation");
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

    /// A 429 that ended the turn said "KodAct waits and retries" with nothing waiting, and
    /// a 404 from a wrong base URL said the model wasn't available and pointed at /model.
    #[test]
    fn an_api_error_says_what_is_true_now() {
        let over =
            human_api_error("API Error: HTTP 429 from http://gw.example.com/v1/chat/completions", "m", "gw").unwrap();
        assert!(over.1.contains("still refused") && !over.1.contains("waits and retries"), "{over:?}");
        let waiting = human_api_error(
            "API Error: HTTP 429 from http://gw.example.com/v1/x. Retrying in 6s (attempt 2).",
            "m",
            "gw",
        )
        .unwrap();
        assert!(waiting.1.contains("waits and retries") && waiting.0.contains("retrying in 6s"), "{waiting:?}");
        let (head, next) = human_api_error(
            r#"API Error: HTTP 404 from http://gw.example.com/v1/nowhere/chat/completions: {"error": {"message": "no such path"}}"#,
            "m",
            "gw",
        )
        .unwrap();
        assert!(head.contains("nothing at that address") && next.contains("KODACT_OPENAI_BASE_URL"), "{head} / {next}");
        let (head, _) = human_api_error(
            r#"API Error: HTTP 404 from http://gw.example.com/v1/chat/completions: {"error": {"message": "The model `m` does not exist."}}"#,
            "m",
            "gw",
        )
        .unwrap();
        assert_eq!(head, "m isn't available on gw.example.com");
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
            ["  • step one", "  ╭─ sh #1", "  │ ls"],
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

    /// A TodoWrite call shows the plan as a checklist, as the reference CLI does: done tasks with
    /// a green check and their text struck through, the one in progress in rose and bold with
    /// a still circle, the rest open; its header says how far the plan is.
    #[test]
    fn a_plan_update_shows_the_plan_as_a_checklist() {
        let t = Theme { color: true, light: false, accent: None, depth: Default::default() };
        let todos = json!([
            {"content": "Read the signup form", "status": "completed", "activeForm": "Reading the signup form"},
            {"content": "Add the age check", "status": "in_progress", "activeForm": "Adding the age check"},
            {"content": "Run the tests", "status": "pending", "activeForm": "Running the tests"},
        ]);
        let result = json!({"oldTodos": [], "newTodos": todos});
        let lines = result_lines(&t, "TodoWrite", "Todos have been modified successfully.", false, Some(&result));
        assert_eq!(
            texts(&lines),
            ["  └ ✓ Read the signup form", "    ◉ Add the age check", "    ○ Run the tests"],
            "the plan, not the tool's message"
        );
        let style = |row: usize, span: usize| lines[row].spans[span].style;
        assert_eq!(style(0, 1), t.success(), "a green check");
        assert!(style(0, 2).add_modifier.contains(Modifier::CROSSED_OUT), "done: struck through");
        assert_eq!(style(1, 1), t.accent(), "the task in progress in rose");
        assert!(style(1, 2).add_modifier.contains(Modifier::BOLD));
        assert_eq!(style(2, 1), t.subtle(), "open");
        let header: String =
            tool_header(&t, "TodoWrite", &json!({"todos": todos}), true).iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(header, "Updated the plan 1 of 3 done");
        // An emptied plan says so; a refused call still shows its error.
        let empty = json!({"oldTodos": todos, "newTodos": []});
        assert_eq!(texts(&result_lines(&t, "TodoWrite", "ok", false, Some(&empty))), ["  └ No tasks"]);
        let refused = result_lines(&t, "TodoWrite", "todos[0].content must not be empty", true, None);
        assert_eq!(texts(&refused), ["  └ todos[0].content must not be empty"]);
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
    fn slash_menu_works_mid_prompt_and_preserves_surrounding_text() {
        let mut a = app();
        a.commands.extend([
            CommandInfo { name: "simplify".into(), args: String::new(), description: "Simplify code".into() },
            CommandInfo { name: "ship".into(), args: String::new(), description: "Ship release".into() },
            CommandInfo { name: "review-pr".into(), args: "<n>".into(), description: "Review PR".into() },
            CommandInfo { name: "mcp__notes__daily".into(), args: String::new(), description: "MCP prompt".into() },
        ]);
        a.skills = vec!["simplify".into(), "ship".into()];

        // 1. Mid-prompt shows skills, custom commands, MCP prompts; omits built-ins.
        typed(&mut a, "check ");
        typed(&mut a, "/");
        let names: Vec<&str> = a.menu().iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["simplify", "ship", "review-pr", "mcp__notes__daily"]);

        // 2. Typing a partial prefix and pressing Enter completes in-place instead of sending.
        typed(&mut a, "sim");
        assert_eq!(a.menu().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["simplify"]);
        assert!(a.on_key(key(KeyCode::Enter)).is_empty(), "partial mid-prompt Enter completes");
        assert_eq!(a.editor.text(), "check /simplify ");
        assert!(a.menu().is_empty());

        // 3. Chaining another skill after text/skill works; Tab completes; exact Enter submits.
        typed(&mut a, "and /sh");
        assert_eq!(a.menu().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["ship"]);
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.editor.text(), "check /simplify and /ship ");
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Send("check /simplify and /ship ".into())]);
        assert_eq!(a.skills_used, [("ship".into(), 1), ("simplify".into(), 1)]);
        a.on_event(UiEvent::Idle);

        // 4. Exact match typed mid-prompt submits directly on Enter.
        typed(&mut a, "run /ship");
        assert_eq!(a.menu().len(), 1);
        assert_eq!(a.on_key(key(KeyCode::Enter)), vec![Action::Send("run /ship".into())]);
        a.on_event(UiEvent::Idle);

        // 5. Multiline after Shift+Enter opens the menu on line 2 and preserves line 1.
        typed(&mut a, "first line");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        typed(&mut a, "/rev");
        assert_eq!(a.menu().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["review-pr"]);
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.editor.text(), "first line\n/review-pr ");
        a.editor.clear();

        // 6. Paths like src/main.rs or /usr/bin do not open the menu.
        typed(&mut a, "open src/main.rs and /usr/bin");
        assert!(a.menu().is_empty());
        a.editor.clear();

        // 7. Completing in the middle of a sentence reuses existing space (no double space).
        a.editor.set("please /sim this file");
        for _ in 0.." this file".len() {
            a.on_key(key(KeyCode::Left));
        }
        assert_eq!(a.menu().iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["simplify"]);
        a.on_key(key(KeyCode::Tab));
        assert_eq!(a.editor.text(), "please /simplify this file");
    }

    #[test]
    fn esc_ctrl_c_and_shift_tab() {
        let mut a = app();
        a.status.mode = "default".into();
        let stab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(a.on_key(stab), vec![Action::SetMode(PermissionMode::AcceptEdits)]);
        assert_eq!(a.status.mode, "acceptEdits");
        assert_eq!(a.on_key(stab), vec![Action::SetMode(PermissionMode::Auto)]);
        assert_eq!(a.status.mode, "auto");
        assert_eq!(a.on_key(stab), vec![Action::SetMode(PermissionMode::Plan)]);
        assert_eq!(a.status.mode, "plan");
        // Reaching Dangerous for the first time arms the confirmation instead of switching.
        assert!(a.on_key(stab).is_empty() && a.danger_armed && a.status.mode == "plan");
        // Another Shift+Tab or any non-`y` key steps past it to Manual without typing into the input.
        assert_eq!(a.on_key(stab), vec![Action::SetMode(PermissionMode::Default)]);
        assert!(!a.danger_armed && !a.danger_confirmed && a.status.mode == "default");
        a.status.mode = "plan".into();
        assert!(a.on_key(stab).is_empty() && a.danger_armed);
        assert_eq!(a.on_key(key(KeyCode::Char('n'))), vec![Action::SetMode(PermissionMode::Default)]);
        assert!(!a.danger_armed && !a.danger_confirmed && a.status.mode == "default" && a.editor.is_empty());
        // Reach Plan again, arm Dangerous, confirm with `y`: now it's on and stays confirmed.
        a.status.mode = "plan".into();
        assert!(a.on_key(stab).is_empty() && a.danger_armed);
        assert_eq!(a.on_key(key(KeyCode::Char('y'))), vec![Action::SetMode(PermissionMode::BypassPermissions)]);
        assert!(a.danger_confirmed && !a.danger_armed && a.status.mode == "bypassPermissions");
        assert_eq!(a.on_key(stab), vec![Action::SetMode(PermissionMode::Default)]);
        a.status.mode = "plan".into();
        assert_eq!(a.on_key(stab), vec![Action::SetMode(PermissionMode::BypassPermissions)]);
        // When managed settings disable it, Plan cycles straight back to Manual.
        a.danger_blocked = true;
        a.status.mode = "plan".into();
        assert_eq!(a.on_key(stab), vec![Action::SetMode(PermissionMode::Default)]);
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

    #[test]
    fn mode_switch_emits_chat_notification_with_definition_and_resolves_open_dialog() {
        let mut a = app();
        a.status.mode = "default".into();
        let stab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        assert_eq!(a.on_key(stab), vec![Action::SetMode(PermissionMode::AcceptEdits)]);
        assert!(a.take_pending().is_empty(), "nothing is written while Shift+Tab may still cycle");
        assert!(a.hint.as_ref().is_some_and(|(h, _)| h.contains("Accept edits mode")));
        a.note_mode();
        let note: String = a.take_pending().iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(note.contains("── ◆ Accept edits mode"), "{note}");
        assert!(note.contains(PermissionMode::AcceptEdits.short_rule()), "{note}");
        assert!(note.contains(PermissionMode::AcceptEdits.definition()), "{note}");
        assert!(!note.contains("■ "), "must not collide with turn summary marker: {note}");

        // While the LLM is working and waiting on an Edit permission dialog, Shift+Tab
        // switches mode, notifies in chat with "(active immediately)", and auto-approves Edit.
        a.status.mode = "default".into();
        a.busy = true;
        let (tx, mut rx) = oneshot::channel();
        a.on_event(UiEvent::Ask { prompt: prompt("Edit", json!({"file_path": "a.rs"})), reply: tx });
        assert!(a.dialog.is_some());
        assert_eq!(a.on_key(stab), vec![Action::SetMode(PermissionMode::AcceptEdits)]);
        assert!(a.dialog.is_none(), "AcceptEdits auto-resolves pending Edit permission");
        assert!(matches!(rx.try_recv(), Ok(PermissionAnswer::Allow { .. })));
        a.note_mode();
        let live_note: String = a.take_pending().iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert!(live_note.contains("── ◆ Accept edits mode"), "{live_note}");
        assert!(live_note.contains("active immediately"), "{live_note}");
    }

    #[test]
    fn the_welcome_cards_take_the_keyboard_from_an_empty_prompt() {
        let mut a = app();
        a.front = true;
        a.look = crate::tui::look::Look::Frame;
        a.cards_drawn.set(true);
        let k = |c: KeyCode| KeyEvent::new(c, KeyModifiers::NONE);
        // Tab from an empty prompt: the first card; arrows move along, wrapping.
        assert!(a.on_key(k(KeyCode::Tab)).is_empty());
        assert_eq!(a.card, Some(0));
        a.on_key(k(KeyCode::Right));
        assert_eq!(a.card, Some(1));
        a.on_key(k(KeyCode::Left));
        a.on_key(k(KeyCode::Left));
        assert_eq!(a.card, Some(2), "wraps to the last");
        // Esc gives the keyboard back; typing does too, and the key lands in the prompt.
        a.on_key(k(KeyCode::Esc));
        assert_eq!(a.card, None);
        a.on_key(k(KeyCode::Down));
        assert_eq!(a.card, Some(0), "↓ also enters the cards");
        a.on_key(k(KeyCode::Char('h')));
        assert_eq!(a.card, None);
        assert_eq!(a.editor.text(), "h");
        // With text in the prompt, Tab is not the cards'.
        a.on_key(k(KeyCode::Tab));
        assert_eq!(a.card, None);
        // Enter on a card sends its prompt; `/init` as a command.
        a.editor.clear();
        a.on_key(k(KeyCode::Tab));
        let sent = a.on_key(k(KeyCode::Enter));
        assert_eq!(sent, vec![Action::Send(crate::tui::front::CARDS[0].3.to_string())]);
        assert_eq!(a.card, None);
        let mut b = app();
        b.front = true;
        b.look = crate::tui::look::Look::Frame;
        b.cards_drawn.set(true);
        b.on_key(k(KeyCode::Tab));
        b.on_key(k(KeyCode::Right));
        b.on_key(k(KeyCode::Right));
        assert_eq!(b.on_key(k(KeyCode::Enter)), vec![Action::Send("/init".into())]);
        // Cards that are not on screen (a short window, the inline screen) don't take Tab, or
        // Enter would send a suggestion nobody saw.
        let mut hidden = app();
        hidden.front = true;
        hidden.look = crate::tui::look::Look::Frame;
        hidden.on_key(k(KeyCode::Tab));
        assert_eq!(hidden.card, None);
        a.card = Some(1);
        a.cards_drawn.set(false);
        a.on_key(k(KeyCode::Right));
        assert_eq!(a.card, None, "focus is dropped when the cards stop being drawn");
        // Other styles have no cards: Tab is left alone.
        let mut c = app();
        c.front = true;
        c.look = crate::tui::look::Look::Bare;
        c.on_key(k(KeyCode::Tab));
        assert_eq!(c.card, None);
    }

    #[test]
    fn cycling_modes_writes_only_where_it_lands() {
        let mut a = app();
        a.status.mode = "default".into();
        let stab = KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT);
        for _ in 0..3 {
            a.on_key(stab);
        }
        assert!(a.take_pending().is_empty(), "no banner per step of the cycle");
        let landed = PermissionMode::parse(&a.status.mode).unwrap();
        a.submit("hi".into());
        let text: String = a.take_pending().iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert_eq!(text.matches("── ◆").count(), 1, "{text}");
        assert!(text.contains(&format!("{} mode", landed.label())), "{text}");

        // Cycling all the way back to where it started writes nothing.
        let mut b = app();
        b.status.mode = "default".into();
        b.on_key(stab);
        b.set_mode(PermissionMode::Default);
        b.note_mode();
        assert!(b.take_pending().iter().all(|l| !l.to_string().contains("── ◆")));
    }

    #[test]
    fn a_running_turn_notes_the_mode_once_cycling_rests() {
        let mut a = app();
        a.status.mode = "default".into();
        a.busy = true;
        a.set_mode(PermissionMode::AcceptEdits);
        a.on_event(UiEvent::Engine(EngineEvent::Notice { level: NoticeLevel::Info, text: "x".into() }));
        assert!(a.take_pending().iter().all(|l| !l.to_string().contains("── ◆")), "too soon: still cycling");
        a.set_mode(PermissionMode::Plan);
        a.mode_note.as_mut().unwrap().1 -= std::time::Duration::from_secs(3);
        a.busy = false; // the turn may end before the note is written; the tag still says it was live
        a.on_event(UiEvent::Engine(EngineEvent::Notice { level: NoticeLevel::Info, text: "y".into() }));
        let text: String = a.take_pending().iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n");
        assert_eq!(text.matches("── ◆").count(), 1, "{text}");
        assert!(text.contains("Plan mode") && text.contains("active immediately"), "{text}");
        assert!(a.mode_note.is_none());
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

    #[test]
    fn ask_user_question_dialog_handles_esc_typing_multiselect_and_shapes() {
        let mut a = app();

        // Esc cancels from an option row and from "Type an answer" (instead of trapping or opening typing).
        for down in [false, true] {
            let (tx, mut rx) = oneshot::channel();
            let q = json!({"questions": [{"question": "Which db?", "options": ["sqlite", "postgres"]}]});
            a.on_event(UiEvent::Ask { prompt: prompt("AskUserQuestion", q), reply: tx });
            assert_eq!(
                a.dialog_options(),
                vec![
                    ("sqlite".into(), String::new()),
                    ("postgres".into(), String::new()),
                    ("Type an answer".into(), String::new()),
                ]
            );
            if down {
                a.on_key(key(KeyCode::End));
            }
            a.on_key(key(KeyCode::Esc));
            assert!(matches!(rx.try_recv().unwrap(), PermissionAnswer::Deny { interrupt: true, .. }));
            assert!(a.dialog.is_none());
        }

        // Inside "Type an answer", Esc returns to options, cursor/word edits work, and Enter submits.
        let (tx, mut rx) = oneshot::channel();
        let q = json!({"question": "Which cache?", "options": ["redis", "memcached"]});
        a.on_event(UiEvent::Ask { prompt: prompt("AskUserQuestion", q.clone()), reply: tx });
        a.on_key(key(KeyCode::Char('3')));
        assert_eq!(a.dialog_text().1, ["❯ ▏"]);
        a.on_key(key(KeyCode::Esc));
        assert!(a.dialog_text().1.is_empty(), "Esc leaves typing back to options");
        a.on_key(key(KeyCode::Enter));
        typed(&mut a, "dragonfly old");
        a.on_key(KeyEvent::new(KeyCode::Char('w'), KeyModifiers::CONTROL));
        a.on_key(key(KeyCode::Enter));
        match rx.try_recv().unwrap() {
            PermissionAnswer::Allow { updated_input: Some(v), .. } => {
                assert_eq!(v["answers"], json!({"Which cache?": "dragonfly"}));
            }
            other => panic!("{other:?}"),
        }

        // Multi-select: digit keys toggle options without premature submit; Enter confirms all picked.
        let (tx, mut rx) = oneshot::channel();
        let q6 = json!({"questions": [{
            "question": "Allow which?",
            "multiSelect": true,
            "options": ["one", "two", "three", "four", "five"]
        }]});
        a.on_event(UiEvent::Ask { prompt: prompt("AskUserQuestion", q6), reply: tx });
        a.on_key(key(KeyCode::Char('1')));
        a.on_key(key(KeyCode::Char('5')));
        assert!(rx.try_recv().is_err(), "multi-select digits toggle rather than submitting early");
        a.on_key(key(KeyCode::Enter));
        match rx.try_recv().unwrap() {
            PermissionAnswer::Allow { updated_input: Some(v), .. } => {
                assert_eq!(v["answers"], json!({"Allow which?": "one, five"}));
            }
            other => panic!("{other:?}"),
        }

        // Open-ended question (0 options): starts directly in typing mode; Esc cancels.
        let (tx, mut rx) = oneshot::channel();
        a.on_event(UiEvent::Ask {
            prompt: prompt("AskUserQuestion", json!({"questions": [{"question": "Service name?"}]})),
            reply: tx,
        });
        assert!(a.dialog_options().is_empty());
        assert_eq!(a.dialog_text().1, ["❯ ▏"]);
        a.on_key(key(KeyCode::Esc));
        assert!(matches!(rx.try_recv().unwrap(), PermissionAnswer::Deny { interrupt: true, .. }));

        // Transcript header and result lines read as a built-in question, not an outside tool.
        let header = texts(&[Line::from(tool_header(&a.theme, "AskUserQuestion", &q, true))]);
        assert_eq!(header, ["Asked Which cache?"]);
        let res = texts(&result_lines(
            &a.theme,
            "AskUserQuestion",
            "The user answered",
            false,
            Some(&json!({"answers": {"Which cache?": "dragonfly"}})),
        ));
        assert_eq!(res, ["  └ dragonfly"]);
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
        assert_eq!(a.dialog_options()[1].0, "Haiku ✓");
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
        assert!(a.take_pending().is_empty(), "nothing written into the conversation");
        let (screen, _, _) = a.viewer().expect("a panel");
        assert_eq!(screen.title, "/keybindings");
        assert!(screen.rows.iter().any(|r| r.spans.iter().any(|s| s.text.contains("Shift+Tab"))));
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

    #[test]
    fn ctrl_o_toggles_verbose_and_expands_thoughts_and_diffs() {
        use kodact_types::{Delta, StreamEvent};
        let mut a = app();
        assert!(!a.verbose && !a.has_collapsible);

        // Thoughts > 600 chars collapse to 1 summary line by default and expand completely via Ctrl+O.
        let long_thought = format!("Step one: {}\nStep two: final conclusion", "a".repeat(650));
        let ev = |event| UiEvent::Engine(EngineEvent::Stream { event, parent_tool_use_id: None });
        a.on_event(ev(StreamEvent::ContentBlockDelta {
            index: 0,
            delta: Delta::ThinkingDelta { thinking: long_thought },
        }));
        a.on_event(ev(StreamEvent::ContentBlockStop { index: 0 }));
        assert!(a.has_collapsible);
        let pending = a.take_pending();
        let collapsed = texts(&lay_out(pending.clone(), 100));
        assert!(
            collapsed
                .iter()
                .any(|l| l.contains("∴ Thought for") && l.contains("2 lines") && l.contains("ctrl+o to show")),
            "{collapsed:?}"
        );
        assert!(!collapsed.iter().any(|l| l.contains("final conclusion")), "{collapsed:?}");

        a.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert!(a.verbose && a.layout_version == 1);
        assert!(a.hint.as_ref().is_some_and(|(h, _)| h.contains("ctrl+o to collapse")));
        let expanded = texts(&lay_out_mode(pending, 100, a.verbose));
        assert!(expanded.iter().any(|l| l.contains("∴ Thought for") && l.contains("ctrl+o to hide")));
        assert!(expanded.iter().any(|l| l.contains("final conclusion")), "untruncated tail must show: {expanded:?}");

        // Long diffs collapse cleanly past DIFF_LINES and expand cleanly in verbose mode.
        let many: Vec<String> = (1..=30).map(|i| format!("+line {i}")).collect();
        let big = json!({"structuredPatch": [{"oldStart": 1, "newStart": 1, "lines": many}]});
        let diff_placeholder =
            result_lines(&Theme::named("dark", false, Default::default()), "Edit", "", false, Some(&big));
        let collapsed_diff = texts(&lay_out_mode(diff_placeholder.clone(), 100, false));
        assert!(collapsed_diff.last().unwrap().contains("more changed lines"), "{collapsed_diff:?}");
        let expanded_diff = texts(&lay_out_mode(diff_placeholder, 100, true));
        assert!(expanded_diff.iter().any(|l| l.contains("+ line 30")), "{expanded_diff:?}");
        assert!(!expanded_diff.iter().any(|l| l.contains("more changed lines")), "{expanded_diff:?}");

        // Toggle back off & reset clears has_collapsible.
        a.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert!(!a.verbose && a.layout_version == 2);
        a.on_event(UiEvent::Reset { replay: vec![], recent: vec![], echo: None });
        assert!(!a.has_collapsible);

        // Collapsible shell output (> shellbox::TAIL) sets has_collapsible on EngineEvent::User.
        a.on_event(UiEvent::Engine(EngineEvent::Assistant {
            message: kodact_types::ApiMessage {
                id: "m1".into(),
                kind: "message".into(),
                role: kodact_types::Role::Assistant,
                model: "m".into(),
                content: vec![ContentBlock::ToolUse {
                    id: "b1".into(),
                    name: "Bash".into(),
                    input: json!({"command": "seq 1 25"}),
                    cache_control: None,
                    provider: None,
                }],
                stop_reason: None,
                stop_sequence: None,
                usage: Default::default(),
            },
            uuid: "u1".into(),
            parent_tool_use_id: None,
        }));
        let out: String = (1..=25).map(|i| format!("row {i}\n")).collect();
        a.on_event(UiEvent::Engine(EngineEvent::User {
            message: kodact_types::Message::user(vec![ContentBlock::ToolResult {
                tool_use_id: "b1".into(),
                content: kodact_types::ToolResultContent::Text(out),
                is_error: Some(false),
                cache_control: None,
            }]),
            uuid: "u2".into(),
            tool_use_result: None,
            is_meta: false,
            parent_tool_use_id: None,
        }));
        assert!(a.has_collapsible);
    }

    /// A sub-agent sent to another model carries a badge with the model and endpoint it ran on;
    /// one that couldn't get the model it asked for says so in amber; one on the session's model
    /// has none.
    #[test]
    fn sub_agents_on_other_models_show_where_they_ran() {
        let mut a = app();
        a.status.model = "deep-thinking".into();
        let call = |a: &mut App, id: &str, input: Value| {
            a.on_event(UiEvent::Engine(EngineEvent::Assistant {
                message: kodact_types::ApiMessage {
                    id: format!("m-{id}"),
                    kind: "message".into(),
                    role: kodact_types::Role::Assistant,
                    model: "deep-thinking".into(),
                    content: vec![ContentBlock::ToolUse {
                        id: id.into(),
                        name: "Task".into(),
                        input,
                        cache_control: None,
                        provider: None,
                    }],
                    stop_reason: None,
                    stop_sequence: None,
                    usage: Default::default(),
                },
                uuid: format!("u-{id}"),
                parent_tool_use_id: None,
            }));
        };
        let done = |a: &mut App, id: &str, result: Value| {
            a.on_event(UiEvent::Engine(EngineEvent::User {
                message: kodact_types::Message::user(vec![ContentBlock::ToolResult {
                    tool_use_id: id.into(),
                    content: kodact_types::ToolResultContent::Text("hi".into()),
                    is_error: Some(false),
                    cache_control: None,
                }]),
                uuid: format!("r-{id}"),
                tool_use_result: Some(result),
                is_meta: false,
                parent_tool_use_id: None,
            }));
        };
        a.submit("orchestrate".into());
        a.take_pending();

        call(&mut a, "t1", json!({"description": "Summarize the logs", "prompt": "p", "model": "kgpt-reasoning-text"}));
        // While it runs, the model it was sent to.
        let asked = RanOn::asked(&a.running[0].input, &a.status.model).expect("a badge while it runs");
        assert_eq!(asked.model, "kgpt-reasoning-text");
        done(&mut a, "t1", json!({"model": "kgpt-reasoning-text", "endpoint": "work"}));
        let rows = texts(&a.take_pending());
        assert!(
            rows.iter().any(|l| l.contains("Summarize the logs") && l.contains("◆ kgpt-reasoning-text · work")),
            "{rows:?}"
        );

        call(&mut a, "t2", json!({"description": "Review the diff", "prompt": "p", "model": "sonnet"}));
        done(&mut a, "t2", json!({"model": "deep-thinking"}));
        let rows = texts(&a.take_pending());
        assert!(rows.iter().any(|l| l.contains("◆ sonnet → deep-thinking")), "{rows:?}");

        call(&mut a, "t3", json!({"description": "Find the config", "prompt": "p"}));
        assert!(RanOn::asked(&a.running[0].input, &a.status.model).is_none());
        done(&mut a, "t3", json!({"model": "deep-thinking"}));
        let rows = texts(&a.take_pending());
        assert!(
            rows.iter().any(|l| l.contains("Find the config")) && !rows.iter().any(|l| l.contains('◆')),
            "{rows:?}"
        );
    }

    #[test]
    fn frame_prompt_surface_and_wide_prose_measure() {
        use super::super::look::Look;
        let dark = Theme::named("dark", true, super::super::palette::Depth::True);
        let expected_bg = super::super::palette::PROMPT_BG.color(dark.depth, dark.light);
        let mut a = App::new(dark, vec![]);
        a.look = Look::Frame;
        a.echo_prompt("first line\nsecond line");
        let lines = a.take_pending();
        let prompt_lines: Vec<_> = lines.iter().filter(|l| l.style.bg.is_some()).collect();
        assert_eq!(prompt_lines.len(), 2, "both prompt rows carry PROMPT_BG tint: {lines:?}");
        assert!(prompt_lines.iter().all(|l| l.style.bg == Some(expected_bg)));

        // Ansi16 and no-color terminals skip the PROMPT_BG tint.
        let mut ansi16 = App::new(Theme::named("dark", true, super::super::palette::Depth::Ansi16), vec![]);
        ansi16.look = Look::Frame;
        ansi16.echo_prompt("plain ansi16");
        assert!(ansi16.take_pending().iter().all(|l| l.style.bg.is_none()));

        // Mixed transcript at 174 content cols: prose wraps <= PROSE_MAX_COLS while
        // shellbox, split diff, table, and mode banners stay full-width.
        a.echo_prompt(&format!("{} ", "promptword".repeat(14)));
        let long_prose = (0..30).map(|i| format!("word{i:02}")).collect::<Vec<_>>().join(" ");
        a.commit_text_line(&long_prose);
        let shell = super::super::shellbox::placeholder(
            &dark,
            "cargo test --workspace",
            "test result: ok",
            super::super::shellbox::State::Ended { code: Some(0), failed: false },
        );
        let diff_json = json!({
            "structuredPatch": [{"oldStart": 1, "newStart": 1, "lines": ["-old_value_line()", "+new_value_line()"]}]
        });
        let diff = result_lines(&dark, "Edit", "", false, Some(&diff_json));
        let mut md = Markdown::default();
        md.push(
            "| LeftColumnNameWithExtraChars | RightColumnNameWithExtraCharsAndMuchMoreDetailsHereToExceedMeasureWidthNow |",
            &dark,
        );
        md.push("| --- | --- |", &dark);
        md.push(
            "| alpha_identifier_one_two_three | beta_identifier_with_very_long_explanation_spanning_columns_cleanly_ok_yes |",
            &dark,
        );
        let table = md.finish(&dark);
        let mut items = a.take_pending();
        a.busy = true;
        a.set_mode(kodact_permissions::PermissionMode::AcceptEdits);
        a.note_mode();
        let banner = a.take_pending();
        items.push(shell);
        items.extend(diff);
        items.extend(table);
        items.extend(banner);

        let wrapped = wrap_lines(items, 174, false);
        let row_texts = texts(&wrapped);
        // Prompt rows extend their PROMPT_BG band cleanly to PROSE_MAX_COLS across spans, not Line::style.
        let tinted_rows: Vec<_> = wrapped
            .iter()
            .filter(|l| {
                let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                (text.contains("first line") || text.contains("second line") || text.contains("promptword"))
                    && l.spans.iter().any(|s| s.style.bg == Some(expected_bg))
            })
            .collect();
        assert_eq!(tinted_rows.len(), 2, "wrapped prompt spans keep PROMPT_BG band: {tinted_rows:?}");
        for r in &tinted_rows {
            assert_eq!(r.style, Style::default(), "Line::style cleared after padding spans");
            let w: usize = r.spans.iter().map(|s| super::super::text::width(&s.content)).sum();
            assert_eq!(w, PROSE_MAX_COLS as usize);
        }
        // Prose rows stay within PROSE_MAX_COLS.
        let prose_rows: Vec<_> = row_texts.iter().filter(|r| r.contains("word00") || r.contains("word25")).collect();
        assert!(prose_rows.len() >= 2, "180+ char prose wraps into >=2 rows: {prose_rows:?}");
        for r in &prose_rows {
            assert!(super::super::text::width(r) <= PROSE_MAX_COLS as usize, "prose exceeded measure: {r}");
        }
        // Structured elements exceed PROSE_MAX_COLS cleanly.
        assert!(
            row_texts.iter().any(|r| r.starts_with("  ╭─") && super::super::text::width(r) == 174),
            "{row_texts:?}"
        );
        assert!(
            row_texts.iter().any(|r| r.contains("old_value_line") && r.contains(" │ ") && r.contains("new_value_line")),
            "split diff keeps full width >= SPLIT_DIFF_COLS: {row_texts:?}"
        );
        assert!(
            row_texts
                .iter()
                .any(|r| r.contains("LeftColumnName") && super::super::text::width(r) > PROSE_MAX_COLS as usize),
            "table uses full width: {row_texts:?}"
        );
        assert!(
            row_texts
                .iter()
                .any(|r| r.contains("◆ Accept edits mode") && super::super::text::width(r) > PROSE_MAX_COLS as usize),
            "mode banner uses full width: {row_texts:?}"
        );
    }

    #[test]
    fn alt_c_and_has_code_tracking_annotate_last_fence() {
        let mut a = app();
        assert!(!a.has_code);
        assert_eq!(
            a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::ALT)),
            vec![Action::Send("/copy code last".into())]
        );

        let reply = kodact_types::Message::assistant(vec![ContentBlock::text(
            "One:\n```rust\nfn a() {}\n```\nTwo:\n```python\nprint(2)\n```",
        )]);
        a.on_event(UiEvent::Reset { replay: vec![reply.clone()], recent: vec![], echo: None });
        assert!(a.has_code, "replayed fenced code sets has_code");

        let pending = a.take_pending();
        let wide = texts(&wrap_lines(pending.clone(), 80, false));
        assert!(wide.iter().any(|l| l == "  ╭─ rust #1"), "earlier fence keeps clean header: {wide:?}");
        assert!(wide.iter().any(|l| l == "  ╭─ python #2 · alt+c"), "last fence gets alt+c hint: {wide:?}");

        let narrow = texts(&wrap_lines(pending, 14, false));
        assert!(
            narrow.iter().all(|l| !l.contains("alt+c")),
            "narrow width omits alt+c instead of wrapping fence header: {narrow:?}"
        );

        a.on_event(UiEvent::Reset { replay: vec![], recent: vec![], echo: None });
        assert!(!a.has_code, "clearing replay clears has_code");

        a.on_event(UiEvent::Engine(EngineEvent::Assistant {
            message: kodact_types::ApiMessage {
                id: "m".into(),
                kind: "message".into(),
                role: Role::Assistant,
                model: "m".into(),
                content: reply.content,
                stop_reason: None,
                stop_sequence: None,
                usage: Usage::default(),
            },
            uuid: "u".into(),
            parent_tool_use_id: None,
        }));
        assert!(a.has_code, "non-streamed assistant fence sets has_code");
    }

    #[test]
    fn slash_skill_surfaces_loaded_skills_in_menu() {
        let mut a = app();
        a.on_event(UiEvent::Commands(vec![
            CommandInfo { name: "skills".into(), args: "".into(), description: "List available skills".into() },
            CommandInfo { name: "clear".into(), args: "".into(), description: "Clear".into() },
            CommandInfo { name: "i-have-adhd".into(), args: "".into(), description: "Focus helper".into() },
        ]));
        a.on_event(UiEvent::Skills(vec!["i-have-adhd".into()]));

        a.editor.set("/skill");
        let names: Vec<&str> = a.menu().into_iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["skills", "i-have-adhd"]);

        // Mid-prompt `/skill` hides built-in `/skills` and surfaces the skill directly.
        a.editor.set("please use /skill");
        let mid: Vec<&str> = a.menu().into_iter().map(|c| c.name.as_str()).collect();
        assert_eq!(mid, vec!["i-have-adhd"]);
    }
}
