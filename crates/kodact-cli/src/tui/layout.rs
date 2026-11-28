//! Transcript layout, wrapping, collapsible thoughts, and diff rendering.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::format::{DIFF_LINES, RESULT_INDENT};
use super::text::Theme;

pub(super) const DIFF_MARK: &str = "\u{e000}kodact-diff";
/// The start of a placeholder line that stands for a completed reasoning block until laid out.
pub const THOUGHT_MARK: &str = "\u{e000}kodact-thought";
/// Width from which a diff is drawn side by side, as GitHub's split view: each side then
/// has room for a line number, its sign and about 50 columns of code.
pub const SPLIT_DIFF_COLS: u16 = 110;
/// Maximum reading width for prose, prompts, and thoughts on wide terminals.
pub const PROSE_MAX_COLS: u16 = 104;

/// The styles a diff is drawn in, from the theme: GitHub's, with the line's tint behind an
/// added or removed line, a stronger one behind the characters that changed, a band for a
/// hunk's header and the empty side of a split row. Without colour, red and green text and
/// the signs carry it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DiffStyles {
    /// The `-` and `+` signs.
    pub del: Style,
    pub add: Style,
    /// Context lines' numbers, notes.
    pub quiet: Style,
    /// The rule between the two sides.
    pub rule: Style,
    pub del_line: Style,
    pub add_line: Style,
    pub del_word: Style,
    pub add_word: Style,
    pub hunk: Style,
    pub filler: Style,
    pub del_gutter: Style,
    pub add_gutter: Style,
}

/// How many styles a diff placeholder carries.
const DIFF_STYLES: usize = 12;

impl DiffStyles {
    pub fn new(t: &Theme) -> Self {
        use super::palette as p;
        let bg = |r: p::Role| t.surface(r).map(|c| Style::default().bg(c));
        let (del, add) = (t.removed(), t.added());
        let tinted = bg(p::DIFF_ADD_LINE).is_some();
        DiffStyles {
            del,
            add,
            quiet: t.subtle(),
            rule: t.hairline(),
            del_line: bg(p::DIFF_DEL_LINE).unwrap_or(del),
            add_line: bg(p::DIFF_ADD_LINE).unwrap_or(add),
            del_word: bg(p::DIFF_DEL_WORD).unwrap_or(del.add_modifier(ratatui::style::Modifier::BOLD)),
            add_word: bg(p::DIFF_ADD_WORD).unwrap_or(add.add_modifier(ratatui::style::Modifier::BOLD)),
            hunk: match bg(p::DIFF_HUNK_BG) {
                Some(s) if tinted => s.fg(p::DIFF_HUNK_FG.color(t.depth, t.light)),
                _ => t.info(),
            },
            filler: bg(p::DIFF_FILLER).unwrap_or_default(),
            del_gutter: bg(p::DIFF_DEL_GUTTER).map(|s| s.patch(t.subtle())).unwrap_or(t.subtle()),
            add_gutter: bg(p::DIFF_ADD_GUTTER).map(|s| s.patch(t.subtle())).unwrap_or(t.subtle()),
        }
    }

    fn all(&self) -> [Style; DIFF_STYLES] {
        [
            self.del,
            self.add,
            self.quiet,
            self.rule,
            self.del_line,
            self.add_line,
            self.del_word,
            self.add_word,
            self.hunk,
            self.filler,
            self.del_gutter,
            self.add_gutter,
        ]
    }

    fn from(s: [Style; DIFF_STYLES]) -> Self {
        let [del, add, quiet, rule, del_line, add_line, del_word, add_word, hunk, filler, del_gutter, add_gutter] = s;
        DiffStyles {
            del,
            add,
            quiet,
            rule,
            del_line,
            add_line,
            del_word,
            add_word,
            hunk,
            filler,
            del_gutter,
            add_gutter,
        }
    }
}

/// A line that stands for an edit's changes: the styles they are drawn in, and
/// the hunks. [`lay_out`] turns it into rows at the width of the moment.
pub(super) fn diff_placeholder(t: &Theme, hunks: &[Value]) -> Line<'static> {
    let styles = DiffStyles::new(t).all();
    let mut spans = vec![Span::styled(DIFF_MARK, styles[0])];
    spans.extend(styles[1..].iter().map(|st| Span::styled("", *st)));
    spans.push(Span::raw(Value::Array(hunks.to_vec()).to_string()));
    Line::from(spans)
}

/// A diff placeholder's styles and hunks; `None` for any other line.
fn diff_of(l: &Line<'static>) -> Option<(DiffStyles, Vec<Value>)> {
    if l.spans.first()?.content != DIFF_MARK || l.spans.len() != DIFF_STYLES + 1 {
        return None;
    }
    let mut styles = [Style::default(); DIFF_STYLES];
    for (i, st) in styles.iter_mut().enumerate() {
        *st = l.spans[i].style;
    }
    let hunks = serde_json::from_str(&l.spans[DIFF_STYLES].content).unwrap_or_default();
    Some((DiffStyles::from(styles), hunks))
}

/// Expand collapsible placeholders (shell boxes, diffs, thoughts) with default collapsed view.
#[cfg(test)]
pub fn lay_out(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    lay_out_mode(lines, width, false)
}

/// Lines ready to wrap at `width`, optionally expanding thoughts, truncated diffs, and shell output.
#[cfg(test)]
pub fn lay_out_mode(lines: Vec<Line<'static>>, width: u16, verbose: bool) -> Vec<Line<'static>> {
    let last_fence = lines.iter().rposition(is_fence_opener);
    let mut out = Vec::with_capacity(lines.len());
    for (idx, mut l) in lines.into_iter().enumerate() {
        match l.spans.first() {
            Some(_) if super::shellbox::is_placeholder(&l) => {
                out.extend(super::shellbox::lay_out_mode(&l, width, verbose))
            }
            Some(_) if diff_of(&l).is_some() => {
                let (styles, hunks) = diff_of(&l).expect("checked");
                out.extend(diff_rows(&hunks, &styles, width, verbose));
            }
            Some(s) if s.content == THOUGHT_MARK && l.spans.len() == 3 => {
                let (subtle, italic) = (l.spans[0].style, l.spans[1].style);
                let v: Value = serde_json::from_str(&l.spans[2].content).unwrap_or_default();
                let secs = v["secs"].as_u64().unwrap_or(1).max(1);
                let count = v["lines"].as_u64().unwrap_or(1).max(1);
                let body = v["body"].as_str().unwrap_or("");
                out.extend(thought_rows(secs, count, body, subtle, italic, verbose));
            }
            // A table, anywhere on the line: what comes before it (the answer's mark) leads it.
            _ => match l.spans.iter().position(|s| s.content == super::text::TABLE_MARK) {
                Some(at) => {
                    let lead = l.spans[..at].to_vec();
                    out.extend(super::text::lay_out_table(lead, &l.spans[at..], width));
                }
                None => {
                    if Some(idx) == last_fence {
                        hint_fence(&mut l, width);
                    }
                    out.push(l);
                }
            },
        }
    }
    out
}

pub(super) fn is_fence_opener(l: &Line<'_>) -> bool {
    let has_gutter = l.spans.iter().any(|s| s.content == "╭─");
    let has_num = l.spans.last().is_some_and(|s| s.content.trim_start().starts_with('#'));
    has_gutter && has_num && !l.spans.iter().any(|s| s.content.contains("alt+c"))
}

pub(super) fn hint_fence(l: &mut Line<'static>, max_w: u16) {
    let used: usize = l.spans.iter().map(|s| super::text::width(&s.content)).sum();
    if (max_w as usize) >= used + 9 {
        let style = l.spans.last().map_or_else(Style::default, |s| s.style);
        l.spans.push(Span::styled(" · alt+c", style));
    }
}

pub(super) fn is_mode_banner(l: &Line<'_>) -> bool {
    l.spans.first().is_some_and(|s| s.content == "── ") && l.spans.get(1).is_some_and(|s| s.content == "◆ ")
}

/// Lay out placeholders and wrap transcript lines: structured full-width elements
/// (`shellbox`, split diffs, Markdown tables, mode banners) use full `width`, while prose,
/// thoughts, and prompts wrap to `width.min(PROSE_MAX_COLS)`. Prompt rows carrying a
/// `Line::style` background tint are padded to the prose measure so their surface forms a band.
pub fn wrap_lines(lines: Vec<Line<'static>>, width: u16, verbose: bool) -> Vec<Line<'static>> {
    let full_w = width.max(1);
    let prose_w = full_w.min(PROSE_MAX_COLS);
    let last_fence = lines.iter().rposition(is_fence_opener);
    let mut out = Vec::with_capacity(lines.len());
    for (idx, mut l) in lines.into_iter().enumerate() {
        if super::shellbox::is_placeholder(&l) {
            out.extend(super::text::wrap(super::shellbox::lay_out_mode(&l, full_w, verbose), full_w));
        } else if let Some((styles, hunks)) = diff_of(&l) {
            out.extend(super::text::wrap(diff_rows(&hunks, &styles, full_w, verbose), full_w));
        } else if let Some(at) = l.spans.iter().position(|s| s.content == super::text::TABLE_MARK) {
            let lead = l.spans[..at].to_vec();
            out.extend(super::text::wrap(super::text::lay_out_table(lead, &l.spans[at..], full_w), full_w));
        } else if l.spans.first().is_some_and(|s| s.content == THOUGHT_MARK) && l.spans.len() == 3 {
            let (subtle, italic) = (l.spans[0].style, l.spans[1].style);
            let v: Value = serde_json::from_str(&l.spans[2].content).unwrap_or_default();
            let secs = v["secs"].as_u64().unwrap_or(1).max(1);
            let count = v["lines"].as_u64().unwrap_or(1).max(1);
            let body = v["body"].as_str().unwrap_or("");
            out.extend(super::text::wrap(thought_rows(secs, count, body, subtle, italic, verbose), prose_w));
        } else if is_mode_banner(&l) {
            out.extend(super::text::wrap(vec![l], full_w));
        } else {
            if Some(idx) == last_fence {
                hint_fence(&mut l, prose_w);
            }
            let band = l.style;
            for mut row in super::text::wrap(vec![l], prose_w) {
                if band.bg.is_some() {
                    let used: usize = row.spans.iter().map(|s| super::text::width(&s.content)).sum();
                    let pad = (prose_w as usize).saturating_sub(used);
                    if pad > 0 {
                        row.spans.push(Span::styled(" ".repeat(pad), band));
                    }
                    row.style = Style::default();
                }
                out.push(row);
            }
        }
    }
    out
}

pub(super) fn thought_rows(
    secs: u64,
    lines: u64,
    body: &str,
    subtle: Style,
    italic: Style,
    verbose: bool,
) -> Vec<Line<'static>> {
    if !verbose {
        let unit = if lines == 1 { "line" } else { "lines" };
        return vec![Line::from(vec![
            Span::styled("∴ ", subtle),
            Span::styled(format!("Thought for {secs}s"), italic),
            Span::styled(format!(" · {lines} {unit} · ctrl+o to show"), subtle),
        ])];
    }
    let mut out = vec![Line::from(vec![
        Span::styled("∴ ", subtle),
        Span::styled(format!("Thought for {secs}s"), italic),
        Span::styled(" · ctrl+o to hide", subtle),
    ])];
    for para in body.lines().map(str::trim).filter(|l| !l.is_empty()) {
        out.push(Line::from(vec![Span::raw("  "), Span::styled(para.to_string(), italic)]));
    }
    out
}

/// One row of a diff, before it is drawn: a hunk's header, a line both sides share, or a
/// removed and/or added line (paired in a run, with what changed in each).
enum DiffRow {
    Header(String),
    Context { old: u64, new: u64, text: String },
    Change { del: Option<(u64, String, (usize, usize))>, add: Option<(u64, String, (usize, usize))> },
}

/// The characters that differ between `a` and `b`, as a char range in each: what is left once
/// their common start and end are set aside. Empty when nothing is shared (the whole line is
/// the change, and its tint says so).
fn changed_span(a: &str, b: &str) -> ((usize, usize), (usize, usize)) {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let start = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let rest = a.len().min(b.len()) - start;
    let end = a.iter().rev().zip(b.iter().rev()).take(rest).take_while(|(x, y)| x == y).count();
    if start == 0 && end == 0 {
        return ((0, 0), (0, 0));
    }
    ((start, a.len() - end), (start, b.len() - end))
}

/// An edit's hunks as rows, in order: each hunk's header, its context lines, and its removed
/// and added lines paired in runs (the first removed with the first added, and so on).
fn diff_model(hunks: &[Value]) -> (Vec<DiffRow>, usize) {
    let mut rows = vec![];
    let mut changed = 0;
    for h in hunks {
        let (mut old, mut new) = (h["oldStart"].as_u64().unwrap_or(1), h["newStart"].as_u64().unwrap_or(1));
        let count = |k: &str| h[k].as_u64().unwrap_or(0);
        rows.push(DiffRow::Header(format!("@@ -{},{} +{},{} @@", old, count("oldLines"), new, count("newLines"))));
        let (mut dels, mut adds): (Vec<(u64, String)>, Vec<(u64, String)>) = (vec![], vec![]);
        let flush = |rows: &mut Vec<DiffRow>, dels: &mut Vec<(u64, String)>, adds: &mut Vec<(u64, String)>| {
            for i in 0..dels.len().max(adds.len()) {
                let (d, a) = (dels.get(i).cloned(), adds.get(i).cloned());
                let (dr, ar) = match (&d, &a) {
                    (Some(d), Some(a)) => changed_span(&d.1, &a.1),
                    _ => ((0, 0), (0, 0)),
                };
                rows.push(DiffRow::Change { del: d.map(|(n, t)| (n, t, dr)), add: a.map(|(n, t)| (n, t, ar)) });
            }
            dels.clear();
            adds.clear();
        };
        for l in h["lines"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            let (sign, body) = l.split_at(l.chars().next().map(char::len_utf8).unwrap_or(0));
            let body = body.replace('\t', "    ");
            match sign {
                "-" => {
                    if !adds.is_empty() {
                        flush(&mut rows, &mut dels, &mut adds);
                    }
                    dels.push((old, body));
                    old += 1;
                    changed += 1;
                }
                "+" => {
                    adds.push((new, body));
                    new += 1;
                    changed += 1;
                }
                _ => {
                    flush(&mut rows, &mut dels, &mut adds);
                    rows.push(DiffRow::Context { old, new, text: body });
                    old += 1;
                    new += 1;
                }
            }
        }
        flush(&mut rows, &mut dels, &mut adds);
    }
    (rows, changed)
}

/// `text` as spans `w` columns wide: the changed chars `range` in `word`, the rest in `line`,
/// cut with `…` when too long, padded with `line`.
fn code_cell(text: &str, range: (usize, usize), w: usize, line: Style, word: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = text.chars().collect();
    let cut = super::text::width(text) > w;
    let keep = if cut { w.saturating_sub(1) } else { w };
    let mut out = vec![];
    let mut used = 0;
    let mut i = 0;
    while i < chars.len() && used < keep {
        let in_word = range.0 < range.1 && i >= range.0 && i < range.1;
        let mut seg = String::new();
        while i < chars.len() && used < keep && (range.0 < range.1 && i >= range.0 && i < range.1) == in_word {
            let cw = super::text::width(&chars[i].to_string()).max(1);
            if used + cw > keep {
                break;
            }
            seg.push(chars[i]);
            used += cw;
            i += 1;
        }
        if seg.is_empty() {
            break;
        }
        out.push(Span::styled(seg, if in_word { word } else { line }));
    }
    if cut {
        out.push(Span::styled("…", line));
        used += 1;
    }
    if used < w {
        out.push(Span::styled(" ".repeat(w - used), line));
    }
    out
}

/// An edit's changes as rows, as GitHub shows them: each hunk under its `@@` header, with its
/// context lines; split on a terminal at least [`SPLIT_DIFF_COLS`] wide (the old file on the
/// left, the new on the right, each line numbered, a removed line paired with the line that
/// replaced it), unified below that (both line numbers, then the sign). Changed lines sit on
/// their tint and the characters that changed on a stronger one. Capped unless `verbose`.
pub(super) fn diff_rows(hunks: &[Value], st: &DiffStyles, width: u16, verbose: bool) -> Vec<Line<'static>> {
    let (rows, changed) = diff_model(hunks);
    let max_rows = if verbose { usize::MAX } else { DIFF_LINES + 8 };
    let avail = (width as usize).saturating_sub(RESULT_INDENT.len()).max(20);
    let mut out = vec![];
    let mut shown = 0;
    let split = width >= SPLIT_DIFF_COLS;
    for row in &rows {
        if out.len() >= max_rows {
            break;
        }
        let mut spans = vec![Span::raw(RESULT_INDENT)];
        match row {
            DiffRow::Header(h) => {
                let text = format!(" {h}");
                let pad = avail.saturating_sub(super::text::width(&text));
                spans.push(Span::styled(format!("{text}{}", " ".repeat(pad)), st.hunk));
            }
            DiffRow::Context { old, new, text } if split => {
                let half = (avail - 1) / 2;
                let other = avail - 1 - half;
                for (n, w) in [(old, half), (new, other)] {
                    spans.push(Span::styled(format!("{n:>5}   "), st.quiet));
                    spans.extend(code_cell(text, (0, 0), w.saturating_sub(8), Style::default(), Style::default()));
                    if w == half {
                        spans.push(Span::styled("│", st.rule));
                    }
                }
            }
            DiffRow::Context { old, new, text } => {
                spans.push(Span::styled(format!("{old:>5}{new:>5}   "), st.quiet));
                spans.extend(code_cell(text, (0, 0), avail.saturating_sub(13), Style::default(), Style::default()));
            }
            DiffRow::Change { del, add } if split => {
                let half = (avail - 1) / 2;
                let other = avail - 1 - half;
                let side = |c: &Option<(u64, String, (usize, usize))>,
                            w: usize,
                            (sign, sign_st, gutter, line, word): (&str, Style, Style, Style, Style)|
                 -> Vec<Span<'static>> {
                    match c {
                        Some((n, text, range)) => {
                            let mut v = vec![
                                Span::styled(format!("{n:>5} "), gutter),
                                Span::styled(format!("{sign} "), sign_st.patch(line)),
                            ];
                            v.extend(code_cell(text, *range, w.saturating_sub(8), line, word));
                            v
                        }
                        None => vec![Span::styled(" ".repeat(w), st.filler)],
                    }
                };
                shown += usize::from(del.is_some()) + usize::from(add.is_some());
                spans.extend(side(del, half, ("-", st.del, st.del_gutter, st.del_line, st.del_word)));
                spans.push(Span::styled("│", st.rule));
                spans.extend(side(add, other, ("+", st.add, st.add_gutter, st.add_line, st.add_word)));
            }
            DiffRow::Change { del, add } => {
                // Unified: the removed line, then the added one, each on its own row.
                if let Some((n, text, range)) = del {
                    let mut s = spans.clone();
                    s.push(Span::styled(format!("{n:>5}     "), st.del_gutter));
                    s.push(Span::styled(" - ", st.del.patch(st.del_line)));
                    s.extend(code_cell(text, *range, avail.saturating_sub(13), st.del_line, st.del_word));
                    out.push(Line::from(s));
                    shown += 1;
                    if out.len() >= max_rows {
                        break;
                    }
                }
                if let Some((n, text, range)) = add {
                    spans.push(Span::styled(format!("     {n:>5}"), st.add_gutter));
                    spans.push(Span::styled(" + ", st.add.patch(st.add_line)));
                    spans.extend(code_cell(text, *range, avail.saturating_sub(13), st.add_line, st.add_word));
                    shown += 1;
                } else {
                    continue;
                }
            }
        }
        out.push(Line::from(spans));
    }
    if changed > shown {
        out.push(Line::from(Span::styled(
            format!("{RESULT_INDENT}… {} more changed lines (ctrl+o to show them)", changed - shown),
            st.quiet,
        )));
    }
    out
}
