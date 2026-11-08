//! The interactive terminal UI (M8). The design is in docs/TUI.md.
//!
//! By default the UI takes the whole window (the terminal's alternate
//! screen): a centred front screen until the first message, then the
//! transcript above the live region (the answer being written, the spinner,
//! a dialog, the input box, the status line). On exit the transcript is
//! printed into the terminal's scrollback. With `KODACT_TUI=inline`, an inline
//! viewport at the bottom shows what is live and finished output goes into
//! the terminal's own scrollback as it happens.
//!
//! - `editor`: the prompt's text editor;
//! - `text`: theme, one-line markdown, wrapping;
//! - `app`: the UI state machine (keys and session events);
//! - `render`: the live region, drawn from the app;
//! - `session`: the task that owns the driver;
//! - this module: the terminal, the event loop and prompt history.

pub mod app;
pub mod brand;
pub mod editor;
pub mod front;
pub mod keys;
pub mod look;
pub mod palette;
pub mod render;
pub mod session;
pub mod text;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event, EventStream, KeyCode,
    KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseEventKind, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use futures::StreamExt;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::layout::Position;
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};
use ratatui::{Terminal, TerminalOptions, Viewport};
use tokio::sync::mpsc;

use app::{Action, App, UiEvent};
use render::LiveView;
use session::ToSession;

use crate::args::Opts;
use crate::exit::Fail;

/// Rows written into scrollback per `insert_before` call.
const COMMIT_CHUNK: usize = 100;
/// The fastest the live region is redrawn.
const FRAME: Duration = Duration::from_millis(16);
/// Prompts kept from history for this directory.
const HISTORY_ENTRIES: usize = 500;

/// A backend that remembers where the cursor is instead of asking the terminal.
///
/// ratatui asks for the cursor position whenever an inline viewport is made or
/// resized. Asking means writing a query and reading the reply from stdin, but
/// once the UI runs, crossterm's event reader owns stdin, so the reply never
/// arrives and the query fails after two seconds ("The cursor position could
/// not be read"), which ended the session (seen with `/model`). Only the first
/// viewport asks the terminal; after that every move goes through this
/// backend, so it knows.
pub struct Tracked<B: Backend> {
    inner: B,
    pos: std::rc::Rc<std::cell::Cell<Option<Position>>>,
}

impl<B: Backend> Backend for Tracked<B> {
    fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        let mut last = None;
        let cells: Vec<_> = content.inspect(|(x, y, _)| last = Some(Position { x: *x, y: *y })).collect();
        self.inner.draw(cells.into_iter())?;
        if let Some(p) = last {
            self.pos.set(Some(Position { x: p.x.saturating_add(1), y: p.y }));
        }
        Ok(())
    }

    fn append_lines(&mut self, n: u16) -> std::io::Result<()> {
        self.inner.append_lines(n)?;
        if let Some(p) = self.pos.get() {
            let rows = self.inner.size().map(|s| s.height).unwrap_or(u16::MAX);
            self.pos.set(Some(Position { x: p.x, y: p.y.saturating_add(n).min(rows.saturating_sub(1)) }));
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> std::io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> std::io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> std::io::Result<Position> {
        match self.pos.get() {
            Some(p) => Ok(p),
            None => {
                let p = self.inner.get_cursor_position()?;
                self.pos.set(Some(p));
                Ok(p)
            }
        }
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> std::io::Result<()> {
        let p = position.into();
        self.inner.set_cursor_position(p)?;
        self.pos.set(Some(p));
        Ok(())
    }

    fn clear(&mut self) -> std::io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> std::io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> std::io::Result<ratatui::layout::Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> std::io::Result<ratatui::backend::WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// The live viewport and the scrollback above it.
pub struct Screen<B: Backend> {
    terminal: Terminal<Tracked<B>>,
    /// Makes a fresh backend when the viewport changes height.
    make: Box<dyn FnMut() -> B>,
    height: u16,
    pos: std::rc::Rc<std::cell::Cell<Option<Position>>>,
}

impl<B: Backend> Screen<B> {
    pub fn new(mut make: Box<dyn FnMut() -> B>, height: u16) -> std::io::Result<Self> {
        let pos = std::rc::Rc::new(std::cell::Cell::new(None));
        let backend = Tracked { inner: make(), pos: pos.clone() };
        let terminal = Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(height) })?;
        Ok(Screen { terminal, make, height, pos })
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    pub fn width(&self) -> u16 {
        self.terminal.size().map(|s| s.width).unwrap_or(80)
    }

    pub fn rows(&self) -> u16 {
        self.terminal.size().map(|s| s.height).unwrap_or(24)
    }

    /// Change the viewport's height. ratatui can't resize an inline viewport
    /// in place: clear it (the cursor goes to its top) and make a new one there.
    pub fn set_height(&mut self, h: u16) -> std::io::Result<()> {
        let h = h.max(1);
        if h == self.height {
            return Ok(());
        }
        self.terminal.clear()?;
        let backend = Tracked { inner: (self.make)(), pos: self.pos.clone() };
        self.terminal = Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(h) })?;
        self.height = h;
        Ok(())
    }

    /// Write finished lines into scrollback, above the viewport.
    pub fn commit(&mut self, lines: Vec<Line<'static>>) -> std::io::Result<()> {
        let rows = text::wrap(app::lay_out(lines, self.width()), self.width());
        for chunk in rows.chunks(COMMIT_CHUNK) {
            let chunk = chunk.to_vec();
            self.terminal.insert_before(chunk.len() as u16, |buf| Paragraph::new(chunk).render(buf.area, buf))?;
        }
        Ok(())
    }

    /// Draw the live region. A view shorter than the viewport sits at its bottom.
    pub fn draw(&mut self, view: &LiveView) -> std::io::Result<()> {
        let pad = (self.height as usize).saturating_sub(view.lines.len());
        let mut lines = vec![Line::default(); pad];
        lines.extend(view.lines.iter().cloned());
        self.terminal.draw(|f| {
            let area = f.area();
            f.render_widget(Paragraph::new(lines), area);
            if let Some((x, y)) = view.cursor {
                let y = y + pad as u16;
                if x < area.width && y < area.height {
                    f.set_cursor_position((area.x + x, area.y + y));
                }
            }
        })?;
        Ok(())
    }

    /// Clear the live region (Ctrl+L redraws after this).
    pub fn clear(&mut self) -> std::io::Result<()> {
        self.terminal.clear()
    }

    #[cfg(test)]
    pub fn backend(&self) -> &B {
        &self.terminal.backend().inner
    }
}

/// Columns left empty on each side of the full screen: 2, or none on a terminal too
/// narrow to spare them.
fn gutter(width: u16) -> u16 {
    if width >= 40 {
        2
    } else {
        0
    }
}

/// Columns the transcript is set in from the input box's edge, so its marks (`●`, `❯`)
/// line up with the box's `❯` and its text with the text typed in the box.
const TRANSCRIPT_INDENT: u16 = 2;

/// `lines` wrapped to `width` less the transcript's indent, each row indented.
fn indented(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let indent = TRANSCRIPT_INDENT.min(width.saturating_sub(4));
    text::wrap(app::lay_out(lines, width - indent), width - indent)
        .into_iter()
        .map(|l| {
            if indent == 0 || l.spans.is_empty() {
                return l;
            }
            let mut spans = vec![ratatui::text::Span::raw(" ".repeat(indent as usize))];
            spans.extend(l.spans);
            Line::from(spans)
        })
        .collect()
}

/// The full-screen UI: the whole window, and the transcript it keeps itself
/// (wrapped once per width, as lines arrive).
pub struct Full<B: Backend> {
    terminal: Terminal<B>,
    /// Finished lines, as committed.
    transcript: Vec<Line<'static>>,
    /// The transcript wrapped to `wrapped_w`.
    wrapped: Vec<Line<'static>>,
    wrapped_w: u16,
    /// Rows up from the transcript's end.
    pub scroll: usize,
    max_scroll: usize,
    /// What the front page shows about the session.
    info: front::FrontInfo,
}

impl<B: Backend> Full<B> {
    pub fn new(backend: B, info: front::FrontInfo) -> std::io::Result<Self> {
        let terminal = Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Fullscreen })?;
        Ok(Full { terminal, transcript: vec![], wrapped: vec![], wrapped_w: 0, scroll: 0, max_scroll: 0, info })
    }

    /// The width text is laid out in: the window less its gutters.
    pub fn width(&self) -> u16 {
        let w = self.terminal.size().map(|s| s.width).unwrap_or(80);
        w - 2 * gutter(w)
    }

    pub fn rows(&self) -> u16 {
        self.terminal.size().map(|s| s.height).unwrap_or(24)
    }

    /// Finished lines join the transcript. A view scrolled up stays where it is.
    pub fn commit(&mut self, lines: Vec<Line<'static>>) {
        let w = self.width();
        let rows = indented(lines.clone(), w);
        if self.scroll > 0 {
            self.scroll += rows.len();
        }
        if w == self.wrapped_w {
            self.wrapped.extend(rows);
        }
        self.transcript.extend(lines);
    }

    /// Scroll by `rows` (up when positive), within the transcript.
    pub fn scroll_by(&mut self, rows: isize) {
        self.scroll = (self.scroll as isize + rows).clamp(0, self.max_scroll as isize) as usize;
    }

    pub fn draw(&mut self, app: &App) -> std::io::Result<()> {
        let size = self.terminal.size()?;
        // A gutter on both sides, so text never touches the terminal's edge.
        let pad = gutter(size.width);
        let full_w = size.width - 2 * pad;
        // The sidebar takes the right of a wide window, once the conversation has begun.
        let side = app.sidebar && !app.front && size.width >= app::SIDEBAR_MIN_COLS;
        let side_w = if side { render::sidebar_width(size.width) } else { 0 };
        app.sidebar_shown.set(side);
        let width = full_w - side_w - if side { 2 } else { 0 };
        if width != self.wrapped_w {
            self.wrapped = indented(self.transcript.clone(), width);
            self.wrapped_w = width;
        }
        let page = front::front_page(&app.theme, app.look, &self.info, width as usize);
        let (view, max) = render::full_view(app, width, size.height, &self.wrapped, &page, self.scroll);
        self.max_scroll = max;
        self.scroll = self.scroll.min(max);
        self.terminal.draw(|f| {
            let full = f.area();
            let area = ratatui::layout::Rect { x: full.x + pad, width, ..full };
            f.render_widget(Paragraph::new(view.lines.clone()), area);
            if side {
                let rect = ratatui::layout::Rect { x: area.x + width + 2, width: side_w, ..full };
                let lines = render::sidebar(app, side_w as usize, full.height as usize, Instant::now());
                // Panes float on the window's own background: no slab behind them.
                f.render_widget(Paragraph::new(lines), rect);
            }
            if let Some((x, y)) = view.cursor {
                if x < area.width && y < area.height {
                    f.set_cursor_position((area.x + x, area.y + y));
                }
            }
        })?;
        Ok(())
    }

    pub fn clear(&mut self) -> std::io::Result<()> {
        self.terminal.clear()
    }

    /// The transcript as text with ANSI styles, wrapped to `cols`, for the
    /// terminal's scrollback once the full screen has closed.
    pub fn transcript_text(&self, cols: u16) -> Vec<String> {
        text::wrap(app::lay_out(self.transcript.clone(), cols), cols).iter().map(brand::ansi).collect()
    }
}

/// Where the UI draws: the whole window, or an inline viewport.
enum Display {
    Full(Box<Full<CrosstermBackend<std::io::Stdout>>>),
    Inline(Screen<CrosstermBackend<std::io::Stdout>>),
}

impl Display {
    fn clear(&mut self) -> std::io::Result<()> {
        match self {
            Display::Full(f) => f.clear(),
            Display::Inline(s) => s.clear(),
        }
    }
}

/// The full-screen UI, unless `KODACT_TUI=inline` asks for the inline one.
fn full_screen() -> bool {
    std::env::var("KODACT_TUI").map(|v| !v.trim().eq_ignore_ascii_case("inline")).unwrap_or(true)
}

// ---- the terminal's modes ----

static ENHANCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The UI is on the alternate screen, with the mouse captured (for the wheel).
static FULL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The terminal turned on the keyboard protocol (Shift+Enter is its own key).
pub fn keyboard_protocol() -> bool {
    ENHANCED.load(std::sync::atomic::Ordering::SeqCst)
}

/// Put `text` on the clipboard through the terminal (OSC 52).
fn osc52(text: &str) -> String {
    use base64::Engine as _;
    format!("\x1b]52;c;{}\x07", base64::engine::general_purpose::STANDARD.encode(text))
}

/// Put the terminal back as it was: keyboard flags, paste, raw mode, cursor.
fn restore() {
    let mut out = std::io::stdout();
    if ENHANCED.swap(false, std::sync::atomic::Ordering::SeqCst) {
        let _ = crossterm::execute!(out, PopKeyboardEnhancementFlags);
    }
    if FULL.swap(false, std::sync::atomic::Ordering::SeqCst) {
        let _ = crossterm::execute!(out, DisableMouseCapture, crossterm::terminal::LeaveAlternateScreen);
    }
    let _ = crossterm::execute!(out, DisableBracketedPaste, crossterm::cursor::Show);
    let _ = crossterm::terminal::disable_raw_mode();
    let _ = out.flush();
}

/// Restores the terminal when dropped, on every exit path.
struct Guard;

impl Drop for Guard {
    fn drop(&mut self) {
        restore();
    }
}

fn setup(full: bool) -> std::io::Result<Guard> {
    crossterm::terminal::enable_raw_mode()?;
    let guard = Guard;
    let mut out = std::io::stdout();
    crossterm::execute!(out, EnableBracketedPaste)?;
    if full {
        crossterm::execute!(out, crossterm::terminal::EnterAlternateScreen, EnableMouseCapture)?;
        FULL.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false) {
        // Shift+Enter becomes a key of its own.
        crossterm::execute!(out, PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES))?;
        ENHANCED.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore();
        prev(info);
    }));
    Ok(guard)
}

// ---- prompt history ----

pub fn history_path() -> PathBuf {
    kodact_config::state_dir().join("history.jsonl")
}

/// The last prompts typed in `cwd`, oldest first.
pub fn load_history(path: &Path, cwd: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    let mut out: Vec<String> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .filter(|v| v["cwd"] == cwd)
        .filter_map(|v| v["text"].as_str().map(str::to_string))
        .collect();
    if out.len() > HISTORY_ENTRIES {
        out.drain(..out.len() - HISTORY_ENTRIES);
    }
    out
}

/// Add a prompt to the history file (created with mode 0600).
pub fn append_history(path: &Path, cwd: &str, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut o = std::fs::OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(path)?;
    writeln!(f, "{}", serde_json::json!({"text": text, "cwd": cwd}))
}

// ---- `@` completion ----

/// Most paths offered for `@` completion.
const MAX_FILES: usize = 20_000;

/// The project's files and directories (relative, `/`-separated; directories end in `/`),
/// skipping what `.gitignore` and hidden-file rules skip.
pub fn project_files(root: &Path) -> Vec<String> {
    let mut out = vec![];
    for e in ignore::WalkBuilder::new(root).build().flatten() {
        let Ok(rel) = e.path().strip_prefix(root) else { continue };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let mut p = rel.to_string_lossy().replace('\\', "/");
        if e.file_type().is_some_and(|t| t.is_dir()) {
            p.push('/');
        }
        out.push(p);
        if out.len() >= MAX_FILES {
            break;
        }
    }
    out
}

fn walk_files(root: String, ui: mpsc::UnboundedSender<UiEvent>) {
    tokio::task::spawn_blocking(move || {
        let _ = ui.send(UiEvent::Files(project_files(Path::new(&root))));
    });
}

// ---- the loop ----

/// The interactive UI: until `/exit`, Ctrl-D or Ctrl-C twice.
pub async fn run(prompt: Option<String>, o: Opts) -> Result<i32, Fail> {
    let (ui_tx, mut ui_rx) = mpsc::unbounded_channel::<UiEvent>();
    let theme = text::Theme::named("auto", crate::term::get().color, palette::Depth::detect());
    let cols = crossterm::terminal::size().map(|(c, _)| c as usize).unwrap_or(80);
    let (driver, warnings) = match session::build(&o, ui_tx.clone()).await {
        Ok(built) => built,
        // Nothing to talk to yet: the first-run card says how to set one up.
        Err(f) if f.first_run => {
            for l in brand::first_run(&theme, kodact_core::VERSION, &f.message, cols) {
                println!("{}", brand::ansi(&l));
            }
            return Ok(f.code);
        }
        Err(f) => return Err(f),
    };
    let live = driver.live();
    let cwd = driver.info.cwd.display().to_string();
    let persist = !o.no_session_persistence;
    let hist_path = history_path();
    // The `theme` setting arrives from the session first thing.
    let mut app = App::new(theme, load_history(&hist_path, &cwd));
    // Bad entries in keybindings.json are warnings, never a failure.
    let (keymap, key_warnings) = keys::Keymap::load();
    app.keymap = keymap;
    let warnings: Vec<String> = warnings.into_iter().chain(key_warnings).collect();
    let provider = driver.engine.provider();
    let endpoint = endpoint_label(provider.base_url().as_deref(), provider.origin().map(|o| o.backend));
    let recent = kodact_core::commands::recent_sessions(&driver, 3);
    let full = full_screen();
    let branch = kodact_git::current_branch(&driver.info.cwd);
    let info = front::FrontInfo {
        version: kodact_core::VERSION.to_string(),
        model: driver.handle().model(),
        endpoint: endpoint.clone(),
        folder: short_path(&cwd),
        branch: branch.clone(),
        mcp: driver.info.init.mcp_servers.iter().map(|s| (s.name.clone(), s.status.clone())).collect(),
        tools: driver.info.init.tools.len(),
        recent: recent.clone(),
    };
    app.front = true;
    app.first_run = recent.is_empty();
    app.mcp = info.mcp.clone();
    app.mcp_tools = mcp_tool_counts(&app.mcp, &driver.info.init.tools);
    app.skills = driver.info.init.skills.clone();
    app.endpoint = endpoint;
    app.branch = branch.unwrap_or_default();
    if !full {
        let rows = crossterm::terminal::size().map(|(_, r)| r as usize).unwrap_or(24);
        app.pending.extend(brand::welcome(
            &theme,
            &brand::Welcome { recent: &recent[..recent.len().min(1)] },
            cols,
            rows,
        ));
    }
    for w in warnings {
        app.on_event(UiEvent::Engine(kodact_engine::EngineEvent::Notice {
            level: kodact_engine::NoticeLevel::Warning,
            text: w,
        }));
    }

    let (to_session, rx) = mpsc::unbounded_channel::<ToSession>();
    walk_files(cwd.clone(), ui_tx.clone());
    // Re-walked when the session moves (/cd). A weak sender, so the channel still
    // closes when the session task ends.
    let files_tx = ui_tx.downgrade();
    let mut files_root = cwd.clone();
    // The session task holds the other senders (here and in its sink and prompter): when it
    // ends, the UI sees the channel close.
    let task = tokio::spawn(session::run(driver, rx, ui_tx));
    let guard = setup(full).map_err(|e| Fail::config(format!("cannot set up the terminal: {e}")))?;
    let draw_fail = |e: std::io::Error| Fail::config(format!("cannot draw on the terminal: {e}"));
    if full {
        app.transcript_indent = TRANSCRIPT_INDENT as usize;
    }
    let mut screen = if full {
        Display::Full(Box::new(Full::new(CrosstermBackend::new(std::io::stdout()), info).map_err(draw_fail)?))
    } else {
        Display::Inline(Screen::new(Box::new(|| CrosstermBackend::new(std::io::stdout())), 1).map_err(draw_fail)?)
    };

    let send = |text: String, to: &mpsc::UnboundedSender<ToSession>| {
        let _ = append_history(&hist_path, &cwd, &text);
        let _ = to.send(ToSession::Input(text));
    };
    if let Some(p) = prompt.filter(|p| !p.trim().is_empty()) {
        for a in app.submit(p) {
            if let Action::Send(t) = a {
                send(t, &to_session);
            }
        }
    }

    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(render::SPINNER_STEP);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_draw = Instant::now() - FRAME;
    let mut had_dialog = false;
    let mut exiting = false;
    loop {
        if !app.pending.is_empty() {
            match &mut screen {
                Display::Inline(s) => s.commit(app.take_pending()).map_err(io_fail)?,
                Display::Full(f) => f.commit(app.take_pending()),
            }
            app.dirty = true;
        }
        if let Some(text) = app.clipboard.take() {
            let mut out = std::io::stdout();
            let _ = out.write_all(osc52(&text).as_bytes());
            let _ = out.flush();
        }
        if app.exit {
            break;
        }
        while let Some(t) = app.next_queued() {
            send(t, &to_session);
        }
        if !app.status.cwd.is_empty() && app.status.cwd != files_root {
            files_root = app.status.cwd.clone();
            if let Some(tx) = files_tx.upgrade() {
                walk_files(files_root.clone(), tx);
            }
        }
        let now = Instant::now();
        if app.dirty && now.duration_since(last_draw) >= FRAME {
            match &mut screen {
                Display::Inline(screen) => {
                    let max = screen.rows().saturating_sub(1).max(1);
                    let view = render::live_view(&app, screen.width(), max);
                    let want = (view.lines.len() as u16).min(max);
                    let dialog_closed = had_dialog && app.dialog.is_none();
                    if want > screen.height() || (want < screen.height() && (!app.busy || dialog_closed)) {
                        screen.set_height(want).map_err(io_fail)?;
                    }
                    screen.draw(&view).map_err(io_fail)?;
                }
                Display::Full(f) => f.draw(&app).map_err(io_fail)?,
            }
            had_dialog = app.dialog.is_some();
            app.dirty = false;
            last_draw = now;
        }
        let hint = app.hint.as_ref().is_some_and(|(_, at)| at.elapsed() < render::HINT_FOR + render::SPINNER_STEP);
        let frame_wait = (last_draw + FRAME).saturating_duration_since(Instant::now());
        tokio::select! {
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release
                    && app.keymap.translate(k).is_some_and(|k| {
                        k.code == KeyCode::Char('l') && k.modifiers.contains(KeyModifiers::CONTROL)
                    }) => {
                    screen.clear().map_err(io_fail)?;
                    app.dirty = true;
                }
                // The full screen scrolls its transcript: the wheel, and PgUp / PgDn with no dialog open.
                Some(Ok(Event::Mouse(m))) if matches!(screen, Display::Full(_)) => {
                    if let Display::Full(f) = &mut screen {
                        match m.kind {
                            MouseEventKind::ScrollUp => f.scroll_by(3),
                            MouseEventKind::ScrollDown => f.scroll_by(-3),
                            _ => {}
                        }
                    }
                    app.dirty = true;
                }
                Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release
                    && app.dialog.is_none()
                    && matches!(k.code, KeyCode::PageUp | KeyCode::PageDown)
                    && matches!(screen, Display::Full(_)) => {
                    if let Display::Full(f) = &mut screen {
                        let page = (f.rows() as isize / 2).max(1);
                        f.scroll_by(if k.code == KeyCode::PageUp { page } else { -page });
                    }
                    app.dirty = true;
                }
                Some(Ok(Event::Resize(..))) => app.dirty = true,
                Some(Ok(ev)) => {
                    for a in app.on_term_event(ev) {
                        match a {
                            Action::Send(t) => {
                                // Sending goes back to the newest lines.
                                if let Display::Full(f) = &mut screen {
                                    f.scroll = 0;
                                }
                                send(t, &to_session)
                            }
                            Action::Picker(t) => {
                                let _ = to_session.send(ToSession::Picker(t));
                            }
                            Action::Interrupt => {
                                live.handle().interrupt();
                                app.close_dialog_if_stale();
                            }
                            Action::SetMode(m) => live.handle().set_permission_mode(m),
                            Action::Exit => {
                                let _ = to_session.send(ToSession::Exit);
                                exiting = true;
                            }
                        }
                    }
                    if exiting {
                        break;
                    }
                }
                // The terminal is gone.
                Some(Err(_)) | None => break,
            },
            ev = ui_rx.recv() => match ev {
                Some(ev) => {
                    app.on_event(ev);
                    // Take what else is ready, so a burst of deltas is one frame.
                    while let Ok(ev) = ui_rx.try_recv() {
                        app.on_event(ev);
                    }
                }
                None => break,
            },
            _ = tick.tick(), if app.busy || hint || render::sidebar_moving(&app) => app.dirty = true,
            _ = tokio::time::sleep(frame_wait), if app.dirty && !frame_wait.is_zero() => {}
        }
    }
    // Whatever is left goes to scrollback, then the live region is cleared. The full
    // screen closes, and its transcript goes into the terminal's scrollback.
    let transcript = match &mut screen {
        Display::Inline(s) => {
            let _ = s.commit(app.take_pending());
            let _ = s.set_height(1);
            let _ = s.clear();
            vec![]
        }
        Display::Full(f) => {
            f.commit(app.take_pending());
            let cols = f.width();
            f.transcript_text(cols)
        }
    };
    drop(screen);
    drop(guard);
    if !transcript.is_empty() {
        let mut out = std::io::stdout();
        for l in transcript {
            let _ = writeln!(out, "{l}");
        }
        let _ = out.flush();
    }
    let _ = to_session.send(ToSession::Exit);
    drop(to_session);
    // The session ends its own way (hooks, MCP); don't wait for it forever.
    let _ = tokio::time::timeout(Duration::from_secs(5), task).await;
    if persist {
        eprintln!("Resume this conversation with: kodact --resume {}", live.session_id());
    }
    Ok(crate::exit::OK)
}

/// How many tools each MCP server brings, for the sidebar: the session's tools named
/// `mcp__<server>__<tool>`, counted by server.
fn mcp_tool_counts(servers: &[(String, String)], tools: &[String]) -> std::collections::HashMap<String, usize> {
    servers
        .iter()
        .map(|(name, _)| {
            let prefix = kodact_mcp::tools::tool_name(name, "");
            (name.clone(), tools.iter().filter(|t| t.starts_with(&prefix)).count())
        })
        .collect()
}

/// Where requests go, for the welcome banner: the endpoint's host, or its kind.
fn endpoint_label(url: Option<&str>, backend: Option<kodact_api::auth::Backend>) -> String {
    if let Some(u) = url {
        let rest = u.split_once("://").map(|(_, r)| r).unwrap_or(u);
        let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
        // No user name or password in a banner.
        let host = host.rsplit('@').next().unwrap_or(host);
        if !host.is_empty() {
            return host.to_string();
        }
    }
    match backend {
        Some(kodact_api::auth::Backend::OpenAi) => "an OpenAI-compatible endpoint".into(),
        Some(kodact_api::auth::Backend::Gemini) => "the Gemini API".into(),
        _ => "the Messages API".into(),
    }
}

/// A directory as people write it: `~/proj` under the home directory.
pub fn short_path(p: &str) -> String {
    kodact_config::short_path(std::path::Path::new(p))
}

fn io_fail(e: std::io::Error) -> Fail {
    Fail::config(format!("terminal error: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_banner_names_the_endpoint_host_without_credentials() {
        use kodact_api::auth::Backend;
        assert_eq!(endpoint_label(Some("http://127.0.0.1:11434/v1"), Some(Backend::OpenAi)), "127.0.0.1:11434");
        assert_eq!(endpoint_label(Some("https://u:p@gw.example.com/v1?k=x"), None), "gw.example.com");
        assert_eq!(endpoint_label(None, Some(Backend::Messages)), "the Messages API");
        assert_eq!(endpoint_label(None, Some(Backend::OpenAi)), "an OpenAI-compatible endpoint");
    }
    use ratatui::backend::TestBackend;

    /// A terminal that answers the cursor-position query only once, like a
    /// real one whose stdin the event reader has taken over.
    struct AsksOnce {
        inner: TestBackend,
        asked: std::rc::Rc<std::cell::Cell<u32>>,
    }

    impl Backend for AsksOnce {
        fn draw<'a, I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>>(&mut self, c: I) -> std::io::Result<()> {
            self.inner.draw(c)
        }
        fn append_lines(&mut self, n: u16) -> std::io::Result<()> {
            self.inner.append_lines(n)
        }
        fn hide_cursor(&mut self) -> std::io::Result<()> {
            self.inner.hide_cursor()
        }
        fn show_cursor(&mut self) -> std::io::Result<()> {
            self.inner.show_cursor()
        }
        fn get_cursor_position(&mut self) -> std::io::Result<Position> {
            self.asked.set(self.asked.get() + 1);
            if self.asked.get() > 1 {
                return Err(std::io::Error::other("The cursor position could not be read within a normal duration"));
            }
            self.inner.get_cursor_position()
        }
        fn set_cursor_position<P: Into<Position>>(&mut self, p: P) -> std::io::Result<()> {
            self.inner.set_cursor_position(p)
        }
        fn clear(&mut self) -> std::io::Result<()> {
            self.inner.clear()
        }
        fn clear_region(&mut self, t: ratatui::backend::ClearType) -> std::io::Result<()> {
            self.inner.clear_region(t)
        }
        fn size(&self) -> std::io::Result<ratatui::layout::Size> {
            self.inner.size()
        }
        fn window_size(&mut self) -> std::io::Result<ratatui::backend::WindowSize> {
            self.inner.window_size()
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.inner.flush()
        }
    }

    /// `/model` ended the session: opening its picker resized the viewport, which asked the
    /// terminal for the cursor position after the event reader owned stdin. Only the first
    /// viewport may ask.
    #[test]
    fn resizing_the_viewport_never_asks_the_terminal_again() {
        let asked = std::rc::Rc::new(std::cell::Cell::new(0));
        let a = asked.clone();
        let mut s =
            Screen::new(Box::new(move || AsksOnce { inner: TestBackend::new(40, 12), asked: a.clone() }), 2).unwrap();
        s.draw(&LiveView { lines: vec![Line::from("> ")], cursor: Some((2, 0)) }).unwrap();
        s.set_height(9).expect("growing for a picker works");
        s.draw(&LiveView { lines: vec![Line::from("picker"); 9], cursor: None }).unwrap();
        s.set_height(3).expect("and shrinking back");
        s.commit(vec![Line::from("answer")]).unwrap();
        assert_eq!(asked.get(), 1, "only the first viewport asked");
    }

    fn rows(b: &TestBackend) -> Vec<String> {
        let buf = b.buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().trim_end().into()
            })
            .collect()
    }

    #[test]
    fn the_full_screen_keeps_a_gutter_on_both_sides() {
        let theme = text::Theme { color: false, light: false, accent: None, depth: Default::default() };
        let mut a = app::App::new(theme, vec![]);
        a.submit("fix it".into());
        a.front = false;
        let mut f = Full::new(TestBackend::new(60, 24), front::FrontInfo::default()).unwrap();
        assert_eq!(f.width(), 56, "text is laid out between the gutters");
        f.commit(a.take_pending());
        f.commit(vec![Line::from("x".repeat(70))]);
        f.draw(&a).unwrap();
        let rows = rows(f.terminal.backend());
        for r in &rows {
            let cells: Vec<char> = r.chars().collect();
            assert!(cells.len() <= 58, "nothing in the right gutter: {r:?}");
            assert!(cells.iter().take(2).all(|c| *c == ' '), "nothing in the left gutter: {r:?}");
        }
        // The transcript's marks line up with the box's `❯`, its text with the typed text.
        let col = |r: &String, c: char| r.chars().position(|x| x == c);
        let echoed = rows.iter().find(|r| r.contains("fix it")).expect("the message");
        let boxed = rows.iter().find(|r| r.starts_with("  │ ❯")).expect("the box");
        assert_eq!(col(echoed, '❯'), col(boxed, '❯'), "{rows:#?}");
        assert!(rows.iter().any(|r| r == &format!("    {}", "x".repeat(54))), "long lines wrap inside: {rows:#?}");
        // Too narrow to spare the columns: no gutter.
        let f = Full::new(TestBackend::new(30, 8), front::FrontInfo::default()).unwrap();
        assert_eq!(f.width(), 30);
    }

    #[test]
    fn each_mcp_server_counts_its_own_tools() {
        let servers = vec![("git".to_string(), "connected".to_string()), ("my server".into(), "connected".into())];
        let tools: Vec<String> = ["Read", "mcp__git__log", "mcp__git__diff", "mcp__github__pr", "mcp__my_server__ping"]
            .map(String::from)
            .to_vec();
        let counts = mcp_tool_counts(&servers, &tools);
        // `github`'s tools aren't `git`'s; a name with a space is matched as tools are named.
        assert_eq!(counts["git"], 2);
        assert_eq!(counts["my server"], 1);
    }

    #[test]
    fn ctrl_b_opens_the_sidebar_on_a_wide_screen() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let theme = text::Theme { color: false, light: false, accent: None, depth: Default::default() };
        let mut a = app::App::new(theme, vec![]);
        a.submit("add an age check to signups".into());
        a.front = false;
        a.mcp = vec![("jira".into(), "connected".into()), ("figma".into(), "failed".into())];
        a.todos = vec![
            ("completed".into(), "Read the form".into()),
            ("in_progress".into(), "Add the field".into()),
            ("pending".into(), "Run the tests".into()),
        ];
        a.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(a.sidebar);
        let mut f = Full::new(TestBackend::new(160, 30), front::FrontInfo::default()).unwrap();
        f.commit(a.take_pending());
        f.draw(&a).unwrap();
        let wide = rows(f.terminal.backend());
        let all = wide.join("\n");
        for want in [
            "add an age check to signups",
            "Context",
            "MCP",
            "● jira",
            "× figma",
            "failed",
            "Plan",
            "✓ Read the form",
            "Add the field",
            "○ Run the tests",
        ] {
            assert!(all.contains(want), "{want}\n{all}");
        }
        // The sidebar is on the right; the conversation keeps the left.
        let col = wide.iter().find_map(|r| r.chars().collect::<String>().find("Context")).unwrap();
        assert!(col >= 160 - render::SIDEBAR_W as usize - 2, "{col}");
        // Too narrow: no sidebar, whatever the switch says; Ctrl+B again closes it.
        let mut f = Full::new(TestBackend::new(120, 30), front::FrontInfo::default()).unwrap();
        f.draw(&a).unwrap();
        assert!(!rows(f.terminal.backend()).join("\n").contains("MCP"));
        a.on_key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        assert!(!a.sidebar);
    }

    #[test]
    fn commit_wraps_into_scrollback_above_the_viewport() {
        let mut s = Screen::new(Box::new(|| TestBackend::new(10, 6)), 2).unwrap();
        let view = LiveView { lines: vec![Line::from("> input")], cursor: Some((2, 0)) };
        s.draw(&view).unwrap();
        s.commit(vec![Line::from("the quick brown fox"), Line::from("end")]).unwrap();
        // The loop redraws the live region after each commit.
        s.draw(&view).unwrap();
        let r = rows(s.backend());
        // Committed rows are wrapped to the width and sit above the live region.
        let at = r.iter().position(|l| l == "the quick").expect("committed");
        assert_eq!(r[at + 1], "brown fox");
        assert_eq!(r[at + 2], "end");
        assert!(r[at + 3..].iter().any(|l| l == "> input"), "{r:?}");
    }

    #[test]
    fn commit_writes_long_output_in_chunks() {
        let mut s = Screen::new(Box::new(|| TestBackend::new(20, 8)), 1).unwrap();
        let lines: Vec<Line<'static>> = (0..250).map(|i| Line::from(format!("row {i}"))).collect();
        s.commit(lines).unwrap();
        let r = rows(s.backend());
        // The screen keeps the newest rows just above the viewport.
        assert!(r.iter().any(|l| l == "row 249"), "{r:?}");
        assert!(!r.iter().any(|l| l == "row 0"));
    }

    #[test]
    fn history_is_per_directory_and_private() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("state/history.jsonl");
        append_history(&p, "/a", "first").unwrap();
        append_history(&p, "/b", "elsewhere").unwrap();
        append_history(&p, "/a", "multi\nline").unwrap();
        assert_eq!(load_history(&p, "/a"), ["first", "multi\nline"]);
        assert!(load_history(&d.path().join("none"), "/a").is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    #[test]
    fn project_files_follow_ignore_rules() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::create_dir_all(d.path().join("target")).unwrap();
        std::fs::write(d.path().join("src/main.rs"), "").unwrap();
        std::fs::write(d.path().join("target/out"), "").unwrap();
        std::fs::write(d.path().join(".gitignore"), "target/\n").unwrap();
        std::process::Command::new("git").arg("init").arg("-q").current_dir(d.path()).status().ok();
        let mut files = project_files(d.path());
        files.sort();
        assert_eq!(files, ["src/", "src/main.rs"]);
    }

    #[test]
    fn osc52_carries_base64() {
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }
}

/// Screens drawn to HTML, to look at the full-screen UI without a terminal:
/// `KODACT_SNAPSHOT_DIR=/tmp/s cargo test -p kodact-cli --bin kodact snapshots -- --ignored`.
#[cfg(test)]
mod snapshots {
    use super::app::{App, StatusView, UiEvent};
    use super::text::Theme;
    use super::*;
    use kodact_types::{ApiMessage, ContentBlock, Role, StopReason, Usage};
    use ratatui::backend::TestBackend;
    use ratatui::style::Color;

    fn css(c: Color, default: &str) -> String {
        match c {
            Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
            Color::Reset => default.into(),
            Color::White => "#e5e5e5".into(),
            Color::Black => "#101214".into(),
            Color::Gray | Color::DarkGray => "#8a8a8a".into(),
            Color::LightGreen | Color::Green => "#9fe870".into(),
            Color::Red | Color::LightRed => "#ff6b6b".into(),
            Color::Yellow | Color::LightYellow => "#f5d76e".into(),
            _ => default.into(),
        }
    }

    fn html(buf: &ratatui::buffer::Buffer) -> String {
        let area = buf.area;
        let mut s = String::from(
            "<!doctype html><meta charset=utf-8><style>body{margin:0;background:#0b0c0e}\
             pre{margin:0;padding:8px;font:16px/1.15 'DejaVu Sans Mono',monospace;color:#d8d8d8;background:#0b0c0e}\
             span{white-space:pre;display:inline-block;height:1.15em;vertical-align:top}</style><pre>",
        );
        for y in 0..area.height {
            for x in 0..area.width {
                let c = &buf[(x, y)];
                let mut st = format!("color:{};", css(c.fg, "#d8d8d8"));
                if c.bg != Color::Reset {
                    st.push_str(&format!("background:{};", css(c.bg, "transparent")));
                }
                if c.modifier.contains(ratatui::style::Modifier::BOLD) {
                    st.push_str("font-weight:bold;");
                }
                if c.modifier.contains(ratatui::style::Modifier::DIM) {
                    st.push_str("opacity:.6;");
                }
                let sym = c.symbol().replace('&', "&amp;").replace('<', "&lt;");
                s.push_str(&format!("<span style=\"{st}\">{sym}</span>"));
            }
            s.push('\n');
        }
        s + "</pre>"
    }

    fn app() -> App {
        let mut a = App::new(Theme { color: true, light: false, accent: None, depth: Default::default() }, vec![]);
        a.front = true;
        a.transcript_indent = TRANSCRIPT_INDENT as usize;
        a.endpoint = "gpt.technica-engineering.net".into();
        a.branch = "main".into();
        a.on_event(UiEvent::Status(StatusView {
            model: "quick-thinking".into(),
            mode: "default".into(),
            cwd: "/home/kryptonite/KodAct".into(),
            cost: 0.0,
            cost_unknown: true,
            context_pct: Some(4),
            context_used: true,
            window: 200_000,
            context_tokens: 8_800,
            compact_at: 155_000,
            window_guessed: true,
            ..Default::default()
        }));
        a
    }

    fn info(recent: bool) -> front::FrontInfo {
        front::FrontInfo {
            version: "0.1.0".into(),
            model: "quick-thinking".into(),
            endpoint: "gpt.technica-engineering.net".into(),
            folder: "~/KodAct".into(),
            branch: Some("main".into()),
            recent: if recent { vec![("fix the token meter".into(), "1m ago".into())] } else { vec![] },
            ..Default::default()
        }
    }

    fn shot_with(name: &str, a: &mut App, w: u16, h: u16, recent: bool) {
        let Ok(dir) = std::env::var("KODACT_SNAPSHOT_DIR") else { return };
        let mut f = Full::new(TestBackend::new(w, h), info(recent)).unwrap();
        f.commit(a.take_pending());
        f.draw(a).unwrap();
        std::fs::write(format!("{dir}/{name}.html"), html(f.terminal.backend().buffer())).unwrap();
    }

    fn shot(name: &str, a: &mut App, w: u16, h: u16) {
        shot_with(name, a, w, h, true)
    }

    fn msg(content: Vec<ContentBlock>, stop: StopReason, parent: Option<&str>) -> UiEvent {
        UiEvent::Engine(kodact_engine::EngineEvent::Assistant {
            message: ApiMessage {
                id: "m".into(),
                kind: "message".into(),
                role: Role::Assistant,
                model: "m".into(),
                content,
                stop_reason: Some(stop),
                stop_sequence: None,
                usage: Usage::default(),
            },
            uuid: "u".into(),
            parent_tool_use_id: parent.map(str::to_string),
        })
    }

    /// A model turn: a request, the answer, then the turn ends a few seconds and some
    /// tokens later, as in a real session.
    fn answer(a: &mut App, text: &str) {
        let start = ApiMessage {
            id: "m".into(),
            kind: "message".into(),
            role: Role::Assistant,
            model: "m".into(),
            content: vec![],
            stop_reason: None,
            stop_sequence: None,
            usage: Usage::default(),
        };
        a.on_event(UiEvent::Engine(kodact_engine::EngineEvent::Stream {
            event: kodact_types::StreamEvent::MessageStart { message: start },
            parent_tool_use_id: None,
        }));
        a.on_event(msg(vec![ContentBlock::text(text)], StopReason::EndTurn, None));
        a.busy_since = Some(std::time::Instant::now() - std::time::Duration::from_secs(7));
        let mut s = a.status.clone();
        s.session_tokens += 9_400;
        a.on_event(UiEvent::Status(s));
        a.on_event(UiEvent::Idle);
    }

    fn call(id: &str, name: &str, input: serde_json::Value) -> ContentBlock {
        ContentBlock::ToolUse { id: id.into(), name: name.into(), input, cache_control: None, provider: None }
    }

    fn done(a: &mut App, id: &str, out: &str, failed: bool, result: Option<serde_json::Value>) {
        a.on_event(UiEvent::Engine(kodact_engine::EngineEvent::User {
            message: kodact_types::Message::user(vec![ContentBlock::tool_result(id.to_string(), out, failed)]),
            uuid: "r".into(),
            tool_use_result: result,
            is_meta: false,
            parent_tool_use_id: None,
        }));
    }

    #[test]
    #[ignore]
    fn snapshots() {
        use serde_json::json;
        // The screens as on the user's machine: their home folder, for `~` in paths.
        // SAFETY: an ignored test run on its own; nothing else reads HOME meanwhile.
        unsafe { std::env::set_var("HOME", "/home/kryptonite") };
        // 1. First run here.
        let mut a = app();
        shot_with("01-front-first-run", &mut a, 140, 40, false);
        // 2. Coming back: one conversation to resume.
        let mut a = app();
        shot("02-front-returning", &mut a, 140, 40);

        // 3. A formatted answer: heading, list, code and a table.
        let mut a = app();
        a.submit("how do I add an index in postgres?".into());
        a.front = false;
        answer(
            &mut a,
            "## Adding an index\n\nTwo ways, depending on whether the table is busy:\n\n\
             - **Plain**: locks writes while it builds.\n- **Concurrently**: no write lock, slower.\n\n\
             ```sql\nCREATE INDEX CONCURRENTLY idx_users_email ON users (email);\n```\n\n\
             | Option | Locks writes | Speed |\n|---|---|---|\n| `CREATE INDEX` | yes | fast |\n\
             | `CREATE INDEX CONCURRENTLY` | no | slower |\n\nUse `CONCURRENTLY` on a live table.",
        );
        shot("03-answer-markdown", &mut a, 140, 36);

        // 4. Tools: read, edit with its diff, a passing and a failing command, then the answer.
        let mut a = app();
        a.submit("fix the subtraction bug in calc.py and run the tests".into());
        a.front = false;
        a.on_event(msg(
            vec![
                ContentBlock::text("I'll look at the file first."),
                call("r1", "Read", json!({"file_path": "/home/kryptonite/KodAct/src/calc.py"})),
            ],
            StopReason::ToolUse,
            None,
        ));
        done(
            &mut a,
            "r1",
            "     1\tdef add(a, b):\n     2\t    return a - b",
            false,
            Some(
                json!({"type": "text", "file": {"filePath": "/home/kryptonite/KodAct/src/calc.py", "numLines": 2, "startLine": 1, "totalLines": 2}}),
            ),
        );
        a.on_event(msg(
            vec![call(
                "e1",
                "Edit",
                json!({"file_path": "/home/kryptonite/KodAct/src/calc.py", "old_string": "a - b", "new_string": "a + b"}),
            )],
            StopReason::ToolUse,
            None,
        ));
        done(
            &mut a,
            "e1",
            "The file was updated",
            false,
            Some(json!({"filePath": "/home/kryptonite/KodAct/src/calc.py", "structuredPatch": [{
            "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 2,
            "lines": [" def add(a, b):", "-    return a - b", "+    return a + b"]}]})),
        );
        a.on_event(msg(
            vec![call("b1", "Bash", json!({"command": "pytest -q", "description": "Run the tests"}))],
            StopReason::ToolUse,
            None,
        ));
        done(
            &mut a,
            "b1",
            "....F\nFAILED tests/test_calc.py::test_div - ZeroDivisionError\n1 failed, 4 passed in 0.12s",
            true,
            None,
        );
        a.on_event(msg(
            vec![call("b2", "Bash", json!({"command": "pytest -q tests/test_calc.py::test_add"}))],
            StopReason::ToolUse,
            None,
        ));
        done(&mut a, "b2", ".\n1 passed in 0.03s", false, None);
        answer(&mut a, "Fixed `add` (it subtracted). `test_add` passes; `test_div` was already failing on division by zero and is unrelated.");
        shot("04-tools", &mut a, 140, 40);

        // 5. Sub-agents at work.
        let mut a = app();
        a.submit("explore this repo with several agents".into());
        a.front = false;
        let mut content = vec![ContentBlock::text("I'll fan out 4 agents across the repo's areas.")];
        for (i, what) in ["Explore core crates", "Explore tools crates", "Explore evals and docs", "Skim jobs and git"]
            .iter()
            .enumerate()
        {
            content.push(call(
                &format!("t{i}"),
                "Task",
                json!({"description": what, "prompt": "p", "subagent_type": "Explore"}),
            ));
        }
        a.on_event(msg(content, StopReason::ToolUse, None));
        for (i, path) in [
            (0, "crates/kodact-engine/src/engine.rs"),
            (1, "crates/kodact-tools/src/bash.rs"),
            (1, "crates/kodact-tools/src/edit.rs"),
        ] {
            a.on_event(msg(
                vec![call("s", "Read", json!({"file_path": format!("/home/kryptonite/KodAct/{path}")}))],
                StopReason::ToolUse,
                Some(&format!("t{i}")),
            ));
        }
        shot("05-subagents", &mut a, 140, 30);
        // The wave's frames, for an animation of the scene.
        for f in 0..8u64 {
            let now = std::time::Instant::now();
            for (i, r) in a.running.iter_mut().enumerate() {
                r.since = now - std::time::Duration::from_millis(90 * f + 270 * i as u64);
            }
            shot(&format!("wave-{f:02}"), &mut a, 140, 30);
        }

        // 6. Working: an answer streaming in, the spinner, a queued message.
        let mut a = app();
        a.submit("summarize the README".into());
        a.front = false;
        a.live = "KodAct is a terminal coding agent that runs on the model you choose. It reads".into();
        a.activity = "Writing".into();
        a.submit("then list the open TODOs".into());
        shot("06-working-with-queue", &mut a, 140, 24);

        // 7. A permission question.
        let mut a = app();
        a.submit("clean the build folder".into());
        a.front = false;
        a.on_event(msg(
            vec![call("p1", "Bash", json!({"command": "rm -rf build/", "description": "Delete the build folder"}))],
            StopReason::ToolUse,
            None,
        ));
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::mem::forget(rx);
        a.on_event(UiEvent::Ask {
            prompt: kodact_engine::PermissionPrompt {
                tool_name: "Bash".into(),
                tool_use_id: "p1".into(),
                input: json!({"command": "rm -rf build/", "description": "Delete the build folder"}),
                reason: String::new(),
                suggestions: vec![],
                blocked_path: None,
            },
            reply: tx,
        });
        shot("07-permission", &mut a, 140, 30);

        // 8. The /model picker.
        let mut a = app();
        a.front = false;
        let c = |l: &str, d: &str, cur: bool| kodact_core::commands::picker::Choice {
            label: l.into(),
            detail: d.into(),
            pick: kodact_core::commands::picker::Pick::Run(String::new()),
            current: cur,
        };
        a.on_event(UiEvent::Picker(kodact_core::commands::picker::Picker {
            title: "Choose a model".into(),
            choices: vec![
                c("quick-thinking", "fast answers", true),
                c("deep-thinking", "slower, reasons longer", false),
                c("code-large", "long files", false),
            ],
        }));
        shot("08-model-picker", &mut a, 140, 24);

        // 9. A warning and an error.
        let mut a = app();
        a.on_event(UiEvent::Engine(kodact_engine::EngineEvent::Notice {
            level: kodact_engine::NoticeLevel::Warning,
            text: "KodAct doesn't know the model quick-thinking: it assumes a 200,000-token context window. Set its limits with \"modelLimits\" in settings.".into(),
        }));
        a.submit("hi".into());
        a.front = false;
        a.on_event(UiEvent::Engine(kodact_engine::EngineEvent::Notice {
            level: kodact_engine::NoticeLevel::Error,
            text: "API Error: HTTP 400 (error) from https://gpt.technica-engineering.net/api/v1/chat/completions: {\"detail\":\"Model not found\"}".into(),
        }));
        a.on_event(UiEvent::Idle);
        shot("09-warning-and-error", &mut a, 140, 20);

        // 10. Plan mode, mid-conversation.
        let mut a = app();
        a.status.mode = "plan".into();
        a.submit("plan a refactor of the session store".into());
        a.front = false;
        answer(&mut a, "Here's the plan:\n\n1. Split `store.rs` into reading and listing.\n2. Move titles into their own module.\n3. Add tests for each.");
        shot("10-plan-mode", &mut a, 140, 22);

        // 11. A small terminal: 80 by 24.
        let mut a = app();
        a.submit("what does this repo do?".into());
        a.front = false;
        answer(&mut a, "It's **KodAct**, a terminal coding agent written in Rust: a CLI, an engine that runs turns, tools, MCP, sessions and an eval harness.");
        shot("11-small-terminal", &mut a, 80, 24);
        let mut a = app();
        shot("12-front-small-terminal", &mut a, 80, 24);

        // 13. A long session nearing compaction: the gauge fills and turns amber, then red.
        for (name, used) in [("13-context-amber", 118_000), ("14-context-red", 146_000)] {
            let mut a = app();
            a.status.context_tokens = used;
            a.submit("keep going with the refactor".into());
            a.front = false;
            answer(&mut a, "Done with the session store; moving on to the transcript writer.");
            shot(name, &mut a, 140, 14);
        }

        // 15. The sidebar mid-turn: the plan, MCP servers with their tools (one failed,
        // one still connecting), the skills used, and two sub-agents at work.
        let mut a = app();
        a.submit("add an age check to signups".into());
        a.front = false;
        a.sidebar = true;
        a.status.session_tokens = 41_000;
        a.mcp = vec![
            ("jira".into(), "connected".into()),
            ("github".into(), "connected".into()),
            ("figma".into(), "failed".into()),
            ("linear".into(), "pending".into()),
        ];
        a.mcp_tools = [("jira", 12), ("github", 8)].into_iter().map(|(n, c)| (n.to_string(), c)).collect();
        a.on_event(UiEvent::Skills((1..=14).map(|i| format!("skill{i}")).collect()));
        for s in ["verify", "simplify", "verify"] {
            a.used_skill(s);
        }
        a.on_event(msg(
            vec![call(
                "p",
                "TodoWrite",
                json!({"todos": [
                    {"content": "Read the signup form", "status": "completed", "activeForm": "Reading"},
                    {"content": "Add the age field and its check", "status": "in_progress", "activeForm": "Adding"},
                    {"content": "Run the tests", "status": "pending", "activeForm": "Running"},
                    {"content": "Update the changelog", "status": "pending", "activeForm": "Updating"}
                ]}),
            )],
            StopReason::ToolUse,
            None,
        ));
        done(&mut a, "p", "ok", false, None);
        a.on_event(msg(
            vec![
                call(
                    "t1",
                    "Task",
                    json!({"subagent_type": "Explore", "description": "find the form validators", "prompt": "…"}),
                ),
                call(
                    "t2",
                    "Task",
                    json!({"subagent_type": "general-purpose", "description": "check the API docs", "prompt": "…"}),
                ),
            ],
            StopReason::ToolUse,
            None,
        ));
        for (step, parent) in [("src/forms/signup.ts", "t1"), ("src/forms/rules.ts", "t1"), ("docs/api.md", "t2")] {
            a.on_event(msg(vec![call("s", "Read", json!({"file_path": step}))], StopReason::ToolUse, Some(parent)));
        }
        let ago = |ms: u64| std::time::Instant::now() - std::time::Duration::from_millis(ms);
        a.busy_since = Some(ago(41_000));
        a.running[0].since = ago(41_000 + 280);
        a.running[1].since = ago(12_000 + 700);
        shot("15-sidebar", &mut a, 170, 40);
        // The marks' frames, for an animation of the sidebar: each fills at its own phase.
        let step = render::FILL_STEP.as_millis() as u64;
        for f in 0..render::FILL.len() as u64 {
            a.busy_since = Some(ago(41_000 + step * f));
            a.running[0].since = ago(41_000 + 280 + step * f);
            a.running[1].since = ago(12_000 + 700 + step * f);
            shot(&format!("sidebar-{f:02}"), &mut a, 170, 40);
        }
    }
}
