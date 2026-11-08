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
use super::palette;
use super::text::{self, width as text_width};

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
const MENU_ROWS: usize = 8;
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

/// Split one line of text into rows of at most `w` columns, by character.
/// Returns the rows' character ranges.
fn char_rows(chars: &[char], w: usize) -> Vec<(usize, usize)> {
    let w = w.max(1);
    let mut rows = vec![];
    let mut start = 0;
    let mut used = 0;
    for (i, c) in chars.iter().enumerate() {
        let cw = c.width().unwrap_or(0);
        if used + cw > w && i > start {
            rows.push((start, i));
            start = i;
            used = 0;
        }
        used += cw;
    }
    rows.push((start, chars.len()));
    rows
}

fn status_left(app: &App, now: Instant) -> (String, Style) {
    if let Some((h, at)) = &app.hint {
        if now.duration_since(*at) < HINT_FOR {
            return (h.clone(), app.theme.dim());
        }
    }
    let t = app.theme;
    // The default style's box names the mode, so the line keeps the folder.
    let mode = if app.look == super::look::Look::Frame { "" } else { app.status.mode.as_str() };
    match mode {
        "acceptEdits" => (format!("{} accept edits on (shift+tab to cycle)", glyphs::MODE_ACCEPT_EDITS), t.warning()),
        "plan" => (format!("{} plan mode on (shift+tab to cycle)", glyphs::MODE_PLAN), t.success()),
        "bypassPermissions" => (format!("{} bypass permissions on", glyphs::MODE_BYPASS), t.error()),
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
fn guess(s: &StatusView) -> &'static str {
    if s.window_guessed {
        "~"
    } else {
        ""
    }
}

/// What the session has used, in tokens (nothing before the first request).
/// The UI shows no money: `/cost` and `/usage` give the session's cost when asked.
fn spend(app: &App) -> Option<String> {
    Some(app.status.session_tokens).filter(|n| *n > 0).map(|n| format!("{} used", tokens(n)))
}

/// `s` in at most `max` columns, its end kept: `…/demo/proj`.
fn keep_end(s: &str, max: usize) -> String {
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

/// A permission mode as people say it.
pub(super) fn mode_name(mode: &str) -> String {
    match mode {
        "acceptEdits" => "Accept edits",
        "plan" => "Plan",
        "auto" => "Auto",
        "dontAsk" => "Don't ask",
        "bypassPermissions" => "Bypass permissions",
        _ => "Default",
    }
    .to_string()
}

/// A token count as people read it: `950 tokens`, `15.5k tokens`, `1.2M tokens`.
fn tokens(n: u64) -> String {
    match n {
        0..=999 => format!("{n} tokens"),
        1_000..=999_999 => format!("{:.1}k tokens", n as f64 / 1000.0).replace(".0k", "k"),
        _ => format!("{:.1}M tokens", n as f64 / 1_000_000.0).replace(".0M", "M"),
    }
}

/// A rounded box around `body` rows, `w` columns wide.
fn boxed(title: Line<'static>, body: Vec<Line<'static>>, w: usize, border: Style) -> Vec<Line<'static>> {
    let inner = w.saturating_sub(4).max(1);
    let mut out = vec![Line::from(Span::styled(format!("╭{}╮", "─".repeat(w.saturating_sub(2))), border))];
    for l in std::iter::once(title).chain(body) {
        for row in text::wrap(vec![l], inner as u16) {
            let used: usize = row.spans.iter().map(|s| text_width(&s.content)).sum();
            let mut spans = vec![Span::styled("│ ", border)];
            spans.extend(row.spans);
            spans.push(Span::raw(" ".repeat(inner.saturating_sub(used))));
            spans.push(Span::styled(" │", border));
            out.push(Line::from(spans));
        }
    }
    out.push(Line::from(Span::styled(format!("╰{}╯", "─".repeat(w.saturating_sub(2))), border)));
    out
}

/// `body` rows between lime corner marks, `w` columns wide: the frame
/// style's way of setting a question apart.
fn cornered(title: Line<'static>, body: Vec<Line<'static>>, w: usize, mark: Style) -> Vec<Line<'static>> {
    let inner = w.saturating_sub(4).max(1);
    let edge = |l: &str, r: &str| {
        Line::from(vec![
            Span::styled(l.to_string(), mark),
            Span::raw(" ".repeat(w.saturating_sub(4))),
            Span::styled(r.to_string(), mark),
        ])
    };
    let mut out = vec![edge("┌─", "─┐")];
    for l in std::iter::once(title).chain(body) {
        for row in text::wrap(vec![l], inner as u16) {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(row.spans);
            out.push(Line::from(spans));
        }
    }
    out.push(edge("└─", "─┘"));
    out
}

/// A screen's spans, styled.
fn screen_spans(app: &App, spans: &[kodact_core::commands::screens::Span]) -> Vec<Span<'static>> {
    use kodact_core::commands::screens::{context_parts, Tone};
    let t = app.theme;
    spans
        .iter()
        .map(|s| {
            let (text, st) = match s.tone {
                Tone::Plain => (s.text.clone(), Style::default()),
                Tone::Dim => (s.text.clone(), t.dim()),
                Tone::Bold => (s.text.clone(), t.bold()),
                Tone::Accent => (s.text.clone(), t.accent()),
                Tone::Added => (s.text.clone(), t.success()),
                Tone::Removed => (s.text.clone(), t.error()),
                // Without colour, each part has its own letter.
                Tone::Part(p) if !t.color => {
                    let glyph = context_parts().get(p as usize).map(|x| x.1).unwrap_or('?');
                    (s.text.replace(['⛁', '⛶'], &glyph.to_string()), Style::default())
                }
                Tone::Part(p) => (s.text.clone(), t.part(p)),
            };
            Span::styled(text, st)
        })
        .collect()
}

/// Rows a screen's box takes besides its rows: borders, title, blank, footer.
const SCREEN_CHROME: usize = 5;

/// A screen, at most `room` rows tall. Rows are cut to the width, never wrapped, so
/// scrolling counts rows exactly.
fn viewer(app: &App, w: usize, room: usize) -> Vec<Line<'static>> {
    let t = app.theme;
    let Some((screen, cursor, top)) = app.viewer() else { return vec![] };
    let page = room.saturating_sub(SCREEN_CHROME).max(1);
    app.viewer_page.set(page);
    let inner = w.saturating_sub(6).max(1);
    // The highlighted row stays in view even if the terminal shrank.
    let top = top.min(cursor).max((cursor + 1).saturating_sub(page));
    // A screen with nothing to choose scrolls; it highlights no row.
    let pick = screen.rows.iter().any(|r| r.action.is_some());
    let cursor = if pick { cursor } else { usize::MAX };
    let mut body = vec![];
    for (i, row) in screen.rows.iter().enumerate().skip(top).take(page) {
        let mark = if i == cursor { "❯ " } else { "  " };
        let mut spans = vec![Span::styled(mark, t.accent())];
        let mut used = 0;
        for sp in screen_spans(app, &row.spans) {
            let room = inner.saturating_sub(used);
            if room == 0 {
                break;
            }
            let text = fit(&sp.content, room);
            used += text_width(&text);
            // The highlighted row keeps its colours (diff sides, context parts) and turns bold.
            let st = if i == cursor { sp.style.add_modifier(ratatui::style::Modifier::BOLD) } else { sp.style };
            spans.push(Span::styled(text, st));
        }
        body.push(Line::from(spans));
    }
    let n = screen.rows.len();
    let shown = if n > page { format!("{}-{} of {n} · ", top + 1, (top + page).min(n)) } else { String::new() };
    let hint = match screen.rows.get(cursor).and_then(|r| r.action.as_ref()) {
        Some(kodact_core::commands::screens::RowAction::Jump(_)) => "Enter open · ",
        Some(_) => "Enter choose · ",
        None => "",
    };
    body.push(Line::default());
    body.push(Line::from(Span::styled(
        fit(&format!("{shown}↑↓ PgUp PgDn · {hint}Esc back"), w.saturating_sub(4)),
        t.dim(),
    )));
    boxed(Line::from(Span::styled(fit(&screen.title, w.saturating_sub(4)), t.bold())), body, w, t.accent())
}

/// A form: one row per field, the command it will run, the keys.
fn form(app: &App, w: usize) -> Vec<Line<'static>> {
    use kodact_core::commands::screens::FieldKind;
    let t = app.theme;
    let Some((form, at)) = app.form() else { return vec![] };
    let pad = form.fields.iter().map(|f| text_width(&f.label)).max().unwrap_or(0);
    let mut body = vec![];
    for (i, f) in form.fields.iter().enumerate() {
        let here = i == at;
        let mut spans = vec![
            Span::styled(if here { "❯ " } else { "  " }, t.accent()),
            Span::styled(format!("{:<pad$}  ", f.label), if here { t.bold() } else { Style::default() }),
        ];
        match &f.kind {
            FieldKind::Text(v) => {
                spans.push(Span::raw(v.clone()));
                if here {
                    spans.push(Span::styled("▏", t.accent()));
                }
            }
            FieldKind::Choice { options, at: c } => {
                let v = options.get(*c).cloned().unwrap_or_default();
                spans.push(Span::styled(if here { format!("‹ {v} ›") } else { v }, Style::default()));
            }
            FieldKind::Multi { options, picked, at: c } => {
                for (j, (o, p)) in options.iter().zip(picked).enumerate() {
                    let st = if here && j == *c { t.selected() } else { Style::default() };
                    spans.push(Span::styled(format!("[{}] {o}", if *p { "x" } else { " " }), st));
                    spans.push(Span::raw(" "));
                }
            }
        }
        body.push(Line::from(spans));
    }
    body.push(Line::default());
    body.push(Line::from(Span::styled(format!("Runs: {}", form.command()), t.dim())));
    body.push(Line::from(Span::styled("Tab next field · ←→ choose · Space toggle · Enter run · Esc cancel", t.dim())));
    boxed(Line::from(Span::styled(form.title.clone(), t.bold())), body, w, t.accent())
}

fn dialog(app: &App, w: usize, room: usize) -> Vec<Line<'static>> {
    if app.viewer().is_some() {
        return viewer(app, w, room);
    }
    if app.form().is_some() {
        return form(app, w);
    }
    let t = app.theme;
    let (title, body) = app.dialog_text();
    // A proposed edit's lines: removed in red, added in green.
    let mut rows: Vec<Line<'static>> = body
        .into_iter()
        .map(|l| {
            if l.starts_with("  - ") {
                Line::from(Span::styled(l, t.removed()))
            } else if l.starts_with("  + ") {
                Line::from(Span::styled(l, t.added()))
            } else {
                Line::from(l)
            }
        })
        .collect();
    // What a destructive command would do, in red, with a glyph and a word.
    if let Some(danger) = app.dialog_danger() {
        rows.push(Line::default());
        rows.push(Line::from(vec![
            Span::styled("▲ ", t.error()),
            Span::styled(format!("This {danger}. It can't be undone."), t.error()),
        ]));
    }
    rows.push(Line::default());
    let selected = app.dialog_selected();
    for (i, (label, desc)) in app.dialog_options().into_iter().enumerate() {
        let mark = if i == selected { "❯ " } else { "  " };
        let st = if i == selected { t.selected() } else { Style::default() };
        let mut spans = vec![Span::styled(format!("{mark}{}. {label}", i + 1), st)];
        if !desc.is_empty() {
            spans.push(Span::styled(format!("  {desc}"), t.dim()));
        }
        rows.push(Line::from(spans));
    }
    rows.push(Line::default());
    let options = app.dialog_options();
    let permission = options.first().is_some_and(|(_, k)| k == "y");
    // The keys the rows show, and no others.
    let keys: Vec<&str> = options.iter().map(|(_, k)| k.as_str()).filter(|k| k.len() == 1).collect();
    let hint = if !permission {
        "Enter to select · Esc to cancel".to_string()
    } else if app.dialog_danger().is_some() {
        let always = if keys.contains(&"a") { " · a always" } else { "" };
        format!("y to allow{always} · Enter, n or Esc to deny")
    } else {
        format!("Enter or {} to answer · Esc to deny", keys.join("·"))
    };
    rows.push(Line::from(Span::styled(hint, t.dim())));
    let title = if permission {
        Line::from(vec![Span::styled("◆ ", t.accent()), Span::styled(title, t.bold())])
    } else {
        Line::from(Span::styled(title, t.bold()))
    };
    use super::look::Look;
    match app.look {
        // Lime corner marks around the question, no full frame.
        Look::Frame => cornered(title, rows, w, t.accent()),
        // A centred sheet, narrower than the terminal.
        Look::Status if w >= 60 => {
            let bw = w.min(76);
            let pad = " ".repeat((w - bw) / 2);
            boxed(title, rows, bw, t.accent())
                .into_iter()
                .map(|l| {
                    let mut spans = vec![Span::raw(pad.clone())];
                    spans.extend(l.spans);
                    Line::from(spans)
                })
                .collect()
        }
        // No box: a rule, then the question.
        Look::Bare => {
            let mut out = vec![Line::from(Span::styled("─".repeat(w), t.hairline()))];
            out.extend(text::wrap(std::iter::once(title).chain(rows).collect(), w as u16));
            out
        }
        _ => boxed(title, rows, w, t.accent()),
    }
}

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
        let chars: Vec<char> = line.chars().collect();
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
            rows.push(Line::from(vec![prefix, Span::raw(chars[*a..*b].iter().collect::<String>())]));
        }
    }
    (rows, cursor)
}

/// The keys that help now, for the frame style's status line: while the model
/// works, how to stop it; in a menu, how to choose; with nothing typed, the
/// commands, files and modes; while typing, how to send.
fn hints(app: &App, room: usize) -> Vec<Span<'static>> {
    let t = app.theme;
    let keys: &[(&str, &str)] = if app.dialog.is_some() {
        // The dialog says its own keys.
        &[]
    } else if app.busy {
        &[("esc", "stop"), ("enter", "queue a message")]
    } else if !app.menu().is_empty() || !app.file_menu().is_empty() {
        &[("↑↓", "choose"), ("tab", "complete"), ("esc", "close")]
    } else if app.editor.is_empty() && app.status.mode == "plan" {
        // Plan mode: how to leave it and act on the plan.
        &[("shift+tab", "leave plan mode"), ("/", "commands"), ("?", "keys")]
    } else if app.editor.is_empty() {
        &[("/", "commands"), ("@", "files"), ("shift+tab", "mode"), ("?", "keys")]
    } else {
        &[("enter", "send"), ("shift+enter", "new line")]
    };
    let mut out: Vec<Span<'static>> = vec![];
    let mut used = 0;
    for (i, (k, what)) in keys.iter().enumerate() {
        let gap = if i == 0 { "" } else { "   " };
        let w = text_width(gap) + text_width(k) + 1 + text_width(what);
        if used + w > room {
            break;
        }
        used += w;
        out.push(Span::raw(gap));
        out.push(Span::styled(k.to_string(), if t.color { Style::default() } else { t.bold() }));
        out.push(Span::styled(format!(" {what}"), t.subtle()));
    }
    out
}

/// The `@` menu: matching paths.
fn file_menu(app: &App, w: usize) -> Vec<Line<'static>> {
    let t = app.theme;
    let items = app.file_menu();
    if items.is_empty() {
        return vec![];
    }
    let sel = app.menu_selected.min(items.len() - 1);
    let first = sel.saturating_sub(MENU_ROWS - 1);
    items[first..items.len().min(first + MENU_ROWS)]
        .iter()
        .enumerate()
        .map(|(i, f)| {
            let st = if first + i == sel { t.selected() } else { Style::default() };
            Line::from(Span::styled(fit(&format!("  {f}"), w), st))
        })
        .collect()
}

fn menu(app: &App, w: usize) -> Vec<Line<'static>> {
    let t = app.theme;
    let items = app.menu();
    if items.is_empty() {
        return file_menu(app, w);
    }
    let sel = app.menu_selected.min(items.len() - 1);
    let first = sel.saturating_sub(MENU_ROWS - 1);
    let shown = &items[first..items.len().min(first + MENU_ROWS)];
    let label = |c: &super::app::CommandInfo| {
        if c.args.is_empty() {
            format!("/{}", c.name)
        } else {
            format!("/{} {}", c.name, c.args)
        }
    };
    let pad = shown.iter().map(|c| text_width(&label(c))).max().unwrap_or(0).min(w / 2);
    shown
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let l = fit(&label(c), pad);
            let l = format!("  {l}{}", " ".repeat(pad.saturating_sub(text_width(&l))));
            let desc = fit(&c.description, w.saturating_sub(text_width(&l) + 2));
            let selected = first + i == sel;
            let st = if selected { t.selected() } else { Style::default() };
            Line::from(vec![Span::styled(l, st), Span::raw("  "), Span::styled(desc, t.dim())])
        })
        .collect()
}

/// KodAct's working shimmer: a soft highlight sweeps across `text`, once
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
fn room_left(s: &StatusView) -> u8 {
    if s.compact_at > 0 {
        let used = s.context_tokens.min(s.compact_at);
        ((s.compact_at - used) * 100 / s.compact_at) as u8
    } else {
        100u8.saturating_sub(s.context_pct.unwrap_or(0))
    }
}

/// The sidebar's narrowest width, columns.
pub const SIDEBAR_W: u16 = 34;
/// The sidebar's widest, on a very wide window.
pub const SIDEBAR_MAX_W: u16 = 46;

/// The sidebar's width on a window `cols` wide: a quarter of it, between
/// [`SIDEBAR_W`] and [`SIDEBAR_MAX_W`], so its panes grow and shrink with the window.
pub fn sidebar_width(cols: u16) -> u16 {
    (cols / 4).clamp(SIDEBAR_W, SIDEBAR_MAX_W)
}

/// A braille cell filled to each of its eight levels, bottom up: the one shape KodAct
/// uses for "how full" (the context) and "in flight" (work that fills and drains).
/// Braille is one column wide in every terminal font, and eight dots give smooth steps.
pub const LEVELS: [&str; 9] = ["⠀", "⡀", "⣀", "⣄", "⣤", "⣦", "⣶", "⣷", "⣿"];

/// The frames of a mark that fills while its work is in flight (a server connecting,
/// a sub-agent working, the plan's step during a turn): it fills from the bottom,
/// dot by dot, and drains again, so it reads as filling, never as done.
pub const FILL: [&str; 14] = ["⡀", "⣀", "⣄", "⣤", "⣦", "⣶", "⣷", "⣿", "⣷", "⣶", "⣦", "⣤", "⣄", "⣀"];

/// The time between frames of a filling mark.
pub const FILL_STEP: Duration = Duration::from_millis(90);

/// A filling mark, `ran` after its work began: each keeps its own phase, so several
/// in flight never pulse in step.
pub fn filling(ran: Duration) -> &'static str {
    FILL[fill_frame(ran)]
}

fn fill_frame(ran: Duration) -> usize {
    (ran.as_millis() / FILL_STEP.as_millis()) as usize % FILL.len()
}

/// A filling mark's colour breathes with it: dim while nearly empty, full when full,
/// in 24-bit colour; elsewhere the colour as it is.
pub fn breathing(t: &text::Theme, base: Style, ran: Duration) -> Style {
    let frame = fill_frame(ran);
    let level = if frame <= 7 { frame + 1 } else { 15 - frame }; // 1..=8
    match (t.color && t.depth == palette::Depth::True, base.fg) {
        (true, Some(ratatui::style::Color::Rgb(r, g, b))) => {
            let bg = palette::PROMPT_BG.color(t.depth, t.light);
            let (br, bgc, bb) = match bg {
                ratatui::style::Color::Rgb(r, g, b) => (r, g, b),
                _ => (0, 0, 0),
            };
            let k = 0.35 + 0.65 * level as f32 / 8.0;
            let mix = |c: u8, under: u8| (under as f32 + (c as f32 - under as f32) * k).round() as u8;
            base.fg(ratatui::style::Color::Rgb(mix(r, br), mix(g, bgc), mix(b, bb)))
        }
        _ => base,
    }
}

/// A bar `cells` wide that fills left to right, each cell dot by dot: `pct` of it in
/// `tone`, the rest a faint track.
pub fn fill_bar(t: &text::Theme, pct: u8, cells: usize, tone: Style) -> Vec<Span<'static>> {
    let dots = (pct.min(100) as usize * cells * 8 + 50) / 100;
    let mut lit = String::new();
    let mut track = String::new();
    for i in 0..cells {
        let k = dots.saturating_sub(i * 8).min(8);
        if k == 0 {
            track.push('⣀');
        } else {
            lit.push_str(LEVELS[k]);
        }
    }
    vec![Span::styled(lit, tone), Span::styled(track, t.hairline())]
}

/// How long something has run, for a glance: `41s`, `2m 05s`, `1h 02m`.
fn ran_for(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, s % 3600 / 60),
    }
}

/// When the UI started: the phase of marks whose work has no start of its own.
fn epoch() -> Instant {
    static AT: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    *AT.get_or_init(Instant::now)
}

/// Sub-agents shown in the sidebar at most; the rest are counted.
const SIDEBAR_AGENTS: usize = 4;

/// Skills shown in the sidebar at most, the most recently used first; the rest are counted.
const SIDEBAR_SKILLS: usize = 5;

/// The sidebar, `w` columns by `h` rows, `now` for the marks that fill. Its
/// sections keep one order, so the eye learns where things are: the session's
/// title, its context, the plan, the MCP servers and the tools each brings, the
/// skills (how many, and the ones this session used), the sub-agents at work,
/// with the folder and the branch under the title. Marks that fill tell work in flight
/// from work done, and every state has a word or a shape as well as a colour. On a
/// short window the lowest sections give way first; the plan never does.
pub fn sidebar(app: &App, w: usize, h: usize, now: Instant) -> Vec<Line<'static>> {
    let t = app.theme;
    let s = &app.status;
    // Inside a pane: its border and a space on each side.
    let inner = w.saturating_sub(4).max(1);
    let width_of = |spans: &[Span<'static>]| spans.iter().map(|s| text_width(&s.content)).sum::<usize>();
    // A section's heading, with what it counts flush right: `MCP          1 failed`.
    let heading = |name: &str, right: Vec<Span<'static>>| -> Line<'static> {
        let gap = inner.saturating_sub(text_width(name) + width_of(&right)).max(1);
        let mut spans = vec![Span::styled(name.to_string(), t.subtle()), Span::raw(" ".repeat(gap))];
        spans.extend(right);
        Line::from(spans)
    };
    // A row: its mark, a name, and a quiet note flush right (`12 tools`, `failed`, `×2`).
    let row = |mark: Span<'static>, name: &str, name_style: Style, note: Span<'static>| -> Line<'static> {
        let note_w = text_width(&note.content);
        let name = fit(name, inner.saturating_sub(2 + note_w + usize::from(note_w > 0)));
        let gap = inner.saturating_sub(2 + text_width(&name) + note_w);
        Line::from(vec![mark, Span::raw(" "), Span::styled(name, name_style), Span::raw(" ".repeat(gap)), note])
    };

    let name = if app.title.is_empty() { "New session".to_string() } else { app.title.clone() };
    let title: Vec<Line<'static>> =
        text::wrap(vec![Line::from(Span::styled(name, t.bold()))], inner as u16).into_iter().take(3).collect();

    let mut context = vec![heading("Context", vec![])];
    if s.context_pct.is_some() {
        context.push(Line::from(gauge(&t, s)));
        // The same room as a bar across the sidebar, filling dot by dot.
        let tone = gauge(&t, s).first().map(|sp| sp.style).unwrap_or_default();
        context.push(Line::from(fill_bar(&t, 100 - room_left(s), inner, tone)));
    }
    if s.session_tokens > 0 {
        context.push(Line::from(Span::styled(format!("{} used", tokens(s.session_tokens)), t.dim())));
    }
    if !s.model.is_empty() {
        context.push(Line::from(Span::styled(fit(&s.model, inner), t.dim())));
    }

    // The plan: done steps quiet with a check, the active one filling in lime while
    // the turn runs (half full while it waits), the rest open. A long step takes the
    // rows it needs while there is room, two when there isn't.
    let plan_rows = |most: usize| -> Vec<Line<'static>> {
        let mut plan = vec![];
        if app.todos.is_empty() {
            return plan;
        }
        let done = app.todos.iter().filter(|(st, _)| st == "completed").count();
        plan.push(heading("Plan", vec![Span::styled(format!("{done}/{}", app.todos.len()), t.subtle())]));
        let busy_for = app.busy_since.map(|b| now.saturating_duration_since(b));
        for (status, content) in &app.todos {
            let (mark, mark_style, body) = match status.as_str() {
                "completed" => ("✓", t.subtle(), t.subtle()),
                "in_progress" => match busy_for.filter(|_| app.busy) {
                    Some(ran) => (filling(ran), breathing(&t, t.accent(), ran), t.accent()),
                    None => (LEVELS[4], t.accent(), t.accent()),
                },
                _ => ("○", t.dim(), t.dim()),
            };
            let rows =
                text::wrap(vec![Line::from(Span::styled(content.clone(), body))], inner.saturating_sub(2) as u16);
            for (i, r) in rows.into_iter().take(most).enumerate() {
                let mut spans =
                    vec![if i == 0 { Span::styled(format!("{mark} "), mark_style) } else { Span::raw("  ") }];
                spans.extend(r.spans);
                plan.push(Line::from(spans));
            }
        }
        plan
    };

    // The MCP servers: connected full and green with the tools it brings, a failed one
    // a red cross with the word, one still connecting filling, a disabled one hollow.
    let mut mcp = vec![];
    if !app.mcp.is_empty() {
        let failed = app.mcp.iter().filter(|(_, st)| st == "failed").count();
        let summary = if failed > 0 {
            Span::styled(format!("{failed} failed"), t.error())
        } else {
            let up = app.mcp.iter().filter(|(_, st)| st == "connected").count();
            Span::styled(format!("{up}/{} up", app.mcp.len()), t.subtle())
        };
        mcp.push(heading("MCP", vec![summary]));
        for (name, status) in &app.mcp {
            let tools = app.mcp_tools.get(name).copied().unwrap_or(0);
            let (mark, mark_style, note, note_style) = match status.as_str() {
                "connected" if tools > 0 => {
                    ("●", t.success(), format!("{tools} tool{}", if tools == 1 { "" } else { "s" }), t.dim())
                }
                "connected" => ("●", t.success(), String::new(), t.dim()),
                "failed" => ("×", t.error(), "failed".to_string(), t.error()),
                "pending" => {
                    let ran = now.saturating_duration_since(epoch());
                    (filling(ran), breathing(&t, t.warning(), ran), "connecting".into(), t.subtle())
                }
                "disabled" => ("○", t.subtle(), "off".to_string(), t.subtle()),
                other => ("○", t.subtle(), other.to_string(), t.subtle()),
            };
            mcp.push(row(Span::styled(mark, mark_style), name, Style::default(), Span::styled(note, note_style)));
        }
    }

    // The skills: how many, and how to list them, then the ones used this session,
    // the latest first (all of them while there is room); the whole list is one
    // command away.
    let skill_rows = |most: usize| -> Vec<Line<'static>> {
        let mut skills = vec![];
        if app.skills.is_empty() && app.skills_used.is_empty() {
            return skills;
        }
        skills.push(heading(
            "Skills",
            vec![Span::styled(format!("{} · ", app.skills.len()), t.subtle()), Span::styled("/skills", t.dim())],
        ));
        for (name, times) in app.skills_used.iter().take(most) {
            let note = if *times > 1 { Span::styled(format!("×{times}"), t.subtle()) } else { Span::raw("") };
            skills.push(row(Span::styled("▸", t.subtle()), name, Style::default(), note));
        }
        if app.skills_used.len() > most {
            skills.push(Line::from(Span::styled(format!("  {} more used", app.skills_used.len() - most), t.subtle())));
        }
        skills
    };

    // The sub-agents at work, while any run, so delegated work stays in view after the
    // conversation has scrolled past it: each filling in lime with what it was asked,
    // then its kind and its steps. While there is room, how long it has run and its
    // latest steps as a small tree; when there isn't, only the latest step.
    let working: Vec<&super::app::Running> = app.running.iter().filter(|r| is_agent(&r.name)).collect();
    let agent_rows = |full: bool| -> Vec<Line<'static>> {
        let mut agents = vec![];
        if working.is_empty() {
            return agents;
        }
        agents.push(heading("Agents", vec![Span::styled(format!("{} running", working.len()), t.subtle())]));
        for r in working.iter().take(SIDEBAR_AGENTS) {
            let kind =
                r.input.get("subagent_type").and_then(|v| v.as_str()).filter(|k| !k.is_empty()).unwrap_or("Agent");
            let asked = r.input.get("description").and_then(|v| v.as_str()).map(str::trim).filter(|d| !d.is_empty());
            let ran = now.saturating_duration_since(r.since);
            let steps = format!("{} step{}", r.steps, if r.steps == 1 { "" } else { "s" });
            agents.push(row(
                Span::styled(filling(ran), breathing(&t, t.accent(), ran)),
                asked.unwrap_or(kind),
                Style::default(),
                Span::styled(steps, t.subtle()),
            ));
            // With room: its kind and how long it has run, then the tree. Without: the
            // latest step alone, which says the most in one row.
            let second = if full {
                format!("{kind} · {}", ran_for(ran))
            } else {
                r.step.clone().unwrap_or_else(|| format!("{kind} · starting"))
            };
            agents.push(Line::from(Span::styled(fit(&format!("  {second}"), inner), t.subtle())));
            if full {
                let last = &r.trail[r.trail.len().saturating_sub(3)..];
                for (i, step) in last.iter().enumerate() {
                    let branch = if i + 1 == last.len() { "  └ " } else { "  ├ " };
                    agents.push(Line::from(vec![
                        Span::styled(branch, t.hairline()),
                        Span::styled(fit(step, inner.saturating_sub(4)), t.dim()),
                    ]));
                }
            }
        }
        if working.len() > SIDEBAR_AGENTS {
            agents.push(Line::from(Span::styled(format!("  {} more", working.len() - SIDEBAR_AGENTS), t.subtle())));
        }
        agents
    };

    // The folder and the branch, in the session's pane under its title: where the
    // session is, read with what it is. A long folder loses whole folders from its start.
    let branch = if app.branch.is_empty() { String::new() } else { format!("  ⎇ {}", fit(&app.branch, 16)) };
    let folder = keep_end(&super::short_path(&s.cwd), inner.saturating_sub(text_width(&branch)).max(8));
    let place = Line::from(vec![Span::styled(folder, t.subtle()), Span::styled(branch, t.subtle())]);
    // A blank row under the panes.
    let room = h.saturating_sub(1);
    // Each section as it shows with room to spare, and as it shows when space is short.
    // The sidebar fills the window: every section at its fullest while they all fit.
    // When they don't, the lowest sections tighten first, then give way, lowest first;
    // the plan tightens but never gives way.
    struct Section {
        full: Vec<Line<'static>>,
        tight: Vec<Line<'static>>,
        tightened: bool,
        gives_way: bool,
        /// Its first line is a heading, set into the pane's top border.
        named: bool,
    }
    let section = |full: Vec<Line<'static>>, tight: Vec<Line<'static>>, gives_way: bool| Section {
        tightened: false,
        full,
        tight,
        gives_way,
        named: true,
    };
    let mut sections: Vec<Section> = vec![
        Section {
            named: false,
            ..section(
                title.iter().cloned().chain(std::iter::once(place.clone())).collect(),
                title.into_iter().take(1).chain(std::iter::once(place)).collect(),
                true,
            )
        },
        section(context.clone(), context, true),
        section(plan_rows(usize::MAX), plan_rows(2), false),
        section(mcp.clone(), mcp, true),
        section(skill_rows(usize::MAX), skill_rows(SIDEBAR_SKILLS), true),
        section(agent_rows(true), agent_rows(false), true),
    ];
    sections.retain(|s| !s.full.is_empty());
    // A pane's height: its rows (the heading goes into the top border) and its two borders;
    // a blank row between panes and one above the first.
    let rows_of = |s: &Section| {
        let n = if s.tightened { s.tight.len() } else { s.full.len() };
        (n - usize::from(s.named)).max(1) + 2
    };
    // Short of room, first the blank rows between panes go, then sections tighten, then
    // they give way, lowest first.
    let mut gap = 1;
    let height = |ss: &[Section], gap: usize| ss.iter().map(|s| rows_of(s) + gap).sum::<usize>();
    while height(&sections, gap) > room {
        if gap == 1 {
            gap = 0;
        } else if let Some(i) = sections.iter().rposition(|s| !s.tightened && s.tight.len() < s.full.len()) {
            sections[i].tightened = true;
        } else if let Some(i) = sections.iter().rposition(|s| s.gives_way) {
            sections.remove(i);
        } else {
            break;
        }
    }
    // Panes: rounded hairline boxes on the window's own background, each section's name
    // and count set into its top border. A pane shows whole or not at all; if even the
    // plan is taller than the room, its pane keeps its borders and says how many more.
    let border = t.hairline();
    let mut out: Vec<Line<'static>> = vec![];
    let mut left = room;
    for s in sections {
        if left < 3 + gap {
            break;
        }
        let named = s.named;
        let mut lines = if s.tightened { s.tight } else { s.full };
        let head = if named { Some(lines.remove(0)) } else { None };
        // A pane with nothing under its heading says so, rather than an empty box.
        if lines.is_empty() {
            lines.push(Line::from(Span::styled("none used yet", t.subtle())));
        }
        let fits = left - gap - 2;
        if lines.len() > fits {
            let more = lines.len() - (fits - 1);
            lines.truncate(fits - 1);
            lines.push(Line::from(Span::styled(format!("… {more} more"), t.subtle())));
        }
        if gap == 1 {
            out.push(Line::default());
        }
        out.push(pane_top(head, w, border, if named { t.subtle() } else { t.accent() }));
        for l in lines {
            out.push(pane_row(l, w, border));
        }
        out.push(Line::from(Span::styled(format!("╰{}╯", "─".repeat(w.saturating_sub(2))), border)));
        left = room.saturating_sub(out.len());
    }
    out.truncate(room);
    out.resize(room, Line::default());
    out.push(Line::default());
    out
}

/// A pane's top border, `w` wide, with the section's name set into it on the left and
/// what it counts on the right: `╭─ Plan ────────── 1/4 ─╮`. Without a heading (the
/// session's title pane), it is named "session" in the accent colour.
fn pane_top(head: Option<Line<'static>>, w: usize, border: Style, name_style: Style) -> Line<'static> {
    // A heading line is the name, a gap, then the count (see `heading`).
    let (name, right): (Vec<Span<'static>>, Vec<Span<'static>>) = match head {
        Some(l) => {
            let mut spans = l.spans.into_iter();
            let name = spans.next().map(|s| Span::styled(s.content, name_style)).into_iter().collect();
            spans.next(); // the gap
            (name, spans.collect())
        }
        None => (vec![Span::styled("session", name_style)], vec![]),
    };
    let width_of = |v: &[Span<'static>]| v.iter().map(|s| text_width(&s.content)).sum::<usize>();
    let right_w = width_of(&right);
    let mut spans = vec![Span::styled("╭─ ", border)];
    spans.extend(name.iter().cloned());
    spans.push(Span::raw(" "));
    // ╭─ + name + space … space + right + ─╮
    let used = 3 + width_of(&name) + 1 + if right_w > 0 { 1 + right_w + 1 } else { 0 } + 2;
    spans.push(Span::styled("─".repeat(w.saturating_sub(used)), border));
    if right_w > 0 {
        spans.push(Span::raw(" "));
        spans.extend(right);
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled("─╮", border));
    Line::from(spans)
}

/// A row inside a pane `w` wide: the border, a space, the content cut or padded to fit,
/// a space and the border, so every pane edge lines up whatever the content.
fn pane_row(l: Line<'static>, w: usize, border: Style) -> Line<'static> {
    let inner = w.saturating_sub(4);
    let mut used = 0;
    let mut spans = vec![Span::styled("│ ", border)];
    for s in l.spans {
        let sw = text_width(&s.content);
        if used + sw <= inner {
            used += sw;
            spans.push(s);
        } else {
            let cut = fit(&s.content, inner - used);
            used += text_width(&cut);
            spans.push(Span::styled(cut, s.style));
            break;
        }
    }
    spans.push(Span::raw(" ".repeat(inner.saturating_sub(used))));
    spans.push(Span::styled(" │", border));
    Line::from(spans)
}

/// The sidebar has a mark that fills with nothing else to redraw it: a server
/// still connecting while the session is idle.
pub fn sidebar_moving(app: &App) -> bool {
    app.sidebar && !app.front && app.mcp.iter().any(|(_, st)| st == "pending")
}

/// The context gauge: a shape that fills as the conversation nears compaction
/// (`○ ◔ ◑ ◕ ●`), then the room left: `◔ 72% left`. Lime while there is room,
/// amber from 30% left, red from 10%; `~` when the window is a guess.
fn gauge(t: &text::Theme, s: &StatusView) -> Vec<Span<'static>> {
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
    // branch after a lime mark, so where you are reads at a glance.
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
    if app.front && app.dialog.is_none() && !app.busy {
        let fb = FrontBox { left: page.box_left, w: page.box_w, under: page.under };
        let live = live_view_inner(app, width, height, now, Some(fb));
        let mut input = live.lines;
        let status = input.pop().unwrap_or_default();
        let room = h.saturating_sub(1);
        let notes: Vec<Line<'static>> = rows.iter().rev().take(6).rev().cloned().collect();
        let mut out: Vec<Line<'static>> = page.header.clone();
        let box_at;
        if page.dock {
            // The page from the top, the input at the bottom.
            out.extend(page.above.iter().cloned());
            if !notes.is_empty() {
                out.push(Line::default());
                out.extend(notes);
            }
            out.truncate(room.saturating_sub(input.len()));
            out.resize(room.saturating_sub(input.len()), Line::default());
            box_at = out.len();
            out.extend(input);
        } else {
            let mut mid: Vec<Line<'static>> = vec![];
            // The style's art goes first when the screen is too short for it all.
            let fits = page.header.len() + page.above.len() + input.len() + page.below.len() <= room;
            if fits {
                mid.extend(page.above.iter().cloned());
            }
            let at = mid.len();
            mid.extend(input);
            if mid.len() + page.below.len() + page.header.len() <= room {
                mid.extend(page.below.iter().cloned());
            }
            if !notes.is_empty() && page.header.len() + mid.len() + 1 + notes.len() <= room {
                mid.push(Line::default());
                mid.extend(notes);
            }
            let free = room.saturating_sub(page.header.len() + mid.len());
            // Centred down the screen, the empty rows split evenly above and below; or from the top.
            let top = if page.top { free.min(1) } else { free / 2 };
            out.extend(vec![Line::default(); top]);
            box_at = out.len() + at;
            out.extend(mid);
        }
        out.truncate(room);
        out.resize(room, Line::default());
        out.push(status);
        let cursor = live.cursor.map(|(x, y)| (x, y + box_at as u16)).filter(|(_, y)| (*y as usize) < room);
        return (LiveView { lines: out, cursor }, 0);
    }
    let live = live_view_at(app, width, height, now);
    // The status style: a bar of the session's readouts across the top.
    let bar: Vec<Line<'static>> =
        if app.look == super::look::Look::Status { vec![readouts(app, width as usize)] } else { vec![] };
    // The conversation: the transcript down to the live region, with a blank row
    // between its last line and the input so the answer doesn't touch the box.
    let gap = usize::from(!rows.is_empty() && !rows.last().is_some_and(|l| line_blank(l)));
    let avail = h.saturating_sub(live.lines.len() + bar.len() + gap);
    let max_scroll = rows.len().saturating_sub(avail);
    let scroll = scroll.min(max_scroll);
    let end = rows.len() - scroll;
    let mut out: Vec<Line<'static>> = rows[end.saturating_sub(avail)..end].to_vec();
    if scroll > 0 && !out.is_empty() {
        let t = app.theme;
        let note = format!("↓ {scroll} more {} · PgDn", if scroll == 1 { "line" } else { "lines" });
        let pad = (width as usize).saturating_sub(text_width(&note)) / 2;
        let last = out.len() - 1;
        out[last] = Line::from(vec![Span::raw(" ".repeat(pad)), Span::styled(note, t.subtle())]);
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
    lines.extend(std::iter::repeat_n(Line::default(), gap));
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
        if is_agent(&r.name) {
            spans.push(Span::styled(format!("  {}s", ran.as_secs()), t.subtle()));
            if r.steps > 0 {
                let n = if r.steps == 1 { "1 step".to_string() } else { format!("{} steps", r.steps) };
                spans.push(Span::styled(format!(" · {n}"), t.subtle()));
            }
        }
        spinner.push(Line::default());
        spinner.extend(text::wrap(vec![Line::from(spans)], tw as u16).into_iter().take(2));
        // Its latest steps in a small tree under it, so delegated work stays visible.
        if is_agent(&r.name) && !r.trail.is_empty() {
            let tree = super::app::agent_tree(&t, &r.trail, true);
            spinner.extend(tree.into_iter().map(|l| text::wrap(vec![l], tw as u16).swap_remove(0)));
        }
    }
    // A permission question shows its own waiting row: no second line saying so.
    let asked_row = app.asking_for().is_some_and(|id| app.running.iter().any(|r| r.id == id));
    if app.busy && !asked_row {
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
                    "" | "Working" if app.look == super::look::Look::Frame => "Thinking".to_string(),
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
        .map(|q| {
            Line::from(vec![
                Span::styled("queued ▸ ", t.warning()),
                Span::styled(fit(&q.replace('\n', " "), tw.saturating_sub(9)), t.dim()),
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
    // Bare and Ledger draw the input under a rule, without a box; Glow's box glows lime.
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
    let (input_rows, cursor) = if app.dialog.is_none() { input(app, inner) } else { (vec![], (0, 0)) };
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
    // Ctrl+R: what is being searched for, above the input.
    let search: Vec<Line<'static>> = app
        .search
        .as_ref()
        .map(|s| {
            let miss = if s.hit.is_none() && !s.query.is_empty() { " (no match)" } else { "" };
            let pad = 0;
            Line::from(vec![
                Span::raw(" ".repeat(pad)),
                Span::styled(fit(&format!("  search history: {}{miss}", s.query), w.saturating_sub(pad)), t.accent()),
            ])
        })
        .into_iter()
        .collect();
    // 6. The `/` or `@` menu, 7. the status line.
    let menu = menu(app, w);
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
fn is_agent(name: &str) -> bool {
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
    let mode = vec![Span::styled(mode_name(&s.mode), t.mode(&s.mode).add_modifier(ratatui::style::Modifier::BOLD))];
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
        if let Some(e) = endpoint.filter(|e| 5 + text_width(&model) + w_of(e) <= free) {
            left.extend(e);
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
        // The placeholder is quiet but readable (muted, over 4.5:1); the prompt marker is KodAct's lime.
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
    fn the_default_style_says_what_it_does_once() {
        let mut a = app(false);
        a.look = crate::tui::look::Look::Frame;
        typed(&mut a, "hello");
        a.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let start = a.busy_since.unwrap();
        let (rows, _) = draw(&live_view_at(&a, 80, 20, start + Duration::from_millis(1300)), 80);
        let text = rows.join("\n");
        assert!(text.contains("Thinking… (1s)"), "a word for what it does, no key: {text}");
        assert!(text.contains("Type to queue a message") && !text.contains("esc stops"), "{text}");
        assert_eq!(text.matches("esc").count(), 1, "the esc key once, on the status line: {text}");
        // Plan mode: the status line says how to leave it.
        let mut b = app(false);
        b.look = crate::tui::look::Look::Frame;
        b.status.mode = "plan".into();
        let (rows, _) = draw(&live_view(&b, 80, 12), 80);
        assert!(rows.last().unwrap().contains("shift+tab leave plan mode"), "{rows:#?}");
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
        assert_eq!(lines.last().unwrap(), "  Default · deep-thinking · 0s · 1.2k tokens", "{lines:?}");
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
        let (rows, buf) = draw(&live_view(&a, 110, 12), 110);
        let n = rows.len();
        // The box: the text, a row of air, the session row; lime all round.
        assert!(rows[n - 6].starts_with('╭') && rows[n - 2].starts_with('╰'), "{rows:#?}");
        assert!(rows[n - 5].starts_with(&format!("│ ❯ {PLACEHOLDER}")), "{rows:#?}");
        assert!(
            rows[n - 4].starts_with('│')
                && rows[n - 4].ends_with('│')
                && rows[n - 4][3..rows[n - 4].len() - 3].trim().is_empty()
        );
        let session = &rows[n - 3];
        assert!(session.starts_with("│ Default  ·  quick-thinking  ·  gpt.example.net"), "{session}");
        assert!(session.ends_with("⡀ ~96% left  ·  25.3k tokens used │"), "the room left on the right: {session}");
        assert_eq!(buf[(0, (n - 6) as u16)].fg, a.theme.accent().fg.unwrap());
        assert_eq!(buf[(2, (n - 3) as u16)].fg, a.theme.mode("default").fg.unwrap(), "the mode in its colour");
        // The status line: the folder and the keys that help; the box has the rest.
        let last = rows[n - 1].trim_end();
        assert!(last.starts_with("  /p") && last.contains("/ commands") && !last.contains("quick-thinking"), "{last}");
        // At 80 columns the endpoint is the first to go, so the context still fits.
        let (rows, _) = draw(&live_view(&a, 80, 12), 80);
        assert!(rows[n - 3].starts_with("│ Default  ·  quick-thinking  ") && !rows[n - 3].contains("gpt.example.net"));
        assert!(rows[n - 3].contains("~96% left"), "{rows:#?}");
        // Plan mode: named in the box in its colour, not repeated on the line.
        a.status.mode = "plan".into();
        let (rows, buf) = draw(&live_view(&a, 80, 12), 80);
        assert!(rows[n - 3].starts_with("│ Plan  ·  quick-thinking"), "{rows:#?}");
        assert_eq!(buf[(2, (n - 3) as u16)].fg, a.theme.mode("plan").fg.unwrap());
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
        assert_eq!(rows[5], "│ ❯ 2. Haiku ✔  small                  │");
        a.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

        // Ctrl+R shows the search and the match in the input box.
        let mut a = App::new(
            Theme { color: false, light: false, accent: None, depth: Default::default() },
            vec!["cargo test".into(), "git status".into()],
        );
        a.on_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL));
        typed(&mut a, "car");
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert_eq!(rows[0], "  search history: car");
        assert!(rows[2].starts_with('│') && rows[2].contains("cargo test"), "the match in the box: {rows:#?}");
        typed(&mut a, "zz");
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert_eq!(rows[0], "  search history: carzz (no match)");

        // `@` lists matching paths.
        let mut a = app(false);
        a.on_event(UiEvent::Files(vec!["src/".into(), "src/main.rs".into(), "README.md".into()]));
        typed(&mut a, "look at @ma");
        let (rows, _) = draw(&live_view(&a, 40, 30), 40);
        assert_eq!(rows[3], "  src/main.rs");
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
        assert_eq!(rows[page + 1], "│ ❯ +New title                         │");
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
            "running is lime, as the spinner"
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
        assert!(rows.iter().any(|r| r.trim() == "↓ 10 more lines · PgDn"), "{rows:#?}");
        // A short transcript sits just above the live region, not at the top with a gap.
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
            "Default  ·  opus  ·  127.0.0.1:11434",
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

    #[test]
    fn work_in_flight_fills_and_settles_when_the_turn_ends() {
        let start = Instant::now();
        let mut a = busy_session(start);
        a.busy_since = Some(start);
        let mark = |a: &App, now: Instant, what: &str| -> String {
            let rows = side(a, 60, now);
            let r = rows.iter().find(|r| r.contains(what)).unwrap_or_else(|| panic!("{what}: {rows:#?}"));
            r.trim_start().chars().next().unwrap().to_string()
        };
        // The sub-agent and the active step fill frame by frame, from empty to full and back.
        let frames: Vec<String> =
            (0..FILL.len() as u32).map(|i| mark(&a, start + FILL_STEP * i, "find the parser")).collect();
        assert_eq!(frames, FILL.map(String::from).to_vec());
        assert_eq!(mark(&a, start + FILL_STEP * 2, "Add the field"), "⣄");
        assert_eq!(mark(&a, start + FILL_STEP * 3, "Add the field"), "⣤");
        // Each mark keeps its own phase: a sub-agent that started later is a frame behind.
        assert_eq!(filling(FILL_STEP * (FILL.len() as u32 + 1)), filling(FILL_STEP));
        // Idle: the open step waits half full, and the agents section is gone.
        a.busy = false;
        a.running.clear();
        assert_eq!(mark(&a, start + FILL_STEP * 3, "Add the field"), "⣤");
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
}
