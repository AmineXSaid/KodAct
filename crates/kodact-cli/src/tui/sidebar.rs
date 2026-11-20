//! Sidebar layout, animation primitives, and fill bars for wide terminals.

use std::time::{Duration, Instant};

use ratatui::style::Style;
use ratatui::text::{Line, Span};

use super::app::App;
use super::palette;
use super::render::{fit, gauge, is_agent, keep_end, room_left, tokens};
use super::text::{self, width as text_width};

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
/// a sub-agent working): it fills from the bottom, dot by dot, and drains again, so it
/// reads as filling, never as done.
pub const FILL: [&str; 14] = ["⡀", "⣀", "⣄", "⣤", "⣦", "⣶", "⣷", "⣿", "⣷", "⣶", "⣦", "⣤", "⣄", "⣀"];

/// The time between frames of a filling mark.
pub const FILL_STEP: Duration = Duration::from_millis(90);

/// The frames of the plan's task in progress in the Plan pane (TodoWrite): a ring of eight
/// braille dots over two columns, four dots wide and four high, so it is round, with a gap of
/// two dots going round its edge clockwise, a dot a frame. The edge turns; the inside stays
/// empty. Clockwise from the top left, the ring's dots are (row, column): (0,1) (0,2) (1,3)
/// (2,3) (3,2) (3,1) (2,0) (1,0); frame `k` leaves out dots `k` and `k + 1`.
pub const RING: [&str; 8] = ["⢆⡰", "⢎⡠", "⢎⡁", "⢎⠑", "⠎⠱", "⠊⡱", "⢈⡱", "⢄⡱"];

/// The time between the ring's frames: a turn in 0.8 s.
pub const RING_STEP: Duration = Duration::from_millis(100);

/// The ring whole and still: the plan's task in progress in the Plan pane while the session
/// waits.
pub const RING_STILL: &str = "⢎⡱";

/// The ring's frame `ran` after the turn began.
pub fn turning(ran: Duration) -> &'static str {
    RING[(ran.as_millis() / RING_STEP.as_millis()) as usize % RING.len()]
}

/// A filling mark, `ran` after its work began: each keeps its own phase, so several
/// in flight never pulse in step.
pub fn filling(ran: Duration) -> &'static str {
    FILL[fill_frame(ran)]
}

pub(super) fn fill_frame(ran: Duration) -> usize {
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
pub(super) fn ran_for(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s}s"),
        60..=3599 => format!("{}m {:02}s", s / 60, s % 60),
        _ => format!("{}h {:02}m", s / 3600, s % 3600 / 60),
    }
}

/// When the UI started: the phase of marks whose work has no start of its own.
pub(super) fn epoch() -> Instant {
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

    // The plan: done steps quiet with a check, the active one in lime with a ring whose gap
    // goes round its edge while the turn runs (whole and still while it waits), the rest
    // open. Every mark takes the ring's two columns, the others flush right, so the steps'
    // text lines up. A long step takes the rows it needs while there is room, two when
    // there isn't.
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
                // Reduced motion: the ring whole and still, as while the session waits.
                "in_progress" => match busy_for.filter(|_| app.busy && !t.calm) {
                    Some(ran) => (turning(ran), t.accent(), t.accent()),
                    None => (RING_STILL, t.accent(), t.accent()),
                },
                _ => ("○", t.dim(), t.dim()),
            };
            let rows =
                text::wrap(vec![Line::from(Span::styled(content.clone(), body))], inner.saturating_sub(3) as u16);
            for (i, r) in rows.into_iter().take(most).enumerate() {
                let mut spans =
                    vec![if i == 0 { Span::styled(format!("{mark:>2} "), mark_style) } else { Span::raw("   ") }];
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
                    let ran = t.motion(now.saturating_duration_since(epoch()));
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
                Span::styled(filling(t.motion(ran)), breathing(&t, t.accent(), t.motion(ran))),
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
pub(super) fn pane_top(head: Option<Line<'static>>, w: usize, border: Style, name_style: Style) -> Line<'static> {
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
pub(super) fn pane_row(l: Line<'static>, w: usize, border: Style) -> Line<'static> {
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
