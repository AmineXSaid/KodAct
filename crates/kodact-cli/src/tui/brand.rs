//! KodAct's name in the terminal: the KodAct wordmark and its mark (a step
//! and a chevron, from code to impact, in lime), drawn as pixels in half blocks
//! (each cell is two pixels stacked, so the pixels come out square), and the
//! two screens they head: the front screen and the first-run card.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use super::palette;
use super::text::{width, Theme};

/// The wordmark's letters, seven pixels wide and ten tall (five terminal
/// rows): capitals K and A, lowercase o, d, c and t on the same baseline,
/// after the logo's geometric letters (the A pointed, the t's foot curled).
/// `.` is empty and `X` a letter.
pub(super) const GLYPHS: [[&str; 10]; 6] = [
    // K
    ["XX...XX", "XX..XX.", "XX.XX..", "XXXX...", "XXX....", "XXX....", "XXXX...", "XX.XX..", "XX..XX.", "XX...XX"],
    // o
    [".......", ".......", ".......", ".XXXXX.", "XXXXXXX", "XX...XX", "XX...XX", "XX...XX", "XXXXXXX", ".XXXXX."],
    // d
    [".....XX", ".....XX", ".....XX", ".XXXXXX", "XXXXXXX", "XX...XX", "XX...XX", "XX...XX", "XXXXXXX", ".XXXXXX"],
    // A
    ["..XXX..", "..XXX..", ".XX.XX.", ".XX.XX.", "XX...XX", "XXXXXXX", "XXXXXXX", "XX...XX", "XX...XX", "XX...XX"],
    // c
    [".......", ".......", ".......", ".XXXXXX", "XXXXXXX", "XX.....", "XX.....", "XX.....", "XXXXXXX", ".XXXXXX"],
    // t
    [".......", ".XX....", ".XX....", "XXXXXX.", "XXXXXX.", ".XX....", ".XX....", ".XX....", ".XXXXXX", "..XXXXX"],
];

/// Columns between letters.
pub(super) const GAP: usize = 2;

/// The wordmark's width in columns.
pub const WORDMARK_W: usize = 6 * 7 + 5 * GAP;

/// The short tagline, for the default front page.
pub const SHORT_TAGLINE: &str = "From code to impact, on any model.";

/// The tagline under the wordmark.
pub const TAGLINE: &str = "From code to impact: a coding agent on the model you choose.";

/// The wordmark's pixel rows: `P` for "Kod" in paper, `G` for "Act" in lime,
/// so the name reads as what it does.
fn wordmark_rows() -> Vec<String> {
    (0..10)
        .map(|y| {
            let mut row = String::new();
            for (i, g) in GLYPHS.iter().enumerate() {
                if i > 0 {
                    row.push_str(&".".repeat(GAP));
                }
                row.extend(g[y].chars().map(|c| match c {
                    'X' if i < 3 => 'P',
                    'X' => 'G',
                    other => other,
                }));
            }
            row
        })
        .collect()
}

/// KodAct's mark, traced from the logo: a step up from the left, then a
/// chevron pointing right, every slant on one slope. 20 pixels by 12 (six
/// terminal rows); `M` is the mark.
const MARK: [&str; 12] = [
    "............##......",
    "...........####.....",
    "..........######....",
    "............#####...",
    "......####...#####..",
    ".....######...#####.",
    "...######......#####",
    "..######.....######.",
    ".######....#######..",
    "######....#######...",
    ".........#######....",
    "........#######.....",
];

/// The mark's width in columns; it is half as many rows tall as [`MARK`] has.
pub const MARK_W: usize = 20;

/// The mark's pixel rows.
pub fn mark_rows() -> Vec<String> {
    MARK.iter().map(|r| r.replace('#', "M")).collect()
}

/// The inks for a light or dark background: hex, and the ANSI colour for
/// 16-colour terminals. Paper letters (ink on a light background), and lime
/// for "Act" and the mark, deepened to an olive that reads on a light background.
fn ink(c: char, light: bool) -> Option<(u32, Color)> {
    Some(match (c, light) {
        ('P', false) => (0xf2f0e9, Color::White),
        ('P', true) => (0x101214, Color::Black),
        ('G' | 'M', false) => (0xc6ff3d, Color::LightGreen),
        ('G' | 'M', true) => (0x4a7200, Color::Green),
        _ => return None,
    })
}

/// Pixel rows in half blocks. Without colour the shape is drawn in the
/// terminal's own colour.
fn sprite(rows: &[String], t: &Theme) -> Vec<Line<'static>> {
    let color_of = |c: Option<char>| -> Option<Color> {
        let (hex, ansi) = ink(c?, t.light)?;
        if !t.color {
            return Some(Color::Reset);
        }
        Some(palette::ink(hex, t.depth, ansi))
    };
    let mut out = vec![];
    for pair in rows.chunks(2) {
        let upper: Vec<char> = pair[0].chars().collect();
        let lower: Vec<char> = pair.get(1).map(|r| r.chars().collect()).unwrap_or_default();
        let mut spans: Vec<Span<'static>> = vec![];
        for x in 0..upper.len() {
            let (u, l) = (color_of(upper.get(x).copied()), color_of(lower.get(x).copied()));
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
        out.push(Line::from(spans));
    }
    out
}

pub(super) fn fg(c: Color) -> Style {
    if c == Color::Reset {
        Style::default()
    } else {
        Style::default().fg(c)
    }
}

/// The wordmark: [`WORDMARK_W`] columns by 5 rows.
pub fn wordmark(t: &Theme) -> Vec<Line<'static>> {
    sprite(&wordmark_rows(), t)
}

/// The mark, [`MARK_W`] columns wide and six rows tall.
pub fn mark(t: &Theme) -> Vec<Line<'static>> {
    sprite(&mark_rows(), t)
}

/// The name in one row, for terminals too narrow for the wordmark: `KodAct`,
/// "Act" in the brand colour.
pub fn wordmark_text(t: &Theme) -> Line<'static> {
    Line::from(vec![Span::styled("Kod", t.bold()), Span::styled("Act", t.brand_strong().add_modifier(Modifier::BOLD))])
}

/// A line centred in `cols` columns.
pub(super) fn centred(l: Line<'static>, cols: usize) -> Line<'static> {
    let w: usize = l.spans.iter().map(|s| width(&s.content)).sum();
    let mut spans = vec![Span::raw(" ".repeat(cols.saturating_sub(w) / 2))];
    spans.extend(l.spans);
    trim_end(Line::from(spans))
}

fn trim_end(mut l: Line<'static>) -> Line<'static> {
    while l.spans.last().is_some_and(|s| s.content.trim().is_empty() && s.style.bg.is_none()) {
        l.spans.pop();
    }
    l
}

/// What the front screen says.
pub struct Welcome<'a> {
    /// The newest other conversations here: (what it was about, how long ago).
    pub recent: &'a [(String, String)],
}

/// Rows the live region takes under the front screen: a gap, the input box, the line under it and the status line.
const LIVE_ROWS: usize = 6;

/// The top of the front screen, centred across `cols`: the wordmark (or
/// the name in one row on a narrow terminal), then the tagline.
pub fn front_head(t: &Theme, cols: usize) -> Vec<Line<'static>> {
    let mut block: Vec<Line<'static>> = if cols >= WORDMARK_W + 4 { wordmark(t) } else { vec![wordmark_text(t)] };
    block.push(Line::default());
    block.push(Line::from(Span::styled(TAGLINE, t.dim())));
    super::text::wrap(block, cols.saturating_sub(2).max(1) as u16).into_iter().map(|l| centred(l, cols)).collect()
}

/// The foot of the front screen, under the input: `/resume` and the newest
/// other conversation here, centred; nothing when there is none.
pub fn front_foot(t: &Theme, w: &Welcome, cols: usize) -> Vec<Line<'static>> {
    let Some((what, ago)) = w.recent.first() else { return vec![] };
    let room = cols.saturating_sub(4 + "/resume  ".len() + "  ·  ".len() + width(ago)).max(8);
    let line = Line::from(vec![
        Span::styled("/resume", t.accent()),
        Span::raw("  "),
        Span::styled(super::render::fit(what, room), t.dim()),
        Span::styled("  ·  ", t.hairline()),
        Span::styled(ago.clone(), t.subtle()),
    ]);
    vec![centred(line, cols)]
}

/// The front screen for the inline UI, printed once at the top of a session:
/// [`front_head`] and [`front_foot`], centred down the `rows` above the input.
pub fn welcome(t: &Theme, w: &Welcome, cols: usize, rows: usize) -> Vec<Line<'static>> {
    let mut block = front_head(t, cols);
    let foot = front_foot(t, w, cols);
    if !foot.is_empty() {
        block.push(Line::default());
        block.push(Line::default());
        block.extend(foot);
    }
    let pad = rows.saturating_sub(block.len() + LIVE_ROWS) / 2;
    let mut out = vec![Line::default(); pad.max(1)];
    out.extend(block);
    out.push(Line::default());
    out
}

/// Ways to give KodAct an endpoint, for the first-run card: what to set, then why.
fn setup_lines(t: &Theme) -> Vec<Line<'static>> {
    let cmd = |var: &str, value: &str| {
        if cfg!(windows) {
            format!("$env:{var}=\"{value}\"")
        } else {
            format!("export {var}={value}")
        }
    };
    let rows: [(&str, &str, &str); 5] = [
        ("KODACT_OPENAI_BASE_URL", "http://localhost:11434/v1", "Ollama; LM Studio is :1234/v1"),
        ("KODACT_OPENAI_API_KEY", "<key>", "only if it asks for one"),
        ("KODACT_MODEL", "<model>", "one the server lists"),
        ("KODACT_API_KEY", "<key>", ""),
        ("KODACT_BASE_URL", "https://gateway.example.com", "for a gateway"),
    ];
    // The notes line up in one column.
    let col = rows.iter().map(|(v, x, _)| width(&cmd(v, x))).max().unwrap_or(0) + 3;
    let set = |(var, value, note): (&str, &str, &str)| -> Line<'static> {
        let c = cmd(var, value);
        let mut spans = vec![Span::raw("    "), Span::styled("$ ", t.accent()), Span::styled(c.clone(), t.code())];
        if !note.is_empty() {
            spans.push(Span::raw(" ".repeat(col - width(&c))));
            spans.push(Span::styled(note.to_string(), t.subtle()));
        }
        Line::from(spans)
    };
    let head = |s: &str| Line::from(vec![Span::raw("  "), Span::styled(s.to_string(), t.bold())]);
    let mut out = vec![head("A local server or an OpenAI-compatible gateway")];
    out.extend(rows[..3].iter().map(|r| set(*r)));
    out.push(Line::default());
    out.push(head("A Messages API endpoint"));
    out.extend(rows[3..].iter().map(|r| set(*r)));
    out
}

/// The card's text under the wordmark, both two columns in; the wordmark
/// only where it fits.
fn text_under_wordmark(t: &Theme, cols: usize, text: Vec<Line<'static>>) -> Vec<Line<'static>> {
    let indent = |l: Line<'static>| {
        let mut spans = vec![Span::raw("  ")];
        spans.extend(l.spans);
        trim_end(Line::from(spans))
    };
    let mut out = vec![];
    if cols >= WORDMARK_W + 4 {
        out.extend(wordmark(t).into_iter().map(indent));
        out.push(Line::default());
    }
    out.extend(text.into_iter().map(indent));
    out
}

/// The first-run card, shown instead of a session when no endpoint is set
/// up: the wordmark over what KodAct is, the servers it works with, how to
/// set one up, and what KodAct found.
pub fn first_run(t: &Theme, version: &str, problem: &str, cols: usize) -> Vec<Line<'static>> {
    let eyebrow = Line::from(Span::styled(format!("KODACT  v{version}"), t.dim().add_modifier(Modifier::BOLD)));
    let headline = Line::from(vec![
        Span::styled("A coding agent that ", t.bold()),
        Span::styled("acts", t.brand_strong().add_modifier(Modifier::BOLD)),
        Span::styled(",", t.bold()),
    ]);
    let headline2 = Line::from(Span::styled("on the model you choose.", t.bold()));
    let lede = Line::from(Span::styled("Set up an endpoint and run kodact again.", t.dim()));
    let mut out = vec![Line::default()];
    let text = vec![eyebrow, Line::default(), headline, headline2, Line::default(), lede];
    out.extend(text_under_wordmark(t, cols, text));
    out.push(Line::default());
    // KodAct's runtime statement: the names stand out, the rest is quiet.
    let mut runtimes = vec![Span::raw("  ")];
    for (i, name) in ["Ollama", "LM Studio", "vLLM", "llama.cpp"].iter().enumerate() {
        if i > 0 {
            runtimes.push(Span::styled(", ", t.dim()));
        }
        runtimes.push(Span::styled(name.to_string(), t.bold()));
    }
    runtimes.push(Span::styled(" and ", t.dim()));
    runtimes.push(Span::styled("Jan", t.bold()));
    runtimes.push(Span::styled(", or any ", t.dim()));
    runtimes.push(Span::styled("OpenAI", t.bold()));
    runtimes.push(Span::styled("- or ", t.dim()));
    runtimes.push(Span::styled("Messages", t.bold()));
    runtimes.push(Span::styled("-compatible gateway.", t.dim()));
    out.push(Line::from(runtimes));
    out.push(Line::default());
    out.extend(setup_lines(t));
    out.push(Line::default());
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("KodAct found  ", t.warning()),
        Span::styled(problem.to_string(), t.dim()),
    ]));
    out.push(Line::from(vec![
        Span::raw("  "),
        Span::styled("kodact doctor --probe", t.code()),
        Span::styled(" checks what is set and asks the endpoint once.", t.dim()),
    ]));
    out.push(Line::default());
    super::text::wrap(out, cols as u16)
}

/// A line as text with ANSI styles, for printing before (or without) the UI.
pub fn ansi(line: &Line) -> String {
    let mut s = String::new();
    for span in &line.spans {
        let st = line.style.patch(span.style);
        let mut codes: Vec<String> = vec![];
        if st.add_modifier.contains(Modifier::BOLD) {
            codes.push("1".into());
        }
        if st.add_modifier.contains(Modifier::DIM) {
            codes.push("2".into());
        }
        if st.add_modifier.contains(Modifier::REVERSED) {
            codes.push("7".into());
        }
        if let Some(c) = st.fg.and_then(|c| sgr(c, false)) {
            codes.push(c);
        }
        if let Some(c) = st.bg.and_then(|c| sgr(c, true)) {
            codes.push(c);
        }
        if codes.is_empty() {
            s.push_str(&span.content);
        } else {
            s.push_str(&format!("\x1b[{}m{}\x1b[0m", codes.join(";"), span.content));
        }
    }
    s
}

fn sgr(c: Color, bg: bool) -> Option<String> {
    let base = if bg { 40 } else { 30 };
    let named = |i: u8| Some((base + i).to_string());
    let bright = |i: u8| Some((base + 60 + i).to_string());
    match c {
        Color::Reset => None,
        Color::Black => named(0),
        Color::Red => named(1),
        Color::Green => named(2),
        Color::Yellow => named(3),
        Color::Blue => named(4),
        Color::Magenta => named(5),
        Color::Cyan => named(6),
        Color::Gray => named(7),
        Color::DarkGray => bright(0),
        Color::LightRed => bright(1),
        Color::LightGreen => bright(2),
        Color::LightYellow => bright(3),
        Color::LightBlue => bright(4),
        Color::LightMagenta => bright(5),
        Color::LightCyan => bright(6),
        Color::White => bright(7),
        Color::Indexed(n) => Some(format!("{};5;{n}", if bg { 48 } else { 38 })),
        Color::Rgb(r, g, b) => Some(format!("{};2;{r};{g};{b}", if bg { 48 } else { 38 })),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::palette::Depth;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect::<String>()).collect()
    }

    const DARK: Theme = Theme { color: true, light: false, accent: None, depth: Depth::True };

    #[test]
    fn the_wordmark_is_kod_in_paper_and_act_in_lime() {
        let rows = wordmark_rows();
        assert_eq!(rows.len(), 10);
        assert!(rows.iter().all(|r| r.chars().count() == WORDMARK_W));
        assert!(
            rows.iter().flat_map(|r| r.chars()).all(|c| c == '.' || ink(c, false).is_some()),
            "every pixel has an ink"
        );
        let m = wordmark(&DARK);
        assert_eq!(m.len(), 5, "five rows");
        assert!(m.iter().all(|l| l.spans.iter().map(|s| width(&s.content)).sum::<usize>() <= WORDMARK_W));
        let cells = |l: &Line| l.spans.iter().flat_map(|s| s.content.chars().map(move |_| s.style)).collect::<Vec<_>>();
        let paper = Some(Color::Rgb(0xf2, 0xf0, 0xe9));
        let lime = Some(Color::Rgb(0xc6, 0xff, 0x3d));
        // The K's stem, at the left, in paper; the t's stem, at the right, in lime.
        assert_eq!(cells(&m[0])[0].fg, paper);
        assert!(cells(&m[4]).iter().rev().take(6).any(|s| s.fg == lime || s.bg == lime), "Act in lime");
        // On a light background the paper letters are ink, and "Act" olive.
        let light = wordmark(&Theme { light: true, ..DARK });
        assert_eq!(cells(&light[0])[0].fg, Some(Color::Rgb(0x10, 0x12, 0x14)));
        assert!(cells(&light[4]).iter().any(|s| s.fg == Some(Color::Rgb(0x4a, 0x72, 0x00))));
    }

    #[test]
    fn the_mark_is_a_step_and_a_chevron() {
        let rows = mark_rows();
        assert_eq!(rows.len(), 12);
        assert!(rows.iter().all(|r| r.chars().count() == MARK_W), "{rows:?}");
        assert!(rows.iter().flat_map(|r| r.chars()).all(|c| c == '.' || ink(c, false).is_some()));
        // The chevron's point: its top at the right of the middle, its tip at the right edge, halfway down.
        assert!(rows[0].trim_matches('.').len() <= 2, "a pointed top");
        assert!(rows[6].ends_with('M'), "the tip, halfway down");
        assert!(rows.iter().enumerate().all(|(y, r)| y == 6 || r.ends_with('.')));
        // Below the top, two strokes side by side: the step, a gap, the chevron.
        let runs = |r: &str| r.split('.').filter(|s| !s.is_empty()).count();
        for (y, row) in rows.iter().enumerate().take(10).skip(4) {
            assert_eq!(runs(row), 2, "{y}: {row}");
        }
        // Every slant on one slope: each stroke's left edge moves one pixel a row.
        let left = |r: &str| r.find('M').unwrap();
        for y in [1, 2, 7, 8, 9] {
            assert_eq!(left(&rows[y - 1]), left(&rows[y]) + 1, "{y}");
        }
        let m = mark(&DARK);
        assert_eq!(m.len(), 6);
        assert!(m.iter().all(|l| l.spans.iter().map(|s| width(&s.content)).sum::<usize>() <= MARK_W));
        assert!(m
            .iter()
            .flat_map(|l| &l.spans)
            .all(|s| s.style.fg.is_none() || s.style.fg == Some(Color::Rgb(0xc6, 0xff, 0x3d))));
    }

    #[test]
    fn without_colour_the_wordmark_keeps_its_shape() {
        let mono = Theme { color: false, ..DARK };
        let m = plain(&wordmark(&mono));
        assert!(m[0].starts_with("██"), "the K's stem: {m:?}");
        assert!(wordmark(&mono).iter().all(|l| l.spans.iter().all(|s| s.style.fg.is_none() && s.style.bg.is_none())));
    }

    #[test]
    fn the_front_screen_is_quiet_and_centred() {
        let recent = vec![("checkout rounding".to_string(), "2h ago".to_string())];
        let w = Welcome { recent: &recent };
        for cols in [120, 80, 60, 50, 40, 30] {
            let lines = welcome(&DARK, &w, cols, 40);
            for l in &lines {
                let lw: usize = l.spans.iter().map(|s| width(&s.content)).sum();
                assert!(lw <= cols, "{cols}: {lw} {:?}", plain(std::slice::from_ref(l)));
            }
            let text = plain(&lines).join(" ").split_whitespace().collect::<Vec<_>>().join(" ");
            for want in ["From code to impact: a coding agent on the model you choose.", "/resume", "check", "2h ago"] {
                assert!(text.contains(want), "{cols}: {want}\n{text}");
            }
            // Only the name, the tagline and the way back: no keys, no welcome line.
            assert!(!text.contains("shortcuts") && !text.contains("commands"), "{text}");
        }
        // Wide: the wordmark, centred across and down.
        let wide = plain(&welcome(&DARK, &w, 100, 40));
        let art: Vec<&String> = wide.iter().filter(|l| l.contains('█') || l.contains('▀') || l.contains('▄')).collect();
        assert_eq!(art.len(), 5);
        let lead = art[0].len() - art[0].trim_start().len();
        assert_eq!(lead, (100 - WORDMARK_W) / 2);
        let first = wide.iter().position(|l| !l.trim().is_empty()).unwrap();
        let last = wide.iter().rposition(|l| !l.trim().is_empty()).unwrap();
        let below = 40 - LIVE_ROWS - (last + 1);
        assert!((first as i32 - below as i32).abs() <= 2, "centred down: {first} above, {below} below");
        // Too narrow for the wordmark: the name in one row.
        let narrow = plain(&welcome(&DARK, &w, 40, 30));
        assert!(narrow.iter().any(|l| l.trim() == "KodAct"), "{narrow:?}");
        assert!(narrow.iter().all(|l| !l.contains('█')));
        // A short terminal still starts on screen; no other conversations, no resume line.
        assert_eq!(welcome(&DARK, &w, 100, 5)[0], Line::default());
        assert!(!plain(&welcome(&DARK, &Welcome { recent: &[] }, 80, 30)).join(" ").contains("/resume"));
    }

    #[test]
    fn the_first_run_card_says_how_to_set_up() {
        let text = plain(&first_run(&DARK, "0.1.0", "no API key for the Messages API", 100)).join("\n");
        for want in [
            "A coding agent that acts",
            "KODACT_OPENAI_BASE_URL",
            "KODACT_API_KEY",
            "KodAct found  no API key",
            "kodact doctor --probe",
        ] {
            assert!(text.contains(want), "{want}\n{text}");
        }
    }

    #[test]
    fn ansi_writes_the_styles() {
        let l = Line::from(vec![
            Span::styled("a", Style::default().fg(Color::Rgb(1, 2, 3)).bg(Color::Indexed(9))),
            Span::raw("b"),
        ]);
        assert_eq!(ansi(&l), "\x1b[38;2;1;2;3;48;5;9ma\x1b[0mb");
    }
}
