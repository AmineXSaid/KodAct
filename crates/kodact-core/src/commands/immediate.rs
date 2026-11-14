//! Immediate commands (contract C17): `/status`, `/usage`, `/tasks`,
//! `/context`, `/mcp` and `/btw`. They read the [`SessionView`], not the
//! driver, so the same code answers them while idle and while a turn is in
//! progress (the TUI and stream-json run them mid-turn).

use std::fmt::Write as _;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::run::{duration, err, ok, thousands, tokens_of};
use super::{parse, Builtin, Catalog, Exec, Invocation};
use crate::view::{Effect, SessionView, ViewState};

/// Is `text` an immediate built-in (aliases included, e.g. `/cost`)? Arguments don't change
/// the answer: `/mcp reconnect x` and `/tasks stop x` are immediate too (the MCP manager
/// serializes each server's changes, and stopping a task is a signal). Built-ins win over
/// custom commands and skills, so the catalog never makes a built-in name mean something else.
pub fn immediate(text: &str, cat: &Catalog) -> bool {
    matches!(parse(text, cat), Invocation::Builtin { spec, .. } if spec.immediate)
}

/// Run the immediate command in `text` from `v`. `None` when `text` isn't one. `cancel` stops a
/// `/btw` request.
pub async fn execute_immediate(v: &SessionView, text: &str, cancel: &CancellationToken) -> Option<Exec> {
    let cat = Catalog::default();
    let Invocation::Builtin { spec, args } = parse(text, &cat) else { return None };
    if !spec.immediate {
        return None;
    }
    Some(match spec.id {
        Builtin::Keybindings | Builtin::TerminalSetup | Builtin::Color | Builtin::Focus => {
            err(format!("/{} works only in the terminal UI.", spec.name))
        }
        id => run(v, id, args.trim(), cancel).await,
    })
}

/// One of the view's commands.
pub(super) async fn run(v: &SessionView, id: Builtin, args: &str, cancel: &CancellationToken) -> Exec {
    match id {
        Builtin::Usage => ok(usage(v)),
        Builtin::Status => ok(status(v)),
        Builtin::Tasks => tasks(v, args),
        Builtin::Context => context(v, args),
        Builtin::Mcp => super::mcp::run(v, args).await,
        Builtin::Btw => btw(v, args, cancel).await,
        _ => err("not an immediate command"),
    }
}

fn usage(v: &SessionView) -> String {
    let d = v.state();
    let st = v.engine();
    let tool_calls: usize = st.messages.iter().map(|m| m.tool_uses().count()).sum();
    // A turn in progress counts as far as it has got.
    let (mut turns, mut api_time, mut prompts) = (d.activity.turns, d.activity.api_time, d.activity.prompts);
    if let Some(t) = st.turn.as_ref().filter(|t| t.api_calls > 0) {
        turns += t.api_calls;
        api_time += std::time::Duration::from_millis(t.api_ms);
        prompts += 1;
    }
    let mut s = String::new();
    let unpriced = st.unpriced.join(", ");
    match (st.unpriced.is_empty(), st.total_cost_usd > 0.0) {
        (true, _) => {
            let _ = writeln!(s, "Total cost:     ${:.4}", st.total_cost_usd);
        }
        (false, false) => {
            let _ = writeln!(
                s,
                "Total cost:     unknown: KodAct has no price for {unpriced} (set it under \"modelPricing\")"
            );
        }
        (false, true) => {
            let _ = writeln!(
                s,
                "Total cost:     ${:.4}, plus {unpriced}, which KodAct has no price for (set it under \"modelPricing\")",
                st.total_cost_usd
            );
        }
    }
    let _ = writeln!(
        s,
        "Duration:       {} wall, {} API · {} model calls for {} prompts · {} tool calls in this conversation",
        duration(d.started.elapsed()),
        duration(api_time),
        turns,
        prompts,
        tool_calls
    );
    if st.model_usage.is_empty() {
        let _ = writeln!(s, "Usage by model: none yet");
    } else {
        let _ = writeln!(s, "Usage by model:");
        for (model, u) in &st.model_usage {
            let n = |k: &str| thousands(u.get(k).and_then(Value::as_u64).unwrap_or(0));
            let cost = if st.unpriced.iter().any(|m| m == model) {
                "price unknown".to_string()
            } else {
                format!("${:.4}", u.get("costUSD").and_then(Value::as_f64).unwrap_or(0.0))
            };
            let _ = writeln!(
                s,
                "  {model}: {} input, {} output, {} cache read, {} cache write ({cost})",
                n("inputTokens"),
                n("outputTokens"),
                n("cacheReadInputTokens"),
                n("cacheCreationInputTokens"),
            );
        }
    }
    let model = d.handle.model();
    let window = st.context_window(&model);
    let _ = write!(s, "Context now:    about {} of {} tokens", thousands(st.context_tokens), thousands(window));
    if kodact_api::models::limits_source(&model) == kodact_api::models::LimitsSource::Guessed {
        let _ = write!(s, " (a guess: KodAct doesn't know {model}'s window; set it under \"modelLimits\")");
    }
    s
}

fn status(v: &SessionView) -> String {
    let d = v.state();
    let rt = d.handle.runtime();
    let mode = d.handle.permissions.read().unwrap().mode;
    let mut s = String::new();
    let _ = writeln!(s, "KodAct {}", crate::VERSION);
    let title = d.transcript.title();
    let _ = writeln!(s, "Session:        {}{}", d.session_id, title.map(|t| format!(" ({t})")).unwrap_or_default());
    let _ = writeln!(s, "Directory:      {}", kodact_config::short_path(&d.cwd));
    let dirs: Vec<String> =
        d.tool_ctx.working_dirs.read().unwrap().iter().skip(1).map(|p| kodact_config::short_path(p)).collect();
    if !dirs.is_empty() {
        let _ = writeln!(s, "Also allowed:   {}", dirs.join(", "));
    }
    let _ = writeln!(
        s,
        "Model:          {} · effort {} · thinking {}",
        rt.model,
        rt.effort.as_deref().unwrap_or("default"),
        match rt.max_thinking_tokens {
            None => "default".to_string(),
            Some(0) => "off".to_string(),
            Some(n) => format!("{n} tokens"),
        }
    );
    let _ = writeln!(s, "Permissions:    {} mode", mode.as_str());
    let _ = writeln!(s, "Output style:   {}", d.init.output_style);
    let sandbox = match d.tool_ctx.sandbox_policy() {
        Some(p) => format!("{} (network {})", p.mode.as_str(), if p.network { "on" } else { "off" }),
        None => "off".into(),
    };
    let _ = writeln!(s, "Sandbox:        {sandbox}");
    let shell = match &d.tool_ctx.shell {
        Ok(sh) => sh.describe(),
        Err(m) => format!("none ({m})"),
    };
    let _ = writeln!(s, "Shell:          {shell}");
    let api = match (d.provider.origin(), d.provider.base_url()) {
        (Some(o), url) => {
            let kind = match o.backend {
                kodact_api::auth::Backend::OpenAi => "OpenAI-compatible endpoint",
                kodact_api::auth::Backend::Messages => "Messages API",
                kodact_api::auth::Backend::Gemini => "Gemini API",
            };
            let at = match url {
                Some(u) => format!(" {u} ({})", o.url_var()),
                None => ", default host".into(),
            };
            let key = match o.api_key_source() {
                "none" => "no key".to_string(),
                k => format!("key from {k}"),
            };
            format!("{kind}{at} · {key}")
        }
        (None, _) => format!("{} via {}", d.init.api_key_source, d.provider.name()),
    };
    let _ = writeln!(s, "API:            {api}");
    let files: Vec<String> = d
        .settings
        .layers
        .iter()
        .map(|l| match &l.path {
            Some(p) => format!("{} ({})", p.display(), l.source.as_str()),
            None => l.source.as_str().to_string(),
        })
        .collect();
    let _ = writeln!(s, "Settings:       {}", if files.is_empty() { "none".into() } else { files.join(", ") });
    let _ = writeln!(s, "Memory:         {} file(s)", d.memory.len());
    if let Some(m) = &d.mcp {
        let st = m.status();
        let count = |w: &str| st.iter().filter(|(_, s)| s == w).count();
        let _ = writeln!(
            s,
            "MCP servers:    {} connected, {} failed, {} disabled",
            count("connected"),
            count("failed"),
            count("disabled")
        );
    }
    let _ = write!(
        s,
        "Hooks:          {}",
        if d.hooks_disabled { "disabled".to_string() } else { format!("{} configured", d.hook_count) }
    );
    s
}

/// The scheduled tasks, one line each.
fn scheduled(d: &ViewState) -> Vec<String> {
    let Some(s) = &d.scheduler else { return vec![] };
    s.lock()
        .unwrap()
        .tasks
        .iter()
        .map(|t| format!("  {} [scheduled, {}] next {}: {}", t.id, t.describe(), t.due.format("%H:%M"), t.prompt))
        .collect()
}

/// `/tasks stop <id>` for a scheduled task.
fn stop_scheduled(d: &ViewState, id: &str) -> Option<String> {
    let s = d.scheduler.as_ref()?;
    let (t, record) = {
        let mut sched = s.lock().unwrap();
        let t = sched.delete(id)?;
        (t, sched.record())
    };
    d.transcript.set_schedule(record);
    Some(format!("Deleted scheduled task {} ({}).", t.id, t.describe()))
}

fn tasks(v: &SessionView, args: &str) -> Exec {
    let d = v.state();
    let shells = &d.tool_ctx.shells;
    if let Some(id) = args.strip_prefix("stop").map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(msg) = stop_scheduled(&d, id) {
            return ok(msg);
        }
        if let Some(msg) = crate::subtask::stop_row(&d.subtasks, &d.finished_subtasks, id) {
            return ok(msg);
        }
        return match shells.get(id) {
            Some(sh) => match sh.stop() {
                kodact_tools::shells::Stop::Asked { change } => ok(format!(
                    "Asked {id} to stop. It is installing or removing packages, so it was sent SIGINT, which {} \
                     stops on without breaking its package database: it finishes the step it is on, then ends by \
                     itself. `/tasks stop {id}` again stops it at once, which can leave the package database \
                     half-updated.",
                    change.manager
                )),
                kodact_tools::shells::Stop::Stopped { change: Some(c) } => {
                    ok(format!("Stopped {id}. {}", c.forced_note()))
                }
                _ => ok(format!("Stopped {id}.")),
            },
            None => err(format!("No background task {id}.")),
        };
    }
    if !args.is_empty() {
        return err("Usage: /tasks [stop <id>]");
    }
    let list = shells.list();
    let scheduled = scheduled(&d);
    if list.is_empty() && scheduled.is_empty() && d.subtasks.is_empty() && d.finished_subtasks.is_empty() {
        return ok("No background tasks.");
    }
    let mut s = String::from("Background tasks:\n");
    for t in &d.subtasks {
        let state = if t.is_running() { "running" } else { "done, reported with your next prompt" };
        let _ = writeln!(
            s,
            "  {} [subtask, {state}, {}, {} tool calls] {}",
            t.id,
            duration(t.started.elapsed()),
            t.tool_calls(),
            t.task.chars().take(100).collect::<String>()
        );
    }
    for f in &d.finished_subtasks {
        let _ = writeln!(
            s,
            "  {} [subtask, {}, took {}, {} tool calls] {}",
            f.id,
            f.status,
            duration(f.duration),
            f.tool_calls,
            f.task.chars().take(100).collect::<String>()
        );
    }
    for sh in list {
        let _ = writeln!(
            s,
            "  {} [{}, {}] {}",
            sh.id(),
            sh.status().label(),
            duration(sh.started.elapsed()),
            sh.command.chars().take(100).collect::<String>()
        );
    }
    for line in scheduled {
        let _ = writeln!(s, "{line}");
    }
    ok(s.trim_end())
}

pub(super) fn pct(n: u64, of: u64) -> String {
    if of == 0 {
        return "-".into();
    }
    format!("{:.1}%", n as f64 * 100.0 / of as f64)
}

/// What fills the context window: the numbers behind `/context` and its grid.
pub(super) struct ContextData {
    pub model: String,
    pub window: u64,
    pub system: u64,
    pub builtin: u64,
    pub mcp: u64,
    pub skills: u64,
    pub memory: u64,
    /// The memory files haven't been sent yet (they ride in the first prompt).
    pub memory_pending: bool,
    pub messages: u64,
    pub measured: u64,
    pub autocompact_at: Option<u64>,
    pub tips: Vec<String>,
    /// Per tool, largest first.
    pub tools: Vec<(String, u64)>,
    pub memory_files: Vec<(String, u64)>,
    pub skill_rows: Vec<(String, u64)>,
}

impl ContextData {
    pub fn total(&self) -> u64 {
        let pending = if self.memory_pending { self.memory } else { 0 };
        self.system + self.builtin + self.mcp + self.skills + self.messages + pending
    }

    pub fn free(&self) -> u64 {
        self.window.saturating_sub(self.total())
    }

    pub fn headline(&self) -> String {
        let total = self.total();
        format!(
            "Context: about {} of {} tokens ({}) for {}",
            thousands(total),
            thousands(self.window),
            pct(total, self.window),
            self.model
        )
    }

    /// What the endpoint counted for the last request, when it said.
    pub fn measured_line(&self) -> Option<String> {
        (self.measured > 0).then(|| format!("The last request measured {} tokens.", thousands(self.measured)))
    }
}

pub(super) fn context_data(v: &SessionView) -> ContextData {
    let d = v.state();
    let snap = v.engine();
    let model = d.handle.model();
    let window = snap.context_window(&model);
    let est = |s: &str| tokens_of(s) as u64;
    let system: u64 = snap.system.iter().map(|b| est(&b.text)).sum();
    let size = |s: &kodact_types::ToolSpec| est(&serde_json::to_string(s).unwrap_or_default());
    let (mut builtin, mut mcp, mut skills) = (0u64, 0u64, 0u64);
    for s in snap.tools.iter() {
        match s.name.as_str() {
            n if n.starts_with("mcp__") => mcp += size(s),
            "Skill" => skills += size(s),
            _ => builtin += size(s),
        }
    }
    let memory: u64 = d.memory.iter().map(|f| est(&f.content)).sum();
    let messages: u64 = snap.messages.iter().map(|m| est(&serde_json::to_string(m).unwrap_or_default())).sum();
    let mut tools: Vec<(String, u64)> = snap.tools.iter().map(|t| (t.name.clone(), size(t))).collect();
    tools.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    let mut c = ContextData {
        model: model.clone(),
        window,
        system,
        builtin,
        mcp,
        skills,
        memory,
        memory_pending: snap.messages.is_empty(),
        messages,
        measured: snap.context_tokens,
        autocompact_at: snap.auto_compact.then(|| snap.autocompact_at(&model)),
        tips: vec![],
        tools,
        memory_files: d.memory.iter().map(|f| (f.path.display().to_string(), est(&f.content))).collect(),
        skill_rows: d.skills.iter().map(|(n, desc)| (n.clone(), est(desc))).collect(),
    };
    let total = c.total();
    if mcp * 5 > total && mcp > 10_000 {
        c.tips.push("MCP tools take a large share; turn off servers you don't need for this task.".to_string());
    }
    if let Some(f) = d.memory.iter().find(|f| est(&f.content) > 5_000) {
        c.tips.push(format!(
            "{} is long (over 5k tokens); trimming it saves context on every request.",
            f.path.display()
        ));
    }
    if total * 10 > window * 6 {
        c.tips.push("Over 60% full: /compact now keeps what matters before automatic compaction does.".to_string());
    }
    c
}

/// What fills the context window, for a host (`get_context_usage`): the
/// numbers `/context` shows, as JSON. Counts are estimates at four
/// characters a token, except `measured`, the size the endpoint reported for
/// the last request (0 when it hasn't said).
pub fn context_usage(v: &SessionView) -> Value {
    let c = context_data(v);
    let rows = |rows: &[(String, u64)], key: &str| -> Vec<Value> {
        rows.iter().map(|(n, t)| serde_json::json!({ key: n, "tokens": t })).collect()
    };
    let mut categories = vec![
        serde_json::json!({"name": "System prompt", "tokens": c.system}),
        serde_json::json!({"name": "Built-in tools", "tokens": c.builtin}),
        serde_json::json!({"name": "MCP tools", "tokens": c.mcp}),
        serde_json::json!({"name": "Skills", "tokens": c.skills}),
    ];
    // Before the first prompt the memory files wait to be sent; after it
    // they are part of the messages, and counted there.
    if c.memory_pending && c.memory > 0 {
        categories.push(serde_json::json!({"name": "Memory files", "tokens": c.memory}));
    }
    categories.push(serde_json::json!({"name": "Messages", "tokens": c.messages}));
    serde_json::json!({
        "model": c.model,
        "window": c.window,
        "total": c.total(),
        "free": c.free(),
        "measured": c.measured,
        "autocompactAt": c.autocompact_at,
        "memoryPending": c.memory_pending,
        "memory": c.memory,
        "categories": categories,
        "tools": rows(&c.tools, "name"),
        "memoryFiles": rows(&c.memory_files, "path"),
        "skills": rows(&c.skill_rows, "name"),
        "tips": c.tips,
    })
}

fn context(v: &SessionView, args: &str) -> Exec {
    let all = match args {
        "" => false,
        "all" => true,
        _ => return err("Usage: /context [all]"),
    };
    let c = context_data(v);
    let window = c.window;
    let mut s = c.headline();
    if let Some(m) = c.measured_line() {
        s.push('\n');
        s.push_str(&m);
    }
    s.push_str("\n\n");
    let mut row = |name: &str, n: u64| {
        let _ = writeln!(s, "  {name:<20} {:>10}  {:>6}", thousands(n), pct(n, window));
    };
    row("System prompt", c.system);
    row("Built-in tools", c.builtin);
    row("MCP tools", c.mcp);
    row("Skills (listing)", c.skills);
    if c.memory_pending && c.memory > 0 {
        row("Memory files", c.memory);
    }
    row("Messages", c.messages);
    if c.memory > 0 && !c.memory_pending {
        row("  of which memory", c.memory);
    }
    row("Free", c.free());
    if let Some(at) = c.autocompact_at {
        let _ = writeln!(s, "\nAuto-compact runs at about {} tokens.", thousands(at));
    }
    if all {
        s.push_str("\nTools:\n");
        for (name, n) in &c.tools {
            let _ = writeln!(s, "  {name:<40} {:>8}", thousands(*n));
        }
        if !c.memory_files.is_empty() {
            s.push_str("\nMemory files:\n");
            for (path, n) in &c.memory_files {
                let _ = writeln!(s, "  {path:<40} {:>8}", thousands(*n));
            }
        }
        if !c.skill_rows.is_empty() {
            s.push_str("\nSkills (only names and descriptions are loaded until one is used):\n");
            for (name, n) in &c.skill_rows {
                let _ = writeln!(s, "  {name:<40} {:>8}", thousands(*n));
            }
        }
    }
    if !c.tips.is_empty() {
        let _ = write!(s, "\nSuggestions:\n  - {}", c.tips.join("\n  - "));
    }
    if !all {
        s.push_str("\n/context all lists each tool, memory file and skill.");
    }
    ok(s.trim_end())
}

/// `/btw`: answered from the view's conversation (mid-turn, as far as it has got). Its cost
/// and the exchange are left for the driver, which records them when it is next free.
async fn btw(v: &SessionView, args: &str, cancel: &CancellationToken) -> Exec {
    let d = v.state();
    // Earlier exchanges, including ones from this turn the driver hasn't recorded yet.
    let earlier = v.side_questions();
    if args.is_empty() {
        return match earlier.last() {
            Some((q, a)) => ok(format!("/btw {q}\n\n{a}")),
            None => ok("No side questions yet. Ask one with /btw <question>: KodAct answers from the conversation, without tools, and leaves the conversation as it was."),
        };
    }
    let rt = d.handle.runtime();
    // Background work (shells, subtasks, scheduled prompts) as /tasks lists it.
    let background = match tasks(v, "") {
        Exec::Local { text, is_error: false } if text != "No background tasks." => Some(text),
        _ => None,
    };
    let req = kodact_engine::side_question_request(&v.engine(), &rt, args, &earlier, background.as_deref());
    let msg = match kodact_api::complete(d.provider.as_ref(), req, cancel).await {
        Ok(m) => m,
        Err(kodact_api::ApiError::Cancelled) => return err("Could not answer: interrupted"),
        Err(e) => return err(format!("Could not answer: {}", e.describe())),
    };
    v.record(Effect::SideUsage { model: rt.model.clone(), usage: msg.usage.clone() });
    let answer = msg.to_message().text().trim().to_string();
    if answer.is_empty() {
        return err("Could not answer: the model gave no answer");
    }
    v.record(Effect::SideQuestion { question: args.to_string(), answer: answer.clone() });
    ok(answer)
}
