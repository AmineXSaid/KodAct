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
pub mod shellbox;
pub mod text;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEventKind, KeyModifiers,
    KeyboardEnhancementFlags, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
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

/// Cells as they are written to the terminal: a control character in one (from pasted
/// input, a file name, a server's name, a status command's output, anything a line was
/// built from) is drawn as its picture, and an `http://` or `https://` address on a row
/// is wrapped in an OSC 8 hyperlink so clicking it in the discussion opens it. Every
/// draw of both displays goes through here.
///
/// `ratatui::Buffer::diff` only sends the cells that changed since the last frame. A URL
/// or `[text](url)` label updated in part (streaming tokens, scrolling over a row that
/// shares a space or letter at the same column) would be cut in two if only the diff
/// cells were inspected. `Grid` keeps the full screen as last drawn so every row that
/// changed has its complete hyperlinks recomputed and any cell whose wrapped symbol
/// changed is emitted in full.
#[derive(Default)]
struct Grid {
    /// Each row's cells paired with `(wide_continuation, had_osc8)`.
    rows: std::collections::BTreeMap<u16, Vec<(ratatui::buffer::Cell, bool, bool)>>,
}

impl Grid {
    fn clear(&mut self) {
        self.rows.clear();
    }

    fn apply<'a>(
        &mut self,
        content: impl Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    ) -> Vec<(u16, u16, ratatui::buffer::Cell)> {
        let mut dirty_rows: std::collections::BTreeSet<u16> = std::collections::BTreeSet::new();
        let mut touched: std::collections::BTreeSet<(u16, u16)> = std::collections::BTreeSet::new();
        for (x, y, cell) in content {
            let mut safe = cell.clone();
            if cell.symbol().chars().any(char::is_control) {
                safe.set_symbol(&text::printable(cell.symbol()));
            }
            let row = self.rows.entry(y).or_default();
            let xi = x as usize;
            if xi > 0
                && touched.contains(&(y, x - 1))
                && row.get(xi - 1).is_some_and(|(c, _, _)| unicode_width::UnicodeWidthStr::width(c.symbol()) > 1)
            {
                continue;
            }
            if row.len() <= xi {
                row.resize(xi + 1, (ratatui::buffer::Cell::EMPTY, false, false));
            }
            // A 2-column glyph in ratatui skips `x + 1` in the diff; mark the trailing
            // column as a continuation so the row's columns stay 1-to-1 with `x`.
            let w = unicode_width::UnicodeWidthStr::width(safe.symbol());
            let prev_osc = row[xi].2;
            row[xi] = (safe, false, prev_osc);
            if w > 1 {
                if row.len() <= xi + 1 {
                    row.resize(xi + 2, (ratatui::buffer::Cell::EMPTY, false, false));
                }
                let next_osc = row[xi + 1].2;
                row[xi + 1] = (ratatui::buffer::Cell::EMPTY, true, next_osc);
            }
            dirty_rows.insert(y);
            touched.insert((y, x));
        }
        let mut out: Vec<(u16, u16, ratatui::buffer::Cell)> = vec![];
        for y in dirty_rows {
            let Some(raw_row) = self.rows.get_mut(&y) else { continue };
            let linked = link_full_row(raw_row);
            for (x, cell) in linked.into_iter().enumerate() {
                let x16 = x as u16;
                let has_osc = cell.symbol().contains("\x1b]8;");
                let had_osc = raw_row[x].2;
                raw_row[x].2 = has_osc;
                if raw_row[x].1 && !touched.contains(&(y, x16)) {
                    continue;
                }
                // Emit every cell that ratatui's diff sent, plus any cell on this row
                // that gained, changed, or lost an OSC 8 boundary.
                if touched.contains(&(y, x16)) || has_osc || had_osc {
                    out.push((x16, y, cell));
                }
            }
        }
        out
    }
}

/// Return a copy of `raw_row` with OSC 8 hyperlink sequences (`\x1b]8;;url\x07…\x1b]8;;\x07`)
/// wrapped around every `http://` / `https://` URL and any underlined `[text]` label
/// immediately preceding ` (https://…)`.
fn link_full_row(raw_row: &[(ratatui::buffer::Cell, bool, bool)]) -> Vec<ratatui::buffer::Cell> {
    let mut row: Vec<ratatui::buffer::Cell> = raw_row.iter().map(|(c, _, _)| c.clone()).collect();
    let row_len = row.len();
    // Map each character of the concatenated row text back to its cell index `x`.
    let mut text = String::new();
    let mut byte_to_cell: Vec<(usize, usize)> = vec![];
    for (x, (c, cont, _)) in raw_row.iter().enumerate() {
        if *cont {
            continue;
        }
        for ch in c.symbol().chars() {
            byte_to_cell.push((text.len(), x));
            text.push(ch);
        }
    }
    let cell_of = |byte: usize| -> usize {
        let idx = byte_to_cell.partition_point(|&(b, _)| b < byte);
        byte_to_cell.get(idx).map(|&(_, x)| x).unwrap_or(row_len)
    };
    let mut at = 0;
    while at < text.len() {
        let rest = &text[at..];
        let Some(rel) = rest.find("https://").into_iter().chain(rest.find("http://")).min() else { break };
        let u_byte = at + rel;
        let after = &text[u_byte..];
        let end_rel =
            after.find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | '"' | '\'' | ']')).unwrap_or(after.len());
        let raw = after[..end_rel].trim_end_matches(['.', ',', ';', ':', '!', '?']);
        if raw.len() > "https://".len() {
            let u_start = cell_of(u_byte);
            let last_char_byte = u_byte + raw.char_indices().next_back().map(|(b, _)| b).unwrap_or(0);
            let u_last = cell_of(last_char_byte);
            if u_start <= u_last && u_last < row.len() {
                let url: String = raw.chars().filter(|c| !c.is_control()).collect();
                wrap_cell_range(&mut row, u_start, u_last, &url);
                // If ` (url)` directly follows underlined link text (`[text](url)`), link the text too.
                if u_byte >= 2 && text.as_bytes()[u_byte - 2] == b' ' && text.as_bytes()[u_byte - 1] == b'(' {
                    let t_end = cell_of(u_byte - 2);
                    let mut t_start = t_end;
                    while t_start > 0
                        && (raw_row[t_start - 1].1
                            || row[t_start - 1].modifier.contains(ratatui::style::Modifier::UNDERLINED))
                    {
                        t_start -= 1;
                    }
                    while t_start < t_end && raw_row[t_start].1 {
                        t_start += 1;
                    }
                    let mut t_last = t_end.saturating_sub(1);
                    while t_last > t_start && raw_row[t_last].1 {
                        t_last -= 1;
                    }
                    if t_start < t_end && row[t_start].modifier.contains(ratatui::style::Modifier::UNDERLINED) {
                        wrap_cell_range(&mut row, t_start, t_last, &url);
                    }
                }
            }
        }
        at = u_byte + end_rel.max(1);
    }
    row
}

fn wrap_cell_range(row: &mut [ratatui::buffer::Cell], first: usize, last: usize, url: &str) {
    let open = format!("\x1b]8;;{url}\x07{}", row[first].symbol());
    row[first].set_symbol(&open);
    let close = format!("{}\x1b]8;;\x07", row[last].symbol());
    row[last].set_symbol(&close);
}

/// A backend that writes only sanitized cells with full-row OSC 8 hyperlinks.
pub struct Safe<B: Backend>(pub B, Grid);

impl<B: Backend> std::ops::Deref for Safe<B> {
    type Target = B;
    fn deref(&self) -> &B {
        &self.0
    }
}

impl<B: Backend> Backend for Safe<B> {
    fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        let cells = self.1.apply(content);
        self.0.draw(cells.iter().map(|(x, y, c)| (*x, *y, c)))
    }
    fn append_lines(&mut self, n: u16) -> std::io::Result<()> {
        self.0.append_lines(n)
    }
    fn hide_cursor(&mut self) -> std::io::Result<()> {
        self.0.hide_cursor()
    }
    fn show_cursor(&mut self) -> std::io::Result<()> {
        self.0.show_cursor()
    }
    fn get_cursor_position(&mut self) -> std::io::Result<Position> {
        self.0.get_cursor_position()
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> std::io::Result<()> {
        self.0.set_cursor_position(position)
    }
    fn clear(&mut self) -> std::io::Result<()> {
        self.1.clear();
        self.0.clear()
    }
    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> std::io::Result<()> {
        self.1.clear();
        self.0.clear_region(clear_type)
    }
    fn size(&self) -> std::io::Result<ratatui::layout::Size> {
        self.0.size()
    }
    fn window_size(&mut self) -> std::io::Result<ratatui::backend::WindowSize> {
        self.0.window_size()
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

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
    grid: Grid,
}

impl<B: Backend> Backend for Tracked<B> {
    fn draw<'a, I>(&mut self, content: I) -> std::io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        let mut last = None;
        let cells = self.grid.apply(content.inspect(|(x, y, _)| last = Some(Position { x: *x, y: *y })));
        self.inner.draw(cells.iter().map(|(x, y, c)| (*x, *y, c)))?;
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
        self.grid.clear();
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> std::io::Result<()> {
        self.grid.clear();
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
    /// The newest lines written above the viewport, as committed: what is written
    /// again when the window changes size.
    tail: std::collections::VecDeque<Line<'static>>,
}

/// Lines kept to write again after a resize: more than the tallest window shows.
const TAIL_LINES: usize = 400;

impl<B: Backend> Screen<B> {
    pub fn new(mut make: Box<dyn FnMut() -> B>, height: u16) -> std::io::Result<Self> {
        let pos = std::rc::Rc::new(std::cell::Cell::new(None));
        let backend = Tracked { inner: make(), pos: pos.clone(), grid: Grid::default() };
        let terminal = Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(height) })?;
        Ok(Screen { terminal, make, height, pos, tail: Default::default() })
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
        let backend = Tracked { inner: (self.make)(), pos: self.pos.clone(), grid: Grid::default() };
        self.terminal = Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(h) })?;
        self.height = h;
        Ok(())
    }

    /// Start again at the top of the window, once `wipe` has cleared it and its
    /// scrollback and sent the cursor home. The position is known, so the terminal
    /// isn't asked for it (see [`Tracked`]).
    pub fn restart(&mut self, wipe: impl FnOnce() -> std::io::Result<()>) -> std::io::Result<()> {
        self.tail.clear();
        self.start_at_top(wipe)
    }

    /// The window changed size. Where the terminal moved the earlier rows is its own
    /// affair (rows pushed into the scrollback, lines rewrapped), and it can't be asked
    /// (see [`Tracked`]), so the remembered cursor position is wrong: drawn from it, the
    /// live region landed past the window's last row, every row on top of the next.
    /// So the window (not its scrollback) is cleared by `wipe`, the viewport starts
    /// again at the top, and the newest lines that fit above it are written again.
    #[cfg(test)]
    pub fn resized(&mut self, wipe: impl FnOnce() -> std::io::Result<()>) -> std::io::Result<()> {
        self.resized_mode(false, wipe)
    }

    pub fn resized_mode(&mut self, verbose: bool, wipe: impl FnOnce() -> std::io::Result<()>) -> std::io::Result<()> {
        self.start_at_top(wipe)?;
        let (width, rows) = (self.width(), self.rows());
        // A viewport taller than the new window would hide its own last rows.
        self.height = self.height.min(rows.saturating_sub(1)).max(1);
        let room = rows.saturating_sub(self.height) as usize;
        let lines: Vec<Line<'static>> = self.tail.iter().cloned().collect();
        let wrapped = text::wrap(app::lay_out_mode(lines, width, verbose), width);
        let newest = wrapped[wrapped.len().saturating_sub(room)..].to_vec();
        self.write_above(newest)
    }

    fn start_at_top(&mut self, wipe: impl FnOnce() -> std::io::Result<()>) -> std::io::Result<()> {
        wipe()?;
        self.pos.set(Some(Position { x: 0, y: 0 }));
        let backend = Tracked { inner: (self.make)(), pos: self.pos.clone(), grid: Grid::default() };
        self.terminal = Terminal::with_options(backend, TerminalOptions { viewport: Viewport::Inline(self.height) })?;
        Ok(())
    }

    /// Write finished lines into scrollback, above the viewport.
    #[cfg(test)]
    pub fn commit(&mut self, lines: Vec<Line<'static>>) -> std::io::Result<()> {
        self.commit_mode(lines, false)
    }

    /// Write finished lines into scrollback using `verbose` detail expansion.
    pub fn commit_mode(&mut self, lines: Vec<Line<'static>>, verbose: bool) -> std::io::Result<()> {
        self.tail.extend(lines.iter().cloned());
        while self.tail.len() > TAIL_LINES {
            self.tail.pop_front();
        }
        let rows = text::wrap(app::lay_out_mode(lines, self.width(), verbose), self.width());
        self.write_above(rows)
    }

    fn write_above(&mut self, rows: Vec<Line<'static>>) -> std::io::Result<()> {
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

/// `lines` wrapped to `width` less the transcript's indent, each row indented, optionally
/// expanding collapsible thoughts, diffs, and shell outputs.
fn indented_mode(lines: Vec<Line<'static>>, width: u16, verbose: bool) -> Vec<Line<'static>> {
    let indent = TRANSCRIPT_INDENT.min(width.saturating_sub(4));
    text::wrap(app::lay_out_mode(lines, width - indent, verbose), width - indent)
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
    terminal: Terminal<Safe<B>>,
    /// Finished lines, as committed.
    transcript: Vec<Line<'static>>,
    /// The transcript wrapped to `wrapped_w` at `wrapped_layout`.
    wrapped: Vec<Line<'static>>,
    wrapped_w: u16,
    wrapped_layout: u64,
    wrapped_verbose: bool,
    /// Rows up from the transcript's end.
    pub scroll: usize,
    max_scroll: usize,
    /// Transcript rows in view at the last draw (0 when the whole transcript fits).
    in_view: usize,
    /// What the front page shows about the session.
    info: front::FrontInfo,
}

impl<B: Backend> Full<B> {
    pub fn new(backend: B, info: front::FrontInfo) -> std::io::Result<Self> {
        let terminal =
            Terminal::with_options(Safe(backend, Grid::default()), TerminalOptions { viewport: Viewport::Fullscreen })?;
        Ok(Full {
            terminal,
            transcript: vec![],
            wrapped: vec![],
            wrapped_w: 0,
            wrapped_layout: 0,
            wrapped_verbose: false,
            scroll: 0,
            max_scroll: 0,
            in_view: 0,
            info,
        })
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
    #[cfg(test)]
    pub fn commit(&mut self, lines: Vec<Line<'static>>) {
        self.commit_mode(lines, self.wrapped_verbose);
    }

    /// Finished lines join the transcript using `verbose` detail expansion.
    ///
    /// The new lines are wrapped to the width the transcript is drawn at (`wrapped_w`,
    /// set by the last draw), not the window's: with the sidebar open the two differ,
    /// and lines wrapped for the window were never added to what is drawn, so a prompt
    /// and its answer did not appear until the window was resized.
    pub fn commit_mode(&mut self, lines: Vec<Line<'static>>, verbose: bool) {
        // Before the first draw there is no width yet: that draw wraps everything.
        if self.wrapped_w > 0 {
            let rows = indented_mode(lines.clone(), self.wrapped_w, verbose);
            if self.scroll > 0 {
                self.scroll += rows.len();
            }
            if self.scroll > 0 || self.max_scroll > 0 {
                self.max_scroll += rows.len();
            }
            self.wrapped.extend(rows);
        }
        self.transcript.extend(lines);
    }

    /// Start over: the transcript is emptied (the conversation on screen was replaced),
    /// and the front page lists `recent`, the newest other conversations here.
    pub fn reset(&mut self, recent: Vec<(String, String)>) {
        self.transcript.clear();
        self.wrapped.clear();
        self.scroll = 0;
        self.max_scroll = 0;
        self.info.recent = recent;
    }

    /// Rows PgUp and PgDn move by: half the window, but never more than the transcript
    /// rows in view less one. In a short window the live region leaves only a few rows
    /// for the transcript, and a half-window page skipped lines no key could bring back.
    pub fn page(&self) -> isize {
        let half = (self.rows() as usize / 2).max(1);
        let page = if self.in_view > 0 { half.min(self.in_view.saturating_sub(1).max(1)) } else { half };
        page as isize
    }

    /// Scroll by `rows` (up when positive), within the transcript.
    pub fn scroll_by(&mut self, rows: isize) {
        self.scroll = (self.scroll as isize + rows).clamp(0, self.max_scroll as isize) as usize;
    }

    /// Update `app.find` match count and scroll the transcript to keep the active match in view.
    pub fn sync_find(&mut self, app: &mut App) {
        if self.wrapped_w > 0 && (app.layout_version != self.wrapped_layout || app.verbose != self.wrapped_verbose) {
            self.wrapped = indented_mode(self.transcript.clone(), self.wrapped_w, app.verbose);
            self.wrapped_layout = app.layout_version;
            self.wrapped_verbose = app.verbose;
            let in_view = if self.in_view > 0 {
                self.in_view
            } else {
                let h = self.rows() as usize;
                let live = render::live_view(app, self.wrapped_w, self.rows()).lines.len();
                let bar = usize::from(app.look == look::Look::Status);
                let gap = usize::from(
                    !self.wrapped.is_empty()
                        && !self.wrapped.last().is_some_and(|l| l.spans.iter().all(|s| s.content.trim().is_empty())),
                );
                h.saturating_sub(live + bar + gap)
            };
            self.max_scroll = self.wrapped.len().saturating_sub(in_view);
            self.in_view = if self.max_scroll > 0 { in_view } else { 0 };
            self.scroll = self.scroll.min(self.max_scroll);
        }
        let Some(f) = app.find.as_mut() else { return };
        if f.query.is_empty() {
            f.total = 0;
            f.current = 0;
            return;
        }
        let q = f.query.to_lowercase();
        let matches: Vec<usize> = self
            .wrapped
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(i, line)| {
                let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                text.to_lowercase().contains(&q).then_some(i)
            })
            .collect();
        f.total = matches.len();
        if matches.is_empty() {
            f.current = 0;
            self.scroll = self.scroll.min(self.max_scroll);
            return;
        }
        f.current = f.current.min(matches.len() - 1);
        let row = matches[f.current];
        let half = self.in_view / 2;
        let desired_end = (row + 1 + half).min(self.wrapped.len());
        self.scroll = self.wrapped.len().saturating_sub(desired_end).min(self.max_scroll);
    }

    /// Whether an unadorned `Up` or `Down` key can scroll the transcript instead of editing
    /// the prompt: only when no dialog, menu, search, multi-line line move or queued-message
    /// recall owns the key.
    pub fn can_scroll_key(&self, app: &App, code: KeyCode) -> bool {
        let in_lines = match code {
            KeyCode::Up => app.editor.has_line_above(),
            KeyCode::Down => app.editor.has_line_below(),
            _ => return false,
        };
        !in_lines
            && (self.max_scroll > 0 || !app.editor.is_browsing())
            && app.dialog.is_none()
            && !app.danger_armed
            && app.search.is_none()
            && app.find.is_none()
            && app.menu().is_empty()
            && app.file_menu().is_empty()
            && !(code == KeyCode::Up && app.editor.is_empty() && !app.queued.is_empty())
    }

    /// Scroll the transcript on an `Up`/`Down` key (or a burst of them from the terminal's
    /// alternate-scroll wheel mode `\x1b[?1007h`). Returns `true` when the key was used for
    /// scrolling so the prompt's text is left untouched.
    pub fn scroll_key(&mut self, app: &App, k: crossterm::event::KeyEvent, ticks: usize, delta: isize) -> bool {
        if k.kind == KeyEventKind::Release || !k.modifiers.is_empty() || !self.can_scroll_key(app, k.code) {
            return false;
        }
        if self.max_scroll == 0 && ticks <= 1 {
            return false;
        }
        let step = if ticks == 1 { delta * 3 } else { delta };
        self.scroll_by(step);
        true
    }

    pub fn draw(&mut self, app: &App) -> std::io::Result<()> {
        let size = self.terminal.size()?;
        // A gutter on both sides, so text never touches the terminal's edge.
        let pad = gutter(size.width);
        let full_w = size.width - 2 * pad;
        // The sidebar takes the right of a wide window, once the conversation has begun.
        let side = app.sidebar && !app.front && size.width >= app::SIDEBAR_MIN_COLS;
        let side_w = if side { render::sidebar_width(size.width) } else { 0 };
        let width = full_w - side_w - if side { 2 } else { 0 };
        if width != self.wrapped_w || app.layout_version != self.wrapped_layout || app.verbose != self.wrapped_verbose {
            self.wrapped = indented_mode(self.transcript.clone(), width, app.verbose);
            self.wrapped_w = width;
            self.wrapped_layout = app.layout_version;
            self.wrapped_verbose = app.verbose;
        }
        let page = front::front_page(&app.theme, app.look, &self.info, width as usize);
        let open = matches!(app.look, look::Look::Bare | look::Look::Ledger);
        let front_w = (page.box_w as u16).saturating_sub(if open { 0 } else { 2 }).max(12).min(width);
        let front_rows;
        let rows = if app.front && app.dialog.is_none() && !app.busy && self.transcript.len() <= 6 && front_w < width {
            front_rows = indented_mode(self.transcript.clone(), front_w, app.verbose);
            if front_rows.len() <= 6 {
                &front_rows
            } else {
                &self.wrapped
            }
        } else {
            &self.wrapped
        };
        let (view, max) = render::full_view(app, width, size.height, rows, &page, self.scroll);
        self.max_scroll = max;
        self.in_view = if max > 0 { self.wrapped.len() - max } else { 0 };
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

    /// The transcript as text with ANSI styles, wrapped to `cols` for the terminal's
    /// scrollback once the full screen has closed, respecting `verbose` expansion.
    pub fn transcript_text_mode(&self, cols: u16, verbose: bool) -> Vec<String> {
        text::wrap(app::lay_out_mode(self.transcript.clone(), cols, verbose), cols).iter().map(brand::ansi).collect()
    }
}

/// Where the UI draws: the whole window, or an inline viewport.
enum Display {
    Full(Box<Full<CrosstermBackend<std::io::Stdout>>>),
    Inline(Box<Screen<CrosstermBackend<std::io::Stdout>>>),
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
/// The UI is on the alternate screen, without capturing the mouse so dragging
/// selects and copies text straight from the terminal.
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
    let _ = out.write_all(b"\x1b[>4;0m");
    if ENHANCED.swap(false, std::sync::atomic::Ordering::SeqCst) {
        let _ = crossterm::execute!(out, PopKeyboardEnhancementFlags);
    }
    if FULL.swap(false, std::sync::atomic::Ordering::SeqCst) {
        let _ = out.write_all(b"\x1b[?1007l");
        let _ = crossterm::execute!(out, crossterm::terminal::LeaveAlternateScreen);
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
        // No EnableMouseCapture: the terminal keeps mouse selection so output can be
        // copied straight from the screen, while ?1007h turns mouse wheel ticks on the
        // alternate screen into Up/Down events that scroll the transcript.
        crossterm::execute!(out, crossterm::terminal::EnterAlternateScreen)?;
        out.write_all(b"\x1b[?1007h")?;
        out.flush()?;
        FULL.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    // Enable xterm modifyOtherKeys so Shift+Enter is reported on terminals without Kitty enhancement.
    let _ = out.write_all(b"\x1b[>4;1m");
    let _ = out.flush();
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

/// The signals that ask the program to end: SIGTERM (`kill`), SIGHUP (the terminal or the
/// SSH connection went away) and SIGINT sent from outside (Ctrl+C is a key here, not a
/// signal, while the terminal is in raw mode).
///
/// Left to their default they killed the process where it stood: the terminal stayed in
/// raw mode on the alternate screen with the mouse captured, and the session never
/// closed its hooks or MCP servers. Caught, they end the UI the way `/exit` does.
struct StopSignals {
    #[cfg(unix)]
    signals: Vec<(i32, tokio::signal::unix::Signal)>,
}

impl StopSignals {
    fn new() -> StopSignals {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let kinds = [(15, SignalKind::terminate()), (1, SignalKind::hangup()), (2, SignalKind::interrupt())];
            StopSignals { signals: kinds.into_iter().filter_map(|(n, k)| signal(k).ok().map(|s| (n, s))).collect() }
        }
        #[cfg(not(unix))]
        StopSignals {}
    }

    /// The number of the next signal to arrive.
    async fn recv(&mut self) -> i32 {
        #[cfg(unix)]
        {
            if self.signals.is_empty() {
                return std::future::pending().await;
            }
            let waits = self.signals.iter_mut().map(|(n, s)| {
                let n = *n;
                Box::pin(async move {
                    s.recv().await;
                    n
                })
            });
            futures::future::select_all(waits).await.0
        }
        #[cfg(not(unix))]
        std::future::pending().await
    }
}

/// The exit status after a signal ended the UI: 128 plus its number, as a shell reports
/// a signalled program (130 for SIGINT, as CLI.md documents for Ctrl-C).
fn signal_status(signal: i32) -> i32 {
    128 + signal
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
        if dir == kodact_config::state_dir() {
            let _ = kodact_config::private_state_dir();
        }
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
/// Most entries looked at to choose them.
const MAX_WALKED: usize = 400_000;

/// The project's files and directories (relative, `/`-separated; directories end in `/`),
/// skipping what `.gitignore` and hidden-file rules skip.
///
/// A project with more than [`MAX_FILES`] keeps the shallowest ones: the walk goes folder
/// by folder, so cutting it at the first 20,000 lost every file after one large folder
/// (a top-level `my notes.md` after a `gen/` of 25,000), and those are the ones people
/// name. A file left out can still be attached by typing its path.
pub fn project_files(root: &Path) -> Vec<String> {
    let mut out: Vec<(usize, String)> = vec![];
    for e in ignore::WalkBuilder::new(root).build().flatten().take(MAX_WALKED) {
        let Ok(rel) = e.path().strip_prefix(root) else { continue };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let mut p = rel.to_string_lossy().replace('\\', "/");
        if e.file_type().is_some_and(|t| t.is_dir()) {
            p.push('/');
        }
        out.push((e.depth(), p));
    }
    if out.len() > MAX_FILES {
        // Stable: within a depth the walk's order stays.
        out.sort_by_key(|(depth, _)| *depth);
        out.truncate(MAX_FILES);
    }
    out.into_iter().map(|(_, p)| p).collect()
}

fn walk_files(root: String, ui: mpsc::UnboundedSender<UiEvent>) {
    tokio::task::spawn_blocking(move || {
        let _ = ui.send(UiEvent::Files(project_files(Path::new(&root))));
    });
}

// ---- the loop ----

/// The app as it starts, already in the saved colours (`theme`) and style (`tuiStyle`).
///
/// The session task announces both too, but only once it runs: an app built without
/// them drew its first frame, and wrote the inline banner into the scrollback for good,
/// in the default colours. With `theme: none` that put colour on a terminal set to
/// show none.
fn starting_app(theme: Option<&str>, style: Option<&str>, color_ok: bool, history: Vec<String>) -> App {
    let mut app = App::new(text::Theme::named("auto", color_ok, palette::Depth::detect()), history);
    // The same events the session sends, so a later `/theme` behaves as it always did.
    if let Some(t) = theme {
        app.on_event(UiEvent::Theme(t.to_string()));
    }
    if let Some(s) = style {
        app.on_event(UiEvent::Look(s.to_string()));
    }
    app
}

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
    // No resume hint for a conversation that could not be saved.
    let persist = !o.no_session_persistence && driver.engine.transcript().path().is_some();
    let hist_path = history_path();
    let mut app = starting_app(
        driver.info.settings.str("/theme"),
        driver.info.settings.str("/tuiStyle"),
        crate::term::get().color,
        load_history(&hist_path, &cwd),
    );
    let theme = app.theme;
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
    app.danger_blocked = driver
        .info
        .settings
        .managed()
        .and_then(|m| m.pointer("/permissions/disableBypassPermissionsMode"))
        .and_then(serde_json::Value::as_str)
        == Some("disable");
    app.danger_confirmed = driver.bypass_allowed();
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
        Display::Inline(Box::new(
            Screen::new(Box::new(|| CrosstermBackend::new(std::io::stdout())), 1).map_err(draw_fail)?,
        ))
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
    let mut stop = StopSignals::new();
    let mut stopped_by = None;
    let mut pending_ev: Option<Event> = None;
    loop {
        // The conversation was replaced (/clear, /resume, /rewind): the screen starts
        // over before the conversation as it is now is written.
        if let Some(recent) = app.reset.take() {
            match &mut screen {
                Display::Full(f) => f.reset(recent),
                // Inline, the earlier lines are the terminal's own: the window and its
                // scrollback are cleared, and the live region starts again at the top.
                Display::Inline(s) => s
                    .restart(|| {
                        crossterm::execute!(
                            std::io::stdout(),
                            crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
                            crossterm::terminal::Clear(crossterm::terminal::ClearType::Purge),
                            crossterm::cursor::MoveTo(0, 0)
                        )
                    })
                    .map_err(io_fail)?,
            }
            app.dirty = true;
        }
        if !app.pending.is_empty() {
            match &mut screen {
                Display::Inline(s) => s.commit_mode(app.take_pending(), app.verbose).map_err(io_fail)?,
                Display::Full(f) => f.commit_mode(app.take_pending(), app.verbose),
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
        if pending_ev.is_none() && app.dirty && now.duration_since(last_draw) >= FRAME {
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
            ev = async {
                if let Some(ev) = pending_ev.take() {
                    Some(Ok(ev))
                } else {
                    events.next().await
                }
            } => match ev {
                Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release
                    && !app.danger_armed
                    && app.keymap.translate(k).is_some_and(|k| {
                        k.code == KeyCode::Char('l') && k.modifiers.contains(KeyModifiers::CONTROL)
                    }) => {
                    screen.clear().map_err(io_fail)?;
                    app.dirty = true;
                }
                // The full screen scrolls its transcript: the wheel (SGR mouse or ?1007h
                // alternate-scroll Up/Down), and PgUp / PgDn with no dialog open.
                Some(Ok(Event::Mouse(m))) if matches!(screen, Display::Full(_)) => {
                    let delta = match m.kind {
                        MouseEventKind::ScrollUp => Some(-3),
                        MouseEventKind::ScrollDown => Some(3),
                        _ => None,
                    };
                    if let Some(d) = delta {
                        if !app.scroll_dialog(d) {
                            if let Display::Full(f) = &mut screen {
                                f.scroll_by(-d);
                            }
                        }
                    }
                    app.dirty = true;
                }
                Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release
                    && app.dialog.is_none()
                    && !app.danger_armed
                    && matches!(k.code, KeyCode::PageUp | KeyCode::PageDown)
                    && matches!(screen, Display::Full(_)) => {
                    if let Display::Full(f) = &mut screen {
                        let page = f.page();
                        f.scroll_by(if k.code == KeyCode::PageUp { page } else { -page });
                    }
                    app.dirty = true;
                }
                Some(Ok(Event::Key(k))) if k.kind != KeyEventKind::Release
                    && k.code == KeyCode::End
                    && k.modifiers.is_empty()
                    && app.editor.is_empty()
                    && app.dialog.is_none()
                    && !app.danger_armed
                    && app.search.is_none()
                    && matches!(screen, Display::Full(ref f) if f.scroll > 0) => {
                    if let Display::Full(f) = &mut screen {
                        f.scroll = 0;
                    }
                    app.dirty = true;
                }
                Some(Ok(Event::Key(k)))
                    if k.kind != KeyEventKind::Release
                        && k.modifiers.is_empty()
                        && matches!(screen, Display::Full(ref f) if f.can_scroll_key(&app, k.code)) =>
                {
                    let mut delta: isize = if k.code == KeyCode::Up { 1 } else { -1 };
                    let mut ticks: usize = 1;
                    let needs_burst = matches!(screen, Display::Full(ref f) if f.max_scroll == 0);
                    if needs_burst {
                        while let Ok(Some(Ok(evn))) =
                            tokio::time::timeout(Duration::from_millis(20), events.next()).await
                        {
                            match evn {
                                Event::Key(kn)
                                    if kn.kind != KeyEventKind::Release
                                        && kn.modifiers.is_empty()
                                        && matches!(kn.code, KeyCode::Up | KeyCode::Down) =>
                                {
                                    delta += if kn.code == KeyCode::Up { 1 } else { -1 };
                                    ticks += 1;
                                }
                                other => {
                                    pending_ev = Some(other);
                                    break;
                                }
                            }
                        }
                    }
                    let scrolled = if let Display::Full(f) = &mut screen {
                        f.scroll_key(&app, k, ticks, delta)
                    } else {
                        false
                    };
                    if scrolled {
                        app.dirty = true;
                    } else {
                        for a in app.on_term_event(Event::Key(k)) {
                            match a {
                                Action::Send(t) => {
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
                        if app.find.is_some() {
                            if let Display::Full(f) = &mut screen {
                                f.sync_find(&mut app);
                            }
                        }
                        if exiting {
                            break;
                        }
                    }
                }
                Some(Ok(Event::Resize(..))) => {
                    if let Display::Inline(s) = &mut screen {
                        s.resized_mode(app.verbose, || {
                            crossterm::execute!(
                                std::io::stdout(),
                                crossterm::terminal::Clear(crossterm::terminal::ClearType::All),
                                crossterm::cursor::MoveTo(0, 0)
                            )
                        })
                        .map_err(io_fail)?;
                    }
                    app.dirty = true;
                }
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
                    if app.find.is_some() {
                        if let Display::Full(f) = &mut screen {
                            f.sync_find(&mut app);
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
            signal = stop.recv() => {
                // The turn in flight is stopped as Esc stops it, so the session can close.
                live.handle().interrupt();
                stopped_by = Some(signal);
                break;
            }
            _ = tick.tick(), if app.busy || hint || render::sidebar_moving(&app) => app.dirty = true,
            _ = tokio::time::sleep(frame_wait), if app.dirty && !frame_wait.is_zero() => {}
        }
    }
    // Whatever is left goes to scrollback, then the live region is cleared. The full
    // screen closes, and its transcript goes into the terminal's scrollback.
    let transcript = match &mut screen {
        Display::Inline(s) => {
            let _ = s.commit_mode(app.take_pending(), app.verbose);
            let _ = s.set_height(1);
            let _ = s.clear();
            vec![]
        }
        Display::Full(f) => {
            f.commit_mode(app.take_pending(), app.verbose);
            let cols = f.width();
            f.transcript_text_mode(cols, app.verbose)
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
        // Not `eprintln!`: after a hangup there is no terminal, and a failed print must not panic.
        let _ = writeln!(std::io::stderr(), "Resume this conversation with: kodact --resume {}", live.session_id());
    }
    Ok(stopped_by.map(signal_status).unwrap_or(crate::exit::OK))
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

    /// `theme: none` in settings still drew the first frame and the inline banner in colour:
    /// the app was built in the default colours and learned the setting an event later.
    #[test]
    fn the_app_starts_in_the_saved_colours_and_style() {
        let a = starting_app(Some("none"), Some("ledger"), true, vec![]);
        assert!(!a.theme.color, "theme none: no colour from the first frame");
        assert_eq!(a.look, look::Look::Ledger);
        let banner = brand::welcome(&a.theme, &brand::Welcome { recent: &[] }, 100, 30);
        assert!(
            banner.iter().flat_map(|l| &l.spans).all(|s| s.style.fg.is_none() && s.style.bg.is_none()),
            "the banner written at start has no colour"
        );
        let mut a = a;
        a.on_event(UiEvent::Theme("dark".into()));
        assert!(a.theme.color, "/theme dark brings colour back");
        assert!(starting_app(Some("light"), None, true, vec![]).theme.light);
        let a = starting_app(None, Some("no-such-style"), true, vec![]);
        assert!(a.theme.color && a.look == look::Look::default());
        assert!(!starting_app(Some("dark"), None, false, vec![]).theme.color, "NO_COLOR still wins");
    }

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

    /// A paste holding `ESC [ 2 J` cleared the screen, and one with tabs left the input in
    /// pieces: the prompt's text was written to the terminal as it was. Whatever a line is
    /// built from, neither display writes a control character.
    #[test]
    fn no_control_character_reaches_the_terminal() {
        let theme = text::Theme { color: false, light: false, accent: None, depth: Default::default() };
        let hostile = "before \u{1b}[2J\u{1b}]0;title\u{7} a\tb \u{1b}[?1049l \u{9b}31m \u{7f} after";
        let cells =
            |b: &TestBackend| -> String { b.buffer().content().iter().map(|c| c.symbol().to_string()).collect() };
        let check = |what: &str, drawn: String| {
            assert!(!drawn.chars().any(char::is_control), "{what}: a control character was drawn: {drawn:?}");
            assert!(drawn.contains("before") && drawn.contains("after"), "{what}: the text around them shows");
            assert!(
                drawn.contains("\u{241b}[2J") && drawn.contains("a\u{2409}b"),
                "{what}: as their pictures: {drawn:?}"
            );
        };

        // Links in the discussion become OSC 8 hyperlinks on the screen and in ANSI output,
        // including multi-span labels, wide characters, and partial frame diffs.
        let linked = text::inline("See [**bold** docs](https://kodact.dev/a) or (https://kodact.dev/b).", &theme);
        let ansi = brand::ansi(&Line::from(linked));
        assert!(ansi.contains("\x1b]8;;https://kodact.dev/a\x07bold\x1b]8;;\x07"), "{ansi:?}");
        assert!(ansi.contains("\x1b]8;;https://kodact.dev/a\x07 docs\x1b]8;;\x07"), "{ansi:?}");
        assert!(ansi.contains("\x1b]8;;https://kodact.dev/b\x07https://kodact.dev/b\x1b]8;;\x07"), "{ansi:?}");
        let mut s = Screen::new(Box::new(|| TestBackend::new(80, 4)), 1).unwrap();
        s.commit(vec![Line::from(text::inline("Visit [中文](https://kodact.dev/x).", &theme))]).unwrap();
        let drawn = cells(s.backend());
        assert!(drawn.contains("\x1b]8;;https://kodact.dev/x\x07中") && drawn.contains("文\x1b]8;;\x07"), "{drawn:?}");
        assert!(drawn.contains("\x1b]8;;https://kodact.dev/x\x07https://kodact.dev/x\x1b]8;;\x07"), "{drawn:?}");
        // Streaming a URL across two draws updates the whole URL's OSC 8 target on the row.
        s.draw(&LiveView { lines: vec![Line::from("https://kodact.dev/a")], cursor: None }).unwrap();
        assert!(cells(s.backend()).contains("\x1b]8;;https://kodact.dev/a\x07https://kodact.dev/a\x1b]8;;\x07"));
        s.draw(&LiveView { lines: vec![Line::from("https://kodact.dev/ab")], cursor: None }).unwrap();
        assert!(cells(s.backend()).contains("\x1b]8;;https://kodact.dev/ab\x07https://kodact.dev/ab\x1b]8;;\x07"));

        // Full screen: pasted into the prompt, and in a line a source failed to clean.
        let mut a = app::App::new(theme, vec![]);
        a.front = false;
        a.on_term_event(Event::Paste(hostile.into()));
        assert_eq!(a.editor.text(), hostile, "what was pasted is kept as it is");
        let mut f = Full::new(TestBackend::new(120, 20), front::FrontInfo::default()).unwrap();
        f.draw(&a).unwrap();
        check("the prompt, full screen", cells(f.terminal.backend()));
        a.status_text = Some(hostile.into());
        f.draw(&a).unwrap();
        check("a status command's output", cells(f.terminal.backend()));

        // Inline: the live region and what is written above it.
        let mut s = Screen::new(Box::new(|| TestBackend::new(120, 12)), 3).unwrap();
        let view = render::live_view(&a, 120, 11);
        s.set_height(view.lines.len() as u16).unwrap();
        s.draw(&view).unwrap();
        check("the prompt, inline", cells(s.backend()));
        let mut s = Screen::new(Box::new(|| TestBackend::new(120, 12)), 1).unwrap();
        s.terminal
            .insert_before(1, |buf| Paragraph::new(Line::from(hostile.to_string())).render(buf.area, buf))
            .unwrap();
        check("a line written above the viewport", cells(s.backend()));

        // The cursor is where the next character lands: counted on what is drawn.
        let mut a = app::App::new(theme, vec![]);
        a.on_term_event(Event::Paste("a\tb\u{1b}c".into()));
        let view = render::live_view(&a, 80, 10);
        let row =
            view.lines.iter().map(text::plain).find(|l| l.contains("a\u{2409}b\u{241b}c")).expect("the input row");
        let (x, _) = view.cursor.expect("a cursor");
        assert_eq!(x as usize, text::width(&row[..row.find('c').unwrap() + 1]), "{row:?}");
    }

    /// `@my` offered nothing for a top-level `my notes.md` in a project whose `gen/` folder
    /// held 25,000 files: the walk stopped at its first 20,000 paths, all inside `gen/`.
    #[test]
    fn a_large_folder_does_not_push_shallow_files_out_of_at_completion() {
        let d = tempfile::tempdir().unwrap();
        for dir in 0..210 {
            let p = d.path().join("gen").join(format!("d{dir:03}"));
            std::fs::create_dir_all(&p).unwrap();
            for f in 0..100 {
                std::fs::write(p.join(format!("f{f:02}.txt")), "").unwrap();
            }
        }
        std::fs::write(d.path().join("my notes.md"), "").unwrap();
        std::fs::create_dir_all(d.path().join("src")).unwrap();
        std::fs::write(d.path().join("src/zz_last.rs"), "").unwrap();
        let files = project_files(d.path());
        assert_eq!(files.len(), MAX_FILES, "still capped");
        for shallow in ["my notes.md", "src/", "src/zz_last.rs", "gen/", "gen/d209/"] {
            assert!(files.iter().any(|f| f == shallow), "{shallow} is offered");
        }
        // A small project is listed whole.
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        std::fs::write(d.path().join("a/b/deep.txt"), "").unwrap();
        std::fs::write(d.path().join("top.txt"), "").unwrap();
        let mut all = project_files(d.path());
        all.sort();
        assert_eq!(all, ["a/", "a/b/", "a/b/deep.txt", "top.txt"]);
    }

    /// With the sidebar open (Ctrl+B on a wide window), a prompt and its answer were sent
    /// and answered but never drawn: new lines were wrapped for the window's width and
    /// dropped, because the transcript is drawn at the narrower width beside the sidebar.
    #[test]
    fn new_lines_show_with_the_sidebar_open() {
        let theme = text::Theme { color: false, light: false, accent: None, depth: Default::default() };
        let mut a = app::App::new(theme, vec![]);
        a.front = false;
        let mut f = Full::new(TestBackend::new(160, 30), front::FrontInfo::default()).unwrap();
        f.commit(vec![Line::from("before the sidebar")]);
        f.draw(&a).unwrap();
        a.sidebar = true;
        f.draw(&a).unwrap();
        f.commit(vec![Line::from("typed with the sidebar open"), Line::from("x".repeat(300))]);
        f.draw(&a).unwrap();
        let shown = rows(f.terminal.backend());
        assert!(shown.iter().any(|r| r.contains("typed with the sidebar open")), "{shown:#?}");
        let side = shown.iter().find_map(|r| r.find('╭')).expect("the sidebar's panes");
        let long: Vec<&String> = shown.iter().filter(|r| r.contains("xxxxxxxx")).collect();
        assert!(
            long.len() >= 3 && long.iter().all(|r| r.trim_end().rfind('x').unwrap() < side),
            "wrapped beside it: {shown:#?}"
        );
        // Scrolled up, the view stays on the same lines as more arrive.
        f.commit((0..60).map(|i| Line::from(format!("filler {i}"))).collect());
        f.draw(&a).unwrap();
        f.scroll_by(10);
        f.draw(&a).unwrap();
        let before = rows(f.terminal.backend());
        f.commit(vec![Line::from("arrives while scrolled up"), Line::from("and another")]);
        f.draw(&a).unwrap();
        let after = rows(f.terminal.backend());
        assert_eq!(before[..10], after[..10], "the view stays put");
        // And closing the sidebar draws everything at the window's width again.
        a.sidebar = false;
        f.scroll = 0;
        f.draw(&a).unwrap();
        assert!(rows(f.terminal.backend()).iter().any(|r| r.contains("and another")));
    }

    /// In a 40x12 window PgUp moved six rows while four rows of the transcript were in
    /// view: two rows in every six could not be read. Paging never skips a row.
    #[test]
    fn paging_never_skips_a_row_in_a_short_window() {
        let theme = text::Theme { color: false, light: false, accent: None, depth: Default::default() };
        let mut a = app::App::new(theme, vec![]);
        a.front = false;
        for (cols, lines) in [(40u16, 12u16), (60, 20), (110, 36), (200, 50)] {
            let mut f = Full::new(TestBackend::new(cols, lines), front::FrontInfo::default()).unwrap();
            f.commit((1..=300).map(|i| Line::from(format!("row {i:03}"))).collect());
            f.draw(&a).unwrap();
            let mut seen = std::collections::BTreeSet::new();
            for _ in 0..400 {
                for r in rows(f.terminal.backend()) {
                    if let Some(n) = r.trim().strip_prefix("row ").and_then(|n| n.parse::<u32>().ok()) {
                        seen.insert(n);
                    }
                }
                f.scroll_by(f.page());
                f.draw(&a).unwrap();
            }
            let missing: Vec<u32> = (1..=300).filter(|n| !seen.contains(n)).collect();
            assert!(missing.is_empty(), "{cols}x{lines}: PgUp skipped rows {missing:?}");
            assert!(f.page() >= 1 && f.page() <= (lines as isize / 2).max(1), "{cols}x{lines}: {}", f.page());
        }
        // A tall window still pages by half of itself.
        let mut f = Full::new(TestBackend::new(110, 36), front::FrontInfo::default()).unwrap();
        f.commit((1..=300).map(|i| Line::from(format!("row {i:03}"))).collect());
        f.draw(&a).unwrap();
        assert_eq!(f.page(), 18);
    }

    #[test]
    fn a_signal_ends_the_ui_with_the_shells_status_for_it() {
        assert_eq!([signal_status(2), signal_status(15), signal_status(1)], [130, 143, 129]);
    }

    /// SIGTERM used to kill the process where it stood, the terminal left in raw mode on
    /// the alternate screen. The signals are caught, so the loop can end as `/exit` does.
    #[cfg(unix)]
    #[tokio::test]
    async fn stop_signals_are_caught_not_fatal() {
        let mut stop = StopSignals::new();
        assert_eq!(stop.signals.len(), 3, "SIGTERM, SIGHUP and SIGINT have handlers");
        // SIGHUP to this process: with the handler in place it arrives as a value.
        let me = std::process::id().to_string();
        assert!(std::process::Command::new("kill").args(["-HUP", &me]).status().unwrap().success());
        let got = tokio::time::timeout(Duration::from_secs(5), stop.recv()).await.expect("the signal is delivered");
        assert_eq!(got, 1);
    }

    /// A terminal whose window can change size under the UI.
    #[derive(Clone)]
    struct Window(std::rc::Rc<std::cell::RefCell<TestBackend>>);

    impl Backend for Window {
        fn draw<'a, I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>>(&mut self, c: I) -> std::io::Result<()> {
            self.0.borrow_mut().draw(c)
        }
        fn append_lines(&mut self, n: u16) -> std::io::Result<()> {
            self.0.borrow_mut().append_lines(n)
        }
        fn hide_cursor(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn show_cursor(&mut self) -> std::io::Result<()> {
            Ok(())
        }
        fn get_cursor_position(&mut self) -> std::io::Result<Position> {
            self.0.borrow_mut().get_cursor_position()
        }
        fn set_cursor_position<P: Into<Position>>(&mut self, p: P) -> std::io::Result<()> {
            self.0.borrow_mut().set_cursor_position(p)
        }
        fn clear(&mut self) -> std::io::Result<()> {
            self.0.borrow_mut().clear()
        }
        fn clear_region(&mut self, t: ratatui::backend::ClearType) -> std::io::Result<()> {
            self.0.borrow_mut().clear_region(t)
        }
        fn size(&self) -> std::io::Result<ratatui::layout::Size> {
            self.0.borrow().size()
        }
        fn window_size(&mut self) -> std::io::Result<ratatui::backend::WindowSize> {
            self.0.borrow_mut().window_size()
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Inline, a window made smaller left the live region drawn from a cursor row that no
    /// longer existed: the input box, its border and the status line all on the last row.
    /// After a resize the whole live region is on screen, under the newest lines.
    #[test]
    fn the_inline_viewport_is_whole_after_the_window_changes_size() {
        let window = Window(std::rc::Rc::new(std::cell::RefCell::new(TestBackend::new(80, 30))));
        let w = window.clone();
        let mut s = Screen::new(Box::new(move || w.clone()), 4).unwrap();
        s.commit((1..=40).map(|i| Line::from(format!("answer line {i}"))).collect()).unwrap();
        let view = LiveView {
            lines: ["top of the box", "> typed", "bottom of the box", "the status line"].map(Line::from).to_vec(),
            cursor: Some((7, 1)),
        };
        s.draw(&view).unwrap();
        let screen = |w: &Window| rows(&w.0.borrow());
        assert_eq!(screen(&window)[29], "the status line", "at the bottom of the tall window");

        for (cols, lines) in [(40, 12), (30, 6), (120, 40), (40, 12)] {
            window.0.borrow_mut().resize(cols, lines);
            s.resized(|| window.0.borrow_mut().clear()).unwrap();
            s.draw(&view).unwrap();
            let now = screen(&window);
            let at = |text: &str| now.iter().position(|r| r == text).unwrap_or_else(|| panic!("{text:?} in {now:#?}"));
            let top = at("top of the box");
            assert_eq!(
                [at("> typed"), at("bottom of the box"), at("the status line")],
                [top + 1, top + 2, top + 3],
                "the live region is whole at {cols}x{lines}: {now:#?}"
            );
            // The newest lines are above it again, in order, none twice.
            assert_eq!(now[top - 1], "answer line 40", "{now:#?}");
            let shown: Vec<&String> = now.iter().filter(|r| r.starts_with("answer line")).collect();
            assert_eq!(shown.len(), top.min(40), "{now:#?}");
            assert_eq!(window.0.borrow_mut().get_cursor_position().unwrap(), Position { x: 7, y: top as u16 + 1 });
        }

        // A viewport taller than the new window gives up rows instead of hiding its last ones.
        s.set_height(20).unwrap();
        window.0.borrow_mut().resize(40, 8);
        s.resized(|| window.0.borrow_mut().clear()).unwrap();
        assert!(s.height() < 8);
        // /clear forgets the lines: a resize after it writes none of the old conversation.
        s.restart(|| window.0.borrow_mut().clear()).unwrap();
        s.resized(|| window.0.borrow_mut().clear()).unwrap();
        assert!(!screen(&window).iter().any(|r| r.starts_with("answer line")));
    }

    /// `/clear` in the inline UI: the viewport starts again at the top of the cleared
    /// window, without asking the terminal where the cursor is.
    #[test]
    fn restarting_the_viewport_never_asks_the_terminal() {
        let asked = std::rc::Rc::new(std::cell::Cell::new(0));
        let a = asked.clone();
        let mut s =
            Screen::new(Box::new(move || AsksOnce { inner: TestBackend::new(40, 12), asked: a.clone() }), 2).unwrap();
        s.commit(vec![Line::from("the old conversation")]).unwrap();
        let mut wiped = false;
        s.restart(|| {
            wiped = true;
            Ok(())
        })
        .expect("starting over works");
        assert!(wiped);
        s.commit(vec![Line::from("the new one")]).unwrap();
        s.draw(&LiveView { lines: vec![Line::from("> ")], cursor: Some((2, 0)) }).unwrap();
        assert_eq!(asked.get(), 1, "only the first viewport asked");
        assert_eq!(s.height(), 2);
    }

    #[test]
    fn the_full_screen_starts_over_when_the_conversation_is_replaced() {
        let theme = text::Theme { color: false, light: false, accent: None, depth: Default::default() };
        let mut a = app::App::new(theme, vec![]);
        let mut f = Full::new(TestBackend::new(100, 30), front::FrontInfo::default()).unwrap();
        a.submit("the old question".into());
        f.commit(a.take_pending());
        f.scroll = 3;
        f.draw(&a).unwrap();
        assert!(rows(f.terminal.backend()).iter().any(|r| r.contains("the old question")));
        // /clear: the session says the conversation is empty and what can be resumed.
        a.on_event(app::UiEvent::Reset {
            replay: vec![],
            recent: vec![("the old question".into(), "just now".into())],
            echo: Some("/clear".into()),
        });
        a.on_event(app::UiEvent::Idle);
        f.reset(a.reset.take().expect("the app asks for the screen to start over"));
        f.commit(a.take_pending());
        f.draw(&a).unwrap();
        let rows = rows(f.terminal.backend());
        assert_eq!(f.scroll, 0);
        assert!(!rows.iter().any(|r| r.contains("❯ the old question")), "the old conversation is gone: {rows:#?}");
        assert!(rows.iter().any(|r| r.contains("/resume") && r.contains("the old question")), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains("❯ /clear")), "{rows:#?}");
        // A long warning on the front page wraps once to the box's width (never pre-wrapped at
        // the window's width and then wrapped a second time into a jagged short line) and lines
        // up with `/resume` and `❯`.
        f.reset(vec![("hi".into(), "just now".into())]);
        a.on_event(app::UiEvent::Engine(kodact_engine::EngineEvent::Notice {
            level: kodact_engine::NoticeLevel::Warning,
            text: "KodAct doesn't know the model deep-thinking: it assumes a 1,000,000-token context window \
                   and 32,000 output tokens, and has no price for it. Set its limits with \"modelLimits\" in \
                   settings (and its price with \"modelPricing\"), or KODACT_CONTEXT_WINDOW for every unknown model."
                .into(),
        }));
        f.commit(a.take_pending());
        f.draw(&a).unwrap();
        let wrows = self::rows(f.terminal.backend());
        assert!(
            !wrows.iter().any(|r| r.trim() == "settings (and its price"),
            "no double-wrapped orphan line: {wrows:#?}"
        );
        let box_row = wrows.iter().find(|r| r.contains("What shall we build")).unwrap();
        let resume_row = wrows.iter().find(|r| r.contains("/resume")).unwrap();
        let warn_row = wrows.iter().find(|r| r.contains("▲ KodAct")).unwrap();
        let col = |r: &str, s: &str| text::width(&r[..r.find(s).unwrap()]);
        assert_eq!(col(resume_row, "/resume"), col(box_row, "❯"), "`/resume` aligns with `❯`: {wrows:#?}");
        assert_eq!(col(warn_row, "▲"), col(box_row, "❯"), "warning aligns with `❯`: {wrows:#?}");
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

    #[test]
    fn the_mouse_wheel_scrolls_the_discussion_without_changing_the_prompt() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let theme = text::Theme { color: false, light: false, accent: None, depth: Default::default() };
        let mut a = app::App::new(theme, vec!["earlier prompt".into(), "latest prompt".into()]);
        a.front = false;
        let mut f = Full::new(TestBackend::new(80, 16), front::FrontInfo::default()).unwrap();

        // On a short discussion (`max_scroll == 0`), a wheel burst (`ticks > 1`) is absorbed
        // without replacing the prompt with history, while a single Up key (`ticks == 1`)
        // still recalls history.
        f.commit(vec![Line::from("• short answer")]);
        f.draw(&a).unwrap();
        assert_eq!(f.max_scroll, 0);
        let up = KeyEvent::new(KeyCode::Up, KeyModifiers::NONE);
        let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
        assert!(f.scroll_key(&a, up, 3, 3), "wheel burst on a short screen is absorbed");
        assert!(a.editor.is_empty(), "chatbox stays empty after a wheel burst");
        assert!(!f.scroll_key(&a, up, 1, 1), "a single Up key on a short screen recalls history");

        // Once the LLM discussion overflows the viewport (`max_scroll > 0`), rolling the wheel
        // (whether 1 event per notch or a 3-event ?1007h burst) scrolls up and down through
        // the discussion and never overwrites the chatbox text.
        f.commit((1..=40).map(|i| Line::from(format!("• discussion line {i:02}"))).collect());
        f.draw(&a).unwrap();
        assert!(f.max_scroll > 0);
        a.editor.set("draft in chatbox");
        assert!(f.scroll_key(&a, up, 1, 1));
        assert_eq!(f.scroll, 3);
        assert_eq!(a.editor.text(), "draft in chatbox");
        assert!(f.scroll_key(&a, up, 3, 3));
        assert_eq!(f.scroll, 6);
        assert_eq!(a.editor.text(), "draft in chatbox");
        assert!(f.scroll_key(&a, down, 1, -1));
        assert_eq!(f.scroll, 3);
        assert_eq!(a.editor.text(), "draft in chatbox");
        assert!(f.scroll_key(&a, down, 3, -3));
        assert_eq!(f.scroll, 0);
        // Rolling further down at the bottom or up at the top clamps cleanly without touching the chatbox.
        assert!(f.scroll_key(&a, down, 1, -1));
        assert_eq!(f.scroll, 0);
        assert_eq!(a.editor.text(), "draft in chatbox");

        // Ctrl+P / Ctrl+N still recall prompt history even when the discussion is scrollable.
        a.editor.clear();
        a.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(a.editor.text(), "latest prompt");
        a.on_key(KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL));
        assert_eq!(a.editor.text(), "earlier prompt");
        a.on_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
        assert_eq!(a.editor.text(), "latest prompt");

        // A `/` menu or multi-line edit still owns Up/Down.
        a.on_event(app::UiEvent::Commands(vec![app::CommandInfo {
            name: "clear".into(),
            description: "Clear".into(),
            args: String::new(),
        }]));
        a.editor.set("/");
        assert!(!f.can_scroll_key(&a, KeyCode::Down));
        a.editor.set("line one\nline two");
        assert!(!f.can_scroll_key(&a, KeyCode::Up), "Up from line 2 moves to line 1 first");
    }

    #[test]
    fn ctrl_f_searches_discussion_and_ctrl_r_shows_match_list() {
        use crossterm::event::KeyEvent;
        let mut a = app::App::new(
            text::Theme::named("dark", false, palette::Depth::default()),
            vec!["Fix Bug One".into(), "add feature two".into()],
        );
        a.front = false;
        let mut f = Full::new(TestBackend::new(80, 16), front::FrontInfo::default()).unwrap();
        let mut lines: Vec<Line<'static>> = vec![Line::from("• target alpha at the top")];
        lines.extend((1..=40).map(|i| Line::from(format!("• filler line {i:02}"))));
        lines.push(Line::from("• target beta at the bottom"));
        f.commit(lines);
        f.draw(&a).unwrap();
        assert_eq!(f.scroll, 0);

        // Ctrl+F opens discussion find, counts matches, and Enter jumps to the older match.
        a.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
        assert!(a.find.is_some());
        for c in "TARGET".chars() {
            a.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
            f.sync_find(&mut a);
        }
        assert_eq!(a.find.as_ref().unwrap().total, 2);
        assert_eq!(a.find.as_ref().unwrap().current, 0);
        // Jump to older match (top of transcript): scroll increases.
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        f.sync_find(&mut a);
        assert_eq!(a.find.as_ref().unwrap().current, 1);
        assert!(f.scroll > 0, "jumping to older match scrolls up: {}", f.scroll);
        f.draw(&a).unwrap();
        let buf: String = f.terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(buf.contains("find: TARGET") && buf.contains("2/2"), "find bar renders count:\n{buf}");
        a.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(a.find.is_none());

        // Ctrl+R searches case-insensitively and renders the match list with counts.
        a.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        for c in "bug".chars() {
            a.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        assert_eq!(a.editor.text(), "Fix Bug One");
        f.draw(&a).unwrap();
        let buf: String = f.terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(buf.contains("history: bug") && buf.contains("1/1") && buf.contains("Fix Bug One"), "{buf}");
    }

    #[test]
    fn ctrl_o_rewraps_full_transcript_and_updates_find() {
        use crossterm::event::KeyEvent;
        use kodact_types::{Delta, StreamEvent};
        let mut a = app::App::new(text::Theme::named("dark", false, palette::Depth::default()), vec![]);
        a.front = false;
        let mut f = Full::new(TestBackend::new(80, 14), front::FrontInfo::default()).unwrap();

        let thought: String = (1..=20)
            .map(|i| if i == 1 { "needle in thought row 1".to_string() } else { format!("reasoning step {i}") })
            .collect::<Vec<_>>()
            .join("\n");
        let ev = |event| UiEvent::Engine(kodact_engine::EngineEvent::Stream { event, parent_tool_use_id: None });
        a.on_event(ev(StreamEvent::ContentBlockDelta { index: 0, delta: Delta::ThinkingDelta { thinking: thought } }));
        a.on_event(ev(StreamEvent::ContentBlockStop { index: 0 }));

        f.commit_mode(a.take_pending(), a.verbose);
        f.draw(&a).unwrap();
        let buf: String = f.terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(buf.contains("ctrl+o to show"), "{buf}");
        assert!(!buf.contains("needle in thought"), "{buf}");

        // Search while collapsed finds 0 matches inside the hidden thought.
        a.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
        for c in "needle".chars() {
            a.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        f.sync_find(&mut a);
        assert_eq!(a.find.as_ref().unwrap().total, 0);

        // Pressing Ctrl+O while Ctrl+F is open keeps find open, expands the thought in place,
        // updates max_scroll & match count, and brings the hidden match into view!
        a.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert!(a.find.is_some() && a.verbose);
        f.sync_find(&mut a);
        assert_eq!(a.find.as_ref().unwrap().total, 1);
        assert!(f.max_scroll > 0);
        f.draw(&a).unwrap();
        let expanded_buf: String = f.terminal.backend().buffer().content().iter().map(|c| c.symbol()).collect();
        assert!(expanded_buf.contains("needle in thought"), "{expanded_buf}");

        // Collapsing via Ctrl+O again re-wraps and clamps scroll back cleanly.
        a.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert!(!a.verbose);
        f.sync_find(&mut a);
        assert_eq!(a.find.as_ref().unwrap().total, 0);
        assert_eq!(f.scroll, 0);
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

    /// The screen as a page, a cell a column wide as in a terminal, whatever font draws its
    /// glyph: DejaVu Sans Mono has no braille, so FreeMono draws it, a monospace font too.
    fn html(buf: &ratatui::buffer::Buffer, light: bool) -> String {
        let area = buf.area;
        // A light terminal's page for a light theme, a dark one's otherwise.
        let (bg, fg) = if light { ("#fafafa", "#1f2328") } else { ("#0b0c0e", "#d8d8d8") };
        let mut s = format!(
            "<!doctype html><meta charset=utf-8><style>body{{margin:0;background:{bg}}}\
             pre{{margin:0;padding:8px;font:16px/1.15 'DejaVu Sans Mono','FreeMono',monospace;color:{fg};background:{bg}}}\
             span{{white-space:pre;display:inline-block;width:1ch;height:1.15em;vertical-align:top}}</style><pre>",
        );
        for y in 0..area.height {
            for x in 0..area.width {
                let c = &buf[(x, y)];
                let mut st = format!("color:{};", css(c.fg, fg));
                if c.bg != Color::Reset {
                    st.push_str(&format!("background:{};", css(c.bg, "transparent")));
                }
                if c.modifier.contains(ratatui::style::Modifier::BOLD) {
                    st.push_str("font-weight:bold;");
                }
                if c.modifier.contains(ratatui::style::Modifier::DIM) {
                    st.push_str("opacity:.6;");
                }
                if c.modifier.contains(ratatui::style::Modifier::CROSSED_OUT) {
                    st.push_str("text-decoration:line-through;");
                }
                if c.modifier.contains(ratatui::style::Modifier::ITALIC) {
                    st.push_str("font-style:italic;");
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
        std::fs::write(format!("{dir}/{name}.html"), html(f.terminal.backend().buffer(), a.theme.light)).unwrap();
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
        // one still connecting), the skills used, and two sub-agents at work, in a dark and a
        // light terminal. Drawn afresh for each frame: a shot takes the transcript into its own
        // screen.
        let sidebar_scene = |offset: std::time::Duration, light: bool| -> App {
            let mut a = app();
            a.theme.light = light;
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
            let plan = json!([
                {"content": "Read the signup form", "status": "completed", "activeForm": "Reading"},
                {"content": "Add the age field and its check", "status": "in_progress", "activeForm": "Adding"},
                {"content": "Run the tests", "status": "pending", "activeForm": "Running"},
                {"content": "Update the changelog", "status": "pending", "activeForm": "Updating"}
            ]);
            done(&mut a, "p", "ok", false, Some(json!({"oldTodos": [], "newTodos": plan})));
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
            let ago = |ms: u64| std::time::Instant::now() - std::time::Duration::from_millis(ms) - offset;
            a.busy_since = Some(ago(41_000));
            a.running[0].since = ago(41_000 + 280);
            a.running[1].since = ago(12_000 + 700);
            a
        };
        shot("15-sidebar", &mut sidebar_scene(std::time::Duration::ZERO, false), 170, 40);
        shot("15-sidebar-light", &mut sidebar_scene(std::time::Duration::ZERO, true), 170, 40);
        // The marks' frames, for an animation of the sidebar: each fills at its own phase.
        for f in 0..render::FILL.len() as u32 {
            shot(&format!("sidebar-{f:02}"), &mut sidebar_scene(render::FILL_STEP * f, false), 170, 40);
        }
        // The plan's task in progress, its ring's gap going round, for an animation of the Plan pane.
        for f in 0..render::RING.len() as u32 {
            shot(&format!("15-sidebar-ring-{f}"), &mut sidebar_scene(render::RING_STEP * f, false), 170, 40);
        }

        // 16. The plan in the transcript, then its progress, with the spinner saying the task in
        // progress: at 100 columns (no sidebar), in a dark and a light terminal.
        for (name, light) in [("16-plan", false), ("16-plan-light", true)] {
            let mut a = App::new(Theme { color: true, light, accent: None, depth: Default::default() }, vec![]);
            a.transcript_indent = TRANSCRIPT_INDENT as usize;
            a.submit("add an age check to signups".into());
            a.front = false;
            let todo = |content: &str, status: &str, active: &str| json!({"content": content, "status": status, "activeForm": active});
            let first = json!([
                todo("Read the signup form", "in_progress", "Reading the signup form"),
                todo("Add the age field and its check", "pending", "Adding the age field and its check"),
                todo("Run the tests", "pending", "Running the tests"),
                todo("Update the changelog", "pending", "Updating the changelog"),
            ]);
            a.on_event(msg(vec![call("p1", "TodoWrite", json!({"todos": first}))], StopReason::ToolUse, None));
            done(&mut a, "p1", "ok", false, Some(json!({"oldTodos": [], "newTodos": first})));
            a.on_event(msg(
                vec![call("r1", "Read", json!({"file_path": "/home/kryptonite/KodAct/src/forms/signup.ts"}))],
                StopReason::ToolUse,
                None,
            ));
            let read = json!({"type": "text", "file": {"filePath": "src/forms/signup.ts", "numLines": 84, "startLine": 1, "totalLines": 84}});
            done(&mut a, "r1", "1\tconst form = …", false, Some(read));
            let second = json!([
                todo("Read the signup form", "completed", "Reading the signup form"),
                todo("Add the age field and its check", "in_progress", "Adding the age field and its check"),
                todo("Run the tests", "pending", "Running the tests"),
                todo("Update the changelog", "pending", "Updating the changelog"),
            ]);
            a.on_event(msg(vec![call("p2", "TodoWrite", json!({"todos": second}))], StopReason::ToolUse, None));
            done(&mut a, "p2", "ok", false, Some(json!({"oldTodos": first, "newTodos": second})));
            a.busy_since = Some(std::time::Instant::now() - std::time::Duration::from_secs(23));
            shot(name, &mut a, 100, 24);
        }
    }
}
