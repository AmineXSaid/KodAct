//! Dialogs, modal frames, menus, match highlighting, and contextual key hints.

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::app::App;
use super::look::Look;
use super::palette;
use super::render::{fit, MENU_ROWS};
use super::text::{self, width as text_width};

/// A rounded box around `body` rows, `w` columns wide.
pub(super) fn boxed(title: Line<'static>, body: Vec<Line<'static>>, w: usize, border: Style) -> Vec<Line<'static>> {
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

/// `body` rows between corner marks, bounded to a scannable width inside `w`:
/// the frame style's way of setting a question apart.
pub(super) fn cornered(
    title: Line<'static>,
    body: Vec<Line<'static>>,
    w: usize,
    mark: Style,
    rule: Style,
) -> Vec<Line<'static>> {
    let line_width =
        |l: &Line<'static>| -> usize { l.spans.iter().map(|s| text_width(&s.content.replace('\t', "    "))).sum() };
    let content_w = std::iter::once(&title).chain(&body).map(line_width).max().unwrap_or(0);
    let bw = (content_w + 4).clamp(60.min(w), 96.min(w)).min(w);
    let inner = bw.saturating_sub(4).max(1);
    let edge = |l: &str, r: &str| {
        if bw < 4 {
            Line::from(Span::styled("─".repeat(bw), mark))
        } else {
            Line::from(vec![
                Span::styled(l.to_string(), mark),
                Span::styled("─".repeat(bw - 4), rule),
                Span::styled(r.to_string(), mark),
            ])
        }
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
pub(super) fn screen_spans(app: &App, spans: &[kodact_core::commands::screens::Span]) -> Vec<Span<'static>> {
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
pub(super) const SCREEN_CHROME: usize = 5;

/// A screen, at most `room` rows tall. Rows are cut to the width, never wrapped, so
/// scrolling counts rows exactly.
pub(super) fn viewer(app: &App, w: usize, room: usize) -> Vec<Line<'static>> {
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
pub(super) fn form(app: &App, w: usize) -> Vec<Line<'static>> {
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

pub(super) fn dialog(app: &App, w: usize, room: usize) -> Vec<Line<'static>> {
    if app.viewer().is_some() {
        return viewer(app, w, room);
    }
    if app.form().is_some() {
        return form(app, w);
    }
    let t = app.theme;
    let (title, body) = app.dialog_text();
    let bash_cmd_lines = app.dialog_bash_cmd_lines();
    let cmd_style = if t.color && t.depth != palette::Depth::Ansi16 {
        Style::default()
            .fg(palette::PROMPT_FG.color(t.depth, t.light))
            .bg(palette::PROMPT_BG.color(t.depth, t.light))
            .add_modifier(ratatui::style::Modifier::BOLD)
    } else {
        t.bold()
    };
    // A proposed edit's lines: removed in red, added in green; Bash commands stand on a raised band.
    let rows: Vec<Line<'static>> = body
        .into_iter()
        .enumerate()
        .map(|(i, l)| {
            if bash_cmd_lines > 0 && i < bash_cmd_lines {
                if let Some(cmd) = l.strip_prefix("  ") {
                    Line::from(vec![Span::raw("  "), Span::styled(cmd.to_string(), cmd_style)])
                } else {
                    Line::from(Span::styled(l, cmd_style))
                }
            } else if l.starts_with("  - ") {
                Line::from(Span::styled(l, t.removed()))
            } else if l.starts_with("  + ") {
                Line::from(Span::styled(l, t.added()))
            } else if bash_cmd_lines > 0 {
                Line::from(Span::styled(l, t.subtle()))
            } else {
                Line::from(l)
            }
        })
        .collect();
    let preview = rows;
    let mut danger_rows = vec![];
    // What a destructive command would do, in red, with a glyph and a word.
    if let Some(danger) = app.dialog_danger() {
        danger_rows.push(Line::default());
        danger_rows.push(Line::from(vec![
            Span::styled("▲ ", t.error()),
            Span::styled(format!("This {danger}. It can't be undone."), t.error()),
        ]));
    }
    let selected = app.dialog_selected();
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
    let title = if permission {
        Line::from(vec![Span::styled("◆ ", t.accent()), Span::styled(title, t.bold())])
    } else {
        Line::from(Span::styled(title, t.bold()))
    };
    let option = |i: usize, one_row: Option<usize>| {
        let (label, desc) = &options[i];
        let mark = if i == selected { "❯ " } else { "  " };
        let st = if i == selected { t.selected() } else { Style::default() };
        let head = format!("{mark}{}. {label}", i + 1);
        match one_row {
            // Cut to one row, so the rows shown can be counted.
            Some(width) => {
                let head = fit(&head, width);
                let left = width.saturating_sub(text_width(&head) + 2);
                let mut spans = vec![Span::styled(head, st)];
                if !desc.is_empty() && left >= 4 {
                    spans.push(Span::styled(format!("  {}", fit(desc, left)), t.dim()));
                }
                if i == selected {
                    return tint_row(&t, spans, 0, width);
                }
                Line::from(spans)
            }
            None => {
                let mut spans = vec![Span::styled(head, st)];
                if !desc.is_empty() {
                    spans.push(Span::styled(format!("  {desc}"), t.dim()));
                }
                Line::from(spans)
            }
        }
    };
    let whole = |head: &[Line<'static>], shown: Vec<Line<'static>>, foot: Option<String>| {
        let mut all = head.to_vec();
        all.extend(danger_rows.iter().cloned());
        all.push(Line::default());
        all.extend(shown);
        all.push(Line::default());
        all.push(Line::from(Span::styled(foot.unwrap_or_else(|| hint.clone()), t.dim())));
        framed(app, title.clone(), all, w)
    };
    let option_lines: Vec<Line<'static>> = (0..options.len()).map(|i| option(i, None)).collect();
    let all = whole(&preview, option_lines.clone(), None);
    if all.len() <= room || options.is_empty() {
        return all;
    }
    if permission && preview.len() > 1 {
        let fixed = whole(&[], option_lines.clone(), None).len();
        let preview_room = room.saturating_sub(fixed);
        if preview_room >= 2 {
            let show = preview_room.saturating_sub(1).max(1);
            let max_start = preview.len().saturating_sub(show);
            let start = app.dialog_scroll().min(max_start);
            let end = (start + show).min(preview.len());
            let mut sliced = preview[start..end].to_vec();
            let above = start;
            let below = preview.len().saturating_sub(end);
            sliced.push(Line::from(Span::styled(
                format!("  ↑ {above} above · ↓ {below} below · PgUp/PgDn or wheel"),
                t.dim(),
            )));
            let candidate = whole(&sliced, option_lines, None);
            if candidate.len() <= room {
                return candidate;
            }
        }
    }
    // More than the window holds (an endpoint that lists a hundred models, a small
    // window): the title, the filter and the keys stay, and the options scroll in what
    // is left, around the selected one. Before, the box ran off the top of the window
    // and took its title, its filter and its first rows with it.
    let frame_w = match app.look {
        super::look::Look::Frame => w.min(96),
        super::look::Look::Status if w >= 60 => w.min(76),
        _ => w,
    };
    let inner = if app.look == super::look::Look::Bare { frame_w } else { frame_w.saturating_sub(4) }.max(8);
    let mut head = preview;
    loop {
        let fixed = whole(&head, vec![], None).len();
        let fit_rows = room.saturating_sub(fixed);
        if fit_rows >= options.len().min(3) || head.len() <= 1 {
            let n = fit_rows.clamp(1, options.len());
            let first = selected.saturating_sub(n / 2).min(options.len() - n);
            let foot = format!("{}-{} of {} · ↑↓ PgUp PgDn · {hint}", first + 1, first + n, options.len());
            return whole(&head, (first..first + n).map(|i| option(i, Some(inner))).collect(), Some(fit(&foot, inner)));
        }
        // Still too tall: what is asked about gives up its first rows, never the choices.
        head.remove(0);
    }
}

/// A question's rows in the style's frame.
pub(super) fn framed(app: &App, title: Line<'static>, rows: Vec<Line<'static>>, w: usize) -> Vec<Line<'static>> {
    let t = app.theme;
    match app.look {
        // Corner marks around the question connected by hairline rules, bounded cleanly.
        Look::Frame => cornered(title, rows, w, t.accent(), t.hairline()),
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

pub(super) fn highlight_match(text: &str, query: &str, hi: Style) -> Vec<Span<'static>> {
    let q_lower: Vec<char> = query.chars().flat_map(char::to_lowercase).collect();
    if q_lower.is_empty() {
        return vec![Span::raw(text.to_string())];
    }
    let t_chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut plain = String::new();
    while i < t_chars.len() {
        if i + q_lower.len() <= t_chars.len()
            && t_chars[i..i + q_lower.len()].iter().flat_map(|c| c.to_lowercase()).eq(q_lower.iter().copied())
        {
            if !plain.is_empty() {
                out.push(Span::raw(std::mem::take(&mut plain)));
            }
            let matched: String = t_chars[i..i + q_lower.len()].iter().collect();
            out.push(Span::styled(matched, hi));
            i += q_lower.len();
        } else {
            plain.push(t_chars[i]);
            i += 1;
        }
    }
    if !plain.is_empty() {
        out.push(Span::raw(plain));
    }
    out
}

/// The keys that help now, for the frame style's status line: while the model
/// works, how to stop it; in find or a menu, how to step; with nothing typed, the
/// relevant commands, search, fold toggle, code copy, and modes; while typing, how to send.
pub(super) fn hints(app: &App, room: usize) -> Vec<Span<'static>> {
    let t = app.theme;
    let mut keys: Vec<(&'static str, &'static str)> = Vec::new();
    if app.dialog.is_some() {
        // The dialog says its own keys.
    } else if app.card.is_some() {
        keys.extend([("enter", "open"), ("←→", "choose"), ("esc", "back to the prompt")]);
    } else if app.find.is_some() {
        keys.extend([("enter", "next"), ("↑↓", "match"), ("esc", "close")]);
    } else if app.busy && !app.queued.is_empty() && app.editor.is_empty() {
        if app.forcing_queue {
            keys.extend([("esc", "stop"), ("↑", "take back")]);
        } else {
            keys.extend([("enter", "force next"), ("↑", "take back"), ("esc", "stop")]);
        }
        if app.has_collapsible {
            keys.push(("ctrl+o", if app.verbose { "less" } else { "thoughts" }));
        }
        keys.push(("shift+tab", "mode"));
    } else if app.busy {
        if !app.editor.is_empty() {
            keys.extend([("enter", "queue"), ("ctrl+enter", "force"), ("esc", "stop")]);
        } else {
            keys.extend([("esc", "stop"), ("enter", "queue a message")]);
        }
        if app.has_collapsible {
            keys.push(("ctrl+o", if app.verbose { "less" } else { "thoughts" }));
        }
        keys.push(("shift+tab", "mode"));
    } else if !app.menu().is_empty() || !app.file_menu().is_empty() {
        keys.extend([("↑↓", "choose"), ("tab", "complete"), ("esc", "close")]);
    } else if app.editor.is_empty() && app.status.mode == "plan" {
        // Plan mode: how to leave it and act on the plan, plus active transcript shortcuts.
        keys.extend([("shift+tab", "leave plan mode"), ("/", "commands")]);
        if app.talked {
            keys.push(("ctrl+f", "find"));
        }
        if app.has_collapsible {
            keys.push(("ctrl+o", if app.verbose { "less" } else { "thoughts" }));
        }
        if app.has_code {
            keys.push(("alt+c", "copy code"));
        }
        keys.push(("?", "keys"));
    } else if app.editor.is_empty() {
        if app.front && app.look == super::look::Look::Frame && app.cards_drawn.get() {
            keys.push(("tab", "suggestions"));
        }
        keys.extend([("/", "commands"), ("@", "files"), ("shift+tab", "mode")]);
        if app.talked {
            keys.push(("ctrl+f", "find"));
        }
        if app.has_collapsible {
            keys.push(("ctrl+o", if app.verbose { "less" } else { "thoughts" }));
        }
        if app.has_code {
            keys.push(("alt+c", "copy code"));
        }
        keys.push(("?", "keys"));
    } else {
        keys.extend([("enter", "send"), ("shift+enter", "new line")]);
    };
    let mut out: Vec<Span<'static>> = vec![];
    let mut used = 0;
    for (i, (k, what)) in keys.iter().enumerate() {
        // Each key drawn as a key, on a raised surface, then what it does.
        let gap = if i == 0 { "" } else { "  " };
        let w = text_width(gap) + text_width(k) + 2 + 1 + text_width(what);
        if used + w > room {
            break;
        }
        used += w;
        out.push(Span::raw(gap));
        out.push(t.keycap(k));
        out.push(Span::styled(format!(" {what}"), t.subtle()));
    }
    out
}

/// The `@` menu: matching paths.
pub(super) fn file_menu(app: &App, left: usize, w: usize, lead: &str) -> Vec<Line<'static>> {
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
            let mut spans = vec![];
            if left > 0 {
                spans.push(Span::raw(" ".repeat(left)));
            }
            spans.push(Span::styled(fit(&format!("{lead}{f}"), w), st));
            if first + i == sel {
                return tint_row(&t, spans, left, w);
            }
            Line::from(spans)
        })
        .collect()
}

/// A selected row: the wine tint behind it, from its first text to `w` columns past `left`.
fn tint_row(t: &super::text::Theme, mut spans: Vec<Span<'static>>, left: usize, w: usize) -> Line<'static> {
    let Some(bg) = t.tint() else { return Line::from(spans) };
    let used: usize = spans.iter().map(|s| text_width(&s.content)).sum();
    spans.push(Span::raw(" ".repeat((left + w).saturating_sub(used))));
    for s in spans.iter_mut().skip(usize::from(left > 0)) {
        s.style = s.style.bg(bg);
    }
    Line::from(spans)
}

pub(super) fn menu(app: &App, left: usize, w: usize, lead: &str) -> Vec<Line<'static>> {
    let t = app.theme;
    let items = app.menu();
    if items.is_empty() {
        return file_menu(app, left, w, lead);
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
            let l = format!("{lead}{l}{}", " ".repeat(pad.saturating_sub(text_width(&l))));
            let is_skill = app.skills.iter().any(|s| s.eq_ignore_ascii_case(&c.name));
            let desc_room = w.saturating_sub(text_width(&l) + 2 + if is_skill { 8 } else { 0 });
            let desc = fit(&c.description, desc_room);
            let selected = first + i == sel;
            let st = if selected { t.selected() } else { Style::default() };
            let mut spans = vec![];
            if left > 0 {
                spans.push(Span::raw(" ".repeat(left)));
            }
            spans.extend([Span::styled(l, st), Span::raw("  ")]);
            if is_skill {
                spans.push(Span::styled("skill · ", if selected { t.accent() } else { t.subtle() }));
            }
            spans.push(Span::styled(desc, t.dim()));
            if selected {
                return tint_row(&t, spans, left, w);
            }
            Line::from(spans)
        })
        .collect()
}
