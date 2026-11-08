//! A shell command and its output, drawn as one terminal box:
//!
//! ```text
//!   ╭──────────────────────────────────────────╮
//!   │ $ python3 -m pytest -q                   │   the input, on a raised band
//!   ├──────────────────────────────────────────┤
//!   │ … 31 earlier lines                       │
//!   │ E   AssertionError: assert 10.0 == 10.01 │   the output, its last lines
//!   ├──────────────────────────────────────────┤
//!   │ exit 1   36 lines                        │   the exit code and the size
//!   ╰──────────────────────────────────────────╯
//! ```
//!
//! The command and its output never mix: the command sits on its own band above a
//! rule, the output below it. The box is a placeholder line in the transcript,
//! laid out at the width of the moment by [`super::app::lay_out`], so it stays whole
//! when the window is resized.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::{json, Value};
use unicode_width::UnicodeWidthChar;

use super::palette;
use super::text::Theme;

/// The first span of a box's placeholder line.
pub const SHELL_MARK: &str = "\u{e000}kodact-shell";

/// Output lines a box shows, from the end, where a command's result is.
pub const TAIL: usize = 8;

/// Narrower than this, the box drops its walls and keeps the rule and the lines.
const MIN_BOX: usize = 24;

/// The styles a box is drawn in, in the order its placeholder carries them.
#[derive(Clone, Copy)]
struct Styles {
    border: Style,
    prompt: Style,
    program: Style,
    flag: Style,
    string: Style,
    operator: Style,
    word: Style,
    out: Style,
    error: Style,
    quiet: Style,
    ok_badge: Style,
    fail_badge: Style,
    running: Style,
    band: Style,
}

impl Styles {
    fn of(t: &Theme) -> Styles {
        let deep = t.color && t.depth != palette::Depth::Ansi16;
        let bg =
            |r: palette::Role| if deep { Style::default().bg(r.color(t.depth, t.light)) } else { Style::default() };
        Styles {
            border: t.hairline(),
            prompt: t.accent().add_modifier(Modifier::BOLD),
            program: t.role(palette::SUCCESS).add_modifier(Modifier::BOLD),
            flag: t.role(palette::INFO),
            string: t.role(palette::CODE),
            operator: t.role(palette::DANGER),
            word: t.role(palette::CODE_TEXT),
            out: t.dim(),
            error: t.error(),
            quiet: t.subtle(),
            ok_badge: t.role(palette::SUCCESS).patch(bg(palette::SUCCESS_BG)),
            fail_badge: t.error().patch(bg(palette::DANGER_BG)),
            running: t.accent(),
            band: bg(palette::PROMPT_BG),
        }
    }

    fn all(&self) -> [Style; 14] {
        [
            self.border,
            self.prompt,
            self.program,
            self.flag,
            self.string,
            self.operator,
            self.word,
            self.out,
            self.error,
            self.quiet,
            self.ok_badge,
            self.fail_badge,
            self.running,
            self.band,
        ]
    }

    fn from(s: &[Span<'static>]) -> Styles {
        let g = |i: usize| s.get(i).map(|x| x.style).unwrap_or_default();
        Styles {
            border: g(0),
            prompt: g(1),
            program: g(2),
            flag: g(3),
            string: g(4),
            operator: g(5),
            word: g(6),
            out: g(7),
            error: g(8),
            quiet: g(9),
            ok_badge: g(10),
            fail_badge: g(11),
            running: g(12),
            band: g(13),
        }
    }
}

/// How a command stands: still running, or ended with an exit code (`None` when a
/// failure gave none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Running,
    Ended { code: Option<i64>, failed: bool },
}

/// A command's result text, split into its exit code and its output: the engine
/// starts a failed command's result with `Exit code N`.
pub fn split_exit(text: &str) -> (Option<i64>, String) {
    let text = text.trim_start_matches('\n');
    if let Some(rest) = text.strip_prefix("Exit code ") {
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '-').collect();
        if let Ok(code) = digits.parse() {
            let after = rest[digits.len()..].trim_start_matches([' ', '\r', '\n']);
            return (Some(code), after.to_string());
        }
    }
    (None, text.to_string())
}

/// The placeholder line for a command's box: its styles, then the command, its
/// output and how it ended, as JSON.
pub fn placeholder(t: &Theme, command: &str, output: &str, state: State) -> Line<'static> {
    let styles = Styles::of(t);
    let mut spans: Vec<Span<'static>> = styles.all().iter().map(|s| Span::styled("", *s)).collect();
    spans[0].content = SHELL_MARK.into();
    let (code, failed, running) = match state {
        State::Running => (None, false, true),
        State::Ended { code, failed } => (code, failed, false),
    };
    let body = json!({ "command": command, "output": output, "code": code, "failed": failed, "running": running });
    spans.push(Span::raw(body.to_string()));
    Line::from(spans)
}

/// Whether a line is a box's placeholder.
pub fn is_placeholder(l: &Line<'_>) -> bool {
    l.spans.first().is_some_and(|s| s.content == SHELL_MARK) && l.spans.len() == 15
}

/// A box's rows at `width` columns, from its placeholder line.
pub fn lay_out(l: &Line<'static>, width: u16) -> Vec<Line<'static>> {
    let st = Styles::from(&l.spans);
    let v: Value = serde_json::from_str(&l.spans[14].content).unwrap_or_default();
    let command = v["command"].as_str().unwrap_or("");
    let output = v["output"].as_str().unwrap_or("");
    let state = if v["running"].as_bool() == Some(true) {
        State::Running
    } else {
        State::Ended { code: v["code"].as_i64(), failed: v["failed"].as_bool() == Some(true) }
    };
    rows(&st, command, output, state, width as usize)
}

/// A box for a command still running, at `width`: its command and a cursor.
pub fn running(t: &Theme, command: &str, width: usize) -> Vec<Line<'static>> {
    rows(&Styles::of(t), command, "", State::Running, width)
}

type Cells = Vec<(char, Style)>;

fn rows(st: &Styles, command: &str, output: &str, state: State, width: usize) -> Vec<Line<'static>> {
    // Two columns in, under the step's words; the box fills the rest.
    let lead = 2;
    let bw = width.saturating_sub(lead);
    let walls = bw >= MIN_BOX;
    let inner = if walls { bw - 4 } else { bw.max(1) };

    let mut out: Vec<Line<'static>> = vec![];
    let edge = |l: char, r: char| -> Line<'static> {
        Line::from(vec![
            Span::raw(" ".repeat(lead)),
            Span::styled(format!("{l}{}{r}", "─".repeat(bw.saturating_sub(2))), st.border),
        ])
    };
    let rule = |out: &mut Vec<Line<'static>>| {
        if walls {
            out.push(edge('├', '┤'));
        } else {
            out.push(Line::from(vec![Span::raw(" ".repeat(lead)), Span::styled("─".repeat(bw), st.border)]));
        }
    };
    // A row of cells inside the walls, padded to the inside; `fill` styles the padding.
    let row = |cells: &[(char, Style)], fill: Style| -> Line<'static> {
        // Never wider than the inside: a foot or a note too long for it is cut.
        let mut w = 0;
        let cells: Vec<(char, Style)> = cells
            .iter()
            .copied()
            .take_while(|(c, _)| {
                w += c.width().unwrap_or(0);
                w <= inner
            })
            .collect();
        let cells = cells.as_slice();
        let used: usize = cells.iter().map(|(c, _)| c.width().unwrap_or(0)).sum();
        let mut spans = vec![Span::raw(" ".repeat(lead))];
        if walls {
            spans.push(Span::styled("│", st.border));
            spans.push(Span::styled(" ", fill));
        }
        spans.extend(spans_of(cells));
        if walls {
            spans.push(Span::styled(" ".repeat(inner.saturating_sub(used) + 1), fill));
            spans.push(Span::styled("│", st.border));
        }
        Line::from(spans)
    };

    if walls {
        out.push(edge('╭', '╮'));
    } else {
        rule(&mut out);
    }
    // The input: `$` and the command in its colours, on the band.
    let band = |s: Style| s.patch(Style { bg: st.band.bg, ..Style::default() });
    let mut cmd: Cells = vec![('$', band(st.prompt)), (' ', band(st.word))];
    let first = cmd.len();
    for (i, line) in command.trim_end().lines().enumerate() {
        if i > 0 {
            cmd.push(('\n', st.word));
        }
        cmd.extend(highlight(st, line).into_iter().map(|(c, s)| (c, band(s))));
    }
    for r in wrap_cells(&cmd, inner, first) {
        out.push(row(&r, st.band));
    }
    rule(&mut out);

    // The output: its last lines, the ones naming an error in red.
    let (body, total) = output_lines(output);
    match state {
        State::Running => {
            out.push(row(&[('▍', st.running)], Style::default()));
        }
        _ if body.is_empty() => {
            let s = st.quiet.add_modifier(Modifier::ITALIC);
            out.push(row(&"No output".chars().map(|c| (c, s)).collect::<Vec<_>>(), Style::default()));
        }
        _ => {
            let skip = body.len().saturating_sub(TAIL);
            if skip > 0 {
                let note = format!("… {skip} earlier {}", if skip == 1 { "line" } else { "lines" });
                out.push(row(&note.chars().map(|c| (c, st.quiet)).collect::<Vec<_>>(), Style::default()));
            }
            let failed = matches!(state, State::Ended { failed: true, .. });
            for l in &body[skip..] {
                let s = if failed && super::app::names_an_error(l) { st.error } else { st.out };
                let cells: Cells = l.chars().map(|c| (c, s)).collect();
                for r in wrap_cells(&cells, inner, 0) {
                    out.push(row(&r, Style::default()));
                }
            }
        }
    }
    rule(&mut out);

    // The foot: how it ended, and how much it printed.
    let mut foot: Cells = vec![];
    let push = |foot: &mut Cells, s: &str, style: Style| foot.extend(s.chars().map(|c| (c, style)));
    let right = match state {
        State::Running => {
            push(&mut foot, "running", st.running);
            "esc stops it"
        }
        State::Ended { code, failed } => {
            let badge = match code {
                Some(c) => format!(" exit {c} "),
                None if failed => " failed ".to_string(),
                None => " exit 0 ".to_string(),
            };
            push(&mut foot, &badge, if failed { st.fail_badge } else { st.ok_badge });
            if total > 0 {
                push(&mut foot, &format!("  {total} {}", if total == 1 { "line" } else { "lines" }), st.quiet);
            }
            ""
        }
    };
    let used = foot.len();
    if !right.is_empty() && used + 2 + right.chars().count() <= inner {
        foot.extend(std::iter::repeat_n((' ', Style::default()), inner - used - right.chars().count()));
        push(&mut foot, right, st.quiet);
    }
    out.push(row(&foot, Style::default()));
    if walls {
        out.push(edge('╰', '╯'));
    } else {
        rule(&mut out);
    }
    out
}

/// A command's output as lines to show: escapes, blank edges and progress dots gone,
/// tabs as spaces; and how many lines it printed.
fn output_lines(output: &str) -> (Vec<String>, usize) {
    let clean = strip_escapes(output);
    let all: Vec<String> = clean
        .lines()
        .map(|l| expand_tabs(l.trim_end_matches('\r')))
        .collect::<Vec<_>>()
        .into_iter()
        .skip_while(|l| l.trim().is_empty())
        .collect();
    let mut all = all;
    while all.last().is_some_and(|l| l.trim().is_empty()) {
        all.pop();
    }
    let total = all.len();
    let body: Vec<String> = all.iter().filter(|l| !super::app::progress_only(l)).cloned().collect();
    (if body.is_empty() { all } else { body }, total)
}

fn expand_tabs(l: &str) -> String {
    let mut s = String::new();
    let mut col = 0;
    for c in l.chars() {
        if c == '\t' {
            let n = 4 - col % 4;
            s.push_str(&" ".repeat(n));
            col += n;
        } else if !c.is_control() {
            s.push(c);
            col += c.width().unwrap_or(0);
        }
    }
    s
}

/// Text without terminal escape sequences (colours, cursor moves).
fn strip_escapes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            if it.peek() == Some(&'[') {
                it.next();
                for d in it.by_ref() {
                    if ('@'..='~').contains(&d) {
                        break;
                    }
                }
            } else {
                it.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A command line in colour: each program bold, flags, quoted strings and the
/// operators that join commands (`&&`, `|`, `;`, `>`) each in their own.
#[cfg(test)]
pub fn highlight_spans(t: &Theme, line: &str) -> Vec<Span<'static>> {
    spans_of(&highlight(&Styles::of(t), line))
}

fn highlight(st: &Styles, line: &str) -> Cells {
    let mut out: Cells = vec![];
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    let mut expect_program = true;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            out.push((c, st.word));
            i += 1;
            continue;
        }
        if c == '"' || c == '\'' {
            let start = i;
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' && c == '"' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(chars.len());
            out.extend(chars[start..i].iter().map(|&ch| (ch, st.string)));
            expect_program = false;
            continue;
        }
        if matches!(c, '|' | '&' | ';' | '>' | '<') {
            let start = i;
            while i < chars.len() && matches!(chars[i], '|' | '&' | ';' | '>' | '<' | '1' | '2') && i - start < 4 {
                i += 1;
            }
            out.extend(chars[start..i].iter().map(|&ch| (ch, st.operator)));
            expect_program = !matches!(c, '>' | '<');
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].is_whitespace() && !matches!(chars[i], '|' | ';' | '"' | '\'') {
            if chars[i] == '&' && chars.get(i + 1) == Some(&'&') {
                break;
            }
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        let style = if expect_program && !word.contains('=') {
            expect_program = false;
            st.program
        } else if word.starts_with('-') {
            st.flag
        } else {
            st.word
        };
        out.extend(word.chars().map(|ch| (ch, style)));
    }
    out
}

/// Cells cut into rows of at most `width` columns; a `\n` cell starts a new row.
/// Continuation rows start under the text, `indent` columns in.
fn wrap_cells(cells: &[(char, Style)], width: usize, indent: usize) -> Vec<Cells> {
    let width = width.max(1);
    let indent = if indent * 2 < width { indent } else { 0 };
    let mut rows: Vec<Cells> = vec![vec![]];
    let mut used = 0;
    for &(c, s) in cells {
        if c == '\n' {
            rows.push(vec![(' ', s); indent]);
            used = indent;
            continue;
        }
        let w = c.width().unwrap_or(0);
        if used + w > width {
            let pad = rows.last().and_then(|r| r.get(indent.saturating_sub(1))).map(|x| x.1).unwrap_or(s);
            rows.push(vec![(' ', pad); indent]);
            used = indent;
        }
        rows.last_mut().unwrap().push((c, s));
        used += w;
    }
    rows
}

fn spans_of(cells: &[(char, Style)]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = vec![];
    for &(c, s) in cells {
        match spans.last_mut() {
            Some(last) if last.style == s => last.content.to_mut().push(c),
            _ => spans.push(Span::styled(c.to_string(), s)),
        }
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(rows: &[Line<'_>]) -> Vec<String> {
        rows.iter().map(|l| l.spans.iter().map(|s| s.content.as_ref()).collect()).collect()
    }

    fn theme() -> Theme {
        Theme::named("dark", true, palette::Depth::True)
    }

    #[test]
    fn a_failed_command_shows_its_input_then_its_output_then_how_it_ended() {
        let t = theme();
        let out: String = (1..=30).map(|i| format!("line {i}\n")).collect::<String>() + "E   AssertionError: boom\n";
        let l = placeholder(&t, "python3 -m pytest -q", &format!("Exit code 1\n{out}"), {
            let (code, _) = split_exit("Exit code 1\n");
            State::Ended { code, failed: true }
        });
        let rows = text(&lay_out(&l, 60));
        assert!(rows[0].starts_with("  ╭") && rows[0].ends_with('╮'), "{rows:#?}");
        assert!(rows[1].starts_with("  │ $ python3 -m pytest -q"), "{rows:#?}");
        assert!(rows[2].starts_with("  ├"), "the rule between input and output: {rows:#?}");
        assert!(rows.iter().any(|r| r.contains("… 24 earlier lines")), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains("E   AssertionError: boom")), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains(" exit 1   32 lines")), "{rows:#?}");
        assert!(rows.last().unwrap().starts_with("  ╰"), "{rows:#?}");
        for r in &rows {
            assert_eq!(unicode_width::UnicodeWidthStr::width(r.as_str()), 60, "every row the box's width: {r:?}");
        }
    }

    #[test]
    fn the_box_stays_whole_at_any_width_and_keeps_long_commands() {
        let t = theme();
        let cmd = "cargo test --workspace --all-targets -- --nocapture && echo \"all good\"";
        let l = placeholder(&t, cmd, "ok\tdone", State::Ended { code: Some(0), failed: false });
        for w in [12u16, 30, 80, 200] {
            let rows = text(&lay_out(&l, w));
            for r in &rows {
                assert!(unicode_width::UnicodeWidthStr::width(r.as_str()) <= w as usize, "{w}: {r:?}");
                assert!(!r.contains('\t'), "{r:?}");
            }
            let all = rows.join("");
            assert!(all.replace([' ', '│'], "").contains("echo\"allgood\""), "the whole command at {w}: {rows:#?}");
        }
    }

    #[test]
    fn a_command_line_is_coloured_by_its_parts() {
        let t = theme();
        let spans = highlight_spans(&t, "npm test -- --watch && echo 'ok' | tee out.txt");
        let style_of = |w: &str| spans.iter().find(|s| s.content.contains(w)).map(|s| s.style).unwrap();
        let st = Styles::of(&t);
        assert_eq!(style_of("npm").fg, st.program.fg);
        assert_eq!(style_of("--watch").fg, st.flag.fg);
        assert_eq!(style_of("'ok'").fg, st.string.fg);
        assert_eq!(style_of("&&").fg, st.operator.fg);
        assert_eq!(style_of("echo").fg, st.program.fg, "a program after &&");
        assert_eq!(style_of("tee").fg, st.program.fg, "a program after |");
    }

    #[test]
    fn the_exit_code_comes_off_the_output() {
        assert_eq!(split_exit("Exit code 5\nno tests ran"), (Some(5), "no tests ran".into()));
        assert_eq!(split_exit("all fine"), (None, "all fine".into()));
    }
}
