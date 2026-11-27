//! `/manager`: the session's agents in one view (docs/TUI.md, "Screens").
//!
//! Drawn straight from the agents the UI saw ([`App::agent_log`]), every frame, so it is always
//! current: a header with the totals, the agents on the left (state, progress bar, model), the
//! highlighted one on the right (where it ran, its time, a wide bar, its steps as they come, its
//! report, its branch), and the keys it takes at the foot. On a narrow terminal the details go
//! under the list. Without colour every state keeps its glyph and its word.

use std::time::Instant;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use super::app::{AgentRecord, AgentState, App};
use super::palette;
use super::render::fit;
use super::text::{width as text_width, Theme};

/// Below this width the details go under the list.
const TWO_PANES: usize = 96;
/// The list pane's share of the width, and its bounds.
const LIST_MIN: usize = 36;
const LIST_MAX: usize = 58;
/// Cells in a list row's bar.
const LIST_BAR: usize = 12;
/// Steps shown in the details, at most (the latest).
const STEPS_SHOWN: usize = 8;
/// Report lines shown in the details, at most.
const REPORT_SHOWN: usize = 8;
/// The spinner of a running agent.
const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const SPIN_MS: u128 = 90;
/// The sweep of an agent without a task list: one cell a step.
const SWEEP_MS: u128 = 70;

/// `secs` as `42s`, `2m 05s`, `1h 03m`.
pub fn clock(secs: u64) -> String {
    match secs {
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        s => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// How long an agent has run (or ran).
pub fn elapsed(a: &AgentRecord, now: Instant) -> u64 {
    match a.duration_ms {
        Some(ms) => ms / 1000,
        None => now.saturating_duration_since(a.started).as_secs(),
    }
}

/// Its steps so far: the tool calls its result counted, else the ones seen.
pub fn steps(app: &App, a: &AgentRecord) -> usize {
    match (app.running.iter().find(|r| r.id == a.id), a.tool_calls) {
        (Some(r), _) => r.steps.max(a.trail.len()),
        (None, Some(n)) => n as usize,
        (None, None) => a.trail.len(),
    }
}

/// Its steps in words: live from its row while it runs, else as kept.
fn trail(app: &App, a: &AgentRecord) -> Vec<String> {
    match app.running.iter().find(|r| r.id == a.id) {
        Some(r) if r.trail.len() >= a.trail.len() => r.trail.clone(),
        _ => a.trail.clone(),
    }
}

/// The model it ran on (or was sent to), and the endpoint when known.
fn model_of(app: &App, a: &AgentRecord) -> (String, Option<String>) {
    let m = a.model.clone().or_else(|| a.asked_model.clone()).unwrap_or_else(|| app.status.model.clone());
    (m, a.endpoint.clone())
}

/// The agents in the order shown: running first (oldest first), then the others newest first.
pub fn ordered(app: &App) -> Vec<&AgentRecord> {
    let mut v: Vec<&AgentRecord> = app.agent_log.iter().filter(|a| a.state == AgentState::Running).collect();
    v.extend(app.agent_log.iter().rev().filter(|a| a.state != AgentState::Running));
    v
}

fn state_style(t: &Theme, s: AgentState) -> Style {
    match s {
        AgentState::Running => t.teal(),
        AgentState::Done => t.success(),
        AgentState::Failed => t.error(),
        AgentState::Stopped => t.dim(),
    }
}

/// The state's glyph: a spinner while it runs.
fn glyph(a: &AgentRecord, now: Instant) -> &'static str {
    match a.state {
        AgentState::Running => {
            SPIN[(now.saturating_duration_since(a.started).as_millis() / SPIN_MS) as usize % SPIN.len()]
        }
        AgentState::Done => "✓",
        AgentState::Failed => "✗",
        AgentState::Stopped => "■",
    }
}

fn word(s: AgentState) -> &'static str {
    match s {
        AgentState::Running => "running",
        AgentState::Done => "done",
        AgentState::Failed => "failed",
        AgentState::Stopped => "stopped",
    }
}

/// A progress bar `cells` wide. Measured from a task list; full when done; without a list while
/// running, a bright head sweeping across with a trail fading behind it (it moves only while the
/// agent works, and measures nothing it can't know). A running agent's filled cells are the
/// sea-glass, the leading one the brightest.
pub fn bar(t: &Theme, a: &AgentRecord, cells: usize, now: Instant) -> Vec<Span<'static>> {
    let cells = cells.max(4);
    let track = if t.color { Style::default().fg(palette::HAIRLINE.color(t.depth, t.light)) } else { Style::default() };
    let filled = match (a.todos, a.state) {
        (_, AgentState::Done) => Some(cells),
        (Some((done, total)), _) => Some((done * cells).checked_div(total).unwrap_or(0).min(cells)),
        (None, AgentState::Running) => None,
        (None, _) => Some(0),
    };
    let mut out = vec![Span::styled("▕", track)];
    match filled {
        Some(n) => {
            for i in 0..cells {
                if i < n {
                    let st = match a.state {
                        AgentState::Running if t.color => {
                            // The sea-glass, its leading cell the brightest.
                            let role = if i + 1 == n { palette::TEAL } else { palette::TEAL_TRAIL[0] };
                            Style::default().fg(role.color(t.depth, t.light))
                        }
                        s => state_style(t, s),
                    };
                    out.push(Span::styled("█", st));
                } else {
                    out.push(Span::styled(if t.color { "█" } else { "░" }, track));
                }
            }
        }
        None => {
            // The sweep: a head and a three-cell trail, going back and forth.
            let period = 2 * (cells - 1);
            let i = (now.saturating_duration_since(a.started).as_millis() / SWEEP_MS) as usize % period;
            let (head, right) = if i < cells - 1 { (i, true) } else { (period - i, false) };
            for c in 0..cells {
                let behind = if right { head.checked_sub(c) } else { c.checked_sub(head) };
                let (sym, st) = match behind {
                    Some(b) if b <= 3 && t.color => {
                        let role = if b == 0 { palette::TEAL } else { palette::TEAL_TRAIL[b - 1] };
                        ("█", Style::default().fg(role.color(t.depth, t.light)))
                    }
                    Some(0) => ("█", Style::default()),
                    Some(b) if b <= 2 => ("▓", Style::default()),
                    _ => (if t.color { "█" } else { "░" }, track),
                };
                out.push(Span::styled(sym, st));
            }
        }
    }
    out.push(Span::styled("▏", track));
    out
}

/// What the bar counts: `3/6` from a task list, else its steps.
fn counted(app: &App, a: &AgentRecord) -> String {
    match a.todos {
        Some((_, total)) if a.state == AgentState::Done => format!("{total}/{total}"),
        Some((done, total)) => format!("{done}/{total}"),
        None => {
            let n = steps(app, a);
            format!("{n} step{}", if n == 1 { "" } else { "s" })
        }
    }
}

/// `spans` cut or padded to exactly `w` columns.
fn exact(spans: Vec<Span<'static>>, w: usize) -> Vec<Span<'static>> {
    let mut out = vec![];
    let mut used = 0;
    for s in spans {
        if used >= w {
            break;
        }
        let text = fit(&s.content, w - used);
        used += text_width(&text);
        out.push(Span::styled(text, s.style));
    }
    if used < w {
        out.push(Span::raw(" ".repeat(w - used)));
    }
    out
}

/// `spans` with a background laid under every span that has none (a selected row).
fn on(bg: Option<ratatui::style::Color>, spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    match bg {
        None => spans,
        Some(c) => spans.into_iter().map(|s| Span::styled(s.content, s.style.bg(s.style.bg.unwrap_or(c)))).collect(),
    }
}

/// `left` and `right` on one row of `w` columns, `right` against the right edge.
fn split(left: Vec<Span<'static>>, right: Vec<Span<'static>>, w: usize) -> Vec<Span<'static>> {
    let rw: usize = right.iter().map(|s| text_width(&s.content)).sum();
    let mut out = exact(left, w.saturating_sub(rw + 1));
    out.push(Span::raw(" "));
    out.extend(right);
    exact(out, w)
}

/// The list: two rows an agent and a blank between them; the selected one tinted, with a bar
/// in the accent on its left edge.
fn list(app: &App, agents: &[&AgentRecord], sel: usize, w: usize, now: Instant) -> Vec<Vec<Span<'static>>> {
    let t = app.theme;
    let mut rows = vec![];
    for (i, a) in agents.iter().enumerate() {
        let here = i == sel;
        let bg = if here { t.tint() } else { None };
        let edge = Span::styled(if here { "▌" } else { " " }, t.accent());
        let title_style = if here { t.strong() } else { Style::default() };
        let mut head = vec![edge.clone(), Span::styled(format!("{} ", glyph(a, now)), state_style(&t, a.state))];
        if a.background {
            head.push(Span::styled("bg ", t.subtle()));
        }
        head.push(Span::styled(a.description.clone(), title_style));
        let time = vec![Span::styled(clock(elapsed(a, now)), t.subtle())];
        rows.push(on(bg, split(head, time, w)));
        let (model, _) = model_of(app, a);
        let mut second = vec![edge, Span::raw("  ")];
        second.extend(bar(&t, a, LIST_BAR, now));
        second.push(Span::styled(format!(" {} ", counted(app, a)), t.subtle()));
        second.push(Span::styled(format!(" ◆ {model}"), t.violet()));
        rows.push(on(bg, exact(second, w)));
        rows.push(exact(vec![], w));
    }
    rows
}

/// A section heading in the details: small caps in the quiet tone.
fn heading(t: &Theme, text: &str) -> Vec<Span<'static>> {
    vec![Span::styled(text.to_uppercase(), t.subtle().add_modifier(Modifier::BOLD))]
}

/// The details of `a`, `w` columns wide.
fn details(app: &App, a: &AgentRecord, w: usize, now: Instant) -> Vec<Vec<Span<'static>>> {
    let t = app.theme;
    let mut rows: Vec<Vec<Span<'static>>> = vec![];
    // Its name and a pill with its state.
    let pill_style = match (t.color, a.state) {
        (true, s) => {
            let fg = state_style(&t, s).fg.unwrap_or(ratatui::style::Color::Reset);
            let ink = if t.light { ratatui::style::Color::White } else { ratatui::style::Color::Rgb(0x14, 0x0d, 0x10) };
            Style::default().bg(fg).fg(ink).add_modifier(Modifier::BOLD)
        }
        (false, _) => Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD),
    };
    let pill = vec![Span::styled(format!(" {} ", word(a.state).to_uppercase()), pill_style)];
    rows.push(split(vec![Span::styled(a.description.clone(), t.strong())], pill, w));
    rows.push(vec![]);
    let (model, endpoint) = model_of(app, a);
    let label = |k: &str| Span::styled(format!("{k:<8}"), t.subtle());
    let mut m = vec![label("Model"), Span::styled(format!("◆ {model}"), t.violet())];
    if let Some(e) = endpoint {
        m.push(Span::styled(format!("  · {e}"), t.subtle()));
    }
    rows.push(m);
    if let Some(asked) = &a.asked_model {
        if a.model.as_ref().is_some_and(|m| *m != kodact_api::resolve_model(asked)) {
            rows.push(vec![label(""), Span::styled(format!("asked for {asked}: no endpoint serves it"), t.warning())]);
        }
    }
    let kind = if a.background { format!("background subtask {}", a.id) } else { format!("{} agent", a.agent_type) };
    rows.push(vec![label("Agent"), Span::raw(kind)]);
    let n = steps(app, a);
    rows.push(vec![
        label("Time"),
        Span::raw(clock(elapsed(a, now))),
        Span::styled(format!(" · {n} tool call{}", if n == 1 { "" } else { "s" }), t.subtle()),
    ]);
    rows.push(vec![]);
    let count = counted(app, a);
    let mut wide = bar(&t, a, w.saturating_sub(text_width(&count) + 4), now);
    wide.push(Span::styled(format!(" {count}"), state_style(&t, a.state).add_modifier(Modifier::BOLD)));
    rows.push(wide);
    let steps = trail(app, a);
    if !steps.is_empty() {
        rows.push(vec![]);
        rows.push(heading(&t, &format!("Steps · {}", steps.len())));
        let skip = steps.len().saturating_sub(STEPS_SHOWN);
        if skip > 0 {
            rows.push(vec![Span::styled(format!("  … {skip} earlier"), t.subtle())]);
        }
        let last = steps.len() - 1;
        for (i, s) in steps.iter().enumerate().skip(skip) {
            let live = i == last && a.state == AgentState::Running;
            let (mark, st) = if live { (glyph(a, now), t.teal()) } else { ("•", t.subtle()) };
            rows.push(vec![
                Span::styled(format!("{mark} "), st),
                Span::styled(s.clone(), if live { Style::default() } else { t.dim() }),
            ]);
        }
    }
    let report: Vec<&str> = a.report.lines().map(str::trim_end).filter(|l| !l.trim().is_empty()).collect();
    if !report.is_empty() {
        rows.push(vec![]);
        let (title, st) = match a.state {
            AgentState::Failed => ("Why it failed", t.error()),
            _ => ("Report", Style::default()),
        };
        rows.push(heading(&t, title));
        for l in report.iter().take(REPORT_SHOWN) {
            rows.push(vec![Span::styled((*l).to_string(), st)]);
        }
        if report.len() > REPORT_SHOWN {
            rows.push(vec![Span::styled(
                format!("… {} more lines · ⏎ to read it all", report.len() - REPORT_SHOWN),
                t.subtle(),
            )]);
        }
    }
    if let (Some(branch), true) = (&a.branch, a.kept) {
        rows.push(vec![]);
        rows.push(heading(&t, "Branch"));
        rows.push(vec![Span::styled(branch.clone(), t.teal())]);
        if let Some(w) = &a.worktree {
            rows.push(vec![Span::styled(w.clone(), t.subtle())]);
        }
    }
    rows.into_iter().map(|r| exact(r, w)).collect()
}

/// The keys for the selected agent, as keycaps; a question in place of them while one waits.
fn foot(app: &App, a: Option<&AgentRecord>, confirm: Option<&str>) -> Vec<Span<'static>> {
    let t = app.theme;
    if let Some(q) = confirm {
        return vec![
            Span::styled(format!("{q}  "), t.warning().add_modifier(Modifier::BOLD)),
            t.keycap("y"),
            Span::styled(" yes  ", t.dim()),
            t.keycap("n"),
            Span::styled(" no", t.dim()),
        ];
    }
    let mut keys = vec![("↑↓", "select")];
    if let Some(a) = a {
        if a.state == AgentState::Running {
            keys.push(("s", "stop"));
        }
        if a.branch.is_some() && a.kept {
            keys.push(("d", "diff"));
            keys.push(("m", "merge"));
        }
        if !a.report.trim().is_empty() {
            keys.push(("⏎", "report"));
        }
    }
    keys.push(("esc", "close"));
    let mut out = vec![];
    for (k, what) in keys {
        out.push(t.keycap(k));
        out.push(Span::styled(format!(" {what}  "), t.dim()));
    }
    out
}

/// The manager, `w` columns wide and at most `room` rows tall: the selected agent is `sel`
/// (in [`ordered`]'s order), and `confirm` is the question waiting for y/n.
pub fn draw(app: &App, sel: usize, confirm: Option<&str>, w: usize, room: usize, now: Instant) -> Vec<Line<'static>> {
    let t = app.theme;
    let border = if t.color { t.accent() } else { Style::default() };
    let agents = ordered(app);
    let sel = sel.min(agents.len().saturating_sub(1));
    let inner = w.saturating_sub(4).max(20);
    // The header: the title, the totals, and on the right the time and the tool calls in all.
    let count = |s: AgentState| agents.iter().filter(|a| a.state == s).count();
    let mut chips = vec![Span::styled(" ◆ Agents ", t.strong())];
    for (s, n) in [
        (AgentState::Running, count(AgentState::Running)),
        (AgentState::Done, count(AgentState::Done)),
        (AgentState::Failed, count(AgentState::Failed)),
        (AgentState::Stopped, count(AgentState::Stopped)),
    ] {
        if n > 0 || s == AgentState::Running {
            let g = match s {
                AgentState::Running => "●",
                AgentState::Done => "✓",
                AgentState::Failed => "✗",
                AgentState::Stopped => "■",
            };
            chips.push(Span::styled(format!(" {g} {n} {} ", word(s)), state_style(&t, s)));
        }
    }
    let total_secs: u64 = agents.iter().map(|a| elapsed(a, now)).sum();
    let total_calls: usize = agents.iter().map(|a| steps(app, a)).sum();
    let summary =
        vec![Span::styled(format!(" {} agent time · {total_calls} tool calls ", clock(total_secs)), t.subtle())];
    let top = {
        let used: usize = chips.iter().chain(&summary).map(|s| text_width(&s.content)).sum();
        let fill = w.saturating_sub(used + 5);
        let mut l = vec![Span::styled("╭─", border)];
        l.extend(chips);
        l.push(Span::styled("─".repeat(fill), border));
        l.extend(summary);
        l.push(Span::styled("─╮", border));
        Line::from(l)
    };
    // The body: as tall as the agents and the details need, within the room there is.
    let need = if agents.is_empty() {
        5
    } else {
        let detail_rows = details(app, agents[sel], inner.min(60), now).len();
        if inner >= TWO_PANES {
            (agents.len() * 3).max(detail_rows)
        } else {
            agents.len() * 3 + 1 + detail_rows
        }
    };
    let body_rows = need.clamp(10, room.saturating_sub(3).max(6));
    let body: Vec<Vec<Span<'static>>> = if agents.is_empty() {
        let mut v = vec![
            vec![],
            vec![Span::styled("No agents in this session yet.", t.strong())],
            vec![],
            vec![Span::styled("Ask the model to split the work across agents (on other models too),", t.dim())],
            vec![Span::styled("or start one in the background with /subtask. They show up here, live.", t.dim())],
        ];
        v.truncate(body_rows);
        v.into_iter().map(|r| exact(r, inner)).collect()
    } else if inner >= TWO_PANES {
        let lw = (inner * 42 / 100).clamp(LIST_MIN, LIST_MAX);
        let rw = inner - lw - 3;
        let mut left = list(app, &agents, sel, lw, now);
        // Keep the selected agent in view.
        let first = (sel * 3 + 2).saturating_sub(body_rows.saturating_sub(1));
        left = left.into_iter().skip(first).collect();
        let right = details(app, agents[sel], rw, now);
        let divider = Span::styled(" │ ", if t.color { t.hairline() } else { Style::default() });
        (0..body_rows)
            .map(|i| {
                let mut row = left.get(i).cloned().unwrap_or_else(|| exact(vec![], lw));
                row.push(divider.clone());
                row.extend(right.get(i).cloned().unwrap_or_else(|| exact(vec![], rw)));
                row
            })
            .collect()
    } else {
        // Narrow: the list, a rule, the details.
        let mut rows = list(app, &agents, sel, inner, now);
        rows.push(exact(vec![Span::styled("─".repeat(inner), t.hairline())], inner));
        rows.extend(details(app, agents[sel], inner, now));
        let first = (sel * 3 + 2).saturating_sub(body_rows.saturating_sub(1)).min(rows.len().saturating_sub(body_rows));
        rows.into_iter().skip(first).take(body_rows).collect()
    };
    let mut out = vec![top];
    for row in body {
        let mut l = vec![Span::styled("│ ", border)];
        l.extend(exact(row, inner));
        l.push(Span::styled(" │", border));
        out.push(Line::from(l));
    }
    // A breath, then the foot: the keys inside the bottom edge.
    let mut gap = vec![Span::styled("│ ", border)];
    gap.extend(exact(vec![], inner));
    gap.push(Span::styled(" │", border));
    out.push(Line::from(gap));
    let keys = exact(foot(app, agents.get(sel).copied(), confirm), inner);
    let mut f = vec![Span::styled("│ ", border)];
    f.extend(keys);
    f.push(Span::styled(" │", border));
    out.push(Line::from(f));
    out.push(Line::from(Span::styled(format!("╰{}╯", "─".repeat(w.saturating_sub(2))), border)));
    out
}
