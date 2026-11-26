//! The live region at the bottom of the terminal, drawn from the app state:
//! the unfinished answer line, the spinner, a dialog, queued messages, the
//! input box, the `/` menu and the status line (docs/TUI.md, "Renderer").

use kodact_core::glyphs;
use std::time::{Duration, Instant};

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use super::app::{App, StatusView};
use super::front::FrontPage;
use super::look::Look;
use super::palette;
use super::text::{self, width as text_width};

use super::overlays::*;
pub use super::sidebar::*;

/// The rows to draw, and where the cursor goes (column, row) when it shows.
#[derive(Debug, Clone, Default)]
pub struct LiveView {
    pub lines: Vec<Line<'static>>,
    pub cursor: Option<(u16, u16)>,
}

const SPINNER: [&str; 10] = kodact_core::glyphs::SPINNER;
pub const SPINNER_STEP: Duration = Duration::from_millis(80);
/// How long a hint stays in the status line.
pub const HINT_FOR: Duration = Duration::from_secs(2);
/// How long an action command's one-line answer stays in the status line.
pub const TOAST_FOR: Duration = Duration::from_secs(5);
pub(super) const MENU_ROWS: usize = 8;
const LIVE_ROWS: usize = 3;
/// The composer's placeholder, as KodAct's says it.
pub const PLACEHOLDER: &str = "What shall we build today?";
/// The default style's placeholder on a first run in a folder: an example of a request.
pub const FIRST_RUN_PLACEHOLDER: &str = "What shall we build? Try \"fix the failing test\"";
/// The frame style's placeholder while the model works.
/// What the empty input says, for the moment: the welcome on the front page, the way
/// on after an error or a plan, else a reply.
fn placeholder(app: &App) -> &'static str {
    if app.look == super::look::Look::Frame && !app.talked && app.first_run {
        FIRST_RUN_PLACEHOLDER
    } else if app.look != super::look::Look::Frame || !app.talked {
        PLACEHOLDER
    } else if app.model_down {
        "Try again, or /model to choose another"
    } else if app.status.mode == "plan" {
        "Refine the plan, or shift+tab to leave plan mode"
    } else {
        "Reply…"
    }
}

pub const QUEUE_PLACEHOLDER: &str = "Type to queue a message";
/// What the spinner says while the model works, in KodAct's voice (its spinner verbs).
pub const VERBS: [&str; 8] =
    ["Acting", "Reading the code", "Thinking", "Building", "Wiring it up", "Checking", "Shaping", "Polishing"];
/// How long one verb stays.
const VERB_FOR: Duration = Duration::from_secs(4);
/// How long the turning dot of a running tool shows one face.
const RUNNING_STEP: Duration = Duration::from_millis(220);

/// `s` cut to `w` columns, with an ellipsis when it was longer.
pub fn fit(s: &str, w: usize) -> String {
    if text_width(s) <= w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if used + cw + 1 > w {
            break;
        }
        out.push(c);
        used += cw;
    }
    if w > 0 {
        out.push('…');
    }
    out
}

use super::editor::char_rows;

fn status_left(app: &App, now: Instant) -> (String, Style) {
    if app.danger_armed {
        return (super::app::DANGER_PROMPT.to_string(), app.theme.error());
    }
    if let Some((h, at)) = &app.hint {
        if now.duration_since(*at) < HINT_FOR {
            return (h.clone(), app.theme.dim());
        }
    }
    // An action command's answer (`/reload-plugins`): longer than a hint, red when it failed.
    if let Some((text, is_error, at)) = &app.toast {
        if now.duration_since(*at) < TOAST_FOR {
            return (text.clone(), if *is_error { app.theme.error() } else { app.theme.accent() });
        }
    }
    let t = app.theme;
    // The default style's box names the mode, so the line keeps the folder.
    let mode = if app.look == super::look::Look::Frame { "" } else { app.status.mode.as_str() };
    match mode {
        "acceptEdits" => (format!("{} accept edits on (shift+tab to cycle)", glyphs::MODE_ACCEPT_EDITS), t.warning()),
        "plan" => (format!("{} plan mode on (shift+tab to cycle)", glyphs::MODE_PLAN), t.success()),
        "auto" => ("auto mode on (shift+tab to cycle)".into(), t.violet()),
        "bypassPermissions" => (format!("{} dangerous mode on (shift+tab to cycle)", glyphs::MODE_BYPASS), t.error()),
        "dontAsk" => ("don't ask mode on (shift+tab to cycle)".into(), t.dim()),
        // The folder the session works in; `?` still lists the keys.
        _ if !app.status.cwd.is_empty() => (super::short_path(&app.status.cwd), t.subtle()),
        _ => ("? for shortcuts".into(), t.subtle()),
    }
}

fn status_right(app: &App) -> String {
    if let Some(t) = &app.status_text {
        return t.clone();
    }
    let s = &app.status;
    let mut parts = vec![];
    if !s.model.is_empty() {
        parts.push(s.model.clone());
    }
    if s.context_pct.is_some() {
        parts.push(format!("{}{}% left", guess(s), room_left(s)));
    }
    parts.extend(spend(app));
    parts.join(" · ")
}

/// `~` before a percentage of a window KodAct guessed.
pub(super) fn guess(s: &StatusView) -> &'static str {
    if s.window_guessed {
        "~"
    } else {
        ""
    }
}

/// What the session has used, in tokens (nothing before the first request).
/// The UI shows no money: `/cost` and `/usage` give the session's cost when asked.
pub(super) fn spend(app: &App) -> Option<String> {
    Some(app.status.session_tokens).filter(|n| *n > 0).map(|n| format!("{} used", tokens(n)))
}

/// `s` in at most `max` columns, its end kept: `…/demo/proj`.
pub(super) fn keep_end(s: &str, max: usize) -> String {
    if text_width(s) <= max {
        return s.to_string();
    }
    // A path loses whole folders from its start, so what stays reads as names:
    // `…/demo/proj2`, never `…f31-5eaf/demo/proj2`.
    if let Some(cut) = s.char_indices().filter(|&(_, c)| c == '/').map(|(i, _)| i).find(|&i| text_width(&s[i..]) < max)
    {
        return format!("…{}", &s[cut..]);
    }
    let mut tail: Vec<char> = vec![];
    let mut used = 1;
    for c in s.chars().rev() {
        let cw = c.width().unwrap_or(0);
        if used + cw > max {
            break;
        }
        used += cw;
        tail.push(c);
    }
    tail.reverse();
    format!("…{}", tail.into_iter().collect::<String>())
}

/// A permission mode as people say it: `default` is Manual (every edit and command is
/// asked about), `bypassPermissions` is Dangerous (nothing is).
pub(super) fn mode_name(mode: &str) -> String {
    match mode {
        "acceptEdits" => "Accept edits",
        "plan" => "Plan",
        "auto" => "Auto",
        "dontAsk" => "Don't ask",
        "bypassPermissions" => "Dangerous",
        _ => "Manual",
    }
    .to_string()
}

/// A token count as people read it: `950 tokens`, `15.5k tokens`, `1.2M tokens`.
pub(super) fn tokens(n: u64) -> String {
    match n {
        0..=999 => format!("{n} tokens"),
        1_000..=999_999 => format!("{:.1}k tokens", n as f64 / 1000.0).replace(".0k", "k"),
        _ => format!("{:.1}M tokens", n as f64 / 1_000_000.0).replace(".0M", "M"),
    }
}

/// takes over.
const FRONT_NOTES: usize = 6;

/// The input box's rows and the cursor's (column, row) inside them.
fn input(app: &App, w: usize) -> (Vec<Line<'static>>, (u16, u16)) {
    let t = app.theme;
    let text_w = w.saturating_sub(2).max(1);
    let bare = app.look == super::look::Look::Bare;
    let prompt = || {
        if bare {
            Span::styled("› ", t.subtle())
        } else {
            Span::styled(format!("{} ", glyphs::PROMPT), t.accent())
        }
    };
    if app.editor.is_empty() && !app.busy {
        let row = Line::from(vec![prompt(), Span::styled(fit(placeholder(app), text_w), t.dim())]);
        return (vec![row], (2, 0));
    }
    // While the model works, the frame style's panel says what typing does.
    if app.editor.is_empty() && app.look == super::look::Look::Frame {
        let row = Line::from(vec![prompt(), Span::styled(fit(QUEUE_PLACEHOLDER, text_w), t.dim())]);
        return (vec![row], (2, 0));
    }
    let (cur_line, cur_col) = app.editor.position();
    let mut rows = vec![];
    let mut cursor = (2u16, 0u16);
    for (li, line) in app.editor.text().split('\n').enumerate() {
        // Control characters (a pasted tab or escape) are drawn as their pictures, one
        // cell each, so the rows and the cursor are counted on what is drawn.
        let chars: Vec<char> = line.chars().map(text::shown).collect();
        let ranges = char_rows(&chars, text_w);
        for (ri, (a, b)) in ranges.iter().enumerate() {
            if li == cur_line {
                // The cursor sits in this row, or at the end of the last one.
                let last = ri + 1 == ranges.len();
                if cur_col >= *a && (cur_col < *b || last) {
                    let col: usize = chars[*a..cur_col].iter().map(|c| c.width().unwrap_or(0)).sum();
                    cursor = ((2 + col).min(w.saturating_sub(1)) as u16, rows.len() as u16);
                }
            }
            let prefix = if rows.is_empty() { prompt() } else { Span::raw("  ") };
            let text: String = chars[*a..*b].iter().collect();
            let mut spans = vec![prefix];
            if let Some(q) =
                app.search.as_ref().filter(|s| s.hit.is_some() && !s.query.is_empty()).map(|s| s.query.as_str())
            {
                spans.extend(highlight_match(
                    &text,
                    q,
                    t.accent().add_modifier(ratatui::style::Modifier::BOLD | ratatui::style::Modifier::UNDERLINED),
                ));
            } else {
                spans.push(Span::raw(text));
            }
            rows.push(Line::from(spans));
        }
    }
    (rows, cursor)
}

/// every two seconds or so.
fn shimmer(t: &text::Theme, text: &str, since: Duration) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let span = chars.len() as i64 + 8;
    let head = ((since.as_millis() / 70) as i64 % span) - 4;
    let mut out: Vec<Span<'static>> = vec![];
    for (i, c) in chars.iter().enumerate() {
        let d = (i as i64 - head).abs();
        let level = match d {
            0 => 3,
            1 => 2,
            2 => 1,
            _ => 0,
        };
        let st = t.shimmer(level);
        match out.last_mut() {
            Some(s) if s.style == st => s.content.to_mut().push(*c),
            _ => out.push(Span::styled(c.to_string(), st)),
        }
    }
    out
}

/// The room left before automatic compaction, in percent (100 when nothing is in use).
/// Measured against the compaction point when the session gave one, else the window.
pub(super) fn room_left(s: &StatusView) -> u8 {
    if s.compact_at > 0 {
        let used = s.context_tokens.min(s.compact_at);
        ((s.compact_at - used) * 100 / s.compact_at) as u8
    } else {
        100u8.saturating_sub(s.context_pct.unwrap_or(0))
    }
}

pub(super) fn gauge(t: &text::Theme, s: &StatusView) -> Vec<Span<'static>> {
    let left = room_left(s);
    let used = 100 - left;
    let shape = LEVELS[(used as usize * 8 / 100).clamp(1, 8)];
    let tone = if left <= 10 {
        t.error()
    } else if left <= 30 {
        t.warning()
    } else {
        t.accent()
    };
    let mut out = vec![Span::styled(shape, tone), Span::styled(format!(" {}{left}% left", guess(s)), t.dim())];
    // Low: what happens next, and what to do about it.
    if left <= 10 {
        out.extend([
            Span::styled(" · ", t.subtle()),
            Span::styled("/compact", t.accent()),
            Span::styled(" now", t.dim()),
        ]);
    } else if left <= 25 {
        out.push(Span::styled(" · summarizes at 0%", t.dim()));
    }
    out
}

fn status(app: &App, w: usize, now: Instant) -> Line<'static> {
    let (left, left_style) = status_left(app, now);
    let lead = 2;
    // The folder keeps its end and at most half the line, so the model and context stay in view.
    let folder = !app.status.cwd.is_empty() && left == super::short_path(&app.status.cwd);
    let left = if folder { keep_end(&left, (w / 2).max(8)) } else { fit(&left, w.saturating_sub(lead)) };
    // The folder always shows: its parents quiet, its own name brighter, then the
    // branch after a rose mark, so where you are reads at a glance.
    let left_spans = if folder {
        let t = app.theme;
        let cut = left.rfind('/').map_or(0, |i| i + 1);
        let mut out =
            vec![Span::styled(left[..cut].to_string(), t.subtle()), Span::styled(left[cut..].to_string(), t.dim())];
        if app.look == super::look::Look::Frame && !app.branch.is_empty() {
            out.push(Span::styled("  ⎇ ", t.accent()));
            out.push(Span::styled(fit(&app.branch, 24), t.subtle()));
        }
        out
    } else {
        vec![Span::styled(left, left_style)]
    };
    let left: String = left_spans.iter().map(|s| s.content.as_ref()).collect();
    let room = w.saturating_sub(text_width(&left) + 2 * lead + 2);
    // The front screen shows the model under its input; the status line keeps the version.
    let right = if app.look == super::look::Look::Frame && app.status_text.is_none() {
        // The box shows the model and the context; the line keeps the keys that help now.
        hints(app, room)
    } else if app.front && app.status_text.is_none() {
        vec![Span::styled(format!("v{}", kodact_core::VERSION), app.theme.subtle())]
    } else {
        status_right_spans(app, room)
    };
    let right_w: usize = right.iter().map(|s| text_width(&s.content)).sum();
    // As far from the right edge as the folder is from the left, under the box's text.
    let gap = w.saturating_sub(lead + text_width(&left) + right_w + lead);
    let mut spans = vec![Span::raw("  ")];
    spans.extend(left_spans);
    spans.push(Span::raw(" ".repeat(gap)));
    spans.extend(right);
    Line::from(spans)
}

/// The right side of the status line in `room` columns: with the context
/// meter when it fits, else the plain text of [`status_right`].
fn status_right_spans(app: &App, room: usize) -> Vec<Span<'static>> {
    let t = app.theme;
    let s = &app.status;
    let plain = status_right(app);
    if app.status_text.is_some() || s.context_pct.is_none() || text_width(&plain) + 2 > room {
        return if room >= 8 { vec![Span::styled(fit(&plain, room), t.dim())] } else { vec![] };
    }
    let sep = || Span::styled(" · ", t.hairline());
    let mut out = vec![];
    if !s.model.is_empty() {
        out.push(Span::styled(s.model.clone(), t.dim()));
        out.push(sep());
    }
    out.extend(gauge(&t, s));
    if let Some(cost) = spend(app) {
        out.push(sep());
        out.push(Span::styled(cost, t.dim()));
    }
    let w: usize = out.iter().map(|s| text_width(&s.content)).sum();
    if w > room {
        return vec![Span::styled(fit(&plain, room), t.dim())];
    }
    out
}

/// The whole screen for the full-screen UI, `width` by `height`. Before the
/// first message, the style's front page (`front.rs`): its header at the top,
/// then its rows with the input among them, centred down the screen (or from
/// the top, or with the input docked at the bottom), and the status line on
/// the last row. After it, the conversation: `rows` (the transcript, already
/// wrapped to `width`) fills the screen down to the live region, `scroll` rows
/// up from its end; the status style adds a bar of readouts on top. Returns
/// the view and the furthest it can scroll.
pub fn full_view(
    app: &App,
    width: u16,
    height: u16,
    rows: &[Line<'static>],
    page: &FrontPage,
    scroll: usize,
) -> (LiveView, usize) {
    full_view_at(app, width, height, rows, page, scroll, Instant::now())
}

pub fn full_view_at(
    app: &App,
    width: u16,
    height: u16,
    rows: &[Line<'static>],
    page: &FrontPage,
    scroll: usize,
    now: Instant,
) -> (LiveView, usize) {
    let h = (height as usize).max(1);
    // Whether the last frame drew the cards: the status line below is built with it, and built
    // again if this frame decides otherwise, so its "tab suggestions" hint never lags a frame.
    let cards_before = app.cards_drawn.get();
    // The front page has room for a note or two under it (a warning at start, the answer
    // to /clear). An answer longer than that takes the page's place: `?` or /status on
    // the front page showed only its last six rows, or none on a short window.
    'front: {
        if !(app.front && app.dialog.is_none() && !app.busy) || rows.len() > FRONT_NOTES {
            break 'front;
        }
        let fb = FrontBox { left: page.box_left, w: page.box_w, under: page.under };
        let live = live_view_inner(app, width, height, now, Some(fb));
        let mut input = live.lines;
        let mut status = input.pop().unwrap_or_default();
        app.cards_drawn.set(false);
        let room = h.saturating_sub(1);
        // Each row in `rows` starts with `TRANSCRIPT_INDENT` (2 spaces), which lines its
        // `❯` up with a boxed input's `│ ❯` when the box starts at column 0. On the front
        // page the box sits at `page.box_left` and is `page.box_w` columns wide; in open
        // styles (`Bare`, `Ledger`) the input has no `│ ` border, so its `›`/`❯` sits at
        // `page.box_left` directly. Strip `TRANSCRIPT_INDENT`, wrap to the text room inside
        // the box, and indent to the box's prompt column so notes neither misalign nor
        // overflow the box's right edge.
        let open = matches!(app.look, Look::Bare | Look::Ledger);
        let note_col = page.box_left + if open { 0 } else { 2 };
        let note_w = page.box_w.saturating_sub(if open { 0 } else { 4 }).max(10) as u16;
        let stripped: Vec<Line<'static>> = rows
            .iter()
            .cloned()
            .map(|mut l| {
                if let Some(first) = l.spans.first_mut() {
                    if let Some(rest) = first.content.strip_prefix("  ") {
                        first.content = rest.to_string().into();
                        if first.content.is_empty() {
                            l.spans.remove(0);
                        }
                    }
                }
                l
            })
            .collect();
        let notes: Vec<Line<'static>> = text::wrap(stripped, note_w)
            .into_iter()
            .map(|mut l| {
                if note_col > 0 && !l.spans.is_empty() {
                    l.spans.insert(0, Span::raw(" ".repeat(note_col)));
                }
                l
            })
            .collect();
        let popup = usize::from(app.search.is_some())
            + if app.search.is_some() { app.search_matches().len().min(4) } else { 0 }
            + usize::from(app.find.is_some())
            + app.menu().len().min(MENU_ROWS)
            + if app.menu().is_empty() { app.file_menu().len().min(MENU_ROWS) } else { 0 };
        let base_input_len = input.len().saturating_sub(popup);
        let mut out: Vec<Line<'static>> = page.header.clone();
        let box_at;
        if page.dock {
            // The page from the top, the input at the bottom.
            out.extend(page.above.iter().cloned());
            if !notes.is_empty() {
                if out.len() + 1 + notes.len() > room.saturating_sub(base_input_len) && !rows.is_empty() {
                    break 'front;
                }
                if out.len() + 1 + notes.len() <= room.saturating_sub(input.len()) {
                    out.push(Line::default());
                    out.extend(notes);
                }
            }
            if out.len() > room.saturating_sub(base_input_len) && !rows.is_empty() {
                break 'front;
            }
            out.truncate(room.saturating_sub(input.len()));
            out.resize(room.saturating_sub(input.len()), Line::default());
            box_at = out.len();
            out.extend(input);
        } else {
            let mut mid: Vec<Line<'static>> = vec![];
            // What a short screen keeps, in order: the input, the style's art over it, the notes
            // (a warning at start, an answer), then the rows under the input (suggestions,
            // recent work), which go first.
            let fits = page.header.len() + page.above.len() + base_input_len <= room
                && page.header.len() + page.above.len() + input.len() <= room;
            if fits {
                mid.extend(page.above.iter().cloned());
            }
            let at = mid.len();
            mid.extend(input);
            let mut base_mid_len = mid.len().saturating_sub(popup);
            let note_rows = if notes.is_empty() { 0 } else { 1 + notes.len() };
            if !notes.is_empty() && page.header.len() + base_mid_len + note_rows > room {
                break 'front;
            }
            let below_fits = page.header.len() + base_mid_len + note_rows + page.below.len() <= room;
            if below_fits {
                base_mid_len += page.below.len();
                if page.header.len() + mid.len() + note_rows + page.below.len() <= room {
                    mid.extend(page.below.iter().cloned());
                    app.cards_drawn.set(app.look == super::look::Look::Frame);
                }
            }
            if !notes.is_empty() {
                base_mid_len += note_rows;
                if page.header.len() + mid.len() + note_rows <= room {
                    mid.push(Line::default());
                    mid.extend(notes);
                }
            }
            let base_free = room.saturating_sub(page.header.len() + base_mid_len);
            let free = room.saturating_sub(page.header.len() + mid.len());
            // Centred down the screen, the empty rows split evenly above and below; or from the top.
            // A popup menu (`/`, `@`, Ctrl+R) opens into the space below the input box without
            // jumping the wordmark and box upward unless the screen is too short to hold the menu.
            let top = if page.top { free.min(1) } else { (base_free / 2).min(free) };
            out.extend(vec![Line::default(); top]);
            box_at = out.len() + at;
            out.extend(mid);
        }
        out.truncate(room);
        out.resize(room, Line::default());
        if app.cards_drawn.get() != cards_before {
            status = live_view_inner(app, width, height, now, Some(fb)).lines.pop().unwrap_or_default();
        }
        out.push(status);
        let cursor = live.cursor.map(|(x, y)| (x, y + box_at as u16)).filter(|(_, y)| (*y as usize) < room);
        return (LiveView { lines: out, cursor }, 0);
    }
    app.cards_drawn.set(false);
    let live = live_view_at(app, width, height, now);
    // The status style: a bar of the session's readouts across the top.
    let bar: Vec<Line<'static>> =
        if app.look == super::look::Look::Status { vec![readouts(app, width as usize)] } else { vec![] };
    // The conversation: the transcript down to the live region, with a blank row
    // between its last line and the input so the answer doesn't touch the box.
    // When the agent is working or the user is scrolled up, draw a crisp hairline
    // divider so chat history and the active working task stay visually distinct.
    let separate = !rows.is_empty() && (app.busy || scroll > 0);
    let gap = usize::from(separate || (!rows.is_empty() && !rows.last().is_some_and(|l| line_blank(l))));
    let avail = h.saturating_sub(live.lines.len() + bar.len() + gap);
    let max_scroll = rows.len().saturating_sub(avail);
    let scroll = scroll.min(max_scroll);
    let end = rows.len() - scroll;
    let mut out: Vec<Line<'static>> = rows[end.saturating_sub(avail)..end].to_vec();
    if let Some(q) = app.find.as_ref().filter(|f| !f.query.is_empty()).map(|f| f.query.as_str()) {
        let hi = app.theme.accent().add_modifier(ratatui::style::Modifier::BOLD | ratatui::style::Modifier::REVERSED);
        out = out
            .into_iter()
            .map(|l| {
                let spans = l
                    .spans
                    .into_iter()
                    .flat_map(|sp| {
                        let base = sp.style;
                        highlight_match(&sp.content, q, hi).into_iter().map(move |mut piece| {
                            if piece.style == Style::default() {
                                piece.style = base;
                            } else {
                                piece.style = base.patch(piece.style);
                            }
                            piece
                        })
                    })
                    .collect::<Vec<_>>();
                Line::from(spans).style(l.style)
            })
            .collect();
    }
    if scroll > 0 && !out.is_empty() {
        let t = app.theme;
        let pct = (end * 100) / rows.len().max(1);
        let word = if scroll == 1 { "line" } else { "lines" };
        let full = format!("↑ {pct}% · ↓ {scroll} more {word} · wheel / PgDn / End bottom");
        let w = width as usize;
        let spans = if text_width(&full) <= w {
            vec![
                Span::styled(format!("↑ {pct}%"), t.dim()),
                Span::styled(format!(" · ↓ {scroll} more {word} · wheel / PgDn / End bottom"), t.subtle()),
            ]
        } else {
            let compact = fit(&format!("↑ {pct}% · ↓ {scroll} · End bottom"), w);
            vec![Span::styled(compact, t.subtle())]
        };
        let note_w: usize = spans.iter().map(|s| text_width(&s.content)).sum();
        let pad = w.saturating_sub(note_w) / 2;
        let last = out.len() - 1;
        let mut line_spans = Vec::with_capacity(spans.len() + 1);
        if pad > 0 {
            line_spans.push(Span::raw(" ".repeat(pad)));
        }
        line_spans.extend(spans);
        out[last] = Line::from(line_spans);
    }
    // A short transcript sits just above the live region, as one flow with the input.
    if out.len() < avail {
        let mut padded = vec![Line::default(); avail - out.len()];
        padded.extend(out);
        out = padded;
    }
    let mut lines = bar;
    let cursor = live.cursor.map(|(x, y)| (x, y + (lines.len() + avail + gap) as u16));
    lines.extend(out);
    if gap > 0 {
        if separate {
            let t = app.theme;
            let w = width as usize;
            let divider = if app.busy {
                let tag = if app.forcing_queue { " ◆ stopping " } else { " ◆ working " };
                let left_r = "─".repeat(2.min(w));
                let right_n = w.saturating_sub(text_width(&left_r) + text_width(tag));
                Line::from(vec![
                    Span::styled(left_r, t.hairline()),
                    Span::styled(tag, t.accent()),
                    Span::styled("─".repeat(right_n), t.hairline()),
                ])
            } else {
                Line::from(Span::styled("─".repeat(w), t.hairline()))
            };
            lines.push(divider);
        } else {
            lines.push(Line::default());
        }
    }
    lines.extend(live.lines);
    (LiveView { lines, cursor }, max_scroll)
}

/// The status style's bar: the model, the context in use, the folder and the branch.
fn readouts(app: &App, w: usize) -> Line<'static> {
    let t = app.theme;
    let raised =
        if t.color { Style::default().bg(palette::PROMPT_BG.color(t.depth, t.light)) } else { Style::default() };
    let mut parts: Vec<(String, String)> =
        vec![("model".into(), fit(&format!("{} · {}", app.status.model, app.endpoint), (w / 3).max(12)))];
    if let Some(p) = app.status.context_pct {
        parts.push(("context".into(), format!("{p}%")));
    }
    parts.push(("folder".into(), keep_end(&super::short_path(&app.status.cwd), (w / 4).max(12))));
    if !app.branch.is_empty() {
        parts.push(("branch".into(), app.branch.clone()));
    }
    let mut spans = vec![Span::styled(" ", raised)];
    for (i, (k, v)) in parts.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  │  ", t.hairline().patch(raised)));
        }
        spans.push(Span::styled(format!("{k} "), t.subtle().patch(raised)));
        spans.push(Span::styled(v.clone(), Style::default().patch(raised)));
    }
    let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
    if used < w {
        spans.push(Span::styled(" ".repeat(w - used), raised));
    }
    // One row, never wrapped: cut at the width.
    let mut out = vec![];
    let mut room = w;
    for sp in spans {
        if room == 0 {
            break;
        }
        let text = if text_width(&sp.content) > room { fit(&sp.content, room) } else { sp.content.to_string() };
        room -= text_width(&text).min(room);
        out.push(Span::styled(text, sp.style));
    }
    Line::from(out)
}

/// Where the front page puts the input: its first column, its width, and
/// whether the mode, model and endpoint go on a line under it.
#[derive(Debug, Clone, Copy)]
pub struct FrontBox {
    pub left: usize,
    pub w: usize,
    pub under: bool,
}

/// Rows of a streaming thought shown under the prompt.
const THOUGHT_ROWS: usize = 3;

/// The live region for a terminal `width` columns wide, at most `max_height` rows.
pub fn live_view(app: &App, width: u16, max_height: u16) -> LiveView {
    live_view_at(app, width, max_height, Instant::now())
}

pub fn live_view_at(app: &App, width: u16, max_height: u16, now: Instant) -> LiveView {
    live_view_inner(app, width, max_height, now, None)
}

fn live_view_inner(app: &App, width: u16, max_height: u16, now: Instant, front_box: Option<FrontBox>) -> LiveView {
    let w = (width as usize).max(4);
    let t = app.theme;
    // The width of the rows that join the transcript (set in by its indent on the full screen).
    let tw = w.saturating_sub(app.transcript_indent).max(4);
    // 1. The unfinished answer line: its last rows.
    let mut live: Vec<Line<'static>> = app.live_line().map(|l| text::wrap(vec![l], tw as u16)).unwrap_or_default();
    if live.len() > LIVE_ROWS {
        live.drain(..live.len() - LIVE_ROWS);
    }
    // The model's reasoning while it streams: its last rows, quiet and in italics, under
    // a `∴` so the wait reads as thought, not a stall. It folds into the transcript when
    // the answer or a step begins.
    if live.is_empty() && !app.thought.trim().is_empty() {
        let words = t.subtle().add_modifier(ratatui::style::Modifier::ITALIC);
        let text = app.thought.split_whitespace().collect::<Vec<_>>().join(" ");
        let rows =
            text::wrap(vec![Line::from(vec![Span::styled("∴ ", t.subtle()), Span::styled(text, words)])], tw as u16);
        let keep = rows.len().saturating_sub(THOUGHT_ROWS);
        live.extend(rows.into_iter().skip(keep));
    }
    // 2. Tool calls still running, each with its dot turning; then the spinner.
    let mut spinner = vec![];
    for r in &app.running {
        let ran = now.duration_since(r.since);
        // Asked about and not yet allowed: the dialog shows the command, so no row here.
        if app.asking_for() == Some(r.id.as_str()) {
            continue;
        }
        let mut spans = if is_agent(&r.name) {
            // A sub-agent: a wave of dots and the word for it, so
            // its state never rests on the motion or the colour alone.
            let mut v = scanner(&t, ran);
            v.push(Span::styled("running  ", t.dim()));
            v
        } else {
            let face = glyphs::RUNNING[(ran.as_millis() / RUNNING_STEP.as_millis()) as usize % 4];
            vec![Span::styled(format!("{face} "), t.accent())]
        };
        // A sub-agent's row says what it explores; the line below says they're agents.
        let skip = usize::from(is_agent(&r.name) && r.header.len() > 1);
        spans.extend(r.header.iter().skip(skip * 2).cloned());
        // A step keeps its own time, from when it started, not the turn's.
        spans.push(Span::styled(format!("  {}s", ran.as_secs()), t.subtle()));
        if is_agent(&r.name) && r.steps > 0 {
            let n = if r.steps == 1 { "1 step".to_string() } else { format!("{} steps", r.steps) };
            spans.push(Span::styled(format!(" · {n}"), t.subtle()));
        }
        spinner.push(Line::default());
        spinner.extend(text::wrap(vec![Line::from(spans)], tw as u16).into_iter().take(2));
        // A shell command runs in its box: the command, and a cursor until it ends.
        if let Some(cmd) = super::app::shell_box(app.look, &r.name, &r.input) {
            spinner.extend(super::shellbox::running(&t, &cmd, tw));
        }
        // Its latest steps in a small tree under it, so delegated work stays visible.
        if is_agent(&r.name) && !r.trail.is_empty() {
            let tree = super::app::agent_tree(&t, &r.trail, true);
            spinner.extend(tree.into_iter().map(|l| text::wrap(vec![l], tw as u16).swap_remove(0)));
        }
    }
    // A permission question shows its own waiting row: no second line saying so.
    let asked_row = app.asking_for().is_some_and(|id| app.running.iter().any(|r| r.id == id));
    // A step on screen already says what runs and for how long: no second line naming it.
    let step_shown = app.running.iter().any(|r| !is_agent(&r.name) && app.asking_for() != Some(r.id.as_str()))
        && !app.running.iter().any(|r| is_agent(&r.name));
    if app.busy && !asked_row && !(step_shown && !app.waiting_for_answer()) {
        let since = app.busy_since.map(|s| now.duration_since(s)).unwrap_or_default();
        let frame = SPINNER[(since.as_millis() / SPINNER_STEP.as_millis()) as usize % SPINNER.len()];
        let mut line = vec![Span::styled(format!("{frame} "), t.accent())];
        if app.waiting_for_answer() {
            // Needs you: the brand colour, and how long you have been asked, not the turn's time.
            let waited = app.asked_at.map(|a| now.saturating_duration_since(a)).unwrap_or_default().as_secs();
            line[0] = Span::styled("◆ ", t.accent());
            line.push(Span::styled("Waiting for your answer", Style::default()));
            line.push(Span::styled(
                fit(&format!(" ({waited}s · ↑↓ to choose, Enter to answer)"), w.saturating_sub(25)),
                t.subtle(),
            ));
        } else {
            // What it is doing, in words: waiting on its agents, running a step, writing,
            // thinking; the default style keeps the esc key on the status line only.
            let agents = app.running.iter().filter(|r| is_agent(&r.name)).count();
            let verb = if agents > 0 {
                format!("Waiting for {agents} {}", if agents == 1 { "agent" } else { "agents" })
            } else {
                match app.activity.as_str() {
                    // The plan's task in progress, as the reference CLI's spinner says it.
                    "" | "Working" if app.todo_active.is_some() => app.todo_active.clone().unwrap_or_default(),
                    // "Thinking" only while reasoning arrives; any other wait is work.
                    "" | "Working" if app.look == super::look::Look::Frame => "Working".to_string(),
                    "" | "Working" => VERBS[(since.as_secs() / VERB_FOR.as_secs()) as usize % VERBS.len()].to_string(),
                    a => a.to_string(),
                }
            };
            line.extend(shimmer(&t, &format!("{verb}…"), since));
            let done = if agents > 0 {
                format!("{} of {} done · ", app.agents_done, agents + app.agents_done)
            } else {
                String::new()
            };
            let rest = if app.look == super::look::Look::Frame {
                format!(" ({done}{}s)", since.as_secs())
            } else {
                format!(" ({done}{}s · esc to interrupt)", since.as_secs())
            };
            let room = w.saturating_sub(2 + text_width(&verb) + 1);
            line.push(Span::styled(fit(&rest, room), t.subtle()));
        }
        spinner.push(Line::default());
        spinner.push(Line::from(line));
    }
    // 3. A dialog.
    let room = (max_height as usize).max(1).saturating_sub(spinner.len() + 1);
    let dialog = if app.dialog.is_some() { dialog(app, w, room) } else { vec![] };
    // 4. Queued messages.
    let mut queued: Vec<Line<'static>> = app
        .queued
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let (tag, style) =
                if i == 0 && app.forcing_queue { ("forcing ▸ ", t.accent()) } else { ("queued ▸ ", t.warning()) };
            Line::from(vec![
                Span::styled(tag, style),
                Span::styled(fit(&q.replace('\n', " "), tw.saturating_sub(tag.len())), t.dim()),
            ])
        })
        .collect();
    // 5. The input box (hidden while a dialog is open).
    // The input sits in a rounded box; its rows are padded to the box's inside. On the
    // front screen the box is centred under the wordmark, at most 80 columns, with the
    // mode, the model and where it runs under it.
    let fb = front_box.or_else(|| {
        (app.front && w >= 60).then(|| {
            let bw = (w - 4).min(80);
            FrontBox { left: (w - bw) / 2, w: bw, under: true }
        })
    });
    let fb = fb.filter(|_| app.front && app.dialog.is_none());
    let front = fb.is_some();
    let (left, bw) = fb.map(|b| (b.left.min(w.saturating_sub(4)), b.w.min(w))).unwrap_or((0, w));
    let margin = " ".repeat(left);
    use super::look::Look;
    // Bare and Ledger draw the input under a rule, without a box; Glow's box glows rose.
    let open = matches!(app.look, Look::Bare | Look::Ledger);
    let border = match app.look {
        Look::Glow | Look::Frame => t.accent(),
        _ if app.editor.is_empty() => t.hairline(),
        _ => t.subtle(),
    };
    let side = if open { 0 } else { 2 };
    let inner = bw.saturating_sub(2 * side).max(1);
    let edge = |l: char, r: char| {
        if open {
            Line::from(vec![Span::raw(margin.clone()), Span::styled("─".repeat(bw), border)])
        } else {
            Line::from(vec![
                Span::raw(margin.clone()),
                Span::styled(format!("{l}{}{r}", "─".repeat(bw.saturating_sub(2))), border),
            ])
        }
    };
    let (top, bottom) = (edge('╭', '╮'), edge('╰', '╯'));
    let (mut input_rows, mut cursor) = if app.dialog.is_none() { input(app, inner) } else { (vec![], (0, 0)) };
    if app.dialog.is_none() && !app.pending_images.is_empty() {
        let mut pills: Vec<Span<'static>> = vec![Span::raw("  ")];
        for (i, img) in app.pending_images.iter().enumerate() {
            if i > 0 {
                pills.push(Span::raw(" "));
            }
            pills.push(Span::styled(format!("[{}]", img.pill_label(i + 1)), t.teal()));
        }
        pills.push(Span::styled("  (Backspace removes)", t.dim()));
        input_rows.insert(0, Line::from(pills));
        cursor.1 = cursor.1.saturating_add(1);
    }
    let input_rows: Vec<Line<'static>> = input_rows
        .into_iter()
        .map(|row| {
            let used: usize = row.spans.iter().map(|s| text_width(&s.content)).sum();
            let mut spans = vec![Span::raw(margin.clone())];
            if !open {
                spans.push(Span::styled("│ ", border));
            }
            spans.extend(row.spans);
            if !open {
                spans.push(Span::raw(" ".repeat(inner.saturating_sub(used))));
                spans.push(Span::styled(" │", border));
            }
            Line::from(spans)
        })
        .collect();
    let cursor = (cursor.0 + side as u16 + margin.len() as u16, cursor.1);
    // The default style keeps the session in the box, under the text: a row of air, then
    // the mode, the model and where it runs on the left, the context on the right.
    let frame = app.look == Look::Frame && !open;
    let mut input_rows = input_rows;
    if frame && !input_rows.is_empty() {
        let boxed = |content: Vec<Span<'static>>| {
            let used: usize = content.iter().map(|s| text_width(&s.content)).sum();
            let mut spans = vec![Span::raw(margin.clone()), Span::styled("│ ", border)];
            spans.extend(content);
            spans.push(Span::raw(" ".repeat(inner.saturating_sub(used))));
            spans.push(Span::styled(" │", border));
            Line::from(spans)
        };
        input_rows.push(boxed(vec![]));
        input_rows.push(boxed(session_row(app, inner)));
    }
    let under: Vec<Line<'static>> = if !frame && front && fb.is_some_and(|b| b.under) && !input_rows.is_empty() {
        let mut parts = vec![Span::styled(mode_name(&app.status.mode), t.accent())];
        for p in [app.status.model.as_str(), app.endpoint.as_str()].into_iter().filter(|p| !p.is_empty()) {
            parts.push(Span::styled("  ·  ", t.hairline()));
            parts.push(Span::styled(p.to_string(), t.subtle()));
        }
        let used: usize = parts.iter().map(|s| text_width(&s.content)).sum();
        if used + 4 <= w {
            let mut spans = vec![Span::raw(" ".repeat((w - used) / 2))];
            spans.extend(parts);
            vec![Line::from(spans)]
        } else {
            vec![]
        }
    } else {
        vec![]
    };
    let lead = if front && open { "" } else { "  " };
    // Ctrl+R: history search panel (header + up to 4 matching rows) or Ctrl+F: discussion find bar.
    let mut search: Vec<Line<'static>> = Vec::new();
    if let Some(s) = app.search.as_ref() {
        let matches = app.search_matches();
        let total = matches.len();
        let pos = s.hit.and_then(|h| matches.iter().position(|(idx, _)| *idx == h));
        let pad = if left > 0 { Some(Span::raw(" ".repeat(left))) } else { None };
        let mut head = Vec::new();
        if let Some(p) = pad.clone() {
            head.push(p);
        }
        head.push(Span::styled(format!("{lead}history: "), t.accent()));
        head.push(Span::styled(format!("{}▏", s.query), t.bold()));
        if total == 0 && !s.query.is_empty() {
            head.push(Span::styled("  (no match)", t.error()));
        } else if total > 0 {
            let count = match pos {
                Some(i) => format!("  {}/{total}", i + 1),
                None => format!("  {total} matches"),
            };
            head.push(Span::styled(count, t.dim()));
        }
        head.push(Span::styled("  · Ctrl+R / ↑↓ · Enter · Esc", t.dim()));
        search.push(Line::from(head));

        let shown_rows = 4usize;
        let active = pos.unwrap_or(0);
        let first = active.saturating_sub(shown_rows / 2).min(total.saturating_sub(shown_rows));
        for &(idx, entry) in matches.iter().skip(first).take(shown_rows) {
            let selected = s.hit == Some(idx);
            let mark = if selected { "❯ " } else { "  " };
            let one_line = entry.lines().next().unwrap_or("");
            let avail = bw.saturating_sub(lead.len() + 2).max(8);
            let snippet = fit(one_line, avail);
            let mut row_spans = Vec::new();
            if let Some(p) = pad.clone() {
                row_spans.push(p);
            }
            row_spans.push(Span::styled(format!("{lead}{mark}"), if selected { t.accent() } else { t.dim() }));
            let base_style = if selected { t.selected() } else { t.dim() };
            if s.query.is_empty() {
                row_spans.push(Span::styled(snippet, base_style));
            } else {
                let hi = t.accent().add_modifier(ratatui::style::Modifier::BOLD | ratatui::style::Modifier::UNDERLINED);
                for mut sp in highlight_match(&snippet, &s.query, hi) {
                    if sp.style == Style::default() {
                        sp.style = base_style;
                    }
                    row_spans.push(sp);
                }
            }
            search.push(Line::from(row_spans));
        }
    } else if let Some(f) = app.find.as_ref() {
        let mut spans = Vec::new();
        if left > 0 {
            spans.push(Span::raw(" ".repeat(left)));
        }
        spans.push(Span::styled(format!("{lead}find: "), t.accent()));
        spans.push(Span::styled(format!("{}▏", f.query), t.bold()));
        if !f.query.is_empty() {
            if f.total == 0 {
                spans.push(Span::styled("  (no matches)", t.error()));
            } else {
                spans.push(Span::styled(format!("  {}/{}", f.current + 1, f.total), t.dim()));
            }
        }
        spans.push(Span::styled("  · Enter / ↑↓ next · Esc close", t.dim()));
        search.push(Line::from(spans));
    }
    // 6. The `/` or `@` menu, 7. the status line.
    let menu = menu(app, left, bw, lead);
    let status = status(app, w, now);

    // The spinner, running steps and queued messages keep a blank row above the input,
    // as the transcript does, so they never touch the box.
    let gap = usize::from(dialog.is_empty() && (!spinner.is_empty() || !queued.is_empty()));
    let fixed = spinner.len()
        + gap
        + search.len()
        + dialog.len()
        + if input_rows.is_empty() { 0 } else { input_rows.len() + if open { 1 } else { 2 } }
        + under.len()
        + menu.len()
        + 1;
    let max = (max_height as usize).max(1);
    let spare = max.saturating_sub(fixed);
    // Too tall: drop the answer's rows first, then queued messages.
    while live.len() + queued.len() > spare {
        if !live.is_empty() {
            live.remove(0);
        } else {
            queued.remove(0);
        }
    }

    // On the full screen the transcript is set in by its indent: what is still live joins it there.
    let set_in = |v: Vec<Line<'static>>| -> Vec<Line<'static>> {
        if app.transcript_indent == 0 {
            return v;
        }
        v.into_iter()
            .map(|l| {
                if l.spans.is_empty() {
                    return l;
                }
                let mut spans = vec![Span::raw(" ".repeat(app.transcript_indent))];
                spans.extend(l.spans);
                Line::from(spans)
            })
            .collect()
    };
    let mut lines = set_in(live);
    lines.extend(set_in(spinner));
    lines.extend(dialog);
    lines.extend(set_in(queued));
    lines.extend(search);
    lines.extend(std::iter::repeat_n(Line::default(), gap));
    let mut cur = None;
    if !input_rows.is_empty() {
        lines.push(top);
        cur = Some((cursor.0, (lines.len() as u16) + cursor.1));
        lines.extend(input_rows);
        if !open {
            lines.push(bottom);
        }
        lines.extend(under);
    }
    lines.extend(menu);
    lines.push(status);
    // The input itself can be taller than the screen: keep the rows around the cursor.
    if lines.len() > max {
        let cut = lines.len() - max;
        let keep_from = cur.map(|(_, y)| (y as usize).min(cut)).unwrap_or(cut);
        lines.drain(..keep_from);
        cur = cur.map(|(x, y)| (x, y - keep_from as u16));
        lines.truncate(max);
        cur = cur.filter(|(_, y)| (*y as usize) < lines.len());
    }
    LiveView { lines, cursor: cur }
}

/// A row with nothing but spaces.
fn line_blank(l: &Line) -> bool {
    l.spans.iter().all(|s| s.content.trim().is_empty())
}

/// The tools that run a sub-agent.
pub(super) fn is_agent(name: &str) -> bool {
    matches!(name, "Task" | "Agent")
}

/// The time the sub-agent scanner's head takes per cell.
const SCAN_STEP: Duration = Duration::from_millis(90);

/// Cells in the sub-agent scanner: short, so it marks the row without crowding it.
const SCAN_W: usize = 5;

/// The scanner's colours, from the head back along its trail, then the unlit
/// cells: KodAct's turquoise fading toward the page, in either theme.
const SCAN_SHADES: [palette::Role; 5] =
    [palette::TEAL, palette::TEAL_TRAIL[0], palette::TEAL_TRAIL[1], palette::TEAL_TRAIL[2], palette::HAIRLINE];

/// Where the scanner's head is at `frame`, sweeping one cell a step to the end and
/// back, and which way it is going (true: right).
fn scan_head(frame: usize) -> (usize, bool) {
    let period = 2 * (SCAN_W - 1);
    let i = frame % period;
    if i < SCAN_W - 1 {
        (i, true)
    } else {
        (period - i, false)
    }
}

/// A sub-agent's working sign: a row of small blocks with a bright turquoise head
/// sweeping back and forth and a trail fading behind it (`■■■■■`), then a space.
/// One text row, five cells wide. Without colour the head and trail are drawn
/// in shapes (`■`, `▪`) over dots, so the motion still shows.
fn scanner(t: &text::Theme, ran: Duration) -> Vec<Span<'static>> {
    let (head, right) = scan_head((ran.as_millis() / SCAN_STEP.as_millis()) as usize);
    let mut out = vec![];
    for x in 0..SCAN_W {
        // How far behind the head this cell is (0 is the head), on the side it came from.
        let behind = if right { head.checked_sub(x) } else { x.checked_sub(head) };
        let level = behind.map(|b| b.min(4)).unwrap_or(4);
        let (sym, style) = if t.color {
            ("■", Style::default().fg(SCAN_SHADES[level].color(t.depth, t.light)))
        } else {
            (["■", "▪", "▪", "·", "·"][level], if level == 0 { t.bold() } else { Style::default() })
        };
        out.push(Span::styled(sym, style));
    }
    out.push(Span::raw(" "));
    out
}

/// The session row inside the default style's box, `room` columns: the mode in
/// its colour, the model and where it runs on the left; the context meter and
/// the spend on the right. The endpoint goes first when space runs out, then the
/// model is shortened, then the meter goes; the mode always stays.
fn session_row(app: &App, room: usize) -> Vec<Span<'static>> {
    let t = app.theme;
    let s = &app.status;
    let w_of = |v: &[Span<'static>]| v.iter().map(|s| text_width(&s.content)).sum::<usize>();
    let sep = || Span::styled("  ·  ", t.subtle());
    // The mode as a filled pill: white on burgundy for Manual, the others in their own colour.
    let mode = if app.danger_armed {
        vec![t.pill("bypassPermissions", "Dangerous (y/n)")]
    } else {
        vec![t.pill(&s.mode, &mode_name(&s.mode))]
    };
    let mut right = vec![];
    if s.context_pct.is_some_and(|p| p > 0 || s.context_used) {
        right.extend(gauge(&t, s));
    }
    if let Some(cost) = spend(app).filter(|_| !right.is_empty()) {
        right.push(Span::styled(format!("  ·  {cost}"), t.subtle()));
    }
    let endpoint = (!app.endpoint.is_empty()).then(|| vec![sep(), Span::styled(app.endpoint.clone(), t.subtle())]);
    let fits = |left: usize, right: &[Span<'static>]| left + 2 + w_of(right) <= room;
    let mut left = mode.clone();
    let model_w = if s.model.is_empty() { 0 } else { 5 + text_width(&s.model) };
    let endpoint_w = endpoint.as_ref().map(|e| w_of(e)).unwrap_or(0);
    if !fits(w_of(&mode) + model_w + endpoint_w, &right) && !fits(w_of(&mode) + model_w.min(12), &right) {
        right.clear();
    }
    let free = room.saturating_sub(w_of(&mode) + w_of(&right) + if right.is_empty() { 0 } else { 2 });
    if !s.model.is_empty() && free >= 9 {
        let model = fit(&s.model, free - 5);
        let model_span = if app.model_down {
            Span::styled(format!("✗ {model}"), t.error())
        } else {
            Span::styled(model.clone(), Style::default())
        };
        left.extend([sep(), model_span]);
        let endpoint_shown =
            endpoint.is_none() || endpoint.as_ref().is_some_and(|e| 5 + text_width(&model) + w_of(e) <= free);
        if let Some(e) = endpoint.filter(|e| 5 + text_width(&model) + w_of(e) <= free) {
            left.extend(e);
        }
        if endpoint_shown && !app.danger_armed {
            let parsed = kodact_permissions::PermissionMode::parse(&s.mode).unwrap_or_default();
            let rule_spans = vec![sep(), Span::styled(parsed.short_rule(), t.subtle())];
            let used = w_of(&left) + w_of(&rule_spans) + w_of(&right) + if right.is_empty() { 0 } else { 4 };
            if used <= room {
                left.extend(rule_spans);
            }
        }
    }
    let gap = room.saturating_sub(w_of(&left) + w_of(&right));
    left.push(Span::raw(" ".repeat(gap)));
    left.extend(right);
    left
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::{CommandInfo, StatusView, UiEvent};
    use crate::tui::text::Theme;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use kodact_engine::PermissionPrompt;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::Paragraph;
    use ratatui::Terminal;
    use serde_json::json;

    #[test]
    fn unknown_cost_is_not_shown_as_zero() {
        let mut a = app(false);
        a.on_event(UiEvent::Status(StatusView {
            model: "local".into(),
            mode: "default".into(),
            cwd: "/p".into(),
            cost: 0.0,
            cost_unknown: true,
            context_pct: Some(3),
            context_used: true,
            window: 200_000,
            ..Default::default()
        }));
        assert_eq!(status_right(&a), "local · 97% left", "no price and nothing used yet: nothing to show");
        a.status.context_pct = Some(0);
        assert_eq!(status_right(&a), "local · 100% left");
        a.status.context_used = false;
        assert_eq!(status_right(&a), "local · 100% left");
        // A model with no price shows the tokens it used.
        assert_eq!(tokens(950), "950 tokens");
        assert_eq!(tokens(15_500), "15.5k tokens");
        assert_eq!(tokens(2_000), "2k tokens");
        assert_eq!(tokens(1_200_000), "1.2M tokens");
    }

    fn app(color: bool) -> App {
        let mut a = App::new(Theme { color, light: false, accent: None, depth: Default::default() }, vec![]);
        // The rounded boxes these tests were written for.
        a.look = crate::tui::look::Look::Card;
        a.on_event(UiEvent::Status(StatusView {
            model: "opus".into(),
            mode: "default".into(),
            cwd: "/p".into(),
            cost: 0.25,
            cost_unknown: false,
            context_pct: Some(12),
            context_used: true,
            window: 200_000,
            ..Default::default()
        }));
        a.on_event(UiEvent::Commands(vec![
            CommandInfo { name: "clear".into(), args: "[name]".into(), description: "Start a new conversation".into() },
            CommandInfo { name: "compact".into(), args: "[instructions]".into(), description: "Summarize".into() },
            CommandInfo { name: "status".into(), args: String::new(), description: "Show status".into() },
        ]));
        a
    }

    /// Draw the view into a test terminal; return its rows (trailing spaces cut).
    fn draw(view: &LiveView, w: u16) -> (Vec<String>, ratatui::buffer::Buffer) {
        let h = view.lines.len() as u16;
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| f.render_widget(Paragraph::new(view.lines.clone()), f.area())).unwrap();
        let buf = term.backend().buffer().clone();
        let rows = (0..h)
            .map(|y| (0..w).map(|x| buf[(x, y)].symbol().to_string()).collect::<String>().trim_end().to_string())
            .collect();
        (rows, buf)
    }

    fn typed(a: &mut App, s: &str) {
        for c in s.chars() {
            a.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
    }

    #[test]
    fn idle_shows_the_placeholder_and_status() {
        let a = app(true);
        let v = live_view(&a, 60, 20);
        let (rows, buf) = draw(&v, 60);
        assert_eq!(rows[0], format!("╭{}╮", "─".repeat(58)));
        assert_eq!(rows[1], format!("│ ❯ {PLACEHOLDER:<54} │"));
        assert_eq!(rows[2], format!("╰{}╯", "─".repeat(58)));
        assert_eq!(rows[3], "  /p                                     opus · ⡀ 88% left");
        assert_eq!(v.cursor, Some((4, 1)));
        // The placeholder is quiet but readable (muted, over 4.5:1); the prompt marker is KodAct's rose.
        assert_eq!(buf[(6, 1)].fg, crate::tui::palette::MUTED.color(Default::default(), false));
        assert_eq!(buf[(2, 1)].fg, crate::tui::palette::BRAND.color(Default::default(), false));
    }

    #[test]
    fn busy_shows_the_answer_line_spinner_and_queue() {
        let mut a = app(true);
        typed(&mut a, "hello");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let start = a.busy_since.unwrap();
        a.live = "partial **answer**".into();
        typed(&mut a, "next");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        typed(&mut a, "ab");
        let v = live_view_at(&a, 50, 20, start + Duration::from_millis(1300));
        let (rows, _) = draw(&v, 50);
        assert_eq!(rows[0], "• partial answer");
        assert_eq!(rows[1], "");
        assert_eq!(rows[2], "⠦ Acting… (1s · esc to interrupt)", "1.3 s is step 16: frame 7 of 10");
        assert_eq!(rows[3], "queued ▸ next", "a queued message says so");
        assert_eq!(rows[4], "", "a blank row above the input");
        assert!(rows[6].starts_with("│ ❯ ab ") && rows[6].ends_with('│'), "{}", rows[6]);
        assert_eq!(v.cursor, Some((6, 6)));
        // Too short a screen drops the answer line first, then queued messages, never the input.
        let v = live_view_at(&a, 50, 6, start);
        let (rows, _) = draw(&v, 50);
        assert!(rows.iter().all(|r| !r.contains("partial")), "{rows:?}");
        assert!(rows.iter().any(|r| r.starts_with("│ ❯ ab")), "{rows:?}");
        assert_eq!(v.lines.len(), 6);
    }

    #[test]
    fn the_menu_lists_matching_commands() {
        let mut a = app(true);
        typed(&mut a, "/c");
        a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        let v = live_view(&a, 60, 20);
        let (rows, buf) = draw(&v, 60);
        assert!(rows[1].starts_with("│ ❯ /c "), "{}", rows[1]);
        assert_eq!(rows[3], "  /clear [name]            Start a new conversation");
        assert_eq!(rows[4], "  /compact [instructions]  Summarize");
        // The selected row is highlighted.
        assert!(buf[(3, 4)].modifier.contains(ratatui::style::Modifier::BOLD));
        assert!(!buf[(3, 3)].modifier.contains(ratatui::style::Modifier::BOLD));
    }

    fn ask(a: &mut App, tool: &str, input: serde_json::Value) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::mem::forget(rx);
        a.on_event(UiEvent::Ask {
            prompt: PermissionPrompt {
                tool_name: tool.into(),
                tool_use_id: "t".into(),
                input,
                reason: String::new(),
                suggestions: vec![],
                blocked_path: None,
            },
            reply: tx,
        });
    }

    #[test]
    fn a_thought_shows_while_it_streams_then_folds_into_the_transcript() {
        use kodact_types::{Delta, StreamEvent};
        let mut a = app(false);
        a.look = crate::tui::look::Look::Frame;
        typed(&mut a, "why");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        a.take_pending();
        let ev = |event| UiEvent::Engine(kodact_engine::EngineEvent::Stream { event, parent_tool_use_id: None });
        a.on_event(ev(StreamEvent::ContentBlockDelta {
            index: 0,
            delta: Delta::ThinkingDelta { thinking: "Rounding per line drifts.".into() },
        }));
        let (rows, _) = draw(&live_view(&a, 80, 20), 80);
        let text = rows.join("\n");
        assert!(text.contains("∴ Rounding per line drifts.") && text.contains("Thinking…"), "{text}");
        a.on_event(ev(StreamEvent::ContentBlockStop { index: 0 }));
        let pending = a.take_pending();
        let collapsed: Vec<String> = crate::tui::app::lay_out(pending.clone(), 80)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(
            collapsed
                .iter()
                .any(|l| l.contains("∴ Thought for") && l.contains("1 line") && l.contains("ctrl+o to show")),
            "{collapsed:?}"
        );
        assert!(!collapsed.iter().any(|l| l.contains("Rounding per line drifts.")), "{collapsed:?}");

        a.on_key(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert!(a.verbose && a.layout_version > 0);
        let expanded: Vec<String> = crate::tui::app::lay_out_mode(pending, 80, a.verbose)
            .iter()
            .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect())
            .collect();
        assert!(expanded.iter().any(|l| l.contains("∴ Thought for") && l.contains("ctrl+o to hide")), "{expanded:?}");
        assert!(expanded.iter().any(|l| l.contains("Rounding per line drifts.")), "{expanded:?}");
        let (rows, _) = draw(&live_view(&a, 80, 20), 80);
        let text = rows.join("\n");
        assert!(!text.contains("Rounding") && text.contains("Working…"), "a wait after the thought is work: {text}");
    }

    #[test]
    fn a_running_step_shows_once_with_its_own_time() {
        let mut a = app(false);
        a.look = crate::tui::look::Look::Frame;
        typed(&mut a, "test");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let start = a.busy_since.unwrap();
        a.running.push(crate::tui::app::Running {
            id: "b1".into(),
            header: vec![Span::raw("Run the cart tests")],
            name: "Bash".into(),
            input: serde_json::json!({"command": "pytest"}),
            since: start + Duration::from_secs(30),
            step: None,
            steps: 0,
            trail: vec![],
        });
        let (rows, _) = draw(&live_view_at(&a, 80, 20, start + Duration::from_secs(37)), 80);
        let text = rows.join("\n");
        assert_eq!(text.matches("Run the cart tests").count(), 1, "{text}");
        assert!(text.contains("Run the cart tests  7s"), "the step's own time: {text}");
    }

    /// The spinner says the plan's task in progress, as the reference CLI's does: its activeForm,
    /// or its content when it has none; with no task in progress, the usual word.
    #[test]
    fn the_spinner_says_the_task_in_progress() {
        use kodact_types::{ApiMessage, ContentBlock, Role, StopReason, Usage};
        let mut a = app(false);
        a.look = crate::tui::look::Look::Frame;
        typed(&mut a, "fix the signup form");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let start = a.busy_since.unwrap();
        let plan = |todos: serde_json::Value| {
            let call = ContentBlock::ToolUse {
                id: "t1".into(),
                name: "TodoWrite".into(),
                input: serde_json::json!({ "todos": todos }),
                cache_control: None,
                provider: None,
            };
            UiEvent::Engine(kodact_engine::EngineEvent::Assistant {
                message: ApiMessage {
                    id: "m".into(),
                    kind: "message".into(),
                    role: Role::Assistant,
                    model: "m".into(),
                    content: vec![call],
                    stop_reason: Some(StopReason::ToolUse),
                    stop_sequence: None,
                    usage: Usage::default(),
                },
                uuid: "u".into(),
                parent_tool_use_id: None,
            })
        };
        // The call answered: the spinner shows again, as it does between steps.
        let answered = || {
            UiEvent::Engine(kodact_engine::EngineEvent::User {
                message: kodact_types::Message::user(vec![ContentBlock::tool_result("t1".to_string(), "ok", false)]),
                uuid: "r".into(),
                tool_use_result: None,
                is_meta: false,
                parent_tool_use_id: None,
            })
        };
        let spinner = |a: &App| draw(&live_view_at(a, 80, 20, start + Duration::from_secs(2)), 80).0.join("\n");
        a.on_event(plan(serde_json::json!([
            {"content": "Read the form", "status": "completed", "activeForm": "Reading the form"},
            {"content": "Run the tests", "status": "in_progress", "activeForm": "Running the tests"},
        ])));
        a.on_event(answered());
        assert!(spinner(&a).contains("Running the tests… (2s)"), "{}", spinner(&a));
        a.on_event(plan(
            serde_json::json!([{"content": "Write the check", "status": "in_progress", "activeForm": ""}]),
        ));
        a.on_event(answered());
        assert!(spinner(&a).contains("Write the check… (2s)"), "its content without an activeForm");
        a.on_event(plan(serde_json::json!([{"content": "Done", "status": "completed", "activeForm": "Doing"}])));
        a.on_event(answered());
        assert!(spinner(&a).contains("Working… (2s)"), "none in progress: {}", spinner(&a));
    }

    #[test]
    fn the_default_style_says_what_it_does_once() {
        let mut a = app(false);
        a.look = crate::tui::look::Look::Frame;
        typed(&mut a, "hello");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let start = a.busy_since.unwrap();
        let (rows, _) = draw(&live_view_at(&a, 80, 20, start + Duration::from_millis(1300)), 80);
        let text = rows.join("\n");
        assert!(text.contains("Working… (1s)"), "a word for what it does, no key: {text}");
        assert!(text.contains("Type to queue a message") && !text.contains("esc stops"), "{text}");
        assert_eq!(text.matches("esc").count(), 1, "the esc key once, on the status line: {text}");
        // Plan mode: the status line says how to leave it.
        let mut b = app(false);
        b.look = crate::tui::look::Look::Frame;
        b.status.mode = "plan".into();
        let (rows, _) = draw(&live_view(&b, 80, 12), 80);
        let last = rows.last().unwrap().split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(last.contains("shift+tab leave plan mode"), "{rows:#?}");
        // Paths from the session's folder; the home folder as ~.
        use crate::tui::app::relative_spans;
        let home = std::env::var("HOME").unwrap_or_default();
        let spans = vec![Span::raw("/p/src/calc.py"), Span::raw(format!("{home}/notes.md"))];
        let out: Vec<String> = relative_spans(spans, "/p").iter().map(|s| s.content.to_string()).collect();
        assert_eq!(out[0], "src/calc.py");
        if home.len() > 1 {
            assert_eq!(out[1], "~/notes.md");
        }
    }

    #[test]
    fn the_session_says_what_happened_and_what_next() {
        use kodact_engine::{EngineEvent, NoticeLevel};
        let mut a = app(false);
        a.look = crate::tui::look::Look::Frame;
        a.status.model = "quick-thinking".into();
        // Before anything is sent: the welcome. After: a reply.
        assert!(draw(&live_view(&a, 80, 12), 80).0.join("\n").contains(PLACEHOLDER));
        typed(&mut a, "hi");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        // The model isn't there: marked in the box, and the input says what to do.
        a.on_event(UiEvent::Engine(EngineEvent::Notice {
            level: NoticeLevel::Error,
            text: r#"API Error: HTTP 400 from https://gw.example.com/v1: {"detail":"Model not found"}"#.into(),
        }));
        a.on_event(UiEvent::Idle);
        let text = draw(&live_view(&a, 100, 12), 100).0.join("\n");
        assert!(text.contains("✗ quick-thinking"), "{text}");
        assert!(text.contains("Try again, or /model to choose another"), "{text}");
        // Another model clears it.
        let mut s = a.status.clone();
        s.model = "deep-thinking".into();
        a.on_event(UiEvent::Status(s));
        let text = draw(&live_view(&a, 100, 12), 100).0.join("\n");
        assert!(!text.contains('✗') && text.contains("Reply…"), "{text}");
        // A turn that reached the model ends with what it took.
        typed(&mut a, "again");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        a.take_pending();
        let start = kodact_types::ApiMessage {
            id: "m".into(),
            kind: "message".into(),
            role: kodact_types::Role::Assistant,
            model: "m".into(),
            content: vec![],
            stop_reason: None,
            stop_sequence: None,
            usage: Default::default(),
        };
        a.on_event(UiEvent::Engine(EngineEvent::Stream {
            event: kodact_types::StreamEvent::MessageStart { message: start },
            parent_tool_use_id: None,
        }));
        let mut s = a.status.clone();
        s.cost = a.status.cost + 0.004;
        s.cost_unknown = false;
        s.session_tokens = a.status.session_tokens + 1_200;
        a.on_event(UiEvent::Status(s));
        a.on_event(UiEvent::Idle);
        let lines: Vec<String> = a.take_pending().iter().map(crate::tui::text::plain).collect();
        // The mode, the model, the time and the tokens; no money.
        assert_eq!(lines.last().unwrap(), "  Manual · deep-thinking · 0s · 1.2k tokens", "{lines:?}");
        // Sub-agents: the word, and how many are done.
        let mut b = app(false);
        b.look = crate::tui::look::Look::Frame;
        typed(&mut b, "go");
        b.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        b.on_event(UiEvent::Engine(EngineEvent::Assistant {
            message: kodact_types::ApiMessage {
                id: "m".into(),
                kind: "message".into(),
                role: kodact_types::Role::Assistant,
                model: "m".into(),
                content: vec![kodact_types::ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "Task".into(),
                    input: json!({"description": "Explore core"}),
                    cache_control: None,
                    provider: None,
                }],
                stop_reason: None,
                stop_sequence: None,
                usage: Default::default(),
            },
            uuid: "u".into(),
            parent_tool_use_id: None,
        }));
        b.agents_done = 2;
        let text = draw(&live_view(&b, 100, 14), 100).0.join("\n");
        assert!(text.contains("running  Explore core"), "{text}");
        assert!(text.contains("Waiting for 1 agent… (2 of 3 done · 0s)"), "{text}");
    }

    #[test]
    fn a_destructive_command_starts_on_deny_and_needs_y() {
        let mut a = app(false);
        a.look = crate::tui::look::Look::Frame;
        // The call is shown while it waits: as waiting, not running.
        a.on_event(UiEvent::Engine(kodact_engine::EngineEvent::Assistant {
            message: kodact_types::ApiMessage {
                id: "m".into(),
                kind: "message".into(),
                role: kodact_types::Role::Assistant,
                model: "m".into(),
                content: vec![kodact_types::ContentBlock::ToolUse {
                    id: "t".into(),
                    name: "Bash".into(),
                    input: json!({"command": "rm -rf build/"}),
                    cache_control: None,
                    provider: None,
                }],
                stop_reason: Some(kodact_types::StopReason::ToolUse),
                stop_sequence: None,
                usage: Default::default(),
            },
            uuid: "u".into(),
            parent_tool_use_id: None,
        }));
        a.busy = true;
        ask(&mut a, "Bash", json!({"command": "rm -rf build/"}));
        let (rows, _) = draw(&live_view(&a, 80, 40), 80);
        let text = rows.join("\n");
        assert!(!text.contains("Waiting rm") && !text.contains("Running rm"), "the dialog shows the command: {text}");
        assert!(!text.contains("Waiting for your answer"), "said once: {text}");
        assert!(text.contains("Run this command?") && text.contains("in /p"), "where it runs: {text}");
        assert!(text.contains("▲ This deletes files: build/. It can't be undone."), "{text}");
        assert!(text.contains("❯ 2. Deny"), "the cursor starts on Deny: {text}");
        assert!(text.contains("y to allow · Enter, n or Esc to deny"), "{text}");
        // Enter on Allow does nothing but say how to allow it.
        a.on_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert!(a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)).is_empty());
        assert!(a.dialog.is_some(), "still asking");
        assert!(a.hint.as_ref().is_some_and(|(h, _)| h.contains("Press y")));
        // y allows it.
        a.on_key(KeyEvent::new(KeyCode::Char('y'), KeyModifiers::NONE));
        assert!(a.dialog.is_none());
        // What counts as destructive.
        use crate::tui::app::danger_of;
        let d = |c: &str| danger_of("Bash", &json!({ "command": c }), "");
        assert_eq!(d("rm -rf build/ dist"), Some("deletes files: build/ dist".into()));
        assert_eq!(d("git reset --hard HEAD~1"), Some("discards uncommitted changes".into()));
        assert_eq!(d("git push --force origin main"), Some("rewrites the remote branch".into()));
        assert_eq!(d("psql -c 'DROP TABLE users'"), Some("deletes data".into()));
        assert_eq!(d("cargo test"), None);
        assert_eq!(d("git push origin main"), None);
        assert_eq!(danger_of("Read", &json!({"file_path": "/x"}), ""), None);
        // Outside the project, said so.
        let out = |c: &str| danger_of("Bash", &json!({ "command": c }), "/home/k/proj");
        assert_eq!(out("rm -rf /home/k/other"), Some("deletes files outside the project: /home/k/other".into()));
        assert_eq!(out("rm -rf ../shared"), Some("deletes files outside the project: ../shared".into()));
        assert_eq!(out("rm -rf build/"), Some("deletes files: build/".into()));
        assert_eq!(out("rm -rf /home/k/proj/build"), Some("deletes files: /home/k/proj/build".into()));
    }

    #[test]
    fn dialogs_draw_in_a_box_without_the_input() {
        let mut a = app(false);
        ask(&mut a, "Bash", json!({"command": "cargo build"}));
        let v = live_view(&a, 40, 30);
        let (rows, buf) = draw(&v, 40);
        assert_eq!(
            rows,
            [
                "╭──────────────────────────────────────╮",
                "│ ◆ Run this command?                  │",
                "│   cargo build                        │",
                "│   in /p                              │",
                "│                                      │",
                "│ ❯ 1. Allow once  y                   │",
                "│   2. Deny, and tell KodAct what to   │",
                "│      do  n                           │",
                "│                                      │",
                "│ Enter or y·n to answer · Esc to deny │",
                "╰──────────────────────────────────────╯",
                "  /p                 opus · ⡀ 88% left",
            ]
        );
        assert_eq!(v.cursor, None, "no cursor while a dialog is open");
        // Without colour the selection is reversed.
        assert!(buf[(4, 5)].modifier.contains(ratatui::style::Modifier::REVERSED));

        let mut a = app(false);
        ask(&mut a, "ExitPlanMode", json!({"plan": "1. Do it"}));
        let (rows, _) = draw(&live_view(&a, 50, 30), 50);
        assert!(rows[1].contains("Go ahead with this plan?") && rows[3].contains("❯ 1. Yes, and accept edits"));

        let mut a = app(false);
        ask(
            &mut a,
            "AskUserQuestion",
            json!({"questions": [{"question": "Which db?", "multiSelect": true,
                "options": [{"label": "sqlite", "description": "file"}, {"label": "pg"}]}]}),
        );
        a.on_key(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
        let (rows, _) = draw(&live_view(&a, 50, 30), 50);
        assert_eq!(rows[1], "│ Which db?                                      │");
        assert_eq!(rows[3], "│ ❯ 1. [x] sqlite  file                          │");
        assert_eq!(rows[5], "│   3. Type an answer                            │");
    }

    #[test]
    fn narrow_terminals_wrap_the_input_and_keep_the_box() {
        let mut a = app(false);
        typed(&mut a, "abcdefghijklmnopqrstuvwxyz");
        let v = live_view(&a, 20, 20);
        let (rows, _) = draw(&v, 20);
        assert_eq!(rows[1], "│ ❯ abcdefghijklmn │");
        assert_eq!(rows[2], "│   opqrstuvwxyz   │");
        assert_eq!(v.cursor, Some((16, 2)));
        assert!(v.lines.iter().all(|l| text::width(&text::plain(l)) <= 20));
        // Moving left goes back across the wrap.
        for _ in 0..9 {
            a.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
        }
        assert_eq!(live_view(&a, 20, 20).cursor, Some((7, 2)));
        // A dialog stays boxed at 20 columns.
        ask(&mut a, "Write", json!({"file_path": "/a/very/long/path/to/a/file.rs"}));
        let v = live_view(&a, 20, 40);
        assert!(v.lines.iter().all(|l| text::width(&text::plain(l)) == 20 || text::plain(l).starts_with("  ")));
    }

    #[test]
    fn the_default_style_keeps_the_session_in_its_box() {
        let mut a = app(true);
        a.look = crate::tui::look::Look::Frame;
        a.endpoint = "gpt.example.net".into();
        a.status.model = "quick-thinking".into();
        a.status.cost = 0.0;
        a.status.cost_unknown = true;
        a.status.context_pct = Some(4);
        a.status.window_guessed = true;
        a.status.session_tokens = 25_300;
        let (rows, buf) = draw(&live_view(&a, 120, 12), 120);
        let n = rows.len();
        // The box: the text, a row of air, the session row; rose all round.
        assert!(rows[n - 6].starts_with('╭') && rows[n - 2].starts_with('╰'), "{rows:#?}");
        assert!(rows[n - 5].starts_with(&format!("│ ❯ {PLACEHOLDER}")), "{rows:#?}");
        assert!(
            rows[n - 4].starts_with('│')
                && rows[n - 4].ends_with('│')
                && rows[n - 4][3..rows[n - 4].len() - 3].trim().is_empty()
        );
        let session = &rows[n - 3];
        assert!(session.starts_with("│  Manual   ·  quick-thinking  ·  gpt.example.net"), "{session}");
        assert!(session.contains("asks edits & shell"), "wide box shows concise mode rule: {session}");
        assert!(session.ends_with("⡀ ~96% left  ·  25.3k tokens used │"), "the room left on the right: {session}");
        assert_eq!(buf[(0, (n - 6) as u16)].fg, a.theme.accent().fg.unwrap());
        assert_eq!(buf[(3, (n - 3) as u16)].bg, a.theme.fill().bg.unwrap(), "the mode as a filled pill");
        // The status line: the folder and the keys that help; the box has the rest.
        let last = rows[n - 1].trim_end();
        assert!(last.starts_with("  /p") && last.contains("/  commands") && !last.contains("quick-thinking"), "{last}");
        // At 80 columns the endpoint is the first to go, so the context still fits.
        let (rows, _) = draw(&live_view(&a, 80, 12), 80);
        assert!(rows[n - 3].starts_with("│  Manual   ·  quick-thinking  ") && !rows[n - 3].contains("gpt.example.net"));
        assert!(rows[n - 3].contains("~96% left"), "{rows:#?}");
        // Plan mode: named in the box in its colour, not repeated on the line.
        a.status.mode = "plan".into();
        let (rows, buf) = draw(&live_view(&a, 80, 12), 80);
        assert!(rows[n - 3].starts_with("│  Plan   ·  quick-thinking"), "{rows:#?}");
        assert_eq!(buf[(3, (n - 3) as u16)].bg, a.theme.mode("plan").fg.unwrap(), "a pill in the mode's colour");
        assert!(!rows[n - 1].contains("plan mode on"), "{rows:#?}");
        // Narrow: the endpoint goes first, then the model shortens; the mode always stays.
        a.status.model = "accounts/fireworks/models/a-very-long-model-name-that-goes-on-and-on".into();
        let (rows, _) = draw(&live_view(&a, 60, 12), 60);
        let session = rows.iter().find(|r| r.contains("Plan")).expect("the mode stays");
        assert!(!session.contains("gpt.example.net") && session.contains('…'), "{session}");
        assert!(rows.iter().all(|r| text_width(r) <= 60), "{rows:#?}");
        // A priced model shows what it used too: the UI shows no money (/cost does).
        a.status.model = "opus".into();
        a.status.window_guessed = false;
        a.status.cost = 0.25;
        a.status.cost_unknown = false;
        let (rows, _) = draw(&live_view(&a, 80, 12), 80);
        assert!(rows[n - 3].ends_with("⡀ 96% left  ·  25.3k tokens used │"), "{rows:#?}");
        assert!(rows.iter().all(|r| !r.contains('$')), "{rows:#?}");
    }

    #[test]
    fn a_first_run_suggests_a_request() {
        let mut a = app(false);
        a.look = crate::tui::look::Look::Frame;
        a.first_run = true;
        let rows = draw(&live_view(&a, 80, 12), 80).0.join("\n");
        assert!(rows.contains(FIRST_RUN_PLACEHOLDER), "{rows}");
        // With history here, the plain welcome; and other styles keep theirs.
        a.first_run = false;
        assert!(draw(&live_view(&a, 80, 12), 80).0.join("\n").contains(PLACEHOLDER));
        a.first_run = true;
        a.look = crate::tui::look::Look::Bare;
        assert!(!draw(&live_view(&a, 80, 12), 80).0.join("\n").contains(FIRST_RUN_PLACEHOLDER));
    }

    #[test]
    fn plan_mode_and_hints_show_in_the_status_line() {
        let mut a = app(true);
        a.status.mode = "plan".into();
        let (rows, _) = draw(&live_view(&a, 70, 10), 70);
        assert!(rows.last().unwrap().starts_with("  ‖ plan mode on (shift+tab to cycle)"));
        a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        let (rows, _) = draw(&live_view(&a, 70, 10), 70);
        assert!(rows.last().unwrap().starts_with("  Press Ctrl-C again to exit"));
        let later = Instant::now() + HINT_FOR;
        let (rows, _) = draw(&live_view_at(&a, 70, 10, later), 70);
        assert!(rows.last().unwrap().starts_with("  ‖ plan mode"));
    }

    #[test]
    fn pickers_search_and_file_menus() {
        use kodact_core::commands::picker::{Choice, Pick, Picker};
        let mut a = app(false);
        let c = |l: &str, d: &str, cur: bool| Choice {
            label: l.into(),
            detail: d.into(),
            pick: Pick::Run(String::new()),
            current: cur,
        };
        a.on_event(UiEvent::Picker(Picker {
            title: "Select a model".into(),
            choices: vec![c("Opus", "big", false), c("Haiku", "small", true)],
        }));
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert_eq!(rows[1], "│ Select a model                       │");
        assert_eq!(rows[2], "│ ❯ type to filter                     │", "a field to type in");
        assert_eq!(rows[4], "│   1. Opus  big                       │");
        assert_eq!(rows[5], "│ ❯ 2. Haiku ✓  small                  │");
        a.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        // Ctrl+R shows the search and the match in the input box.
        let mut a = App::new(
            Theme { color: false, light: false, accent: None, depth: Default::default() },
            vec!["cargo test".into(), "git status".into()],
        );
        a.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        typed(&mut a, "car");
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert!(rows[0].contains("history: car") && rows[0].contains("1/1"), "{rows:#?}");
        assert!(rows[1].contains("cargo test"), "match preview row: {rows:#?}");
        assert!(rows[3].starts_with('│') && rows[3].contains("cargo test"), "the match in the box: {rows:#?}");
        typed(&mut a, "zz");
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert!(rows[0].contains("history: carzz") && rows[0].contains("(no match)"), "{rows:#?}");

        // `@` lists matching paths.
        let mut a = app(false);
        a.on_event(UiEvent::Files(vec!["src/".into(), "src/main.rs".into(), "README.md".into()]));
        typed(&mut a, "look at @ma");
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert_eq!(rows[3], "  src/main.rs");
    }

    /// A picker with more rows than the window (an endpoint listing 40 models) ran off the
    /// top: its title, its filter and its first rows were gone, and nothing paged. Now the
    /// title, filter and keys stay and the rows scroll around the selected one, in every
    /// style and at every size.
    #[test]
    fn a_dialog_longer_than_the_window_scrolls_its_options() {
        use kodact_core::commands::picker::{Choice, Pick, Picker};
        let key = |a: &mut App, code| a.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        for look in crate::tui::look::Look::ALL {
            for (cols, lines) in [(110u16, 36u16), (80, 24), (60, 20), (40, 12)] {
                let mut a = app(false);
                a.look = look;
                a.on_event(UiEvent::Picker(Picker {
                    title: "Select a model".into(),
                    choices: (1..=42)
                        .map(|i| Choice {
                            label: format!("model-{i:02}"),
                            detail: "a description long enough to wrap in a narrow window".into(),
                            pick: Pick::Run(String::new()),
                            current: i == 1,
                        })
                        .collect(),
                }));
                let shown = |a: &App| draw(&live_view(a, cols, lines - 1), cols).0;
                let what = format!("{look:?} {cols}x{lines}");
                let rows = shown(&a);
                assert!(rows.len() < lines as usize, "{what}: the picker fits the window: {} rows", rows.len());
                let text = rows.join("\n");
                assert!(text.contains("Select a model") && text.contains("type to filter"), "{what}: {text}");
                assert!(text.contains("❯ 1. model-01") && text.contains("of 42"), "{what}: {text}");
                assert!(!text.contains("42. model-42"), "{what}: the last row is not shown yet: {text}");
                // PageDown and End move through the list; the selected row is always shown.
                key(&mut a, KeyCode::PageDown);
                assert!(shown(&a).join("\n").contains("❯ 11. model-11"), "{what}: {:#?}", shown(&a));
                key(&mut a, KeyCode::End);
                let text = shown(&a).join("\n");
                assert!(text.contains("❯ 42. model-42") && text.contains("Select a model"), "{what}: {text}");
                assert!(shown(&a).len() < lines as usize, "{what}");
                key(&mut a, KeyCode::PageUp);
                assert!(shown(&a).join("\n").contains("❯ 32. model-32"), "{what}");
                key(&mut a, KeyCode::Home);
                assert!(shown(&a).join("\n").contains("❯ 1. model-01"), "{what}");
            }
        }
        // A short list is drawn as before: every row whole, no counter.
        let mut a = app(false);
        a.on_event(UiEvent::Picker(Picker {
            title: "Select a model".into(),
            choices: vec![Choice {
                label: "Opus".into(),
                detail: "big".into(),
                pick: Pick::Run(String::new()),
                current: false,
            }],
        }));
        let text = draw(&live_view(&a, 80, 23), 80).0.join("\n");
        assert!(text.contains("1. Opus  big") && !text.contains(" of 1"), "{text}");
    }

    fn diff_screen() -> kodact_core::commands::screens::Screen {
        let mut unified = String::from("diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,30 @@\n-fn old() {}\n");
        for i in 0..30 {
            unified.push_str(&format!("+fn new_{i}() {{}}\n"));
        }
        unified.push_str("diff --git a/README.md b/README.md\n--- a/README.md\n+++ b/README.md\n@@ -1 +1 @@\n-Old title\n+New title\n");
        kodact_core::commands::screens::Screen {
            title: "Uncommitted changes (git diff HEAD)".into(),
            rows: kodact_core::commands::screens::diff_rows(&unified, &[], &[]),
        }
    }

    #[test]
    fn screens_scroll_inside_a_box_that_fits_the_terminal() {
        let mut a = app(false);
        a.on_event(UiEvent::Screen(diff_screen()));
        let n = a.viewer().unwrap().0.rows.len();
        let (rows, _) = draw(&live_view(&a, 40, 15), 40);
        assert!(rows.len() <= 15, "{rows:#?}");
        assert_eq!(rows[0], format!("╭{}╮", "─".repeat(38)));
        assert_eq!(rows[1], "│ Uncommitted changes (git diff HEAD)  │");
        assert_eq!(rows[2], "│ ❯ src/lib.rs  +30 -1                 │");
        assert_eq!(rows[3], "│   README.md  +1 -1                   │");
        let page = a.viewer_page.get();
        assert_eq!(page, 15 - 1 - 5, "the terminal minus the status line and the box");
        assert_eq!(rows[page + 3], format!("│ 1-{page} of {n} · ↑↓ PgUp PgDn · Enter op… │"));
        // Enter on a file jumps to its hunks; Esc goes back; scrolling keeps the cursor in view.
        a.on_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let (rows, _) = draw(&live_view(&a, 40, 15), 40);
        assert_eq!(rows[2], "│ ❯ README.md                          │");
        assert_eq!(rows[3], "│   @@ -1 +1 @@                        │");
        assert_eq!(rows[4], "│   -Old title                         │", "no colour: the marker shows the side");
        a.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(a.viewer().unwrap().1, 1, "back on the file list");
        a.on_key(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
        let (rows, _) = draw(&live_view(&a, 40, 15), 40);
        assert_eq!(rows[page + 1], "│ ❯   ↩ Revert README.md               │");
        assert!(rows[page + 3].contains(&format!("{}-{n} of {n}", n - page + 1)), "{}", rows[page + 3]);
        a.on_key(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE));
        assert_eq!(a.viewer().unwrap().1, n - 1 - page);
        a.on_key(KeyEvent::new(KeyCode::Home, KeyModifiers::NONE));
        assert_eq!(a.viewer().unwrap().1, 0);
        a.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(a.dialog.is_none(), "Esc at the top closes it");

        // A tiny terminal still shows the highlighted row.
        a.on_event(UiEvent::Screen(diff_screen()));
        let v = live_view(&a, 40, 4);
        assert!(v.lines.len() <= 4);

        // Colour at 100 columns: additions green, deletions red, hunk headers dim.
        let mut a = app(true);
        a.on_event(UiEvent::Screen(diff_screen()));
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let (rows, buf) = draw(&live_view(&a, 100, 40), 100);
        let y = rows.iter().position(|r| r.contains("-fn old() {}")).unwrap() as u16;
        assert_eq!(buf[(5, y)].fg, crate::tui::palette::DANGER.color(Default::default(), false));
        assert_eq!(buf[(5, y + 1)].fg, crate::tui::palette::SUCCESS.color(Default::default(), false));
        assert_eq!(
            buf[(5, y - 1)].fg,
            crate::tui::palette::MUTED.color(Default::default(), false),
            "{}",
            rows[y as usize - 1]
        );
        assert!(rows.iter().all(|r| r.chars().count() <= 100));
    }

    #[test]
    fn forms_show_fields_and_the_command_they_run() {
        use kodact_core::commands::screens::{Field, Form, Row, RowAction, Screen, Tone};
        let form = Form {
            title: "Add a hook".into(),
            fields: vec![Field::choice("Event", &["PreToolUse", "Stop"]), Field::text("Command", "")],
            template: "/hooks add {0} {1}".into(),
        };
        let mut a = app(false);
        a.on_event(UiEvent::Screen(Screen {
            title: "Hooks".into(),
            rows: vec![Row { action: Some(RowAction::Form(form)), ..Row::text("Add hook…", Tone::Plain) }],
        }));
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        a.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
        a.on_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        typed(&mut a, "echo hi");
        let (rows, _) = draw(&live_view(&a, 60, 20), 60);
        assert_eq!(rows[1], format!("│ {:<56} │", "Add a hook"));
        assert_eq!(rows[2], format!("│ {:<56} │", "  Event    Stop"));
        assert_eq!(rows[3], format!("│ {:<56} │", "❯ Command  echo hi▏"));
        assert_eq!(rows[5], format!("│ {:<56} │", "Runs: /hooks add Stop 'echo hi'"));
        assert_eq!(
            a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            vec![crate::tui::app::Action::Send("/hooks add Stop 'echo hi'".into())]
        );
        assert!(a.dialog.is_none());
    }

    #[test]
    fn context_parts_use_letters_without_colour() {
        use kodact_core::commands::screens::{Row, Screen, Span as S, Tone};
        let row = Row {
            spans: vec![
                S { text: "⛁ ".into(), tone: Tone::Part(0) },
                S { text: "⛁ ".into(), tone: Tone::Part(5) },
                S { text: "⛶ ".into(), tone: Tone::Part(6) },
            ],
            action: None,
        };
        let screen = Screen { title: "Context".into(), rows: vec![row] };
        let mut a = app(false);
        a.on_event(UiEvent::Screen(screen.clone()));
        let (rows, _) = draw(&live_view(&a, 40, 20), 40);
        assert_eq!(rows[2], format!("│ {:<36} │", "  S G ·"), "nothing to choose: no pointer");
        let mut a = app(true);
        a.on_event(UiEvent::Screen(screen));
        let (rows, buf) = draw(&live_view(&a, 40, 20), 40);
        assert_eq!(rows[2], format!("│ {:<36} │", "  ⛁ ⛁ ⛶"));
        let glyphs: Vec<u16> = (0..40).filter(|&x| buf[(x, 2)].symbol() == "⛁").collect();
        assert_eq!(buf[(glyphs[1], 2)].fg, crate::tui::palette::SUCCESS.color(Default::default(), false), "messages");
    }

    #[test]
    fn a_screen_with_nothing_to_choose_scrolls_by_page() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use kodact_core::commands::screens::{Row, Screen, Tone};
        let rows = (1..=30).map(|i| Row::text(format!("line {i}"), Tone::Plain)).collect();
        let mut a = app(false);
        a.on_event(UiEvent::Screen(Screen { title: "Info".into(), rows }));
        let first = draw(&live_view(&a, 40, 14), 40).0;
        let page = a.viewer_page.get();
        assert!(first[2].contains("  line 1 ") && !first.concat().contains('❯'), "{first:#?}");
        let key = |a: &mut App, c| a.on_term_event(crossterm::event::Event::Key(KeyEvent::new(c, KeyModifiers::NONE)));
        key(&mut a, KeyCode::Down);
        assert!(draw(&live_view(&a, 40, 14), 40).0[2].contains("line 2 "), "one row down");
        key(&mut a, KeyCode::End);
        let last = draw(&live_view(&a, 40, 14), 40).0;
        assert!(last[2].contains(&format!("line {} ", 31 - page)), "{last:#?}");
        key(&mut a, KeyCode::Esc);
        assert!(a.dialog.is_none());
    }

    #[test]
    fn a_running_tool_turns_its_dot_until_it_answers() {
        use kodact_types::{ApiMessage, ContentBlock, Role, StopReason, Usage};
        let mut a = app(true);
        a.on_event(UiEvent::Busy);
        let msg = ApiMessage {
            id: "m".into(),
            kind: "message".into(),
            role: Role::Assistant,
            model: "x".into(),
            content: vec![ContentBlock::ToolUse {
                id: "t1".into(),
                name: "Read".into(),
                input: serde_json::json!({"file_path": "src/a.rs"}),
                cache_control: None,
                provider: None,
            }],
            stop_reason: Some(StopReason::ToolUse),
            stop_sequence: None,
            usage: Usage::default(),
        };
        a.on_event(UiEvent::Engine(kodact_engine::EngineEvent::Assistant {
            message: msg,
            uuid: "u".into(),
            parent_tool_use_id: None,
        }));
        let since = a.running[0].since;
        let face = |at: Instant| {
            let (rows, buf) = draw(&live_view_at(&a, 60, 20, at), 60);
            let y = rows.iter().position(|r| r.contains("Reading src/a.rs")).expect("the call shows while it runs");
            (rows[y].chars().next().unwrap(), buf[(0, y as u16)].fg)
        };
        let (first, color) = face(since);
        let (later, _) = face(since + Duration::from_millis(250));
        assert_ne!(first, later, "the dot turns");
        assert!(glyphs::RUNNING.contains(&first.to_string().as_str()));
        assert_eq!(
            color,
            crate::tui::palette::BRAND.color(Default::default(), false),
            "running is rose, as the spinner"
        );
    }

    #[test]
    fn the_shimmer_sweeps_and_the_meter_warns() {
        let t = text::Theme { color: true, light: false, accent: None, depth: Default::default() };
        let at = |ms| shimmer(&t, "Acting…", Duration::from_millis(ms));
        // Where the brightest character is, in characters from the start.
        let bright = |spans: &[Span]| {
            let i = spans.iter().position(|s| s.style == t.shimmer(3))?;
            Some(spans[..i].iter().map(|s| s.content.chars().count()).sum::<usize>())
        };
        assert_ne!(bright(&at(500)), bright(&at(700)), "the highlight moves");
        let text: String = at(500).iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Acting…");
        // The gauge: room left before compaction, as a shape that fills and a number.
        let at = |used: u64| StatusView {
            context_tokens: used,
            compact_at: 100_000,
            context_pct: Some(1),
            ..Default::default()
        };
        let g = |used| gauge(&t, &at(used)).iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(g(4_000), "⡀ 96% left");
        assert_eq!(g(25_000), "⣀ 75% left");
        assert_eq!(g(50_000), "⣤ 50% left");
        assert_eq!(g(75_000), "⣶ 25% left · summarizes at 0%", "low: what happens next");
        assert_eq!(g(95_000), "⣷ 5% left · /compact now", "very low: what to do");
        assert_eq!(g(140_000), "⣿ 0% left · /compact now", "past the point: nothing left, never negative");
        assert_eq!(gauge(&t, &at(4_000))[0].style, t.accent());
        assert_eq!(gauge(&t, &at(75_000))[0].style, t.warning(), "amber from 30% left");
        assert_eq!(gauge(&t, &at(95_000))[0].style, t.error(), "red from 10% left");
        let guessed = StatusView { window_guessed: true, ..at(4_000) };
        assert!(gauge(&t, &guessed)[1].content.contains("~96% left"));
    }

    #[test]
    fn bars_fill_dot_by_dot_and_marks_breathe() {
        let t = crate::tui::text::Theme { color: true, light: false, accent: None, depth: Default::default() };
        let bar = |pct| fill_bar(&t, pct, 4, t.accent()).iter().map(|s| s.content.to_string()).collect::<String>();
        assert_eq!(bar(0), "⣀⣀⣀⣀", "an empty bar is its faint track");
        assert_eq!(bar(50), "⣿⣿⣀⣀");
        assert_eq!(bar(60), "⣿⣿⣄⣀", "a cell part full fills by dots");
        assert_eq!(bar(100), "⣿⣿⣿⣿");
        assert!(text::width(&bar(37)) == 4, "always its width");
        // The mark's colour: dimmer nearly empty than full; untouched without 24-bit colour.
        let empty = breathing(&t, t.accent(), Duration::ZERO);
        let full = breathing(&t, t.accent(), FILL_STEP * 7);
        assert_ne!(empty.fg, full.fg);
        assert_eq!(full.fg, t.accent().fg, "full is the colour itself");
        let sixteen = crate::tui::text::Theme { depth: palette::Depth::Ansi16, ..t };
        assert_eq!(breathing(&sixteen, t.accent(), Duration::ZERO), t.accent());
    }

    /// `?` (or /status, /help) on the front page showed the last six rows of its answer
    /// under the page, or none in a short window. An answer longer than a note takes the
    /// page's place, whole and scrollable; a short one stays under the page.
    #[test]
    fn an_answer_on_the_front_page_is_shown_whole() {
        let mut a = app(false);
        a.front = true;
        let page = |dock| FrontPage {
            above: vec![Line::from("WORDMARK"), Line::default(), Line::from("tagline"), Line::default()],
            box_left: 10,
            box_w: 80,
            under: true,
            dock,
            ..Default::default()
        };
        let answer: Vec<Line<'static>> = (1..=18).map(|i| Line::from(format!("key row {i}"))).collect();
        for dock in [false, true] {
            for (cols, lines) in [(100u16, 30u16), (60, 20), (40, 12)] {
                let (v, max) = full_view(&a, cols, lines, &answer, &page(dock), 0);
                let (rows, _) = draw(&v, cols);
                let what = format!("dock {dock}, {cols}x{lines}: {rows:#?}");
                assert!(rows.iter().any(|r| r == "key row 18"), "the answer's end shows: {what}");
                let shown = rows.iter().filter(|r| r.starts_with("key row")).count();
                assert_eq!(shown + max, 18, "every row is shown or a scroll away: {what}");
                assert!(!rows.iter().any(|r| r == "WORDMARK"), "the page gave way: {what}");
                assert!(rows.iter().any(|r| r.contains('❯')), "the input is in view: {what}");
                let (v, _) = full_view(&a, cols, lines, &answer, &page(dock), max);
                assert!(draw(&v, cols).0.iter().any(|r| r == "key row 1"), "scrolled up, its start shows: {what}");
            }
            // A note or two stays under the page.
            let (v, _) = full_view(&a, 100, 30, &answer[..2], &page(dock), 0);
            let (rows, _) = draw(&v, 100);
            assert!(rows.iter().any(|r| r == "WORDMARK") && rows.iter().any(|r| r.trim() == "key row 2"), "{rows:#?}");
            // Too short a window for the page and the note: the note wins.
            let (v, max) = full_view(&a, 100, 9, &answer[..4], &page(dock), 0);
            let (rows, _) = draw(&v, 100);
            let shown = rows.iter().filter(|r| r.starts_with("key row")).count();
            assert!(rows.iter().any(|r| r == "key row 4") && shown + max == 4, "{rows:#?}");
            assert!(!rows.iter().any(|r| r == "WORDMARK"), "{rows:#?}");
        }
    }

    #[test]
    fn the_full_screen_centres_the_front_then_keeps_the_input_at_the_bottom() {
        let mut a = app(false);
        a.front = true;
        a.endpoint = "127.0.0.1:11434".into();
        let page = FrontPage {
            above: vec![Line::from("WORDMARK"), Line::default(), Line::from("tagline"), Line::default()],
            below: vec![Line::default(), Line::from("/resume  checkout rounding")],
            box_left: 10,
            box_w: 80,
            under: true,
            ..Default::default()
        };
        let (v, _) = full_view(&a, 100, 30, &[], &page, 0);
        let (rows, _) = draw(&v, 100);
        assert_eq!(rows.len(), 30, "the whole screen");
        let mark = rows.iter().position(|r| r == "WORDMARK").unwrap();
        let resume = rows.iter().position(|r| r.contains("/resume")).unwrap();
        assert!(mark > 3 && resume < 25, "centred down the screen: {rows:#?}");
        // As many empty rows above the page as below it (the status line aside).
        let first = rows.iter().position(|r| !r.trim().is_empty()).unwrap();
        let last = rows[..29].iter().rposition(|r| !r.trim().is_empty()).unwrap();
        assert!(first.abs_diff(28 - last) <= 1, "centred: {first} above, {} below\n{rows:#?}", 28 - last);
        assert!(rows[mark + 3..resume].iter().any(|r| r.contains("❯ What shall we build today?")), "the input between");
        assert!(rows[29].ends_with(&format!("v{}", kodact_core::VERSION)), "the status line on the last row");
        let (_, cy) = v.cursor.unwrap();
        assert!(rows[cy as usize].contains('❯'), "the cursor in the input");
        // A note on the front page (`/clear` and its answer) lines up with the box's `❯`
        // and wraps within the box's width, in both boxed and open styles.
        let note_rows =
            vec![Line::from("  ❯ /clear"), Line::from(format!("    Conversation cleared. {}", "word ".repeat(16)))];
        let (vn, _) = full_view(&a, 100, 30, &note_rows, &page, 0);
        let (nrows, _) = draw(&vn, 100);
        let box_prompt = nrows.iter().find(|r| r.contains("What shall we build")).unwrap();
        let clear_row = nrows.iter().find(|r| r.contains("❯ /clear")).unwrap();
        let col = |r: &str, ch: char| text::width(&r[..r.find(ch).unwrap()]);
        assert_eq!(col(clear_row, '❯'), col(box_prompt, '❯'), "aligned with the box: {nrows:#?}");
        assert!(
            nrows
                .iter()
                .filter(|r| r.contains("word"))
                .all(|r| text::width(r) <= (page.box_left + page.box_w) as usize),
            "long notes wrap within the box: {nrows:#?}"
        );
        a.look = Look::Bare;
        let bare_notes = vec![Line::from("  › /clear"), Line::from("    Conversation cleared.")];
        let (vb, _) = full_view(&a, 100, 30, &bare_notes, &page, 0);
        let (brows, _) = draw(&vb, 100);
        let bare_prompt = brows.iter().find(|r| r.contains("What shall we build")).unwrap();
        let bare_clear = brows.iter().find(|r| r.contains("› /clear")).unwrap();
        assert_eq!(col(bare_clear, '›'), col(bare_prompt, '›'), "aligned in open style too: {brows:#?}");
        a.look = Look::Frame;
        // Typing `/`, `@`, or Ctrl+R on the front page lines up with the box's prompt,
        // fits within the box's right edge, and does not jump the wordmark upward.
        a.on_event(UiEvent::Files(vec!["src/main.rs".into()]));
        let base_mark =
            draw(&full_view(&a, 100, 36, &note_rows, &page, 0).0, 100).0.iter().position(|r| r == "WORDMARK").unwrap();
        typed(&mut a, "/");
        let (vm, _) = full_view(&a, 100, 36, &note_rows, &page, 0);
        let (mrows, _) = draw(&vm, 100);
        assert_eq!(
            mrows.iter().position(|r| r == "WORDMARK"),
            Some(base_mark),
            "opening `/` does not jump the wordmark upward: {mrows:#?}"
        );
        let prompt_row = mrows.iter().find(|r| r.contains("❯ /")).unwrap();
        let menu_row = mrows.iter().find(|r| r.contains("/clear")).unwrap();
        assert_eq!(col(menu_row, '/'), col(prompt_row, '❯'), "`/` menu aligns with `❯`: {mrows:#?}");
        assert!(text::width(menu_row) <= page.box_left + page.box_w, "`/` menu stays within the box: {menu_row:?}");
        // Even on a shorter window with notes, opening `/` keeps the front page intact.
        let (vs, _) = full_view(&a, 100, 26, &note_rows, &page, 0);
        let (srows, _) = draw(&vs, 100);
        assert!(srows.iter().any(|r| r == "WORDMARK"), "front page stays intact when `/` opens: {srows:#?}");
        a.editor.set("@");
        let (vf, _) = full_view(&a, 100, 36, &[], &page, 0);
        let (frows, _) = draw(&vf, 100);
        let file_row = frows.iter().find(|r| r.contains("src/main.rs")).unwrap();
        assert_eq!(col(file_row, 's'), col(prompt_row, '❯'), "`@` menu aligns with `❯`: {frows:#?}");
        a.editor.clear();
        a.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        let (vr, _) = full_view(&a, 100, 36, &[], &page, 0);
        let (rrows, _) = draw(&vr, 100);
        let search_row = rrows.iter().find(|r| r.contains("history:")).unwrap();
        assert_eq!(col(search_row, 'h'), col(prompt_row, '❯'), "Ctrl+R aligns with `❯`: {rrows:#?}");
        a.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        // After the first message: the transcript from the top, the input at the bottom.
        a.submit("hello".into());
        let transcript: Vec<Line<'static>> = (1..=40).map(|i| Line::from(format!("line {i}"))).collect();
        let (v, max) = full_view(&a, 100, 30, &transcript, &page, 0);
        let (rows, _) = draw(&v, 100);
        assert_eq!(rows.len(), 30);
        assert!(rows.iter().any(|r| r == "line 40"), "the newest line shows");
        assert!(!rows.iter().any(|r| r == "line 1"));
        assert!(rows[25..].iter().any(|r| r.contains('❯')), "the input at the bottom: {rows:#?}");
        assert!(max > 0);
        // Scrolled up: older lines, and a note of what is below.
        let (v, _) = full_view(&a, 100, 30, &transcript, &page, 10);
        let (rows, _) = draw(&v, 100);
        // The last row turns into the note of what is below.
        assert!(rows.iter().any(|r| r == "line 29") && !rows.iter().any(|r| r == "line 31"), "{rows:#?}");
        assert!(rows.iter().any(|r| r.trim() == "↑ 75% · ↓ 10 more lines · wheel / PgDn / End bottom"), "{rows:#?}");
        // A short transcript sits just above the live region, not at the top with a gap.
        a.busy = false;
        let (v, _) = full_view(&a, 100, 30, &transcript[..2], &page, 0);
        let (rows, _) = draw(&v, 100);
        let at = rows.iter().position(|r| r == "line 2").unwrap();
        assert_eq!(rows[0], "");
        assert_eq!(rows[at + 1], "", "a blank row between the answer and the input");
        assert!(rows[at + 1..].iter().take(6).any(|r| r.contains('╭')), "{rows:#?}");
    }

    fn stream(a: &mut App, ev: kodact_types::StreamEvent) {
        a.on_event(UiEvent::Engine(kodact_engine::EngineEvent::Stream { event: ev, parent_tool_use_id: None }));
    }

    /// A streamed text block, delivered in `parts`.
    fn streamed_text(a: &mut App, parts: &[&str]) {
        use kodact_types::{ContentBlock, Delta, StreamEvent};
        stream(a, StreamEvent::ContentBlockStart { index: 0, content_block: ContentBlock::text("") });
        for p in parts {
            stream(a, StreamEvent::ContentBlockDelta { index: 0, delta: Delta::TextDelta { text: p.to_string() } });
        }
        stream(a, StreamEvent::ContentBlockStop { index: 0 });
    }

    fn assistant(content: Vec<kodact_types::ContentBlock>, parent: Option<&str>) -> UiEvent {
        use kodact_types::{ApiMessage, Role, StopReason, Usage};
        UiEvent::Engine(kodact_engine::EngineEvent::Assistant {
            message: ApiMessage {
                id: "m".into(),
                kind: "message".into(),
                role: Role::Assistant,
                model: "m".into(),
                content,
                stop_reason: Some(StopReason::ToolUse),
                stop_sequence: None,
                usage: Usage::default(),
            },
            uuid: "u".into(),
            parent_tool_use_id: parent.map(str::to_string),
        })
    }

    fn task(id: &str, what: &str) -> kodact_types::ContentBlock {
        kodact_types::ContentBlock::ToolUse {
            id: id.into(),
            name: "Task".into(),
            input: json!({"description": what, "prompt": "look", "subagent_type": "Explore"}),
            cache_control: None,
            provider: None,
        }
    }

    /// The turn from a real session on an OpenAI-compatible gateway: answers that open
    /// with blank lines, answers that are only blank lines before tool calls, and five
    /// sub-agents. No answer mark may stand alone on its row, and every sub-agent shows
    /// it is working and what it does.
    #[test]
    fn a_gateway_turn_with_sub_agents_renders_cleanly() {
        let mut a = app(false);
        a.look = crate::tui::look::Look::Frame;
        typed(&mut a, "run multiple sub agents");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        streamed_text(&mut a, &["\n\n", "Let me get a quick view first.", "\n"]);
        a.on_event(assistant(vec![kodact_types::ContentBlock::text("\n\nLet me get a quick view first.\n")], None));
        // Blank-only text before each delegation.
        let areas = ["core crates", "tools crates", "supporting crates", "evals and docs", "jobs and git"];
        for (i, what) in areas.iter().enumerate() {
            streamed_text(&mut a, &["\n", "\n"]);
            a.on_event(assistant(vec![kodact_types::ContentBlock::text("\n\n"), task(&format!("t{i}"), what)], None));
        }
        // The first sub-agent reads a file.
        let read = kodact_types::ContentBlock::ToolUse {
            id: "s1".into(),
            name: "Read".into(),
            input: json!({"file_path": "/p/src/lib.rs"}),
            cache_control: None,
            provider: None,
        };
        a.on_event(assistant(vec![read], Some("t0")));

        let scroll: Vec<String> = a.take_pending().iter().map(crate::tui::text::plain).collect();
        assert!(
            !scroll.iter().any(|l| l.trim() == kodact_core::glyphs::ANSWER),
            "no answer mark alone on a row:\n{}",
            scroll.join("\n")
        );
        assert!(scroll.iter().any(|l| l.starts_with("• Let me get a quick view first.")), "{scroll:#?}");

        let start = a.running[0].since;
        let v = live_view_at(&a, 100, 40, start + Duration::from_millis(250));
        let (rows, _) = draw(&v, 100);
        let agents: Vec<&String> = rows.iter().filter(|r| r.contains(" running  ")).collect();
        assert_eq!(agents.len(), 5, "{rows:#?}");
        assert!(agents.iter().all(|r| !r.contains("Delegating")), "the line below says they're agents: {agents:#?}");
        assert!(agents[0].ends_with("1 step"), "how many steps: {}", agents[0]);
        let first = rows.iter().position(|r| r.contains(" running  ")).unwrap();
        assert_eq!(
            rows[first + 1].trim(),
            "└ Reading src/lib.rs",
            "its latest step under it, from the folder: {rows:#?}"
        );
        assert!(rows.iter().any(|r| r.contains("Waiting for 5 agents…")), "{rows:#?}");
        assert!(!rows.iter().any(|r| r.contains("Running Task")), "{rows:#?}");
        // The scanner: one row of five blocks, a bright turquoise head sweeping back and forth
        // with a fading trail behind it.
        let t = crate::tui::text::Theme {
            color: true,
            light: false,
            accent: None,
            depth: crate::tui::palette::Depth::True,
        };
        let heads: Vec<usize> = (0..8).map(|f| scan_head(f).0).collect();
        assert_eq!(heads, [0, 1, 2, 3, 4, 3, 2, 1], "to the end and back, a cell a step");
        let at = |f: u64| scanner(&t, SCAN_STEP * f as u32);
        let frame = at(3);
        let text: String = frame.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "■■■■■ ", "one row, five cells");
        let head = Style::default().fg(crate::tui::palette::TEAL.color(t.depth, false));
        assert_eq!(frame[3].style, head, "the head is the palette's turquoise");
        assert_eq!(frame[3].style.fg, t.teal().fg, "the same teal as headings and plan mode");
        assert_ne!(frame[2].style, frame[3].style, "a trail behind it, fading");
        let unlit = Style::default().fg(crate::tui::palette::HAIRLINE.color(t.depth, false));
        assert_eq!(frame[4].style, unlit, "ahead of it: unlit");
        // Going back, the trail is on the other side.
        let back = at(6);
        assert_eq!(back[2].style, head);
        assert_ne!(back[3].style, back[1].style, "the trail follows the head home");
        // Without colour, shapes carry the motion.
        let mono = crate::tui::text::Theme { color: false, ..t };
        let text: String = scanner(&mono, SCAN_STEP * 3).iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "·▪▪■· ");
    }

    #[test]
    fn styles_change_the_bar_the_input_and_the_dialog() {
        use crate::tui::look::Look;
        let mut a = app(false);
        a.endpoint = "127.0.0.1:11434".into();
        a.branch = "main".into();
        // The status style puts a bar of readouts on top of the conversation.
        a.look = Look::Status;
        let page = FrontPage { box_left: 0, box_w: 100, ..Default::default() };
        let (v, _) = full_view(&a, 100, 20, &[Line::from("hello")], &page, 0);
        let (rows, _) = draw(&v, 100);
        assert!(rows[0].contains("model opus · 127.0.0.1:11434") && rows[0].contains("branch main"), "{}", rows[0]);
        a.status.cwd = "/tmp/a/very/long/path/that/goes/on/to/the/project/demo/proj2".into();
        let (v, _) = full_view(&a, 100, 20, &[Line::from("hello")], &page, 0);
        let bar = &draw(&v, 100).0[0];
        assert!(
            bar.contains("…") && bar.contains("proj2") && bar.contains("branch main"),
            "a long folder keeps its end: {bar}"
        );
        // Bare: the input under a rule, no box, a plain mark.
        a.look = Look::Bare;
        let (rows, _) = draw(&live_view(&a, 60, 10), 60);
        assert!(rows.iter().any(|r| r == &"─".repeat(60)));
        assert!(rows.iter().any(|r| r.starts_with("› What shall we build today?")), "{rows:#?}");
        assert!(!rows.iter().any(|r| r.contains('╭')));
        // Frame: the question between corner marks.
        a.look = Look::Frame;
        ask(&mut a, "Bash", serde_json::json!({"command": "ls"}));
        let (rows, _) = draw(&live_view(&a, 60, 30), 60);
        assert!(rows[0].starts_with("┌─") && rows[0].ends_with("─┐"), "{rows:#?}");
        assert!(rows.iter().any(|r| r.starts_with("└─")));
    }

    #[test]
    fn the_front_screen_centres_its_input_until_something_is_sent() {
        let mut a = app(false);
        a.front = true;
        a.endpoint = "127.0.0.1:11434".into();
        let (rows, _) = draw(&live_view(&a, 100, 12), 100);
        let top = rows.iter().position(|r| r.contains('╭')).unwrap();
        assert_eq!(rows[top].find('╭'), Some(10), "an 80-column box, centred: {rows:#?}");
        assert!(rows[top + 1].trim_start().starts_with("│ ❯ What shall we build today?"));
        assert_eq!(
            rows[top + 3].trim(),
            "Manual  ·  opus  ·  127.0.0.1:11434",
            "the mode, model and endpoint under it"
        );
        assert!(
            rows.last().unwrap().ends_with(&format!("v{}", kodact_core::VERSION)),
            "the version on the status line"
        );
        assert_eq!(live_view(&a, 100, 12).cursor, Some((14, top as u16 + 1)));
        // Sending anything ends the front screen: the box spans the terminal again.
        a.submit("hello".into());
        let (rows, _) = draw(&live_view(&a, 100, 12), 100);
        assert!(rows.iter().any(|r| r.starts_with('╭')), "{rows:#?}");
    }

    #[test]
    fn a_long_folder_keeps_its_end_and_leaves_room_for_the_model() {
        let mut a = app(false);
        a.status.cwd = "/tmp/a/very/long/path/that/goes/on/to/the/project/demo/proj2".into();
        let (rows, _) = draw(&live_view(&a, 60, 10), 60);
        let last = rows.last().unwrap();
        assert!(last.starts_with("  …") && last.contains("/demo/proj2"), "{last}");
        assert!(last.contains("opus"), "the model stays: {last}");
    }

    #[test]
    fn a_long_folder_loses_whole_folders_never_half_a_name() {
        let p = "/tmp/claude-0/-home-user-ForgeCLI/f668c950-af31-5eaf-a001-d64346c7dabd/scratchpad/demo/proj2";
        assert_eq!(keep_end(p, 30), "…/scratchpad/demo/proj2");
        assert_eq!(keep_end(p, 12), "…/demo/proj2");
        assert_eq!(keep_end("/a/a-very-long-project-name", 10), "…ject-name", "one name too long is cut");
        assert_eq!(keep_end("~/proj", 30), "~/proj");
    }

    #[test]
    fn the_status_line_always_shows_the_folder_even_with_the_sidebar_open() {
        let mut a = app(false);
        a.status.cwd = "/home/me/proj2".into();
        a.sidebar = true;
        let (rows, _) = draw(&live_view(&a, 100, 10), 100);
        assert!(rows.last().unwrap().contains("proj2"), "{rows:#?}");
    }

    #[test]
    fn a_status_line_command_replaces_the_right_side() {
        let mut a = app(false);
        a.on_event(UiEvent::StatusLine(Some("main · 3 files".into())));
        let (rows, _) = draw(&live_view(&a, 40, 10), 40);
        assert_eq!(rows.last().unwrap(), "  /p                    main · 3 files");
    }

    /// The sidebar's rows as text.
    /// The sidebar's rows as text, read inside its panes: a pane's top border as its
    /// name and count (`Plan  1/3`), a row without its `│` edges, a bottom border blank.
    fn side(a: &App, h: usize, now: Instant) -> Vec<String> {
        sidebar(a, SIDEBAR_W as usize, h, now)
            .iter()
            .map(|l| {
                let r = l.spans.iter().map(|s| s.content.as_ref()).collect::<String>();
                let r = r.trim();
                if let Some(top) = r.strip_prefix("╭─ ").and_then(|t| t.strip_suffix("─╮")) {
                    top.replace('─', " ").trim_end().to_string()
                } else if r.starts_with('╰') {
                    String::new()
                } else {
                    r.strip_prefix("│ ").and_then(|t| t.strip_suffix('│')).unwrap_or(r).trim_end().to_string()
                }
            })
            .collect()
    }

    /// A session as the sidebar sees it mid-turn: a plan, two MCP servers (one failed),
    /// skills (two used, one twice), and a sub-agent six steps in.
    fn busy_session(started: Instant) -> App {
        let mut a = app(false);
        a.submit("add an age check to signups".into());
        a.front = false;
        a.mcp = vec![("jira".into(), "connected".into()), ("figma".into(), "failed".into())];
        a.mcp_tools = [("jira".to_string(), 12), ("figma".to_string(), 0)].into_iter().collect();
        a.todos = vec![
            ("completed".into(), "Read the form".into()),
            ("in_progress".into(), "Add the field".into()),
            ("pending".into(), "Run the tests".into()),
        ];
        a.on_event(UiEvent::Skills(
            (0..14).map(|i| format!("skill{i}")).chain(["simplify".into(), "verify".into()]).collect(),
        ));
        a.used_skill("verify");
        a.used_skill("simplify");
        a.used_skill("/verify");
        a.running.push(crate::tui::app::Running {
            id: "t1".into(),
            header: vec![],
            name: "Task".into(),
            input: serde_json::json!({"subagent_type": "Explore", "description": "find the parser", "prompt": "…"}),
            since: started,
            step: Some("Reading src/parse.rs".into()),
            steps: 6,
            trail: ["Reading src/lexer.rs", "Searching for \"parse\"", "Reading src/parse.rs"]
                .map(String::from)
                .to_vec(),
        });
        a
    }

    #[test]
    fn the_sidebar_panes_stay_whole_at_any_size() {
        let now = Instant::now();
        let a = busy_session(now);
        for w in [SIDEBAR_W as usize, 40, SIDEBAR_MAX_W as usize] {
            for h in 8..=60 {
                let rows: Vec<String> = sidebar(&a, w, h, now)
                    .iter()
                    .map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>())
                    .collect();
                let all = rows.join("\n");
                assert_eq!(rows.len(), h, "{w}x{h}: the sidebar fills its height\n{all}");
                let opened = rows.iter().filter(|r| r.starts_with('╭')).count();
                let closed = rows.iter().filter(|r| r.starts_with('╰')).count();
                assert_eq!(opened, closed, "{w}x{h}: every pane closes\n{all}");
                for r in rows.iter().filter(|r| r.starts_with(['╭', '│', '╰'])) {
                    assert_eq!(text_width(r), w, "{w}x{h}: a pane row is exactly the sidebar's width: {r:?}\n{all}");
                    assert!(r.ends_with(['╮', '│', '╯']), "{w}x{h}: its right edge: {r:?}");
                }
                // A pane is whole or absent: between a top and its bottom only rows.
                let mut open = false;
                for r in &rows {
                    if r.starts_with('╭') {
                        assert!(!open, "{w}x{h}: panes never overlap\n{all}");
                        open = true;
                    } else if r.starts_with('╰') {
                        open = false;
                    } else if open {
                        assert!(r.starts_with('│'), "{w}x{h}: inside a pane only its rows\n{all}");
                    }
                }
                assert!(!open, "{w}x{h}: no pane left open\n{all}");
                if h >= 20 {
                    assert!(all.contains("Plan"), "{w}x{h}: the plan never gives way\n{all}");
                }
            }
        }
        assert_eq!(sidebar_width(136), SIDEBAR_W);
        assert_eq!(sidebar_width(140), 35);
        assert_eq!(sidebar_width(160), 40);
        assert_eq!(sidebar_width(400), SIDEBAR_MAX_W);
    }

    #[test]
    fn the_sidebar_keeps_one_order_and_says_what_each_part_brings() {
        let now = Instant::now();
        let a = busy_session(now);
        let rows = side(&a, 60, now);
        let all = rows.join("\n");
        let at = |want: &str| rows.iter().position(|r| r.contains(want)).unwrap_or_else(|| panic!("{want}\n{all}"));
        // Title and folder, context, plan, MCP, skills, agents.
        let order = ["add an age check", "Context", "Plan", "MCP", "Skills", "Agents"].map(at);
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}\n{all}");
        assert_eq!(at("/p"), at("add an age check") + 1, "the folder under the title\n{all}");
        // Each heading counts what its section holds, flush right.
        assert!(rows[at("Plan")].ends_with("1/3"), "{all}");
        assert!(rows[at("MCP")].ends_with("1 failed"), "{all}");
        assert!(rows[at("Skills")].ends_with("16 · /skills"), "{all}");
        assert!(rows[at("Agents")].ends_with("1 running"), "{all}");
        // A connected server shows the tools it brings; a failed one says so beside a cross.
        let jira = &rows[at("jira")];
        assert!(jira.starts_with("● jira") && jira.ends_with("12 tools"), "{jira}");
        let figma = &rows[at("figma")];
        assert!(figma.starts_with("× figma") && figma.ends_with("failed"), "{figma}");
        // The skills used, the latest first, with how often; never the whole list.
        assert!(at("▸ verify") < at("▸ simplify"), "{all}");
        assert!(rows[at("▸ verify")].ends_with("×2"), "{all}");
        assert!(!all.contains("skill3"), "{all}");
        // The sub-agent: what it was asked, then its kind, its steps and how long it has
        // run, and with room to spare its latest steps as a tree.
        assert!(rows[at("find the parser")].ends_with("6 steps"), "{all}");
        assert!(all.contains("Explore · 0s"), "{all}");
        assert!(at("├ Searching for \"parse\"") < at("└ Reading src/parse.rs"), "{all}");
        // Short of room, the sections tighten before any gives way: the agent keeps one row
        // for its latest step instead of the tree.
        let tight = side(&a, 27, now);
        let joined = tight.join("\n");
        assert!(joined.contains("Agents") && tight.iter().any(|r| r == "  Reading src/parse.rs"), "{joined}");
        assert!(!joined.contains("└"), "{joined}");
        // Plan marks: done checked, the rest open.
        assert!(all.contains("✓ Read the form") && all.contains("○ Run the tests"), "{all}");
        // Every row fits the sidebar.
        assert!(rows.iter().all(|r| text_width(r) <= SIDEBAR_W as usize), "{all}");
    }

    /// The ring is the eight dots round a 4x4 braille square, and each frame is the ring less
    /// two neighbouring dots, the gap one dot further clockwise each frame: only the edge moves.
    #[test]
    fn the_plan_ring_turns_its_edge() {
        // (row, column) clockwise from the top left; a braille cell's dots by row, left column
        // then right column.
        let ring = [(0, 1), (0, 2), (1, 3), (2, 3), (3, 2), (3, 1), (2, 0), (1, 0)];
        let bit = |row: usize, col: usize| -> u32 {
            let left = [0x01, 0x02, 0x04, 0x40];
            let right = [0x08, 0x10, 0x20, 0x80];
            if col.is_multiple_of(2) {
                left[row]
            } else {
                right[row]
            }
        };
        let draw = |lit: &[usize]| -> String {
            let mut cells = [0u32; 2];
            for &i in lit {
                let (row, col) = ring[i % 8];
                cells[col / 2] |= bit(row, col);
            }
            cells.iter().map(|c| char::from_u32(0x2800 + c).unwrap()).collect()
        };
        assert_eq!(RING_STILL, draw(&[0, 1, 2, 3, 4, 5, 6, 7]));
        for (k, frame) in RING.iter().enumerate() {
            let lit: Vec<usize> = (2..8).map(|j| k + j).collect();
            assert_eq!(*frame, draw(&lit), "frame {k}");
            assert_eq!(text_width(frame), 2, "frame {k}");
        }
        assert_eq!(turning(RING_STEP * 9), RING[1]);
    }

    #[test]
    fn work_in_flight_fills_and_settles_when_the_turn_ends() {
        let start = Instant::now();
        let mut a = busy_session(start);
        a.busy_since = Some(start);
        let mark = |a: &App, now: Instant, what: &str| -> String {
            let rows = side(a, 60, now);
            let r = rows.iter().find(|r| r.contains(what)).unwrap_or_else(|| panic!("{what}: {rows:#?}"));
            r.split_whitespace().next().unwrap().to_string()
        };
        // A sub-agent's mark fills frame by frame, from empty to full and back.
        let frames: Vec<String> =
            (0..FILL.len() as u32).map(|i| mark(&a, start + FILL_STEP * i, "find the parser")).collect();
        assert_eq!(frames, FILL.map(String::from).to_vec());
        // The plan's task in progress: a ring whose gap goes round its edge, a dot a frame.
        let round: Vec<String> =
            (0..RING.len() as u32).map(|i| mark(&a, start + RING_STEP * i, "Add the field")).collect();
        assert_eq!(round, RING.map(String::from).to_vec());
        // Each mark keeps its own phase: a sub-agent that started later is a frame behind.
        assert_eq!(filling(FILL_STEP * (FILL.len() as u32 + 1)), filling(FILL_STEP));
        // Idle: the open step waits, its ring whole and still, and the agents section is gone.
        a.busy = false;
        a.running.clear();
        assert_eq!(mark(&a, start + FILL_STEP * 3, "Add the field"), RING_STILL);
        assert!(!side(&a, 60, start).join("\n").contains("Agents"));
        // A server still connecting fills too, and keeps the window redrawing while idle.
        a.mcp.push(("linear".into(), "pending".into()));
        a.sidebar = true;
        assert!(FILL.contains(&mark(&a, start, "linear").as_str()));
        assert!(side(&a, 60, start).iter().any(|r| r.contains("linear") && r.ends_with("connecting")));
        assert!(sidebar_moving(&a));
    }

    #[test]
    fn on_a_short_window_the_lowest_sections_give_way_and_the_plan_never_does() {
        let now = Instant::now();
        let a = busy_session(now);
        let has = |h: usize, what: &str| side(&a, h, now).join("\n").contains(what);
        // Tall: everything.
        assert!(["Context", "Plan", "MCP", "Skills", "Agents"].iter().all(|s| has(60, s)));
        // Shorter and shorter: agents go first, then skills, then MCP, then the context.
        let first_gone = |what: &str| (8..60).rev().find(|&h| !has(h, what)).unwrap();
        let (agents, skills, mcp, context) =
            (first_gone("Agents"), first_gone("Skills"), first_gone("MCP"), first_gone("Context"));
        assert!(agents > skills && skills > mcp && mcp > context, "{agents} {skills} {mcp} {context}");
        // The plan stays at every height that can hold it; the folder stays with the session's pane.
        for h in 10..60 {
            let rows = side(&a, h, now);
            assert_eq!(rows.len(), h);
            assert!(rows.join("\n").contains("Add the field"), "height {h}: {rows:#?}");
            let session = rows.iter().any(|r| r.contains("session"));
            assert_eq!(rows.iter().any(|r| r.contains("/p")), session, "height {h}");
        }
    }

    #[test]
    fn a_skill_counts_as_used_when_the_model_calls_it_or_it_is_typed() {
        let mut a = app(false);
        a.on_event(UiEvent::Skills(vec!["simplify".into(), "verify".into()]));
        // Typed as a command, with words after it.
        a.echo_prompt("/simplify the parser");
        // A command that isn't a skill is not counted.
        a.echo_prompt("/compact");
        // Called by the model.
        a.on_event(UiEvent::Engine(kodact_engine::EngineEvent::Assistant {
            message: kodact_types::ApiMessage {
                id: "m".into(),
                kind: "message".into(),
                role: kodact_types::Role::Assistant,
                model: "m".into(),
                content: vec![kodact_types::ContentBlock::ToolUse {
                    id: "s1".into(),
                    name: "Skill".into(),
                    input: serde_json::json!({"skill": "verify"}),
                    cache_control: None,
                    provider: None,
                }],
                stop_reason: Some(kodact_types::StopReason::ToolUse),
                stop_sequence: None,
                usage: kodact_types::Usage::default(),
            },
            uuid: "u".into(),
            parent_tool_use_id: None,
        }));
        assert_eq!(a.skills_used, vec![("verify".to_string(), 1), ("simplify".to_string(), 1)]);
    }

    #[test]
    fn permission_dialog_scrolls_long_preview() {
        let mut a = app(false);
        let old = (1..=30).map(|i| format!("old line {i:02}")).collect::<Vec<_>>().join("\n");
        let new = (1..=30).map(|i| format!("new line {i:02}")).collect::<Vec<_>>().join("\n");
        ask(
            &mut a,
            "Edit",
            json!({
                "file_path": "/p/src/lib.rs",
                "old_string": old,
                "new_string": new,
            }),
        );
        let rows_top = draw(&live_view(&a, 80, 20), 80).0.join("\n");
        assert!(rows_top.contains("old line 01"), "top of diff visible initially:\n{rows_top}");
        assert!(rows_top.contains("PgUp/PgDn or wheel"), "scroll hint shown:\n{rows_top}");
        assert!(rows_top.contains("1. Allow"), "options stay visible:\n{rows_top}");

        a.on_key(crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::PageDown,
            crossterm::event::KeyModifiers::NONE,
        ));
        let rows_paged = draw(&live_view(&a, 80, 20), 80).0.join("\n");
        assert!(!rows_paged.contains("old line 01"), "top scrolled away after PgDn:\n{rows_paged}");
        assert!(rows_paged.contains("1. Allow"), "options stay pinned while scrolling:\n{rows_paged}");

        assert!(a.scroll_dialog(-20));
        let rows_back = draw(&live_view(&a, 80, 20), 80).0.join("\n");
        assert!(rows_back.contains("old line 01"), "scrolling back up restores top:\n{rows_back}");
    }

    #[test]
    fn frame_dialogs_stay_bounded_and_highlight_bash_commands() {
        let mut a = app(true);
        a.look = crate::tui::look::Look::Frame;
        ask(&mut a, "Bash", json!({"command": "cargo test --workspace", "description": "run all checks"}));
        let (rows, buf) = draw(&live_view(&a, 180, 30), 180);
        let top = rows[0].trim_end();
        let top_w = text_width(top);
        assert!(top.starts_with("┌─") && top.ends_with("─┐"), "{top}");
        assert!((60..=96).contains(&top_w), "wide terminal dialog must stay compact (got {top_w}): {top}");
        let middle = top.strip_prefix("┌─").and_then(|s| s.strip_suffix("─┐")).expect("corners present");
        assert!(middle.chars().all(|c| c == '─'), "corners must join via hairline rule: {top}");
        assert_eq!(buf[(0, 0)].fg, a.theme.accent().fg.unwrap());
        assert_eq!(buf[(3, 0)].fg, a.theme.hairline().fg.unwrap());

        let cmd_y = rows.iter().position(|r| r.contains("cargo test --workspace")).expect("command row") as u16;
        let cmd_x = rows[cmd_y as usize].find("cargo test").unwrap() as u16;
        let prompt_bg = crate::tui::palette::PROMPT_BG.color(a.theme.depth, a.theme.light);
        assert_eq!(buf[(cmd_x, cmd_y)].bg, prompt_bg, "Bash command gets elevated PROMPT_BG");
        let in_y = rows.iter().position(|r| r.contains("in /p")).expect("folder row") as u16;
        let in_x = rows[in_y as usize].find("in /p").unwrap() as u16;
        assert_ne!(buf[(in_x, in_y)].bg, prompt_bg, "folder line stays subtle metadata");
        assert_eq!(buf[(in_x, in_y)].fg, a.theme.subtle().fg.unwrap());

        // Very long Bash commands wrap at <= 96 cols instead of stretching to col 180.
        ask(&mut a, "Bash", json!({"command": format!("echo {}", "a".repeat(140))}));
        let (rows, _) = draw(&live_view(&a, 180, 30), 180);
        let max_w = rows.iter().map(|r| text_width(r.trim_end())).max().unwrap();
        assert!(max_w <= 96, "wrapped frame dialog exceeded 96 cols: {max_w}");

        // Degenerate narrow terminal never panics or exceeds w.
        let tiny = cornered(Line::from("Title"), vec![Line::from("Body")], 3, a.theme.accent(), a.theme.hairline());
        assert!(tiny.iter().all(|l| text_width(&text::plain(l)) <= 3));
    }

    #[test]
    fn the_first_frame_offers_the_cards_it_draws() {
        let mut a = app(false);
        a.look = crate::tui::look::Look::Frame;
        a.front = true;
        let page = crate::tui::front::front_page(&a.theme, a.look, &Default::default(), 120);
        // The very first frame: the cards are drawn, and the status line says how to reach them.
        let (v, _) = full_view(&a, 120, 40, &[], &page, 0);
        let status = draw(&v, 120).0.pop().unwrap();
        assert!(a.cards_drawn.get() && status.contains("suggestions"), "{status}");
        // Too short for them: neither the cards nor the hint.
        let (v, _) = full_view(&a, 120, 16, &[], &page, 0);
        let status = draw(&v, 120).0.pop().unwrap();
        assert!(!a.cards_drawn.get() && !status.contains("suggestions"), "{status}");
    }

    #[test]
    fn contextual_hints_adapt_to_session_state() {
        let mut a = app(false);
        a.look = crate::tui::look::Look::Frame;
        let status_row = |app: &App, w: u16| draw(&live_view(app, w, 12), w).0.pop().unwrap();

        // Fresh idle session: base navigation without find/fold/copy clutter.
        let fresh = status_row(&a, 140).split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(fresh.contains("/ commands") && fresh.contains("@ files") && fresh.contains("shift+tab mode"));
        assert!(!fresh.contains("ctrl+f find") && !fresh.contains("ctrl+o") && !fresh.contains("alt+c copy code"));

        // Active transcript with collapsible thoughts and code fences surfaces contextual keys.
        a.talked = true;
        a.has_collapsible = true;
        a.has_code = true;
        let rich = status_row(&a, 140).split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(rich.contains("ctrl+f find"), "{rich}");
        assert!(rich.contains("ctrl+o thoughts"), "{rich}");
        assert!(rich.contains("alt+c copy code"), "{rich}");

        // Verbose toggle flips the ctrl+o hint label from `thoughts` to `less`.
        a.verbose = true;
        let expanded = status_row(&a, 140).split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(expanded.contains("ctrl+o less"), "{expanded}");

        // Find mode surfaces match navigation hints.
        a.find = Some(crate::tui::app::Find { query: "foo".into(), ..Default::default() });
        let find_row = status_row(&a, 140).split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(find_row.contains("enter next") && find_row.contains("↑↓ match") && find_row.contains("esc close"));
        a.find = None;

        // Busy state keeps `esc` once while surfacing `ctrl+o` and `shift+tab` when space allows.
        a.busy = true;
        a.busy_since = Some(Instant::now());
        a.verbose = false;
        let busy_row = status_row(&a, 140).split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(busy_row.matches("esc").count(), 1, "{busy_row}");
        assert!(busy_row.contains("ctrl+o thoughts") && busy_row.contains("shift+tab mode"), "{busy_row}");

        a.queued.push_back("next prompt".into());
        let queued_row = status_row(&a, 140).split_whitespace().collect::<Vec<_>>().join(" ");
        assert!(
            queued_row.contains("enter force next")
                && queued_row.contains("take back")
                && queued_row.contains("ctrl+o thoughts"),
            "{queued_row}"
        );
    }

    #[test]
    fn scroll_footer_shows_percentage_and_fits_narrow_widths() {
        let mut a = app(false);
        a.front = false;
        let page = FrontPage { box_left: 0, box_w: 100, ..Default::default() };
        let transcript: Vec<Line<'static>> = (1..=80).map(|i| Line::from(format!("line {i}"))).collect();

        // Scroll halfway up (end = 40 of 80 -> 50%).
        let (v, _) = full_view(&a, 100, 30, &transcript, &page, 40);
        let footer = draw(&v, 100).0.into_iter().find(|r| r.contains("more lines")).unwrap();
        assert_eq!(footer.trim(), "↑ 50% · ↓ 40 more lines · wheel / PgDn / End bottom");

        // Single line scroll uses singular "line".
        let (v1, _) = full_view(&a, 100, 30, &transcript, &page, 1);
        let footer1 = draw(&v1, 100).0.into_iter().find(|r| r.contains("more line")).unwrap();
        assert_eq!(footer1.trim(), "↑ 98% · ↓ 1 more line · wheel / PgDn / End bottom");

        // Narrow 40-col screen falls back cleanly while keeping the End shortcut visible.
        let (vn, _) = full_view(&a, 40, 20, &transcript, &page, 40);
        let footern = draw(&vn, 40).0.into_iter().find(|r| r.contains("End bottom")).unwrap();
        assert!(text_width(footern.trim_end()) <= 40, "{footern}");
        assert!(footern.contains("↑ 50%") && footern.contains("↓ 40"));
    }

    #[test]
    fn busy_and_scrolled_views_separate_history_from_live_task() {
        let mut a = app(false);
        a.front = false;
        let page = FrontPage { box_left: 0, box_w: 80, ..Default::default() };
        let transcript: Vec<Line<'static>> = (1..=40).map(|i| Line::from(format!("line {i}"))).collect();

        // Busy view draws the labelled `◆ working` divider between history and live task.
        a.busy = true;
        a.busy_since = Some(Instant::now());
        let (vb, _) = full_view(&a, 80, 24, &transcript, &page, 0);
        let rows_b = draw(&vb, 80).0;
        assert!(rows_b.iter().any(|r| r.contains("── ◆ working ─")), "{rows_b:?}");

        // Scrolled idle view draws a full-width hairline divider below the scroll footer.
        a.busy = false;
        let (vs, _) = full_view(&a, 80, 24, &transcript, &page, 10);
        let rows_s = draw(&vs, 80).0;
        assert!(rows_s.iter().any(|r| r == &"─".repeat(80)), "{rows_s:?}");
    }
}
