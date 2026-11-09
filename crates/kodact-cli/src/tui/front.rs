//! The full-screen front page, one per style (`look.rs`): what the session
//! has, laid out before the first message. Pixel art (the wordmark in four
//! looks, the mark) is drawn in half blocks, two square pixels per cell.

use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use super::brand::{centred, GAP, GLYPHS, MARK_W, SHORT_TAGLINE, TAGLINE, WORDMARK_W};
use super::look::Look;
use super::palette;
use super::render::fit;
use super::text::{width, Theme};

/// One pixel: its colour (hex) and the ANSI colour for 16-colour terminals.
type Px = Option<(u32, Color)>;

/// How the wordmark is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Letters {
    /// "Kod" in paper, "Act" in blue.
    Flat,
    /// Paper and blue letters over a deep navy shadow, one pixel down and right.
    Shadow,
    /// Lit from above: ice blue at the top, a deep Signal blue at the foot.
    Glow,
    /// The letters' edges only, in blue.
    Outline,
}

/// What the front page shows about the session.
#[derive(Debug, Clone, Default)]
pub struct FrontInfo {
    pub version: String,
    pub model: String,
    pub endpoint: String,
    /// The folder, as people write it (`~/proj`).
    pub folder: String,
    pub branch: Option<String>,
    /// MCP servers: (name, status).
    pub mcp: Vec<(String, String)>,
    pub tools: usize,
    /// The newest other conversations here: (what, how long ago).
    pub recent: Vec<(String, String)>,
}

/// A front page for `full_view`: rows pinned to the top, rows above and below
/// the input, where the input sits and how wide it is.
#[derive(Debug, Clone, Default)]
pub struct FrontPage {
    /// Rows at the top of the screen, whatever its height.
    pub header: Vec<Line<'static>>,
    pub above: Vec<Line<'static>>,
    pub below: Vec<Line<'static>>,
    /// The input's first column and width.
    pub box_left: usize,
    pub box_w: usize,
    /// The mode, model and endpoint on a line under the input.
    pub under: bool,
    /// The input sits at the bottom of the screen, not with the page.
    pub dock: bool,
    /// The page starts at the top (under the header) instead of centred down the screen.
    pub top: bool,
}

fn ink(hex: u32, ansi: Color, t: &Theme) -> Color {
    if !t.color {
        return Color::Reset;
    }
    palette::ink(hex, t.depth, ansi)
}

/// A grid of pixels in half blocks: one line per two rows.
fn half_blocks(grid: &[Vec<Px>], t: &Theme) -> Vec<Line<'static>> {
    let colour = |p: Px| p.map(|(hex, ansi)| ink(hex, ansi, t));
    let fg = |c: Color| if c == Color::Reset { Style::default() } else { Style::default().fg(c) };
    let w = grid.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = vec![];
    for pair in grid.chunks(2) {
        let mut spans: Vec<Span<'static>> = vec![];
        for x in 0..w {
            let u = pair[0].get(x).copied().flatten().and_then(|p| colour(Some(p)));
            let l = pair.get(1).and_then(|r| r.get(x).copied().flatten()).and_then(|p| colour(Some(p)));
            let (text, style) = match (u, l) {
                (None, None) => (" ", Style::default()),
                (Some(u), None) => ("▀", fg(u)),
                (None, Some(l)) => ("▄", fg(l)),
                (Some(u), Some(l)) if u == l => ("█", fg(u)),
                (Some(u), Some(l)) => ("▀", fg(u).bg(l)),
            };
            match spans.last_mut() {
                Some(s) if s.style == style => s.content.to_mut().push_str(text),
                _ => spans.push(Span::styled(text.to_string(), style)),
            }
        }
        while spans.last().is_some_and(|s| s.content.trim().is_empty() && s.style.bg.is_none()) {
            spans.pop();
        }
        out.push(Line::from(spans));
    }
    out
}

/// The compact wordmark's letters: one-pixel strokes, eight pixels tall (four
/// terminal rows), so the name sits at the size of a heading and its pixels
/// stay fine. Capitals K and A, lowercase o, d, c and t on one baseline.
const COMPACT: [&[&str; 8]; 6] = [
    // K
    &["X...X", "X..X.", "X.X..", "XX...", "XX...", "X.X..", "X..X.", "X...X"],
    // o
    &[".....", ".....", ".....", ".XXX.", "X...X", "X...X", "X...X", ".XXX."],
    // d
    &["....X", "....X", "....X", ".XXXX", "X...X", "X...X", "X...X", ".XXXX"],
    // A
    &["..X..", ".X.X.", ".X.X.", "X...X", "XXXXX", "X...X", "X...X", "X...X"],
    // c
    &[".....", ".....", ".....", ".XXXX", "X....", "X....", "X....", ".XXXX"],
    // t
    &[".....", ".X...", ".X...", "XXXX.", ".X...", ".X...", ".X...", "..XX."],
];

/// Columns between the compact letters.
const COMPACT_GAP: usize = 1;

/// The compact wordmark's pixels: "Kod" in paper, "Act" in blue.
fn compact_grid(light: bool) -> Vec<Vec<Px>> {
    let (paper, blue) = if light {
        ((0x0b0c0e, Color::Black), (0x0a5cff, Color::Blue))
    } else {
        ((0xf4f5f7, Color::White), (0x4d8dff, Color::LightBlue))
    };
    (0..8)
        .map(|y| {
            let mut row: Vec<Px> = vec![];
            for (gi, g) in COMPACT.iter().enumerate() {
                if gi > 0 {
                    row.extend(std::iter::repeat_n(None, COMPACT_GAP));
                }
                let ink = if gi < 3 { paper } else { blue };
                row.extend(g[y].chars().map(|c| (c == 'X').then_some(ink)));
            }
            row
        })
        .collect()
}

/// The compact wordmark: 4 rows.
pub fn compact_wordmark(t: &Theme) -> Vec<Line<'static>> {
    half_blocks(&compact_grid(t.light), t)
}

/// The compact wordmark's width in columns.
pub fn compact_w() -> usize {
    compact_grid(false)[0].len()
}

/// The wordmark's pixels in one of its looks: 52 by 10, 53 by 11 with a
/// shadow. "Kod" is paper (ink on a light background) and "Act" blue.
fn wordmark_grid(look: Letters, light: bool) -> Vec<Vec<Px>> {
    let (paper, blue, shadow) = if light {
        ((0x0b0c0e, Color::Black), (0x0a5cff, Color::Blue), (0xb7c9ee, Color::Gray))
    } else {
        ((0xf4f5f7, Color::White), (0x4d8dff, Color::LightBlue), (0x16306a, Color::Blue))
    };
    // The glow: ice at the top, settling to a deep Signal blue at the foot.
    const GLOW: [(u32, Color); 10] = [
        (0xe8f0ff, Color::White),
        (0xc9dbff, Color::White),
        (0xa6c5ff, Color::LightBlue),
        (0x86b1ff, Color::LightBlue),
        (0x6a9fff, Color::LightBlue),
        (0x4d8dff, Color::LightBlue),
        (0x3a7bf7, Color::Blue),
        (0x2a6ae6, Color::Blue),
        (0x1f5acc, Color::Blue),
        (0x184aab, Color::Blue),
    ];
    let step = 7 + GAP;
    let base = 6 * 7 + 5 * GAP;
    let (w, h) = if look == Letters::Shadow { (base + 1, 11) } else { (base, 10) };
    let mut letter = vec![vec![None::<usize>; w]; h];
    for (gi, rows) in GLYPHS.iter().enumerate() {
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.chars().enumerate() {
                if c != '.' {
                    letter[y][gi * step + x] = Some(gi);
                }
            }
        }
    }
    let at = |x: isize, y: isize| -> Option<usize> {
        (x >= 0 && y >= 0).then(|| letter.get(y as usize).and_then(|r| r.get(x as usize)).copied().flatten()).flatten()
    };
    let mut grid = vec![vec![None; w]; h];
    for (y, row) in grid.iter_mut().enumerate() {
        for (x, cell) in row.iter_mut().enumerate() {
            let (xi, yi) = (x as isize, y as isize);
            *cell = match at(xi, yi) {
                Some(gi) => match look {
                    Letters::Glow => Some(GLOW[y.min(9)]),
                    Letters::Outline => {
                        let inside =
                            [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().all(|(dx, dy)| at(xi + dx, yi + dy).is_some());
                        (!inside).then_some(blue)
                    }
                    _ if gi < 3 => Some(paper),
                    _ => Some(blue),
                },
                None if look == Letters::Shadow && at(xi - 1, yi - 1).is_some() => Some(shadow),
                None => None,
            };
        }
    }
    grid
}

/// The wordmark in `look`: 5 rows (6 with a shadow).
pub fn wordmark(t: &Theme, look: Letters) -> Vec<Line<'static>> {
    half_blocks(&wordmark_grid(look, t.light), t)
}

/// The wordmark's width in columns.
pub fn wordmark_w(look: Letters) -> usize {
    WORDMARK_W + usize::from(look == Letters::Shadow)
}

/// KodAct's mark (a step and a chevron in Signal blue): [`MARK_W`] columns by 6 rows.
pub fn mark_art(t: &Theme) -> Vec<Line<'static>> {
    super::brand::mark(t)
}

// ---- layout helpers ----

fn line_w(l: &Line) -> usize {
    l.spans.iter().map(|s| width(&s.content)).sum()
}

/// Art `w` columns wide, centred in `cols` as one block, so its rows stay aligned.
fn centre_block(lines: Vec<Line<'static>>, w: usize, cols: usize) -> Vec<Line<'static>> {
    let x = cols.saturating_sub(w) / 2;
    lines.into_iter().map(|l| at(x, l)).collect()
}

/// `l` starting at column `x`.
fn at(x: usize, l: Line<'static>) -> Line<'static> {
    let mut spans = vec![Span::raw(" ".repeat(x))];
    spans.extend(l.spans);
    Line::from(spans)
}

/// Two blocks side by side: `left` at column `x`, `right` at column `rx`, top-aligned.
fn beside(x: usize, left: Vec<Line<'static>>, rx: usize, right: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let n = left.len().max(right.len());
    (0..n)
        .map(|i| {
            let l = left.get(i).cloned().unwrap_or_default();
            let used = x + line_w(&l);
            let mut spans = vec![Span::raw(" ".repeat(x))];
            spans.extend(l.spans);
            if let Some(r) = right.get(i) {
                spans.push(Span::raw(" ".repeat(rx.saturating_sub(used))));
                spans.extend(r.spans.clone());
            }
            Line::from(spans)
        })
        .collect()
}

/// `left` at column `x` and `right` ending at column `end`.
fn spread(x: usize, left: Vec<Span<'static>>, end: usize, right: Vec<Span<'static>>) -> Line<'static> {
    let lw: usize = left.iter().map(|s| width(&s.content)).sum();
    let rw: usize = right.iter().map(|s| width(&s.content)).sum();
    let mut spans = vec![Span::raw(" ".repeat(x))];
    spans.extend(left);
    spans.push(Span::raw(" ".repeat(end.saturating_sub(x + lw + rw).max(1))));
    spans.extend(right);
    Line::from(spans)
}

/// `s` in at most `max` columns, its end kept: `…/acme/shop`.
fn keep_end(s: &str, max: usize) -> String {
    if width(s) <= max {
        return s.to_string();
    }
    let mut tail: Vec<char> = vec![];
    let mut used = 1;
    for c in s.chars().rev() {
        let cw = width(&c.to_string());
        if used + cw > max {
            break;
        }
        used += cw;
        tail.push(c);
    }
    tail.reverse();
    format!("…{}", tail.into_iter().collect::<String>())
}

fn mcp_text(info: &FrontInfo) -> Option<String> {
    let on: Vec<&str> = info.mcp.iter().filter(|(_, s)| s == "connected").map(|(n, _)| n.as_str()).collect();
    (!on.is_empty()).then(|| on.join(", "))
}

fn resume_spans(t: &Theme, what: &str, ago: &str, room: usize) -> Vec<Span<'static>> {
    vec![
        Span::styled("/resume", t.accent()),
        Span::raw("  "),
        Span::styled(fit(what, room.saturating_sub(width(ago) + 14).max(8)), t.dim()),
        Span::styled("  ·  ", t.subtle()),
        Span::styled(ago.to_string(), t.subtle()),
    ]
}

/// `l` cut to `w` columns.
fn clip(l: Line<'static>, w: usize) -> Line<'static> {
    let mut used = 0;
    let mut spans = vec![];
    for s in l.spans {
        if used >= w {
            break;
        }
        let room = w - used;
        if width(&s.content) <= room {
            used += width(&s.content);
            spans.push(s);
        } else {
            let mut text = String::new();
            for c in s.content.chars() {
                let cw = width(&c.to_string());
                if used + cw > w {
                    break;
                }
                used += cw;
                text.push(c);
            }
            spans.push(Span::styled(text, s.style));
        }
    }
    Line::from(spans)
}

/// A rounded box `w` wide at column `x`, its rows padded inside.
fn boxed(x: usize, w: usize, rows: Vec<Line<'static>>, border: Style) -> Vec<Line<'static>> {
    let inner = w.saturating_sub(4);
    let mut out = vec![at(x, Line::from(Span::styled(format!("╭{}╮", "─".repeat(w.saturating_sub(2))), border)))];
    for r in rows {
        let r = clip(r, inner);
        let used = line_w(&r);
        let mut spans = vec![Span::raw(" ".repeat(x)), Span::styled("│ ", border)];
        spans.extend(r.spans);
        spans.push(Span::raw(" ".repeat(inner - used)));
        spans.push(Span::styled(" │", border));
        out.push(Line::from(spans));
    }
    out.push(at(x, Line::from(Span::styled(format!("╰{}╯", "─".repeat(w.saturating_sub(2))), border))));
    out
}

// ---- the pages ----

/// The front page for `look`, `cols` wide. A terminal too narrow for a style's
/// layout gets the bare one.
pub fn front_page(t: &Theme, look: Look, info: &FrontInfo, cols: usize) -> FrontPage {
    let need = match look {
        Look::Card => 84,
        // Essentials draws the name in one row on a narrow terminal.
        Look::Frame => 0,
        Look::Ledger | Look::Status => 80,
        Look::Glow | Look::Bare => 0,
    };
    if cols < need {
        return bare(t, info, cols);
    }
    match look {
        Look::Frame => frame(t, info, cols),
        Look::Glow => glow(t, info, cols),
        Look::Card => mark_card(t, info, cols),
        Look::Ledger => ledger(t, info, cols),
        Look::Status => status_board(t, info, cols),
        Look::Bare => bare(t, info, cols),
    }
}

/// The name in one row, for a terminal too narrow for the wordmark.
fn small_name(t: &Theme) -> Vec<Span<'static>> {
    super::brand::wordmark_text(t).spans
}

/// 1 · Essentials: the compact wordmark alone, centred (four rows), "From code to impact." under them, then the
/// input (the frame style's panel, which names the mode, the model and where
/// it runs) and one line to resume the last conversation. A server that failed
/// to start says so; everything else (the folder, the branch, the version) is
/// on the status line.
fn frame(t: &Theme, info: &FrontInfo, cols: usize) -> FrontPage {
    let one = compact_w();
    let (mut above, lw) = if cols < one + 4 {
        // Too narrow for the letters: the name in one row.
        (vec![centred(Line::from(small_name(t)), cols)], 6)
    } else {
        // The name alone: no mark beside it.
        let (logo, lw) = (compact_wordmark(t), one);
        (centre_block(logo, lw, cols), lw)
    };
    // The panel at least as wide as the name above it, so the logo never overhangs the input.
    let fw = cols.saturating_sub(8).clamp(cols.min(20), 96.max(lw));
    let m = (cols - fw) / 2;
    // The text column inside the panel: the lines under it start there too.
    let col = m + 4;
    let room = fw.saturating_sub(4);
    above.push(Line::default());
    above.push(centred(Line::from(Span::styled(SHORT_TAGLINE, t.dim())), cols));
    above.push(Line::default());
    above.push(Line::default());
    let mut below = vec![];
    match info.recent.first() {
        Some((what, ago)) => {
            below.push(Line::default());
            below.push(at(col, Line::from(resume_spans(t, what, ago, room))));
        }
        // A first run: one thing to try, and where the project's instructions go.
        None => {
            below.push(Line::default());
            below.push(at(
                col,
                Line::from(vec![
                    Span::styled("Try ", t.subtle()),
                    Span::styled("\"explain this project\"", t.dim()),
                    Span::styled("  ·  ", t.subtle()),
                    Span::styled("/init", t.accent()),
                    Span::styled(" writes KODACT.md for it", t.subtle()),
                ]),
            ));
        }
    }
    let failed: Vec<&str> = info.mcp.iter().filter(|(_, s)| s == "failed").map(|(n, _)| n.as_str()).collect();
    if !failed.is_empty() {
        below.push(Line::default());
        below.push(at(
            col,
            Line::from(vec![
                Span::styled("● ", t.warning()),
                Span::styled(fit(&failed.join(", "), room.saturating_sub(24)), t.dim()),
                Span::styled(" didn't start  ·  ", t.subtle()),
                Span::styled("/mcp", t.accent()),
            ]),
        ));
    }
    // The panel names the model and where it runs.
    FrontPage { above, below, box_left: m, box_w: fw, under: true, ..Default::default() }
}

/// 2 · Centred, the name as hot metal with sparks over it and a glowing seam under it.
fn glow(t: &Theme, info: &FrontInfo, cols: usize) -> FrontPage {
    let mut above = vec![];
    let ww = wordmark_w(Letters::Glow);
    if cols >= ww + 4 {
        // Sparks rising off the letters, at fixed places.
        let spark = |hex: u32| Style::default().fg(ink(hex, Color::LightBlue, t));
        let rows: [&[(usize, &str, u32)]; 2] = [
            &[(9, "·", 0x9cc0ff), (21, "˙", 0xe8f0ff), (33, "·", 0x4d8dff)],
            &[(4, "˙", 0x3a7bf7), (15, "·", 0x9cc0ff), (27, "·", 0x6a9fff), (40, "˙", 0x4d8dff)],
        ];
        for r in rows {
            let mut spans = vec![];
            let mut x = 0;
            for (at_x, ch, hex) in r {
                spans.push(Span::raw(" ".repeat(at_x - x)));
                spans.push(Span::styled(ch.to_string(), if t.color { spark(*hex) } else { t.subtle() }));
                x = at_x + 1;
            }
            above.push(at(cols.saturating_sub(ww) / 2, Line::from(spans)));
        }
        above.extend(centre_block(wordmark(t, Letters::Glow), ww, cols));
        // The seam: brightest in the middle.
        let n = 40usize;
        let seam: Vec<Span<'static>> = (0..n)
            .map(|i| {
                let d = (i as f32 - n as f32 / 2.0).abs() / (n as f32 / 2.0);
                let hex = if d < 0.2 {
                    0xe8f0ff
                } else if d < 0.5 {
                    0x4d8dff
                } else if d < 0.8 {
                    0x1f5acc
                } else {
                    0x0f1f40
                };
                Span::styled("━", if t.color { Style::default().fg(ink(hex, Color::LightBlue, t)) } else { t.subtle() })
            })
            .collect();
        above.push(centred(Line::from(seam), cols));
    } else {
        above.push(centred(Line::from(small_name(t)), cols));
    }
    above.push(Line::default());
    let tagline = Line::from(Span::styled(TAGLINE, t.dim()));
    above.extend(
        super::text::wrap(vec![tagline], cols.saturating_sub(4).max(8) as u16).into_iter().map(|l| centred(l, cols)),
    );
    above.push(Line::default());
    above.push(Line::default());
    let bw = cols.saturating_sub(8).min(72);
    let mut below = vec![];
    if let Some((what, ago)) = info.recent.first() {
        below.push(Line::default());
        below.push(centred(Line::from(resume_spans(t, what, ago, cols)), cols));
    }
    FrontPage { above, below, box_left: (cols - bw) / 2, box_w: bw, under: true, ..Default::default() }
}

/// 3 · A card: the mark beside the name and the session's readouts; the input under it.
fn mark_card(t: &Theme, info: &FrontInfo, cols: usize) -> FrontPage {
    let cw = cols.saturating_sub(8).min(86);
    let x = (cols - cw) / 2;
    let border = t.hairline();
    let inner = cw - 4;
    let mut rows = vec![];
    rows.push(spread(
        0,
        vec![Span::styled("kodact", t.accent()), Span::styled(format!("  v{}", info.version), t.subtle())],
        inner,
        vec![Span::styled("● ", t.success()), Span::styled("ready", t.dim())],
    ));
    rows.push(Line::from(Span::styled("─".repeat(inner), border)));
    rows.push(Line::default());
    let mut right = wordmark(t, Letters::Flat);
    right.push(Line::default());
    let tag = if inner >= MARK_W + 5 + width(TAGLINE) { TAGLINE } else { "from code to impact" };
    right.push(Line::from(Span::styled(tag, t.dim())));
    rows.extend(beside(1, mark_art(t), MARK_W + 5, right));
    rows.push(Line::default());
    let kv = |k: &str, v: String, d: String| {
        Line::from(vec![Span::styled(format!(" {k:<8}"), t.subtle()), Span::raw(v), Span::styled(d, t.subtle())])
    };
    rows.push(kv("model", info.model.clone(), format!("  on {}", info.endpoint)));
    let tools = match mcp_text(info) {
        Some(n) => format!("  ·  {n}"),
        None => String::new(),
    };
    rows.push(kv("tools", format!("{} built in", info.tools), tools));
    let branch = info.branch.as_ref().map(|b| format!("  ·  {b}")).unwrap_or_default();
    rows.push(kv("folder", keep_end(&info.folder, inner.saturating_sub(30)), branch));
    let mut below = vec![];
    if let Some((what, ago)) = info.recent.first() {
        below.push(Line::default());
        below.push(centred(Line::from(resume_spans(t, what, ago, cols)), cols));
    }
    FrontPage { above: boxed(x, cw, rows, border), below, box_left: x, box_w: cw, ..Default::default() }
}

/// 4 · Aligned left: the name in blue outline, past work numbered beside this folder,
/// the input docked at the bottom.
fn ledger(t: &Theme, info: &FrontInfo, cols: usize) -> FrontPage {
    let m = 8;
    let end = cols - m;
    let mut header = vec![];
    let side = vec![
        Line::default(),
        Line::from(Span::styled(format!("v{}", info.version), t.subtle())),
        Line::from(Span::styled(info.model.clone(), t.dim())),
        Line::from(Span::styled(info.endpoint.clone(), t.subtle())),
    ];
    let sw = side.iter().map(line_w).max().unwrap_or(0);
    header.extend(beside(m, wordmark(t, Letters::Outline), end - sw, side));
    header.push(Line::default());
    header.push(at(m, Line::from(Span::styled("─".repeat(end - m), t.hairline()))));
    header.push(Line::default());
    let half = m + (end - m) / 2 + 4;
    let mut left = vec![Line::from(Span::styled("Pick up where you left off", t.subtle())), Line::default()];
    if info.recent.is_empty() {
        left.push(Line::from(Span::styled("Nothing yet in this folder.", t.dim())));
    }
    for (i, (what, ago)) in info.recent.iter().take(3).enumerate() {
        left.push(Line::from(vec![
            Span::styled(format!("{:<4}", i + 1), t.accent()),
            Span::raw(fit(what, half.saturating_sub(m + 20))),
            Span::styled(format!("  {ago}"), t.subtle()),
        ]));
    }
    let mut right = vec![
        Line::from(Span::styled("This folder", t.subtle())),
        Line::default(),
        Line::from(keep_end(&info.folder, end.saturating_sub(half))),
    ];
    if let Some(b) = &info.branch {
        right.push(Line::from(Span::styled(format!("⎇ {b}"), t.dim())));
    }
    if let Some(n) = mcp_text(info) {
        right.push(Line::from(vec![Span::styled("● ", t.success()), Span::styled(n, t.dim())]));
    }
    header.extend(beside(m, left, half, right));
    // The page sits centred down the screen, above the docked input.
    FrontPage { above: header, box_left: 2, box_w: cols - 4, dock: true, ..Default::default() }
}

/// 5 · The name over three boxed readouts of the session, the input as wide as them.
fn status_board(t: &Theme, info: &FrontInfo, cols: usize) -> FrontPage {
    let total = cols.saturating_sub(8).min(96);
    let x = (cols - total) / 2;
    let mut above = centre_block(wordmark(t, Letters::Shadow), wordmark_w(Letters::Shadow), cols);
    above.push(Line::default());
    above.push(Line::default());
    let tw = (total - 4) / 3;
    let tile = |k: &str, v: String, d: String| -> Vec<Line<'static>> {
        boxed(
            0,
            tw,
            vec![
                Line::from(Span::styled(k.to_string(), t.accent())),
                Line::from(fit(&v, tw - 4)),
                Line::from(Span::styled(fit(&d, tw - 4), t.subtle())),
            ],
            t.hairline(),
        )
    };
    let tiles = [
        tile("Model", info.model.clone(), info.endpoint.clone()),
        tile(
            "Tools",
            format!("{} built in", info.tools),
            mcp_text(info).map(|n| format!("{n} over MCP")).unwrap_or_else(|| "no MCP servers".into()),
        ),
        tile("Folder", keep_end(&info.folder, tw - 4), info.branch.clone().unwrap_or_default()),
    ];
    for r in 0..tiles[0].len() {
        let mut spans = vec![Span::raw(" ".repeat(x))];
        for (i, tl) in tiles.iter().enumerate() {
            if i > 0 {
                spans.push(Span::raw("  "));
            }
            spans.extend(tl[r].spans.clone());
        }
        above.push(Line::from(spans));
    }
    above.push(Line::default());
    let mut below = vec![];
    if let Some((what, ago)) = info.recent.first() {
        below.push(Line::default());
        below.push(centred(Line::from(resume_spans(t, what, ago, cols)), cols));
    }
    FrontPage { above, below, box_left: x, box_w: total, ..Default::default() }
}

/// 6 · A small name in the corner, one question and the input in the middle, recent work under it.
fn bare(t: &Theme, info: &FrontInfo, cols: usize) -> FrontPage {
    // The model and endpoint on the right, when there is room for them beside the name.
    let where_ = format!("{} · {}", info.model, info.endpoint);
    let right = if cols >= 2 + 9 + 2 + width(&info.version) + 4 + width(&where_) + 2 {
        vec![Span::styled(where_, t.subtle())]
    } else {
        vec![]
    };
    // No room beside the name: the model goes under the input instead.
    let under = right.is_empty();
    let header = vec![
        Line::default(),
        spread(
            2,
            {
                let mut v = small_name(t);
                v.push(Span::styled(format!("  {}", info.version), t.subtle()));
                v
            },
            cols.saturating_sub(2),
            right,
        ),
    ];
    let bw = cols.saturating_sub(4).min(72);
    let x = (cols - bw) / 2;
    let above = vec![centred(Line::from(Span::styled("What shall we build today?", t.bold())), cols), Line::default()];
    let mut below = vec![];
    if !info.recent.is_empty() {
        below.push(Line::default());
        for (what, ago) in info.recent.iter().take(3) {
            below.push(spread(
                x + 2,
                vec![Span::styled(fit(what, bw.saturating_sub(20)), t.dim())],
                x + bw - 2,
                vec![Span::styled(ago.clone(), t.subtle())],
            ));
        }
    }
    FrontPage { header, above, below, box_left: x, box_w: bw, under, ..Default::default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::palette::Depth;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(crate::tui::text::plain).collect()
    }

    const DARK: Theme = Theme { color: true, light: false, accent: None, depth: Depth::True };

    fn info() -> FrontInfo {
        FrontInfo {
            version: "0.1.0".into(),
            model: "stand-in".into(),
            endpoint: "127.0.0.1:11434".into(),
            folder: "~/acme/shop".into(),
            branch: Some("fix/cart-rounding".into()),
            mcp: vec![("jira".into(), "connected".into()), ("figma".into(), "failed".into())],
            tools: 18,
            recent: vec![("checkout rounding".into(), "2h ago".into())],
        }
    }

    #[test]
    fn the_wordmark_has_four_looks_and_the_mark_is_whole() {
        assert_eq!(wordmark(&DARK, Letters::Flat).len(), 5);
        assert_eq!(wordmark(&DARK, Letters::Shadow).len(), 6, "the shadow adds a pixel row");
        let heat = wordmark(&DARK, Letters::Glow);
        let fg = |l: &Line| {
            l.spans.iter().find(|s| s.content.contains('█') || s.content.contains('▀')).and_then(|s| s.style.fg)
        };
        assert_ne!(fg(&heat[0]), fg(&heat[4]), "hot at the top, cooler at the foot");
        let outline = plain(&wordmark(&DARK, Letters::Outline));
        assert!(outline.iter().any(|l| l.contains(' ')), "hollow letters");
        let a = mark_art(&DARK);
        assert_eq!(a.len(), 6);
        assert!(a.iter().all(|l| line_w(l) <= MARK_W));
    }

    #[test]
    fn every_style_fits_and_says_what_the_session_has() {
        for look in Look::ALL {
            for cols in [140, 100, 84, 60, 40] {
                let p = front_page(&DARK, look, &info(), cols);
                let rows: Vec<&Line> = p.header.iter().chain(&p.above).chain(&p.below).collect();
                for l in &rows {
                    assert!(line_w(l) <= cols, "{look:?} {cols}: {:?}", plain(std::slice::from_ref(*l)));
                }
                assert!(p.box_left + p.box_w <= cols, "{look:?} {cols}: the input fits");
                let text = rows.iter().map(|l| crate::tui::text::plain(l)).collect::<Vec<_>>().join("\n");
                assert!(text.contains("stand-in") || p.under, "{look:?} {cols}: the model shows\n{text}");
            }
        }
        let frame = plain(&front_page(&DARK, Look::Frame, &info(), 120).above).join("\n");
        assert!(!frame.contains("Welcome to"), "the name speaks for itself");
        // The compact name: four rows, whether or not there is history.
        let first = FrontInfo { recent: vec![], ..info() };
        let art = |p: &FrontPage| plain(&p.above).iter().filter(|l| l.contains(['▀', '▄', '█'])).count();
        assert_eq!(art(&front_page(&DARK, Look::Frame, &first, 140)), 4, "four rows on a first run");
        assert_eq!(art(&front_page(&DARK, Look::Frame, &info(), 140)), 4, "and after");
        assert!(front_page(&DARK, Look::Frame, &first, 140).above.iter().all(|l| line_w(l) <= 140));
        let first_below = plain(&front_page(&DARK, Look::Frame, &first, 120).below).join("\n");
        assert!(first_below.contains("/init writes KODACT.md"), "a first run says what to try: {first_below}");
        // The panel is never narrower than the name over it.
        let p = front_page(&DARK, Look::Frame, &first, 140);
        assert!(p.box_w >= compact_w(), "{}", p.box_w);
        assert!(frame.contains(SHORT_TAGLINE));
        // Essentials: what the panel or the status line already says is not repeated.
        for gone in ["Version", "tools ready", "KodAct reads", "~/acme/shop", "jira"] {
            assert!(!frame.contains(gone), "{gone}\n{frame}");
        }
        let below = plain(&front_page(&DARK, Look::Frame, &info(), 120).below).join("\n");
        assert!(below.contains("/resume  checkout rounding"), "{below}");
        assert!(below.contains("figma didn't start  ·  /mcp"), "a server that failed says so: {below}");
        let ledger = front_page(&DARK, Look::Ledger, &info(), 120);
        assert!(ledger.dock && plain(&ledger.above).join("\n").contains("1   checkout rounding"));
        let board = plain(&front_page(&DARK, Look::Status, &info(), 120).above).join("\n");
        assert!(board.contains("Model") && board.contains("Tools") && board.contains("Folder"));
        // Too narrow for a style's layout: the bare page.
        let narrow = front_page(&DARK, Look::Card, &info(), 60);
        assert!(plain(&narrow.header).join("").contains("KodAct"));
        // Essentials keeps its layout on a narrow terminal: the name in one row, no second question.
        assert_eq!(art(&front_page(&DARK, Look::Frame, &info(), 50)), 4, "the compact name fits 50 columns");
        let narrow = plain(&front_page(&DARK, Look::Frame, &info(), 30).above).join("\n");
        assert!(narrow.contains("KodAct") && narrow.contains(SHORT_TAGLINE), "{narrow}");
        assert!(!narrow.contains("What shall we build"), "{narrow}");
        // The name alone, in colour or not: no mark beside it, centred.
        let mono = Theme { color: false, ..DARK };
        for t in [DARK, mono] {
            let above = front_page(&t, Look::Frame, &info(), 140).above;
            let art: Vec<&Line> =
                above.iter().filter(|l| plain(&[(*l).clone()])[0].contains(['▀', '▄', '█'])).collect();
            let w = art.iter().map(|l| line_w(l)).max().unwrap();
            assert!(w <= (140 + compact_w()) / 2 + 1, "only the letters, centred: {w}");
        }
    }
}
