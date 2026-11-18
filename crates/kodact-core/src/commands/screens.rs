//! Screens: what the terminal UI shows for `/diff`, `/context`, `/hooks` and
//! `/agents` typed without arguments (docs/TUI.md, "Screens").
//!
//! As with pickers, kodact-core builds the data and the TUI draws it: rows of
//! tagged text, some with an action. Anything a screen changes runs as command
//! text the person could type (`/hooks remove PreToolUse 1`, `/agents create
//! ...`), so the text commands stay the answer on every other surface.

use std::fmt::Write as _;

use super::{parse, Builtin, Invocation};
use crate::driver::Driver;
use crate::view::SessionView;

/// How a piece of text is drawn. The TUI maps these to its theme; with no
/// colour, it falls back to plain text (and `+`/`-` markers for diffs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Dim,
    Bold,
    Accent,
    Added,
    Removed,
    /// A part of the context window (`/context` grid): the index in [`context_parts`].
    Part(u8),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub tone: Tone,
}

/// What Enter does on a row.
#[derive(Debug, Clone, PartialEq)]
pub enum RowAction {
    /// Move to another row of the same screen (Esc comes back).
    Jump(usize),
    /// Run this command text, as if typed.
    Run(String),
    /// Ask first, then run.
    Confirm { question: String, command: String },
    /// Open a form; submitting it runs its command.
    Form(Form),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub spans: Vec<Span>,
    pub action: Option<RowAction>,
}

impl Row {
    pub fn text(text: impl Into<String>, tone: Tone) -> Self {
        Row { spans: vec![Span { text: text.into(), tone }], action: None }
    }

    pub fn blank() -> Self {
        Row { spans: vec![], action: None }
    }

    fn with(mut self, action: RowAction) -> Self {
        self.action = Some(action);
        self
    }

    /// The row's text, without tones.
    pub fn plain(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub title: String,
    pub rows: Vec<Row>,
}

/// One input of a [`Form`].
#[derive(Debug, Clone, PartialEq)]
pub enum FieldKind {
    Text(String),
    /// One of `options` (Left/Right).
    Choice {
        options: Vec<String>,
        at: usize,
    },
    /// Any of `options` (Left/Right to move, Space to toggle); the value joins them with commas.
    Multi {
        options: Vec<String>,
        picked: Vec<bool>,
        at: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub label: String,
    pub kind: FieldKind,
}

impl Field {
    pub fn text(label: &str, value: &str) -> Self {
        Field { label: label.into(), kind: FieldKind::Text(value.into()) }
    }

    pub fn choice(label: &str, options: &[&str]) -> Self {
        Field {
            label: label.into(),
            kind: FieldKind::Choice { options: options.iter().map(|s| s.to_string()).collect(), at: 0 },
        }
    }

    pub fn multi(label: &str, options: Vec<String>) -> Self {
        let picked = vec![false; options.len()];
        Field { label: label.into(), kind: FieldKind::Multi { options, picked, at: 0 } }
    }

    /// The field's value as it goes into the command.
    pub fn value(&self) -> String {
        match &self.kind {
            FieldKind::Text(t) => t.trim().to_string(),
            FieldKind::Choice { options, at } => options.get(*at).cloned().unwrap_or_default(),
            FieldKind::Multi { options, picked, .. } => {
                options.iter().zip(picked).filter(|(_, p)| **p).map(|(o, _)| o.as_str()).collect::<Vec<_>>().join(",")
            }
        }
    }
}

/// Inputs and the command they fill in. `{0}`, `{1}`, ... in `template` are
/// replaced by the fields' values, shell-quoted.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub title: String,
    pub fields: Vec<Field>,
    pub template: String,
}

/// `s` quoted for a command line, so the command's own parser reads it back whole.
pub fn quote(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_./:,@+=*".contains(c)) {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', r"'\''"))
}

impl Form {
    /// The command text the form runs.
    pub fn command(&self) -> String {
        let mut out = self.template.clone();
        for (i, f) in self.fields.iter().enumerate() {
            out = out.replace(&format!("{{{i}}}"), &quote(&f.value()));
        }
        out
    }
}

/// The screen for `text` (a bare `/diff`, `/context`, `/hooks` or `/agents`), or `None`.
pub fn screen(d: &Driver, text: &str) -> Option<Screen> {
    let Invocation::Builtin { spec, args } = parse(text, &d.catalog) else { return None };
    if !args.is_empty() {
        return None;
    }
    match spec.id {
        Builtin::Context => Some(context(&d.view())),
        Builtin::Diff => Some(super::session::diff_screen(d)),
        Builtin::Hooks => Some(super::hooks::screen(d)),
        Builtin::Agents => Some(super::agents::screen(d)),
        _ => None,
    }
}

/// The screen for `text` from the session view alone (while a turn runs): `/context`.
pub fn view_screen(v: &SessionView, text: &str) -> Option<Screen> {
    let cat = super::Catalog::default();
    match parse(text, &cat) {
        Invocation::Builtin { spec, args } if spec.id == Builtin::Context && args.is_empty() => Some(context(v)),
        _ => None,
    }
}

/// The parts of the context window, in grid order: (name, glyph without colour).
pub fn context_parts() -> &'static [(&'static str, char)] {
    &[
        ("System prompt", 'S'),
        ("Built-in tools", 'T'),
        ("MCP tools", 'M'),
        ("Skills (listing)", 'K'),
        ("Memory files", 'F'),
        ("Messages", 'G'),
        ("Free", '·'),
    ]
}

/// Grid cells (each 1% of the window), 10 to a row.
pub const GRID_COLS: usize = 10;
pub const GRID_ROWS: usize = 10;

/// `/context` as a grid: each cell is 1% of the window, coloured by the part
/// that fills it, then the legend with the same numbers as the text form.
fn context(v: &SessionView) -> Screen {
    let c = super::immediate::context_data(v);
    let parts = [c.system, c.builtin, c.mcp, c.skills, c.memory, c.messages, c.free()];
    let cells = GRID_COLS * GRID_ROWS;
    // Each part gets its share of the cells, rounded so used parts show at least one.
    let mut counts: Vec<usize> = parts[..6]
        .iter()
        .map(|&n| if n == 0 || c.window == 0 { 0 } else { ((n * cells as u64 / c.window) as usize).max(1) })
        .collect();
    let used: usize = counts.iter().sum::<usize>().min(cells);
    counts.push(cells - used);
    let mut seq: Vec<u8> = vec![];
    for (i, n) in counts.iter().enumerate() {
        seq.extend(std::iter::repeat_n(i as u8, *n));
    }
    seq.truncate(cells);
    let mut rows = vec![];
    for r in 0..GRID_ROWS {
        let spans = seq[r * GRID_COLS..(r + 1) * GRID_COLS]
            .iter()
            .map(|&p| Span { text: if p == 6 { "⛶ ".into() } else { "⛁ ".into() }, tone: Tone::Part(p) })
            .collect();
        rows.push(Row { spans, action: None });
    }
    rows.push(Row::blank());
    rows.push(Row::text(c.headline(), Tone::Bold));
    if let Some(m) = c.measured_line() {
        rows.push(Row::text(m, Tone::Dim));
    }
    for (i, ((name, _), n)) in context_parts().iter().zip(parts).enumerate() {
        if i == 4 && n == 0 {
            continue;
        }
        let label = if i == 4 && !c.memory_pending { "Memory (in messages)" } else { name };
        rows.push(Row {
            spans: vec![
                Span { text: "⛁ ".into(), tone: Tone::Part(i as u8) },
                Span { text: format!("{label:<22}"), tone: Tone::Plain },
                Span {
                    text: format!("{:>10}  {:>6}", super::run::thousands(n), super::immediate::pct(n, c.window)),
                    tone: Tone::Dim,
                },
            ],
            action: None,
        });
    }
    if let Some(at) = c.autocompact_at {
        rows.push(Row::blank());
        rows.push(Row::text(format!("Auto-compact runs at about {} tokens.", super::run::thousands(at)), Tone::Dim));
    }
    for tip in &c.tips {
        rows.push(Row::text(format!("- {tip}"), Tone::Dim));
    }
    rows.push(Row::text("/context all lists each tool, memory file and skill.", Tone::Dim));
    Screen { title: "Context".into(), rows }
}

/// A unified diff as screen rows: the files first (Enter jumps to one), then each file's hunks.
pub fn diff_rows(unified: &str, untracked: &[String], by_prompt: &[String]) -> Vec<Row> {
    // Split into files at "diff --git" or "--- " headers.
    let mut files: Vec<(String, Vec<&str>)> = vec![];
    let mut pending_minus: Option<&str> = None;
    for line in unified.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            let name = rest.rsplit(" b/").next().unwrap_or(rest).to_string();
            files.push((name, vec![line]));
            pending_minus = None;
        } else if line.starts_with("--- ") && files.last().is_none_or(|(_, l)| l.iter().any(|x| x.starts_with("@@"))) {
            // A file without a "diff --git" header (KodAct's own diffs outside git).
            pending_minus = Some(line);
        } else if let (Some(minus), Some(rest)) = (pending_minus, line.strip_prefix("+++ ")) {
            let name = rest.trim_start_matches("b/").to_string();
            files.push((name, vec![minus, line]));
            pending_minus = None;
        } else if let Some((_, lines)) = files.last_mut() {
            lines.push(line);
        }
    }
    let mut rows = vec![];
    if files.is_empty() && untracked.is_empty() {
        rows.push(Row::text("No changes.", Tone::Dim));
    }
    let header = rows.len();
    // The list: one row per file, filled in with its jump target below.
    for (name, lines) in &files {
        let added = lines.iter().filter(|l| l.starts_with('+') && !l.starts_with("+++")).count();
        let removed = lines.iter().filter(|l| l.starts_with('-') && !l.starts_with("---")).count();
        rows.push(Row {
            spans: vec![
                Span { text: format!("{name}  "), tone: Tone::Plain },
                Span { text: format!("+{added}"), tone: Tone::Added },
                Span { text: " ".into(), tone: Tone::Plain },
                Span { text: format!("-{removed}"), tone: Tone::Removed },
            ],
            action: None,
        });
    }
    for u in untracked {
        rows.push(Row::text(format!("{u}  (untracked)"), Tone::Dim));
    }
    if !by_prompt.is_empty() {
        rows.push(Row::blank());
        rows.push(Row::text("Files KodAct changed, by prompt:", Tone::Bold));
        for p in by_prompt {
            rows.push(Row::text(format!("  {p}"), Tone::Plain));
        }
    }
    for (i, (name, lines)) in files.iter().enumerate() {
        rows.push(Row::blank());
        let at = rows.len();
        rows[header + i] = rows[header + i].clone().with(RowAction::Jump(at));
        rows.push(Row::text(name.clone(), Tone::Bold));
        for l in lines.iter().filter(|l| {
            !(l.starts_with("diff --git") || l.starts_with("index ") || l.starts_with("--- ") || l.starts_with("+++ "))
        }) {
            let tone = if l.starts_with("@@") {
                Tone::Dim
            } else if l.starts_with('+') {
                Tone::Added
            } else if l.starts_with('-') {
                Tone::Removed
            } else {
                Tone::Plain
            };
            rows.push(Row::text(l.to_string(), tone));
        }
        if lines.first().is_some_and(|l| l.starts_with("diff --git")) {
            let quoted = format!("'{}'", name.replace('\'', "'\\''"));
            rows.push(Row::text(format!("  ↩ Revert {name}"), Tone::Dim).with(RowAction::Confirm {
                question: format!("Revert all unstaged changes to {name}?"),
                command: format!("!git checkout -- {quoted}"),
            }));
        }
    }
    rows
}

/// A screen's rows as plain text (for tests and logs).
pub fn plain(s: &Screen) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{}", s.title);
    for r in &s.rows {
        let _ = writeln!(out, "{}", r.plain());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_quote_their_values() {
        let mut f = Form {
            title: "t".into(),
            fields: vec![
                Field::choice("Event", &["PreToolUse", "Stop"]),
                Field::text("Matcher", ""),
                Field::text("Command", "echo 'hi there'"),
                Field::multi("Tools", vec!["Read".into(), "Bash".into(), "Grep".into()]),
            ],
            template: "/hooks add {0} {1} {2} --tools {3}".into(),
        };
        if let FieldKind::Multi { picked, .. } = &mut f.fields[3].kind {
            picked[0] = true;
            picked[2] = true;
        }
        assert_eq!(f.command(), r"/hooks add PreToolUse '' 'echo '\''hi there'\''' --tools Read,Grep");
        assert_eq!(shlex::split(&f.command()).unwrap()[4], "echo 'hi there'");
    }

    #[test]
    fn diffs_list_files_then_hunks_with_jumps() {
        let unified = "diff --git a/src/a.rs b/src/a.rs\nindex 1..2 100644\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,2 @@\n-old\n+new\n same\ndiff --git a/b.txt b/b.txt\n--- a/b.txt\n+++ b/b.txt\n@@ -1 +1,2 @@\n x\n+y\n";
        let rows = diff_rows(unified, &["notes.md".into()], &[]);
        let text: Vec<String> = rows.iter().map(Row::plain).collect();
        assert_eq!(text[0], "src/a.rs  +1 -1");
        assert_eq!(text[1], "b.txt  +1 -0");
        assert_eq!(text[2], "notes.md  (untracked)");
        let Some(RowAction::Jump(to)) = rows[1].action else { panic!("{:?}", rows[1]) };
        assert_eq!(text[to], "b.txt");
        assert_eq!(rows[to + 1].spans[0].tone, Tone::Dim, "the hunk header");
        assert_eq!(rows[to + 3].spans[0].tone, Tone::Added);
        let Some(RowAction::Jump(a)) = rows[0].action else { panic!() };
        assert_eq!((text[a + 2].as_str(), rows[a + 2].spans[0].tone), ("-old", Tone::Removed));

        // KodAct's own diffs outside git have no "diff --git" line.
        let rows = diff_rows("--- a/x.py\n+++ b/x.py\n@@ -1 +1 @@\n-a\n+b\n", &[], &[]);
        assert_eq!(rows[0].plain(), "x.py  +1 -1");
        assert_eq!(diff_rows("", &[], &[])[0].plain(), "No changes.");
    }
}
