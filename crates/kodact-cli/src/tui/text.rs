//! Text for the terminal UI: the colour theme, a light markdown renderer
//! (one line at a time, so streamed text can be written as it arrives), and
//! wrapping of styled lines to the terminal's width.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

use super::palette::{self, Depth};

/// Colours and emphasis: KodAct's palette (`palette.rs`). Without colour
/// (`NO_COLOR`, `--color never`, the `none` theme) only bold, dim and reverse are used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    pub color: bool,
    /// Colours for a light background.
    pub light: bool,
    /// The accent colour chosen with `/color` for this session (`None`: KodAct's own).
    pub accent: Option<Color>,
    /// How many colours the terminal shows.
    pub depth: Depth,
}

/// `/color` names and their colours.
pub const ACCENTS: &[(&str, Color)] = &[
    ("red", Color::Red),
    ("orange", Color::Rgb(0xe0, 0x8a, 0x3c)),
    ("yellow", Color::Yellow),
    ("green", Color::Green),
    ("cyan", Color::Cyan),
    ("blue", Color::Blue),
    ("purple", Color::Rgb(0x9b, 0x7b, 0xf0)),
    ("pink", Color::Rgb(0xe8, 0x7a, 0xb8)),
];

impl Theme {
    /// The theme named in settings (`dark`, `light`, `none`, or `auto`: what
    /// the terminal says of its background, else dark), when colour is allowed at all.
    pub fn named(name: &str, color_ok: bool, depth: Depth) -> Theme {
        let light = match name {
            "light" => true,
            "auto" => palette::light_background().unwrap_or(false),
            _ => false,
        };
        Theme { color: color_ok && name != "none", light, accent: None, depth }
    }

    /// A role's colour as a foreground, or no style without colour.
    pub fn role(&self, r: palette::Role) -> Style {
        if self.color {
            Style::default().fg(r.color(self.depth, self.light))
        } else {
            Style::default()
        }
    }

    /// KodAct's own colour: the prompt marker, the answer marker, selections.
    pub fn accent(&self) -> Style {
        if let (true, Some(c)) = (self.color, self.accent) {
            return Style::default().fg(c);
        }
        if self.color {
            self.role(palette::BRAND)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        }
    }

    /// The brand a step brighter: the wordmark, headings in brand colour.
    pub fn brand_strong(&self) -> Style {
        if self.color && self.accent.is_none() {
            self.role(palette::BRAND_STRONG)
        } else {
            self.accent()
        }
    }

    /// Secondary text. A muted colour where there are enough colours, else dim.
    pub fn dim(&self) -> Style {
        if self.color && self.depth != Depth::Ansi16 {
            self.role(palette::MUTED)
        } else {
            Style::default().add_modifier(Modifier::DIM)
        }
    }

    /// Placeholders and the quietest hints.
    pub fn subtle(&self) -> Style {
        if self.color && self.depth != Depth::Ansi16 {
            self.role(palette::SUBTLE)
        } else {
            Style::default().add_modifier(Modifier::DIM)
        }
    }

    /// Rules and box edges that separate without drawing the eye.
    pub fn hairline(&self) -> Style {
        if self.color && self.depth != Depth::Ansi16 {
            self.role(palette::HAIRLINE)
        } else {
            Style::default().add_modifier(Modifier::DIM)
        }
    }

    pub fn bold(&self) -> Style {
        Style::default().add_modifier(Modifier::BOLD)
    }

    pub fn error(&self) -> Style {
        if self.color {
            self.role(palette::DANGER)
        } else {
            self.bold()
        }
    }

    pub fn warning(&self) -> Style {
        self.role(palette::WARNING)
    }

    pub fn success(&self) -> Style {
        self.role(palette::SUCCESS)
    }

    /// Links, paths and what is running now.
    pub fn info(&self) -> Style {
        self.role(palette::INFO)
    }

    /// A removed line in a diff.
    pub fn removed(&self) -> Style {
        self.role(palette::REMOVED)
    }

    /// An added line in a diff.
    pub fn added(&self) -> Style {
        self.role(palette::ADDED)
    }

    /// The colour of part `i` of the context window (`/context` grid).
    pub fn part(&self, i: u8) -> Style {
        if !self.color {
            return Style::default();
        }
        let r = match i {
            0 => palette::BRAND,
            1 => palette::INFO,
            2 => palette::BRAND_STRONG,
            3 => palette::WARNING,
            4 => palette::CODE,
            5 => palette::SUCCESS,
            _ => return self.subtle(),
        };
        self.role(r)
    }

    /// Teal, lime's first companion. Bold without colour.
    pub fn teal(&self) -> Style {
        if self.color {
            self.role(palette::TEAL)
        } else {
            self.bold()
        }
    }

    /// Violet, lime's second companion. Plain without colour.
    pub fn violet(&self) -> Style {
        self.role(palette::VIOLET)
    }

    /// A permission mode's colour: default in the accent, plan in teal, auto
    /// in violet, accept edits in amber, bypass permissions in red, don't ask quiet.
    pub fn mode(&self, mode: &str) -> Style {
        match mode {
            "acceptEdits" => self.warning(),
            "plan" => self.teal(),
            "auto" => self.violet(),
            "dontAsk" => self.dim(),
            "bypassPermissions" => self.error(),
            _ => self.accent(),
        }
    }

    /// Inline `code`: peach, on a shade of its own where there are enough colours.
    pub fn code(&self) -> Style {
        let st = self.role(palette::CODE);
        if self.color && self.depth != Depth::Ansi16 {
            st.bg(palette::CODE_BG.color(self.depth, self.light))
        } else {
            st
        }
    }

    /// The model's words in a line of an answer: soft paper for plain text,
    /// the brightest paper for bold; spans with a colour of their own keep it.
    pub fn answer(&self, mut line: Line<'static>) -> Line<'static> {
        if !self.color || self.depth == Depth::Ansi16 {
            return line;
        }
        let body = palette::ANSWER.color(self.depth, self.light);
        let strong = palette::STRONG.color(self.depth, self.light);
        for s in &mut line.spans {
            if s.style.fg.is_none() && !s.content.trim().is_empty() {
                s.style.fg = Some(if s.style.add_modifier.contains(Modifier::BOLD) { strong } else { body });
            }
        }
        line
    }

    /// The highlighted row of a menu or dialog.
    pub fn selected(&self) -> Style {
        if self.color {
            self.accent().add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::REVERSED)
        }
    }

    /// The person's own prompts in the transcript: a raised surface.
    pub fn user(&self) -> Style {
        if self.color {
            Style::default()
                .bg(palette::PROMPT_BG.color(self.depth, self.light))
                .fg(palette::PROMPT_FG.color(self.depth, self.light))
        } else {
            self.dim()
        }
    }

    /// The working shimmer: `level` 0 is the resting tone, 3 the brightest.
    pub fn shimmer(&self, level: u8) -> Style {
        if !self.color || self.depth == Depth::Ansi16 {
            return match level {
                0 => Style::default().add_modifier(Modifier::DIM),
                3 => self.bold(),
                _ => Style::default(),
            };
        }
        let (base, peak) = (palette::SHIMMER_BASE, palette::SHIMMER_PEAK);
        let pick = |r: palette::Role| if self.light { r.light } else { r.dark };
        let (a, b) = (pick(base), pick(peak));
        let mix = |shift: u32| -> u32 {
            let (x, y) = ((a >> shift) & 0xff, (b >> shift) & 0xff);
            (x * (3 - level.min(3) as u32) + y * level.min(3) as u32) / 3
        };
        let hex = (mix(16) << 16) | (mix(8) << 8) | mix(0);
        Style::default().fg(palette::ink(hex, self.depth, Color::Reset))
    }
}

/// Markdown, one line at a time, so streamed text is drawn as it arrives:
/// headings by level, bullet, numbered and task lists (nested by indent),
/// quotes, rules, tables (held until they end, so their columns line up),
/// fenced code with its language and highlighting, and inline bold, italic,
/// strikethrough, `code` and links.
#[derive(Debug, Clone, Default)]
pub struct Markdown {
    /// The open code fence: its marker (```` ``` ```` or `~~~`) and language.
    fence: Option<(String, String)>,
    table: Vec<String>,
}

/// The start of a line that stands for a table until it is laid out ([`lay_out_table`]).
pub const TABLE_MARK: &str = "\u{e000}kodact-table";
const TABLE_HEAD: &str = "\u{e001}H";
const TABLE_ROW: &str = "\u{e001}R";
const TABLE_CELL: &str = "\u{e002}L";
const TABLE_CELL_RIGHT: &str = "\u{e002}R";
/// The narrowest a column is squeezed to before the table is shown as stacked rows.
const MIN_COLUMN: usize = 6;

/// A table placeholder laid out in `width` columns, after `lead` (the answer's mark or
/// indent, kept on the first row). With room, aligned columns with `│` between them and a
/// rule under the header. Short of room, the widest columns give way first and long cells
/// wrap inside their own column, so the bars stay straight. Too narrow for that, each row
/// becomes a small block of `Header: value` lines.
pub fn lay_out_table(lead: Vec<Span<'static>>, spans: &[Span<'static>], cols: u16) -> Vec<Line<'static>> {
    let rule = spans[0].style;
    // Decode: rows of cells, each cell its spans; which row is the header; right-aligned columns.
    let mut rows: Vec<(bool, Vec<Vec<Span<'static>>>)> = vec![];
    let mut right: Vec<bool> = vec![];
    for s in &spans[1..] {
        match s.content.as_ref() {
            TABLE_HEAD | TABLE_ROW => rows.push((s.content == TABLE_HEAD, vec![])),
            TABLE_CELL | TABLE_CELL_RIGHT => {
                if let Some((_, cells)) = rows.last_mut() {
                    if right.len() <= cells.len() {
                        right.push(s.content == TABLE_CELL_RIGHT);
                    }
                    cells.push(vec![]);
                }
            }
            _ => {
                if let Some(cell) = rows.last_mut().and_then(|(_, c)| c.last_mut()) {
                    cell.push(s.clone());
                }
            }
        }
    }
    let n = rows.iter().map(|(_, c)| c.len()).max().unwrap_or(0);
    let lead_w: usize = lead.iter().map(|s| width(&s.content)).sum();
    let room = (cols as usize).saturating_sub(lead_w).max(1);
    let cell_w = |c: &[Span<'static>]| c.iter().map(|s| width(&s.content)).sum::<usize>();
    let natural: Vec<usize> = (0..n)
        .map(|i| rows.iter().filter_map(|(_, c)| c.get(i)).map(|c| cell_w(c)).max().unwrap_or(0).max(1))
        .collect();
    let gaps = 3 * n.saturating_sub(1);
    let mut out: Vec<Line<'static>> = vec![];
    let indent = || Span::raw(" ".repeat(lead_w));
    let mut first = true;
    let mut push = |out: &mut Vec<Line<'static>>, mut l: Vec<Span<'static>>| {
        let mut spans = if first { lead.clone() } else { vec![indent()] };
        first = false;
        spans.append(&mut l);
        out.push(Line::from(spans));
    };
    if n == 0 {
        return out;
    }
    // Columns: as they are when they fit; else the widest give way first. A column is never
    // squeezed below its longest word (up to 16 columns): past that, the table stacks.
    let longest_word: Vec<usize> = (0..n)
        .map(|i| {
            rows.iter()
                .filter_map(|(_, c)| c.get(i))
                .flat_map(|c| c.iter().flat_map(|s| s.content.split_whitespace().map(width).collect::<Vec<_>>()))
                .max()
                .unwrap_or(1)
                .clamp(1, 16)
        })
        .collect();
    let mut widths = natural.clone();
    let budget = room.saturating_sub(gaps).max(n);
    while widths.iter().sum::<usize>() > budget {
        let (i, w) = widths.iter().copied().enumerate().max_by_key(|(_, w)| *w).unwrap();
        if w <= MIN_COLUMN.min(natural[i]) {
            break;
        }
        widths[i] = w - 1;
    }
    let squeezed = widths.iter().sum::<usize>() > budget || (0..n).any(|i| widths[i] < longest_word[i]);
    if n > 1 && squeezed {
        // Stacked: each row a block of `Header: value`, the header's own row naming the fields.
        let header: Option<Vec<Vec<Span<'static>>>> = rows.iter().find(|(h, _)| *h).map(|(_, c)| c.clone());
        for (ri, (is_head, cells)) in rows.iter().enumerate() {
            if *is_head {
                continue;
            }
            if ri > 0 && !out.is_empty() {
                push(&mut out, vec![Span::styled("─".repeat(room.min(24)), rule)]);
            }
            for (i, cell) in cells.iter().enumerate() {
                let mut line: Vec<Span<'static>> = vec![];
                if let Some(h) = header.as_ref().and_then(|h| h.get(i)) {
                    line.extend(h.iter().cloned());
                    line.push(Span::styled(": ", rule));
                }
                line.extend(cell.iter().cloned());
                for l in wrap(vec![Line::from(line)], room as u16) {
                    push(&mut out, l.spans);
                }
            }
        }
        return out;
    }
    let bar = || Span::styled(" │ ", rule);
    for (ri, (is_head, cells)) in rows.iter().enumerate() {
        // Each cell wrapped inside its column; the row as tall as its tallest cell.
        let parts: Vec<Vec<Line<'static>>> = (0..n)
            .map(|i| {
                let c = cells.get(i).cloned().unwrap_or_default();
                if c.is_empty() {
                    vec![Line::default()]
                } else {
                    wrap(vec![Line::from(c)], widths[i] as u16)
                }
            })
            .collect();
        let tall = parts.iter().map(Vec::len).max().unwrap_or(1);
        for k in 0..tall {
            let mut line: Vec<Span<'static>> = vec![];
            for i in 0..n {
                if i > 0 {
                    line.push(bar());
                }
                let piece = parts[i].get(k).cloned().unwrap_or_default();
                let used: usize = piece.spans.iter().map(|s| width(&s.content)).sum();
                let gap = Span::raw(" ".repeat(widths[i].saturating_sub(used)));
                if right.get(i).copied().unwrap_or(false) {
                    line.push(gap);
                    line.extend(piece.spans);
                } else {
                    line.extend(piece.spans);
                    if i + 1 < n {
                        line.push(gap);
                    }
                }
            }
            push(&mut out, line);
        }
        if *is_head && ri == 0 {
            let rule_row: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
            push(&mut out, vec![Span::styled(rule_row.join("─┼─"), rule)]);
        }
    }
    out
}

/// A table row: `| a | b |`.
fn is_table_row(s: &str) -> bool {
    let t = s.trim();
    t.starts_with('|') && t.len() > 1
}

fn cells(row: &str) -> Vec<String> {
    let t = row.trim();
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    t.split('|').map(|c| c.trim().to_string()).collect()
}

fn is_separator(cells: &[String]) -> bool {
    !cells.is_empty()
        && cells.iter().all(|c| {
            let c = c.trim_matches(':');
            !c.is_empty() && c.chars().all(|ch| ch == '-')
        })
}

/// A list item's marker: (indent in columns, the marker, the text after it).
enum Item<'a> {
    Bullet(usize, &'a str),
    Task(usize, bool, &'a str),
    Number(usize, &'a str, &'a str),
}

fn indent_of(s: &str) -> (usize, &str) {
    let mut cols = 0;
    for (i, c) in s.char_indices() {
        match c {
            ' ' => cols += 1,
            '\t' => cols += 4,
            _ => return (cols, &s[i..]),
        }
    }
    (cols, "")
}

fn list_item(raw: &str) -> Option<Item<'_>> {
    let (cols, rest) = indent_of(raw);
    let mut chars = rest.chars();
    let first = chars.next()?;
    if matches!(first, '-' | '*' | '+') && rest[1..].starts_with(' ') {
        let body = rest[2..].trim_start();
        for (mark, done) in [("[ ] ", false), ("[x] ", true), ("[X] ", true)] {
            if let Some(b) = body.strip_prefix(mark) {
                return Some(Item::Task(cols, done, b));
            }
        }
        return Some(Item::Bullet(cols, body));
    }
    let digits = rest.chars().take_while(char::is_ascii_digit).count();
    if (1..=3).contains(&digits) {
        let after = &rest[digits..];
        if (after.starts_with(". ") || after.starts_with(") ")) && !after[2..].trim().is_empty() {
            return Some(Item::Number(cols, &rest[..digits], after[2..].trim_start()));
        }
    }
    None
}

/// `---`, `***` or `___`, three or more, spaces allowed between.
fn is_rule(t: &str) -> bool {
    let s: String = t.chars().filter(|c| *c != ' ').collect();
    s.len() >= 3 && ["-", "*", "_"].iter().any(|m| s.chars().all(|c| c.to_string() == *m))
}

impl Markdown {
    /// The lines `raw` becomes: none while a table is being read, the
    /// finished table when a table ends, else one line.
    pub fn push(&mut self, raw: &str, t: &Theme) -> Vec<Line<'static>> {
        if self.fence.is_none() && is_table_row(raw) {
            self.table.push(raw.to_string());
            return vec![];
        }
        let mut out = self.finish(t);
        if let Some(l) = self.line(raw, t) {
            out.push(l);
        }
        out
    }

    /// A table still being held, drawn now (at the end of a message).
    pub fn finish(&mut self, t: &Theme) -> Vec<Line<'static>> {
        if self.table.is_empty() {
            return vec![];
        }
        let rows: Vec<Vec<String>> = std::mem::take(&mut self.table).iter().map(|r| cells(r)).collect();
        let n = rows.iter().map(Vec::len).max().unwrap_or(0);
        // Right-aligned columns: `--:` in the separator row.
        let right: Vec<bool> = rows
            .iter()
            .find(|r| is_separator(r))
            .map(|r| (0..n).map(|i| r.get(i).is_some_and(|c| c.ends_with(':') && !c.starts_with(':'))).collect())
            .unwrap_or_else(|| vec![false; n]);
        // The cells, styled now; laid out later, at the width of the moment (`lay_out_table`),
        // so a narrower window wraps them inside their columns instead of breaking the rows.
        let has_header = rows.get(1).is_some_and(|r| is_separator(r));
        let mut spans = vec![Span::styled(TABLE_MARK, t.hairline())];
        for (ri, r) in rows.iter().enumerate() {
            if is_separator(r) {
                continue;
            }
            let header = ri == 0 && has_header;
            spans.push(Span::raw(if header { TABLE_HEAD } else { TABLE_ROW }));
            for (i, &to_right) in right.iter().enumerate().take(n) {
                spans.push(Span::raw(if to_right { TABLE_CELL_RIGHT } else { TABLE_CELL }));
                let c = r.get(i).map(String::as_str).unwrap_or("");
                let mut cell = inline(c, t);
                // Code in a cell keeps its colour, not its shade: shaded blocks break the columns.
                for s in &mut cell {
                    s.style.bg = None;
                    if header {
                        s.style = s.style.patch(t.teal().add_modifier(Modifier::BOLD));
                    }
                }
                spans.extend(cell.into_iter().filter(|s| !s.content.is_empty()));
            }
        }
        vec![Line::from(spans)]
    }

    /// One line. A code fence opens with its language on a gutter line and
    /// closes with the gutter's foot; code inside is highlighted.
    pub fn line(&mut self, raw: &str, t: &Theme) -> Option<Line<'static>> {
        // The fence's rule in a quiet grey that still reads, so the frame never looks half-drawn.
        let gutter = |s: &'static str| Span::styled(s, t.subtle());
        let trimmed = raw.trim_start();
        if let Some((marker, lang)) = self.fence.clone() {
            if trimmed.starts_with(marker.as_str())
                && trimmed.trim_start_matches(marker.chars().next().unwrap()).trim().is_empty()
            {
                self.fence = None;
                return Some(Line::from(gutter("╰─")));
            }
            let mut spans = vec![gutter("│ ")];
            spans.extend(highlight(raw, &lang, t));
            return Some(Line::from(spans));
        }
        for marker in ["```", "~~~"] {
            if let Some(rest) = trimmed.strip_prefix(marker) {
                let ch = marker.chars().next().unwrap();
                let run = marker.len() + rest.chars().take_while(|c| *c == ch).count();
                let lang = rest.trim_start_matches(ch).split_whitespace().next().unwrap_or("").to_lowercase();
                self.fence = Some((ch.to_string().repeat(run), lang.clone()));
                let label = if lang.is_empty() { String::new() } else { format!(" {lang}") };
                return Some(Line::from(vec![gutter("╭─"), Span::styled(label, t.subtle())]));
            }
        }
        // Headings: `#` to `######` and a space.
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            let text = trimmed[hashes..].trim().trim_end_matches('#').trim_end();
            let style = match hashes {
                1 => t.brand_strong().add_modifier(Modifier::BOLD),
                2 => t.teal().add_modifier(Modifier::BOLD),
                3 => t.violet().add_modifier(Modifier::BOLD),
                _ => t.bold(),
            };
            let spans =
                inline(text, t).into_iter().map(|s| Span::styled(s.content, s.style.patch(style))).collect::<Vec<_>>();
            return Some(Line::from(spans));
        }
        if is_rule(trimmed) && !trimmed.is_empty() {
            return Some(Line::from(Span::styled("─".repeat(40), t.hairline())));
        }
        // Quotes, nested: a bar per level, the text quieter.
        if trimmed.starts_with('>') {
            let mut rest = trimmed;
            let mut depth = 0;
            while let Some(r) = rest.strip_prefix('>') {
                depth += 1;
                rest = r.trim_start();
            }
            let mut spans: Vec<Span<'static>> = (0..depth).map(|_| Span::styled("│ ", t.violet())).collect();
            let quiet = t.dim().add_modifier(Modifier::ITALIC);
            spans.extend(inline(rest, t).into_iter().map(|s| Span::styled(s.content, quiet.patch(s.style))));
            return Some(Line::from(spans));
        }
        if let Some(item) = list_item(raw) {
            let pad = |cols: usize| " ".repeat(cols.div_ceil(2) * 2);
            let mut spans = vec![];
            match item {
                Item::Bullet(cols, text) => {
                    let level = cols / 2;
                    let (glyph, st) = match level % 3 {
                        0 => ("• ", t.accent()),
                        1 => ("◦ ", t.teal()),
                        _ => ("▪ ", t.violet()),
                    };
                    spans.push(Span::raw(pad(cols)));
                    spans.push(Span::styled(glyph, st));
                    spans.extend(inline(text, t));
                }
                Item::Task(cols, done, text) => {
                    spans.push(Span::raw(pad(cols)));
                    if done {
                        spans.push(Span::styled("✓ ", t.success()));
                        let quiet = t.dim().add_modifier(Modifier::CROSSED_OUT);
                        spans
                            .extend(inline(text, t).into_iter().map(|s| Span::styled(s.content, quiet.patch(s.style))));
                    } else {
                        spans.push(Span::styled("○ ", t.subtle()));
                        spans.extend(inline(text, t));
                    }
                }
                Item::Number(cols, n, text) => {
                    spans.push(Span::raw(pad(cols)));
                    spans.push(Span::styled(format!("{n}. "), t.teal()));
                    spans.extend(inline(text, t));
                }
            }
            return Some(Line::from(spans));
        }
        Some(Line::from(inline(raw, t)))
    }

    pub fn reset(&mut self) {
        self.fence = None;
        self.table.clear();
    }
}

/// Inline markdown: `**bold**`, `__bold__`, `*italic*`, `_italic_`,
/// `~~strike~~`, `` `code` ``, `[text](url)`, `<https://…>` and bare links,
/// and `\` escapes. Emphasis nests (`**bold `code`**`).
pub fn inline(s: &str, t: &Theme) -> Vec<Span<'static>> {
    let mut out = vec![];
    inline_into(s, Style::default(), t, &mut out);
    // Neighbours in one style become one span.
    let mut merged: Vec<Span<'static>> = vec![];
    for sp in out {
        match merged.last_mut() {
            Some(m) if m.style == sp.style => m.content.to_mut().push_str(&sp.content),
            _ => merged.push(sp),
        }
    }
    if merged.is_empty() {
        merged.push(Span::raw(String::new()));
    }
    merged
}

fn link_style(t: &Theme) -> Style {
    t.info().add_modifier(Modifier::UNDERLINED)
}

fn inline_into(s: &str, base: Style, t: &Theme, out: &mut Vec<Span<'static>>) {
    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut Vec<Span<'static>>| {
        if !plain.is_empty() {
            out.push(Span::styled(std::mem::take(plain), base));
        }
    };
    let mut i = 0;
    let prev_alnum = |i: usize| s[..i].chars().next_back().is_some_and(|c| c.is_alphanumeric());
    while i < s.len() {
        let rest = &s[i..];
        // Escapes.
        if let Some(r) = rest.strip_prefix('\\') {
            if let Some(c) = r.chars().next().filter(|c| c.is_ascii_punctuation()) {
                plain.push(c);
                i += 1 + c.len_utf8();
                continue;
            }
        }
        // Code: a run of backticks, closed by the same run.
        if rest.starts_with('`') {
            let n = rest.chars().take_while(|c| *c == '`').count();
            let fence = &rest[..n];
            if let Some(end) = rest[n..].find(fence) {
                let body = rest[n..n + end].trim();
                if !body.is_empty() {
                    flush(&mut plain, out);
                    // Without colour, code keeps its backticks: nothing else sets it apart.
                    let text = if t.color { body.to_string() } else { format!("`{body}`") };
                    out.push(Span::styled(text, base.patch(t.code())));
                    i += n + end + n;
                    continue;
                }
            }
            plain.push_str(fence);
            i += n;
            continue;
        }
        // Links: [text](url).
        if rest.starts_with('[') {
            if let Some(close) = rest.find("](") {
                if let Some(end) = rest[close + 2..].find(')') {
                    let text = &rest[1..close];
                    let url = &rest[close + 2..close + 2 + end];
                    if !text.is_empty() && !url.contains(' ') {
                        flush(&mut plain, out);
                        inline_into(text, base.patch(link_style(t)), t, out);
                        if url != text && !url.starts_with('#') {
                            out.push(Span::styled(format!(" ({url})"), base.patch(t.subtle())));
                        }
                        i += close + 2 + end + 1;
                        continue;
                    }
                }
            }
        }
        // Links: <https://…> and bare ones.
        if rest.starts_with("<http") {
            if let Some(end) = rest.find('>') {
                flush(&mut plain, out);
                out.push(Span::styled(rest[1..end].to_string(), base.patch(link_style(t))));
                i += end + 1;
                continue;
            }
        }
        if (rest.starts_with("https://") || rest.starts_with("http://")) && !prev_alnum(i) {
            let end = rest.find(|c: char| c.is_whitespace() || c == ')' || c == '>').unwrap_or(rest.len());
            let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
            flush(&mut plain, out);
            out.push(Span::styled(url.to_string(), base.patch(link_style(t))));
            i += url.len();
            continue;
        }
        // Emphasis.
        let delims: [(&str, Modifier); 5] = [
            ("**", Modifier::BOLD),
            ("__", Modifier::BOLD),
            ("~~", Modifier::CROSSED_OUT),
            ("*", Modifier::ITALIC),
            ("_", Modifier::ITALIC),
        ];
        let mut matched = false;
        for (d, m) in delims {
            if !rest.starts_with(d) {
                continue;
            }
            let after = &rest[d.len()..];
            // Opens only before text, and `_` only at a word's start (snake_case stays).
            if after.starts_with(' ') || after.is_empty() || (d.starts_with('_') && prev_alnum(i)) {
                break;
            }
            let mut from = 0;
            let close = loop {
                match after[from..].find(d) {
                    Some(p) => {
                        let at = from + p;
                        let before_ok = !after[..at].ends_with(' ');
                        let next_ok = !d.starts_with('_')
                            || !after[at + d.len()..].chars().next().is_some_and(char::is_alphanumeric);
                        // `*` must not close on the first `*` of a `**`.
                        let single_star_ok =
                            d != "*" || !after[at..].starts_with("**") || after[at + 1..].starts_with("**");
                        if at > 0 && before_ok && next_ok && single_star_ok {
                            break Some(at);
                        }
                        from = at + d.len();
                    }
                    None => break None,
                }
            };
            if let Some(end) = close {
                flush(&mut plain, out);
                inline_into(&after[..end], base.add_modifier(m), t, out);
                i += d.len() + end + d.len();
                matched = true;
            }
            break;
        }
        if matched {
            continue;
        }
        let c = rest.chars().next().unwrap();
        plain.push(c);
        i += c.len_utf8();
    }
    flush(&mut plain, out);
}

/// Which keywords and comment marker a fence's language uses.
fn lang_family(lang: &str) -> Option<(&'static [&'static str], &'static [&'static str])> {
    const RUST: &[&str] = &[
        "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern", "false", "fn",
        "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return", "self",
        "Self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where", "while", "Some",
        "None", "Ok", "Err",
    ];
    const PY: &[&str] = &[
        "and", "as", "assert", "async", "await", "break", "class", "continue", "def", "del", "elif", "else", "except",
        "False", "finally", "for", "from", "global", "if", "import", "in", "is", "lambda", "None", "not", "or", "pass",
        "raise", "return", "self", "True", "try", "while", "with", "yield",
    ];
    const JS: &[&str] = &[
        "async",
        "await",
        "break",
        "case",
        "catch",
        "class",
        "const",
        "continue",
        "default",
        "delete",
        "do",
        "else",
        "enum",
        "export",
        "extends",
        "false",
        "finally",
        "for",
        "from",
        "function",
        "if",
        "implements",
        "import",
        "in",
        "instanceof",
        "interface",
        "let",
        "new",
        "null",
        "of",
        "private",
        "public",
        "readonly",
        "return",
        "static",
        "switch",
        "this",
        "throw",
        "true",
        "try",
        "type",
        "typeof",
        "undefined",
        "var",
        "void",
        "while",
        "yield",
    ];
    const GO: &[&str] = &[
        "break",
        "case",
        "chan",
        "const",
        "continue",
        "default",
        "defer",
        "else",
        "false",
        "for",
        "func",
        "go",
        "if",
        "import",
        "interface",
        "map",
        "nil",
        "package",
        "range",
        "return",
        "select",
        "struct",
        "switch",
        "true",
        "type",
        "var",
    ];
    const C: &[&str] = &[
        "auto",
        "bool",
        "break",
        "case",
        "char",
        "class",
        "const",
        "continue",
        "default",
        "delete",
        "do",
        "double",
        "else",
        "enum",
        "extern",
        "false",
        "float",
        "for",
        "if",
        "include",
        "int",
        "long",
        "namespace",
        "new",
        "nullptr",
        "private",
        "public",
        "return",
        "short",
        "sizeof",
        "static",
        "struct",
        "switch",
        "template",
        "this",
        "true",
        "typedef",
        "unsigned",
        "using",
        "void",
        "while",
        "final",
        "import",
        "package",
        "extends",
        "implements",
        "interface",
        "throws",
        "fun",
        "val",
        "var",
        "func",
        "let",
        "guard",
        "override",
        "protocol",
    ];
    const SH: &[&str] = &[
        "case", "do", "done", "elif", "else", "esac", "export", "fi", "for", "function", "if", "in", "local", "return",
        "then", "while", "until", "set", "unset", "source", "sudo", "cd", "echo",
    ];
    const SQL: &[&str] = &[
        "select", "from", "where", "and", "or", "not", "insert", "into", "values", "update", "set", "delete", "create",
        "table", "drop", "alter", "join", "left", "right", "inner", "outer", "on", "group", "by", "order", "limit",
        "as", "null", "is", "in", "primary", "key", "index", "having", "distinct", "count", "union",
    ];
    const DATA: &[&str] = &["true", "false", "null", "yes", "no", "on", "off"];
    const SLASH: &[&str] = &["//"];
    const HASH: &[&str] = &["#"];
    const DASH: &[&str] = &["--"];
    const NONE: &[&str] = &[];
    Some(match lang {
        "rust" | "rs" => (RUST, SLASH),
        "python" | "py" | "python3" => (PY, HASH),
        "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" | "mjs" | "cjs" => (JS, SLASH),
        "go" | "golang" => (GO, SLASH),
        "c" | "h" | "cpp" | "c++" | "cc" | "hpp" | "java" | "kotlin" | "kt" | "swift" | "cs" | "csharp" | "scala"
        | "dart" | "php" => (C, SLASH),
        "sh" | "bash" | "zsh" | "shell" | "console" | "fish" | "ps1" | "powershell" => (SH, HASH),
        "sql" | "postgres" | "mysql" | "sqlite" => (SQL, DASH),
        "json" | "jsonc" => (DATA, NONE),
        "yaml" | "yml" | "toml" | "ini" | "dockerfile" | "make" | "makefile" | "ruby" | "rb" | "r" | "perl" | "nix" => {
            (DATA, HASH)
        }
        "lua" | "haskell" | "hs" => (DATA, DASH),
        _ => return None,
    })
}

/// One line of code, coloured by token: keywords, strings, numbers,
/// comments and type names. A diff colours its added and removed lines.
pub fn highlight(line: &str, lang: &str, t: &Theme) -> Vec<Span<'static>> {
    let text = t.role(palette::CODE_TEXT);
    if lang == "diff" || lang == "patch" {
        let st = match line.chars().next() {
            Some('+') if !line.starts_with("+++") => t.added(),
            Some('-') if !line.starts_with("---") => t.removed(),
            Some('@') => t.info(),
            _ => text,
        };
        return vec![Span::styled(line.to_string(), st)];
    }
    let Some((keywords, comments)) = lang_family(lang) else {
        return vec![Span::styled(line.to_string(), text)];
    };
    let sql = keywords.contains(&"select");
    let (kw, string, num, comment, ty) = (
        t.role(palette::CODE_KEYWORD),
        t.role(palette::CODE_STRING),
        t.role(palette::CODE_NUMBER),
        t.role(palette::CODE_COMMENT).add_modifier(Modifier::ITALIC),
        t.role(palette::CODE_TYPE),
    );
    let mut out: Vec<Span<'static>> = vec![];
    let push = |out: &mut Vec<Span<'static>>, s: &str, st: Style| match out.last_mut() {
        Some(l) if l.style == st => l.content.to_mut().push_str(s),
        _ => out.push(Span::styled(s.to_string(), st)),
    };
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut k = 0;
    while k < chars.len() {
        let (i, c) = chars[k];
        let rest = &line[i..];
        if let Some(m) = comments.iter().find(|m| rest.starts_with(**m)) {
            // `#` starts a comment only at a word boundary (not `$#`, `a#b`).
            let ok = *m != "#" || i == 0 || line[..i].ends_with([' ', '\t']);
            if ok {
                push(&mut out, rest, comment);
                break;
            }
        }
        if c == '"' || c == '\'' || (c == '`' && keywords.contains(&"function")) {
            // In Rust, `'a` is a lifetime, not a string.
            let lifetime = c == '\''
                && keywords.contains(&"fn")
                && !rest[1..].chars().nth(1).is_some_and(|n| n == '\'')
                && !rest[1..].starts_with('\\');
            if !lifetime {
                let mut j = k + 1;
                while j < chars.len() && chars[j].1 != c {
                    if chars[j].1 == '\\' {
                        j += 1;
                    }
                    j += 1;
                }
                let end = chars.get(j + 1).map(|x| x.0).unwrap_or(line.len());
                push(&mut out, &line[i..end], string);
                k = j + 1;
                continue;
            }
        }
        if c.is_ascii_digit() && !line[..i].chars().next_back().is_some_and(|p| p.is_alphanumeric() || p == '_') {
            let mut j = k;
            while j < chars.len() && (chars[j].1.is_ascii_alphanumeric() || chars[j].1 == '.' || chars[j].1 == '_') {
                j += 1;
            }
            let end = chars.get(j).map(|x| x.0).unwrap_or(line.len());
            push(&mut out, &line[i..end], num);
            k = j;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let mut j = k;
            while j < chars.len() && (chars[j].1.is_alphanumeric() || chars[j].1 == '_') {
                j += 1;
            }
            let end = chars.get(j).map(|x| x.0).unwrap_or(line.len());
            let word = &line[i..end];
            let is_kw = if sql { keywords.contains(&word.to_lowercase().as_str()) } else { keywords.contains(&word) };
            let st = if is_kw {
                kw
            } else if word.chars().next().is_some_and(char::is_uppercase) && word.chars().any(char::is_lowercase) {
                ty
            } else {
                text
            };
            push(&mut out, word, st);
            k = j;
            continue;
        }
        let mut buf = [0u8; 4];
        push(&mut out, c.encode_utf8(&mut buf), text);
        k += 1;
    }
    if out.is_empty() {
        out.push(Span::styled(String::new(), text));
    }
    out
}

/// Display width of a string.
pub fn width(s: &str) -> usize {
    s.chars().map(|c| c.width().unwrap_or(0)).sum()
}

/// Wrap styled lines to `width` columns, breaking at spaces where it can.
/// The indent a wrapped row continues at: the line's leading spaces plus a
/// leading marker (`● `, `└ `, `❯ `, `- `, `* `, `> `, `12. `), so lists,
/// answers and tool results wrap under their text, not at column 0.
fn hanging_indent(cells: &[(char, Style)], max: usize) -> usize {
    let text: String = cells.iter().map(|(c, _)| *c).collect();
    let lead = text.len() - text.trim_start_matches(' ').len();
    let rest = &text[lead..];
    let marker = [
        "● ", "└ ", "❯ ", "▲ ", "✕ ", "✗ ", "◆ ", "■ ", "· ", "• ", "◦ ", "▪ ", "✓ ", "○ ", "│ ", "› ", "↳ ", "- ",
        "* ", "> ", "‖ ", "» ",
    ]
    .iter()
    .find(|m| rest.starts_with(**m))
    .map(|m| m.chars().count())
    .or_else(|| {
        let digits = rest.chars().take_while(char::is_ascii_digit).count();
        (digits > 0 && digits < 4 && rest[digits..].starts_with(". ")).then_some(digits + 2)
    })
    .unwrap_or(0);
    // Spaces after the marker belong to it too (`↳  text`).
    let after = rest.chars().skip(marker).take_while(|c| *c == ' ').count();
    let start = lead + marker + if marker > 0 { after } else { 0 };
    // Columns (`Ctrl+R      Search earlier prompts`, `Model:   x`, `ok   git   found`):
    // continue under the last one that starts near the left, so a wrapped
    // value doesn't look like a key. Columns are set apart by 2+ spaces.
    let chars: Vec<char> = text.chars().collect();
    let limit = start + COLUMN_LIMIT.min(max.saturating_sub(start));
    let mut column = None;
    let mut i = start;
    while i < chars.len() && i < limit {
        if chars[i] != ' ' {
            i += 1;
            continue;
        }
        let mut j = i;
        while j < chars.len() && chars[j] == ' ' {
            j += 1;
        }
        if j - i >= 2 && j < chars.len() && j <= limit {
            column = Some(j);
        }
        i = j;
    }
    width(&chars[..column.unwrap_or(start).min(chars.len())].iter().collect::<String>())
}

/// How far from the text's start a value column may begin and still be used as the indent.
const COLUMN_LIMIT: usize = 28;

pub fn wrap(lines: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    let width = (width as usize).max(1);
    let mut out = vec![];
    for line in lines {
        let base = line.style;
        let cells: Vec<(char, Style)> =
            line.spans.iter().flat_map(|s| s.content.chars().map(move |c| (c, base.patch(s.style)))).collect();
        if cells.is_empty() {
            out.push(Line::default());
            continue;
        }
        // Continuation rows start under the text; never more than half the width.
        let indent = hanging_indent(&cells, width / 2).min(width / 2);
        let mut start = 0;
        let mut first = true;
        while start < cells.len() {
            let room = if first { width } else { width - indent };
            let width = room.max(1);
            let mut w = 0;
            let mut end = start;
            let mut last_space = None;
            while end < cells.len() {
                let cw = cells[end].0.width().unwrap_or(0);
                if w + cw > width && end > start {
                    break;
                }
                if cells[end].0 == ' ' {
                    last_space = Some(end);
                }
                w += cw;
                end += 1;
            }
            let mut next = end;
            if end < cells.len() {
                // On the first row, a break inside the padding before the value
                // column would leave a bare label: break the long value instead.
                let label = |sp: usize| first && width_of(&cells[start..sp]) <= indent;
                if let Some(sp) = last_space.filter(|sp| *sp > start && !label(*sp)) {
                    // Break after the space; the space ends this row.
                    end = sp;
                    next = sp + 1;
                }
            }
            let mut row = spans_of(&cells[start..end]);
            if !first && indent > 0 {
                row.spans.insert(0, Span::raw(" ".repeat(indent)));
            }
            out.push(row);
            first = false;
            start = next;
            // A continuation row doesn't start with the space it broke at.
            while start < cells.len() && cells[start].0 == ' ' && indent > 0 {
                start += 1;
            }
        }
    }
    out
}

fn width_of(cells: &[(char, Style)]) -> usize {
    cells.iter().map(|(c, _)| c.width().unwrap_or(0)).sum()
}

fn spans_of(cells: &[(char, Style)]) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = vec![];
    for (c, st) in cells {
        match spans.last_mut() {
            Some(s) if s.style == *st => s.content.to_mut().push(*c),
            _ => spans.push(Span::styled(c.to_string(), *st)),
        }
    }
    Line::from(spans)
}

/// A line's text without styles (tests).
#[cfg(test)]
pub fn plain(line: &Line) -> String {
    line.spans.iter().map(|s| s.content.as_ref()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Theme = Theme { color: true, light: false, accent: None, depth: Depth::True };

    #[test]
    fn markdown_lines_keep_track_of_fences() {
        let mut md = Markdown::default();
        assert_eq!(plain(&md.line("## Plan", &T).unwrap()), "Plan");
        let l = md.line("Run `cargo test` and **check** it", &T).unwrap();
        assert_eq!(plain(&l), "Run cargo test and check it");
        assert_eq!(l.spans[1].style, T.code());
        assert_eq!(l.spans[3].style, T.bold());
        let mono = Theme { color: false, ..T };
        let l = Markdown::default().line("Run `cargo test` and **check** it", &mono).unwrap();
        assert_eq!(plain(&l), "Run `cargo test` and check it", "without colour, code keeps its backticks");
        assert_eq!(plain(&md.line("```rust", &T).unwrap()), "╭─ rust", "a fence opens with its language");
        assert_eq!(plain(&md.line("# not a heading", &T).unwrap()), "│ # not a heading");
        assert_eq!(plain(&md.line("```", &T).unwrap()), "╰─");
        assert_eq!(plain(&md.line("# heading", &T).unwrap()), "heading");
        assert_eq!(plain(&md.line("a ` lone backtick", &T).unwrap()), "a ` lone backtick");
    }

    fn one(raw: &str) -> String {
        plain(&Markdown::default().line(raw, &T).unwrap())
    }

    #[test]
    fn answers_and_the_persons_words_have_their_own_colours() {
        let line = T.answer(Line::from(vec![
            Span::raw("plain "),
            Span::styled("bold", Style::default().add_modifier(Modifier::BOLD)),
            Span::styled("code", T.code()),
        ]));
        assert_eq!(line.spans[0].style.fg, Some(palette::ANSWER.color(Depth::True, false)));
        assert_eq!(line.spans[1].style.fg, Some(palette::STRONG.color(Depth::True, false)));
        assert_eq!(line.spans[2].style.fg, T.code().fg, "a span's own colour stays");
        assert!(T.code().bg.is_some(), "inline code on its own shade");
        let sixteen = Theme { depth: Depth::Ansi16, ..T };
        assert_eq!(sixteen.answer(Line::from("x")).spans[0].style.fg, None, "16 colours keep the terminal's own");
    }

    #[test]
    fn every_kind_of_block_is_drawn() {
        // Headings by level: lime, then teal, then violet.
        let h1 = Markdown::default().line("# Title #", &T).unwrap();
        assert_eq!(plain(&h1), "Title");
        assert!(h1.spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(h1.spans[0].style.fg, T.brand_strong().fg);
        assert_eq!(Markdown::default().line("## Plan", &T).unwrap().spans[0].style.fg, T.teal().fg);
        assert_eq!(Markdown::default().line("### Step", &T).unwrap().spans[0].style.fg, T.violet().fg);
        assert_ne!(T.teal().fg, T.accent().fg, "the second level is not lime again");
        assert_eq!(one("#hashtag"), "#hashtag", "no space, no heading");
        // Lists: bullets by level, numbers, tasks.
        assert_eq!(one("- one"), "• one");
        assert_eq!(one("* one"), "• one");
        assert_eq!(one("  - two"), "  ◦ two");
        assert_eq!(one("    + three"), "    ▪ three");
        assert_eq!(one("3. third"), "3. third");
        assert_eq!(one("12) twelfth"), "12. twelfth");
        assert_eq!(one("- [ ] todo"), "○ todo");
        assert_eq!(one("- [x] done"), "✓ done");
        assert!(Markdown::default().line("- [x] done", &T).unwrap().spans[2]
            .style
            .add_modifier
            .contains(Modifier::CROSSED_OUT));
        // Quotes, nested, and rules.
        assert_eq!(one("> said"), "│ said");
        assert_eq!(one(">> twice"), "│ │ twice");
        assert_eq!(one("---"), "─".repeat(40));
        assert_eq!(one("* * *"), "─".repeat(40));
        // Code: a gutter, the language, highlighting, and `~~~` fences too.
        let mut md = Markdown::default();
        assert_eq!(plain(&md.line("~~~python", &T).unwrap()), "╭─ python");
        let l = md.line("def f(x): return \"hi\"  # note", &T).unwrap();
        assert_eq!(plain(&l), "│ def f(x): return \"hi\"  # note");
        let style_of = |w: &str| l.spans.iter().find(|s| s.content.contains(w)).map(|s| s.style).unwrap();
        assert_eq!(style_of("def").fg, T.role(palette::CODE_KEYWORD).fg);
        assert_eq!(style_of("\"hi\"").fg, T.role(palette::CODE_STRING).fg);
        assert!(style_of("# note").add_modifier.contains(Modifier::ITALIC));
        assert_eq!(plain(&md.line("```", &T).unwrap()), "│ ```", "a ~~~ fence closes only with ~~~");
        assert_eq!(plain(&md.line("~~~", &T).unwrap()), "╰─");
        assert_eq!(one("plain"), "plain", "the fence closed");
    }

    #[test]
    fn inline_styles_links_and_escapes() {
        let spans = |s: &str| inline(s, &T);
        let text = |s: &str| spans(s).iter().map(|x| x.content.to_string()).collect::<String>();
        assert_eq!(text("an *italic* and _also_ word"), "an italic and also word");
        assert!(spans("an *italic*")[1].style.add_modifier.contains(Modifier::ITALIC));
        assert_eq!(text("snake_case_name stays"), "snake_case_name stays");
        assert_eq!(text("2 * 3 * 4"), "2 * 3 * 4", "a lone star is not emphasis");
        assert_eq!(text("~~gone~~ now"), "gone now");
        assert!(spans("~~gone~~")[0].style.add_modifier.contains(Modifier::CROSSED_OUT));
        assert_eq!(text("**bold `code`**"), "bold code");
        let nested = spans("**bold `code`**");
        assert!(nested[1].style.add_modifier.contains(Modifier::BOLD) && nested[1].style.fg == T.code().fg);
        assert_eq!(text("see [the docs](https://x.dev/a) now"), "see the docs (https://x.dev/a) now");
        assert!(spans("[docs](https://x.dev)")[0].style.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(text("at https://x.dev/a."), "at https://x.dev/a.");
        assert_eq!(spans("at https://x.dev/a.")[1].content, "https://x.dev/a");
        assert_eq!(text("<https://x.dev>"), "https://x.dev");
        assert_eq!(text("\\*not italic\\*"), "*not italic*");
        assert_eq!(text("``a `b` c``"), "a `b` c");
    }

    #[test]
    fn tables_line_up_when_they_end() {
        // Laid out at a width, as the transcript is.
        let at = |lines: Vec<Line<'static>>, w: u16| -> Vec<String> {
            crate::tui::app::lay_out(lines, w).iter().map(plain).collect()
        };
        let mut md = Markdown::default();
        assert!(md.push("| Check | Result |", &T).is_empty());
        assert!(md.push("| --- | --- |", &T).is_empty());
        assert!(md.push("| `cargo test` | 12 passed |", &T).is_empty());
        let out = at(md.push("After.", &T), 80);
        assert_eq!(out, ["Check      │ Result", "───────────┼──────────", "cargo test │ 12 passed", "After."]);
        assert!(md.push("| a | b |", &T).is_empty());
        assert_eq!(md.finish(&T).len(), 1, "a table at the end of a message is drawn by finish");
        // `--:` aligns a column right.
        for r in ["| Name | Cost |", "| --- | ---: |", "| a | 1 |", "| bb | 100 |"] {
            md.push(r, &T);
        }
        let out = at(md.finish(&T), 80);
        assert_eq!(out[2], "a    │    1");
    }

    #[test]
    fn a_table_fits_a_narrow_window_without_breaking_its_rows() {
        let at = |w: u16| -> Vec<String> {
            let mut md = Markdown::default();
            for r in [
                "| Setting | What it does |",
                "| --- | --- |",
                "| autoCompact | Summarizes the conversation when the context is nearly full |",
                "| theme | dark, light or none |",
            ] {
                md.push(r, &T);
            }
            crate::tui::app::lay_out(md.finish(&T), w).iter().map(plain).collect()
        };
        // Short of room: the long column wraps inside itself; every row stays within the
        // width and the bars stay in one column.
        let rows = at(40);
        assert!(rows.iter().all(|r| width(r) <= 40), "{rows:#?}");
        let bars: Vec<usize> = rows.iter().filter_map(|r| r.chars().position(|c| c == '│' || c == '┼')).collect();
        assert!(bars.len() >= 5 && bars.windows(2).all(|p| p[0] == p[1]), "straight bars: {rows:#?}");
        assert!(rows.iter().any(|r| r.starts_with("autoCompact │ Summarizes")), "{rows:#?}");
        assert!(rows.iter().any(|r| r.starts_with("            │ ")), "a cell continues in its column: {rows:#?}");
        // Very narrow: each row a block of `Header: value`.
        let rows = at(20);
        assert!(rows.iter().all(|r| width(r) <= 20), "{rows:#?}");
        assert!(rows.contains(&"Setting: theme".to_string()), "{rows:#?}");
        assert!(rows.iter().any(|r| r.starts_with("What it does: dark")), "{rows:#?}");
        // Wide: the table as written.
        assert_eq!(at(120)[0], "Setting     │ What it does");
    }

    #[test]
    fn wraps_at_spaces_and_by_display_width() {
        let rows: Vec<String> = wrap(vec![Line::from("the quick brown fox jumps")], 10).iter().map(plain).collect();
        assert_eq!(rows, ["the quick", "brown fox", "jumps"]);
        let rows: Vec<String> = wrap(vec![Line::from("abcdefghijkl")], 5).iter().map(plain).collect();
        assert_eq!(rows, ["abcde", "fghij", "kl"]);
        // Wide characters take two columns.
        let rows: Vec<String> = wrap(vec![Line::from("日本語のテキスト")], 6).iter().map(plain).collect();
        assert_eq!(rows, ["日本語", "のテキ", "スト"]);
        assert_eq!(wrap(vec![Line::default()], 5).len(), 1);
        // Lists, answers and results wrap under their text.
        let rows: Vec<String> = wrap(vec![Line::from("  - one two three four")], 12).iter().map(plain).collect();
        assert_eq!(rows, ["  - one two", "    three", "    four"]);
        let rows: Vec<String> = wrap(vec![Line::from("  ↳  alpha beta gamma")], 14).iter().map(plain).collect();
        assert_eq!(rows, ["  ↳  alpha", "     beta", "     gamma"]);
        let rows: Vec<String> = wrap(vec![Line::from("12. aaaa bbbb")], 9).iter().map(plain).collect();
        assert_eq!(rows, ["12. aaaa", "    bbbb"]);
        // Two columns: the description wraps under itself.
        let rows: Vec<String> =
            wrap(vec![Line::from("Ctrl+C    clear the input or exit")], 24).iter().map(plain).collect();
        assert_eq!(rows, ["Ctrl+C    clear the", "          input or exit"]);
        // A doctor row continues under its detail; a long path breaks in place.
        let rows: Vec<String> =
            wrap(vec![Line::from("FAIL session      no key was sent anywhere")], 40).iter().map(plain).collect();
        assert_eq!(rows, ["FAIL session      no key was sent", "                  anywhere"]);
        let rows: Vec<String> =
            wrap(vec![Line::from("Directory:   /tmp/a-long/path/to/proj")], 30).iter().map(plain).collect();
        assert_eq!(rows, ["Directory:   /tmp/a-long/path/", "             to/proj"]);
        // Styles survive the wrap.
        let l = Line::from(vec![Span::raw("aa "), Span::styled("bbbb", T.bold())]);
        let w = wrap(vec![l], 4);
        assert_eq!(w[1].spans[0].style, T.bold());
    }
}
