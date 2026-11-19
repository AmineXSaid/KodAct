//! Transcript layout, wrapping, collapsible thoughts, and diff rendering.

use ratatui::style::Style;
use ratatui::text::{Line, Span};
use serde_json::Value;

use super::format::{DIFF_LINES, RESULT_INDENT};
use super::text::Theme;

pub(super) const DIFF_MARK: &str = "\u{e000}kodact-diff";
/// The start of a placeholder line that stands for a completed reasoning block until laid out.
pub const THOUGHT_MARK: &str = "\u{e000}kodact-thought";
/// Width from which a diff is drawn side by side.
pub const SPLIT_DIFF_COLS: u16 = 140;
/// Maximum reading width for prose, prompts, and thoughts on wide terminals.
pub const PROSE_MAX_COLS: u16 = 104;

/// A line that stands for an edit's changes: the styles they are drawn in, and
/// the hunks. [`lay_out`] turns it into rows at the width of the moment.
pub(super) fn diff_placeholder(t: &Theme, hunks: &[Value]) -> Line<'static> {
    Line::from(vec![
        Span::styled(DIFF_MARK, t.removed()),
        Span::styled("", t.added()),
        Span::styled("", t.dim()),
        Span::styled("", t.hairline()),
        Span::raw(Value::Array(hunks.to_vec()).to_string()),
    ])
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
            Some(s) if s.content == DIFF_MARK && l.spans.len() == 5 => {
                let styles = [l.spans[0].style, l.spans[1].style, l.spans[2].style, l.spans[3].style];
                let hunks: Vec<Value> = serde_json::from_str(&l.spans[4].content).unwrap_or_default();
                out.extend(diff_rows(&hunks, styles, width, verbose));
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
        } else if l.spans.first().is_some_and(|s| s.content == DIFF_MARK) && l.spans.len() == 5 {
            let styles = [l.spans[0].style, l.spans[1].style, l.spans[2].style, l.spans[3].style];
            let hunks: Vec<Value> = serde_json::from_str(&l.spans[4].content).unwrap_or_default();
            out.extend(super::text::wrap(diff_rows(&hunks, styles, full_w, verbose), full_w));
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

/// One changed line of a diff: its number in the old or new file, and its text.
struct Changed {
    num: u64,
    body: String,
}

/// An edit's changed lines, as rows: removed and added runs paired side by side when
/// `width` allows, else one column with `-` and `+`. Capped at [`DIFF_LINES`] unless `verbose`.
pub(super) fn diff_rows(
    hunks: &[Value],
    [removed, added, quiet, rule]: [Style; 4],
    width: u16,
    verbose: bool,
) -> Vec<Line<'static>> {
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
    let max_rows = if verbose { usize::MAX } else { DIFF_LINES };
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
                if out.len() == max_rows {
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
                if out.len() == max_rows {
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
