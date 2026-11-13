//! `kodact`: argument parsing and dispatch. Handlers stay thin; the work
//! happens in kodact-core (session assembly) and kodact-engine (the loop).
//! Results go to stdout, diagnostics to stderr; exit statuses are in exit.rs.

/// `println!` that ends quietly when stdout's reader is gone (see `output::stdout_line`).
macro_rules! outln {
    () => {
        $crate::output::stdout_line(format_args!(""))
    };
    ($($arg:tt)*) => {
        $crate::output::stdout_line(format_args!($($arg)*))
    };
}

mod args;
mod exit;
mod host;
mod mcp_cmd;
mod output;
mod relay;
mod repl;
mod term;
mod tui;

use std::io::Read;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use clap::{CommandFactory, Parser};
use kodact_core::{build_session, LaunchOptions, Resume};
use kodact_engine::{DenyPrompter, EventSink, PermissionPrompter};
use kodact_types::sdk::{SdkMessage, SystemMessage};
use kodact_types::MessageContent;
use serde_json::{json, Value};
use tokio::sync::mpsc;

use args::{Cli, Command, ConfigAction, InputFormat, Opts, OutputFormat, PermissionPrompts};
use exit::Fail;
use host::{ControlContext, HostPrompter, Input};
use output::{result_message, Out, QuietSink, StreamSink};

fn read_file(p: &PathBuf, what: &str) -> Result<String, Fail> {
    std::fs::read_to_string(p).map_err(|e| Fail::usage(format!("cannot read {what} {}: {e}", p.display())))
}

/// Validate flags and turn them into launch options (no I/O beyond reading named files).
fn launch_options(o: &Opts) -> Result<LaunchOptions, Fail> {
    let cwd = std::env::current_dir().map_err(|e| Fail::config(format!("cannot read the current directory: {e}")))?;
    let system_prompt = match (&o.system_prompt, &o.system_prompt_file) {
        (Some(_), Some(_)) => return Err(Fail::usage("use either --system-prompt or --system-prompt-file, not both")),
        (Some(s), None) => Some(s.clone()),
        (None, Some(f)) => Some(read_file(f, "system prompt file")?),
        (None, None) => None,
    };
    let append = match (&o.append_system_prompt, &o.append_system_prompt_file) {
        (Some(a), Some(f)) => Some(format!("{a}\n\n{}", read_file(f, "append system prompt file")?)),
        (Some(a), None) => Some(a.clone()),
        (None, Some(f)) => Some(read_file(f, "append system prompt file")?),
        (None, None) => None,
    };
    let json_schema = match &o.json_schema {
        Some(s) => Some(
            serde_json::from_str::<Value>(s)
                .map_err(|e| Fail::usage(format!("--json-schema is not valid JSON: {e}")))?,
        ),
        None => None,
    };
    let resume = match (&o.resume, o.continue_) {
        (Some(id), _) if !id.is_empty() => Resume::Id(id.clone()),
        (Some(_), _) | (None, true) => Resume::Latest,
        (None, false) => Resume::New,
    };
    if o.fork_session && resume == Resume::New {
        return Err(Fail::usage("--fork-session needs --resume or --continue"));
    }
    let setting_sources = match &o.setting_sources {
        Some(s) => Some(kodact_config::parse_sources(s).map_err(Fail::usage)?),
        None => None,
    };
    if let Some(m) = &o.permission_mode {
        if kodact_permissions::PermissionMode::parse(m).is_none() {
            return Err(Fail::usage(format!(
                "invalid --permission-mode {m:?}: expected one of acceptEdits, auto, bypassPermissions, manual, dontAsk, plan"
            )));
        }
    }
    if let Some(e) = &o.effort {
        if !["low", "medium", "high", "xhigh", "max"].contains(&e.as_str()) {
            return Err(Fail::usage(format!("invalid --effort {e:?}: expected low, medium, high, xhigh or max")));
        }
    }
    if let Some(b) = o.max_budget_usd {
        if !(b > 0.0 && b.is_finite()) {
            return Err(Fail::usage("--max-budget-usd must be a positive number"));
        }
    }
    if o.max_turns == Some(0) {
        return Err(Fail::usage("--max-turns must be at least 1"));
    }
    if let Some(id) = &o.session_id {
        kodact_session::validate_session_id(id)
            .map_err(|_| Fail::usage(format!("--session-id {id:?} is not a UUID")))?;
    }
    for d in &o.add_dir {
        if !d.is_dir() {
            return Err(Fail::usage(format!("--add-dir {}: not a directory", d.display())));
        }
    }
    Ok(LaunchOptions {
        cwd,
        model: o.model.clone(),
        fallback_models: o.fallback_model.iter().cloned().collect(),
        effort: o.effort.clone(),
        permission_mode: o.permission_mode.clone(),
        dangerously_skip_permissions: o.dangerously_skip_permissions,
        allow_dangerously_skip_permissions: o.allow_dangerously_skip_permissions,
        allowed_tools: o.allowed_tools.clone(),
        disallowed_tools: o.disallowed_tools.clone(),
        tools: o.tools.clone(),
        add_dirs: o.add_dir.clone(),
        system_prompt,
        append_system_prompt: append,
        exclude_dynamic_system_prompt_sections: o.exclude_dynamic_system_prompt_sections,
        settings: o.settings.clone(),
        setting_sources,
        max_turns: o.max_turns,
        max_budget_usd: o.max_budget_usd,
        max_time: o.max_time,
        autonomous: o.autonomous,
        one_shot: o.print && o.input_format != InputFormat::StreamJson,
        json_schema,
        resume,
        fork_session: o.fork_session,
        session_id: o.session_id.clone(),
        no_session_persistence: o.no_session_persistence,
        bare: o.bare || o.safe_mode,
        betas: o.betas.clone(),
        autocompact: o.autocompact.clone(),
        agents_json: o.agents.clone(),
        agent: o.agent.clone(),
        sandbox: o.sandbox.clone(),
        worktree: o.worktree.clone(),
        plugin_dirs: o.plugin_dir.clone(),
        mcp_configs: o.mcp_config.clone(),
        strict_mcp_config: o.strict_mcp_config,
        mcp: None,
        shells: None,
        provider: None,
        store_root: None,
    })
}

fn init_tracing(o: &Opts) {
    if o.debug.is_some() || o.debug_file.is_some() || std::env::var_os("KODACT_LOG").is_some() {
        let filter = std::env::var("KODACT_LOG").unwrap_or_else(|_| "debug".into());
        let builder = tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false);
        match &o.debug_file {
            Some(path) => {
                if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
                    builder.with_writer(Mutex::new(f)).init();
                    kodact_core::debug::set_startup(path.display().to_string());
                }
            }
            None => {
                builder.with_writer(std::io::stderr).init();
                kodact_core::debug::set_startup("stderr");
            }
        }
        return;
    }
    // Off until `/debug` turns it on, then to a file.
    use tracing_subscriber::prelude::*;
    let file: Arc<Mutex<Option<std::fs::File>>> = Arc::new(Mutex::new(None));
    let (filter, reload) = tracing_subscriber::reload::Layer::new(tracing_subscriber::EnvFilter::new("off"));
    let sink = file.clone();
    let layer = tracing_subscriber::fmt::layer().with_ansi(false).with_writer(move || SwitchWriter(sink.clone()));
    if tracing_subscriber::registry().with(filter).with(layer).try_init().is_err() {
        return;
    }
    kodact_core::debug::set_enabler(move |path| {
        if let Some(dir) = path.parent() {
            if dir.starts_with(kodact_config::state_dir()) {
                let _ = kodact_config::private_state_dir();
            }
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let f = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| e.to_string())?;
        *file.lock().unwrap() = Some(f);
        reload
            .reload(tracing_subscriber::EnvFilter::new("debug,hyper=info,h2=info,rustls=info,reqwest=info"))
            .map_err(|e| e.to_string())
    });
}

/// The `/debug` log: discards everything until a file is set.
struct SwitchWriter(Arc<Mutex<Option<std::fs::File>>>);

impl std::io::Write for SwitchWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.0.lock().unwrap().as_mut() {
            Some(f) => f.write(buf),
            None => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self.0.lock().unwrap().as_mut() {
            Some(f) => f.flush(),
            None => Ok(()),
        }
    }
}

/// The prompt for print mode: the argument and/or piped stdin.
fn print_prompt(arg: Option<String>) -> String {
    let mut piped = String::new();
    if !term::get().stdin_tty {
        std::io::stdin().read_to_string(&mut piped).ok();
    }
    match (arg, piped.trim().is_empty()) {
        (Some(a), true) => a,
        (Some(a), false) => format!("{}\n\n{a}", piped.trim_end()),
        (None, false) => piped,
        (None, true) => String::new(),
    }
}

/// A failure before any run started, in the promised machine format.
fn machine_error(format: OutputFormat, f: &Fail) {
    let doc = json!({
        "type": "error",
        "error": {"message": f.message, "hint": f.hint, "exit_code": f.code},
    });
    match format {
        OutputFormat::Json | OutputFormat::StreamJson => outln!("{doc}"),
        OutputFormat::Text => {}
    }
}

async fn run_print(cli_prompt: Option<String>, o: Opts) -> Result<i32, Fail> {
    let stream_out = o.output_format == OutputFormat::StreamJson;
    let stream_in = o.input_format == InputFormat::StreamJson;
    if stream_in && !stream_out {
        return Err(Fail::usage("--input-format=stream-json requires --output-format=stream-json"));
    }
    if o.replay_user_messages && !(stream_in && stream_out) {
        return Err(Fail::usage(
            "--replay-user-messages requires --input-format=stream-json and --output-format=stream-json",
        ));
    }
    if o.include_partial_messages && !stream_out {
        return Err(Fail::usage("--include-partial-messages requires --output-format=stream-json"));
    }
    let checked = launch_options(&o).and_then(|lo| {
        let p = if stream_in { None } else { Some(print_prompt(cli_prompt.clone())) };
        if matches!(&p, Some(p) if p.trim().is_empty()) {
            return Err(Fail::usage("no prompt: pass it as an argument or on stdin")
                .with_hint("kodact -p \"explain this repo\"  or  git diff | kodact -p \"review this\""));
        }
        Ok((lo, p))
    });
    let (mut lo, first_prompt) = match checked {
        Ok(v) => v,
        Err(f) => {
            machine_error(o.output_format, &f);
            return Err(f);
        }
    };

    let out = Arc::new(Out::default());
    let pending = Arc::new(Mutex::new(Default::default()));
    let host_prompts = stream_in
        && !o.no_input
        && o.permission_prompts == PermissionPrompts::Host
        && o.permission_prompt_tool.as_deref().map(|t| t == "stdio").unwrap_or(true);
    let prompter: Arc<dyn PermissionPrompter> =
        if host_prompts { Arc::new(HostPrompter::new(out.clone(), pending.clone())) } else { Arc::new(DenyPrompter) };

    if lo.session_id.is_none() && lo.resume == Resume::New {
        lo.session_id = Some(uuid::Uuid::new_v4().to_string());
    }
    let sink_id = Arc::new(Mutex::new(lo.session_id.clone().unwrap_or_default()));
    let sink: Arc<dyn EventSink> = if stream_out {
        Arc::new(StreamSink {
            out: out.clone(),
            session_id: sink_id.clone(),
            include_partial: o.include_partial_messages,
            replay_user_messages: o.replay_user_messages,
            verbose: o.verbose,
        })
    } else {
        Arc::new(QuietSink { verbose: o.verbose, quiet: o.quiet })
    };

    let (mcp, mcp_warnings) = kodact_core::connect_mcp(&lo).await;
    lo.mcp = Some(mcp.clone());
    let rebuild = (lo.clone(), sink.clone(), prompter.clone());
    let mut session = match build_session(lo, sink, prompter) {
        Ok(s) => s,
        Err(e) => {
            let f = Fail::from(e);
            machine_error(o.output_format, &f);
            mcp.shutdown().await;
            return Err(f);
        }
    };
    if !o.quiet {
        for w in &mcp_warnings {
            eprintln!("{} {w}", term::yellow("kodact: warning:"));
        }
    }
    // The endpoint's window for an unknown model, before the first request uses a guess.
    session.learn_window().await;
    *sink_id.lock().unwrap() = session.session_id.clone();
    if !o.quiet {
        for w in &session.warnings {
            eprintln!("{} {w}", term::yellow("kodact: warning:"));
        }
    }
    let mut session_id = session.session_id.clone();
    if stream_out {
        out.line(&SdkMessage::System(SystemMessage::init(&session.init)));
    }
    let surface =
        if stream_in { kodact_core::commands::Surface::Stream } else { kodact_core::commands::Surface::Print };
    let mut driver = kodact_core::Driver::new(session, surface, Some(mcp.clone()));
    driver.set_rebuild(rebuild.0, rebuild.1, rebuild.2);
    let live = driver.live();

    // Ctrl-C interrupts the current turn (the result is still written); a second one exits at once.
    let handle = live.clone();
    let interrupted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = interrupted.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
            handle.handle().interrupt();
            if tokio::signal::ctrl_c().await.is_ok() {
                std::process::exit(exit::INTERRUPTED);
            }
        }
    });

    let (tx, mut rx) = mpsc::unbounded_channel::<Input>();
    let mut control: Option<Arc<ControlContext>> = None;
    if stream_in {
        // The endpoint's own models (none built in), with what KodAct knows about each.
        let _ = driver.load_models().await;
        let models: Vec<Value> = driver
            .model_choices()
            .into_iter()
            .map(|id| {
                let m = kodact_api::models::model_info(&id);
                json!({"value": id, "displayName": id,
                       "supportsEffort": m.is_some_and(|m| m.supports_effort()),
                       "supportedEffortLevels": m.map(|m| m.effort_levels).unwrap_or_default()})
            })
            .collect();
        let ctx = Arc::new(ControlContext {
            out: out.clone(),
            pending: pending.clone(),
            live: live.clone(),
            mcp: Some(mcp.clone()),
            bypass_allowed: driver.bypass_allowed(),
            init_response: json!({
                "commands": driver.catalog.catalog_json(surface),
                "output_style": driver.info.init.output_style.clone(),
                "available_output_styles": driver.catalog.styles.iter().map(|s| s.name.clone()).collect::<Vec<_>>(),
                "models": models,
                "pid": std::process::id(),
            }),
            tasks: Mutex::new(vec![]),
            view: driver.view(),
        });
        control = Some(ctx.clone());
        tokio::spawn(host::read_stdin(ctx, tx));
    } else {
        let _ = tx.send(Input::User(MessageContent::Text(first_prompt.unwrap_or_default())));
        let _ = tx.send(Input::Eof);
    }

    let mut rep = Reporter {
        format: o.output_format,
        out: out.clone(),
        live: live.clone(),
        code: exit::OK,
        last_json: None,
        limit_hit: false,
    };
    let mut closed = false;
    // Changes each time a subtask finishes (C20).
    let mut finished = driver.subtasks.watch();
    let view = driver.view();
    loop {
        // Mark subtask news seen before looking at the subtasks: one that ends after this line
        // still wakes the waits below.
        let _ = *finished.borrow_and_update();
        // Scheduled tasks (C19) fire while idle. After the input ends, print mode keeps running while
        // any are pending or subtasks are out: until they finish, Ctrl-C, or a turn or budget limit.
        let wait = driver.next_wait();
        if closed {
            let turns_out = o.max_turns.is_some_and(|m| driver.activity.turns >= m);
            let running = driver.subtasks.running() > 0;
            if rep.limit_hit || turns_out {
                if wait.is_some() || !driver.subtasks.is_empty() {
                    // Work was still scheduled or out when a limit stopped the run.
                    rep.fail(exit::LIMIT);
                }
                break;
            }
            // --max-turns counts across the whole run: what runs next gets what is left.
            if let Some(m) = o.max_turns {
                driver.engine.cfg.max_turns = Some(m - driver.activity.turns);
            }
            if !running && !driver.subtasks.is_empty() {
                // Every subtask is back: one more turn gives the model their reports.
                driver.continue_with_subtasks(&mut |r| rep.report(r)).await;
            } else if wait.is_none() && !running {
                break;
            } else {
                let far = Duration::from_secs(365 * 86_400);
                tokio::select! {
                    due = sleep_unless(wait.unwrap_or(far), &interrupted) => {
                        if !due {
                            break;
                        }
                        driver.run_due(&mut |r| rep.report(r)).await;
                    }
                    _ = finished.changed(), if running => {}
                }
            }
        } else {
            // stream-json: a finished subtask is reported at once, and its report rides along with
            // the host's next message (no turn runs by itself). Plain -p hands back after its input.
            if stream_in {
                driver.deliver_subtasks();
            }
            let far = Duration::from_secs(365 * 86_400);
            let input = tokio::select! {
                i = rx.recv() => i,
                _ = tokio::time::sleep(wait.unwrap_or(far)), if wait.is_some() => {
                    driver.run_due(&mut |r| rep.report(r)).await;
                    rep.flush();
                    continue;
                }
                _ = finished.changed(), if stream_in => continue,
                // An immediate command left work for the driver (/btw's cost, /mcp's refresh).
                _ = view.effect_recorded(), if stream_in => {
                    driver.sync_view();
                    continue;
                }
            };
            match input {
                Some(Input::User(content)) => {
                    let mut flow = driver.input(content, &mut |r| rep.report(r)).await;
                    // Messages sent for the turn's next step that it ended before reaching: they
                    // run now, as one turn, in the order sent (the turn refuses new ones once over).
                    loop {
                        let left = driver.handle().take_steers();
                        if left.is_empty() || flow == kodact_core::Flow::Exit {
                            break;
                        }
                        rep.flush();
                        let mut blocks = Vec::new();
                        for (i, steer) in left.into_iter().enumerate() {
                            if i > 0 {
                                blocks.push(kodact_types::ContentBlock::text("\n\n"));
                            }
                            blocks.extend(steer.content.into_blocks());
                        }
                        let joined = kodact_types::MessageContent::Blocks(blocks);
                        flow = driver.input(joined, &mut |r| rep.report(r)).await;
                    }
                    // A single prompt that set a goal fails when the goal ends unmet.
                    if !stream_in && rep.code == exit::OK && driver.goal.as_ref().is_some_and(|g| g.ended_unmet()) {
                        rep.fail(exit::FAILED);
                    }
                    if flow == kodact_core::Flow::Exit {
                        rep.flush();
                        break;
                    }
                }
                Some(Input::SystemPrompt { replace, append }) => driver.set_system_prompt(replace, append),
                Some(Input::McpChanged) => driver.refresh_mcp(),
                Some(Input::Eof) | None => closed = true,
            }
        }
        // JSON output is one object: while subtasks are out, it waits for the turn that hands them back.
        if !(o.output_format == OutputFormat::Json && !driver.subtasks.is_empty()) {
            rep.flush();
        }
        // A command switched sessions (/clear, /resume, /branch, /cd): say so to stream hosts.
        if driver.info.session_id != session_id {
            session_id = driver.info.session_id.clone();
            *sink_id.lock().unwrap() = session_id.clone();
            if stream_out {
                out.line(&SdkMessage::System(SystemMessage::init(&driver.info.init)));
            }
        }
    }
    rep.flush();
    let code = rep.code;
    // MCP restarts a host asked for answer before KodAct exits.
    if let Some(c) = &control {
        c.finish_tasks().await;
    }
    driver.shutdown("other").await;
    if interrupted.load(std::sync::atomic::Ordering::SeqCst) {
        return Ok(exit::INTERRUPTED);
    }
    Ok(code)
}

/// Writes each turn's result in the output format and keeps the exit status.
/// One input can run several turns (a goal, a scheduled task): stream-json gets
/// every result as it finishes; JSON output is one object, the last result.
struct Reporter {
    format: OutputFormat,
    out: Arc<Out>,
    live: kodact_core::driver::Live,
    code: i32,
    last_json: Option<Value>,
    /// A turn or budget limit ended a turn (whatever the exit status already was).
    limit_hit: bool,
}

impl Reporter {
    fn report(&mut self, r: &kodact_engine::TurnResult) {
        let c = exit::for_result(r);
        if c == exit::LIMIT {
            self.limit_hit = true;
        }
        if self.code == exit::OK {
            self.code = c;
        }
        match self.format {
            OutputFormat::StreamJson => {
                self.out.line(&SdkMessage::Result(Box::new(result_message(r, &self.live.session_id()))))
            }
            OutputFormat::Json => {
                let mut v =
                    serde_json::to_value(SdkMessage::Result(Box::new(result_message(r, &self.live.session_id()))))
                        .unwrap_or_default();
                v["exit_code"] = json!(c);
                self.last_json = Some(v);
            }
            OutputFormat::Text => {
                if let Some(b) = &r.prompt_blocked {
                    eprintln!("{} prompt blocked by a UserPromptSubmit hook: {b}", term::red("kodact:"));
                } else if r.is_error {
                    let msg = r
                        .result
                        .clone()
                        .or_else(|| r.errors.first().cloned())
                        .unwrap_or_else(|| "the run failed".into());
                    eprintln!("{} {msg}", term::red("kodact:"));
                } else if let Some(t) = &r.result {
                    outln!("{t}");
                }
            }
        }
    }

    fn fail(&mut self, code: i32) {
        self.code = code;
        if let Some(v) = self.last_json.as_mut() {
            v["exit_code"] = json!(code);
        }
    }

    fn flush(&mut self) {
        if let Some(v) = self.last_json.take() {
            outln!("{v}");
        }
    }
}

/// Sleep for `d`, waking early on Ctrl-C. False when interrupted.
async fn sleep_unless(d: Duration, interrupted: &std::sync::atomic::AtomicBool) -> bool {
    let end = std::time::Instant::now() + d;
    loop {
        if interrupted.load(std::sync::atomic::Ordering::SeqCst) {
            return false;
        }
        let left = end.saturating_duration_since(std::time::Instant::now());
        if left.is_zero() {
            return true;
        }
        tokio::time::sleep(left.min(Duration::from_millis(200))).await;
    }
}

/// The terminal UI, unless `--no-tui` or `KODACT_TUI=0|false|off`, or stdout isn't a terminal.
fn use_tui(o: &Opts, t: term::Term) -> bool {
    let env_off = std::env::var("KODACT_TUI")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "0" | "false" | "off" | "no"))
        .unwrap_or(false);
    t.stdin_tty && t.stdout_tty && !o.no_tui && !env_off
}

async fn run_doctor(probe: bool) -> Result<i32, Fail> {
    let cwd = std::env::current_dir().map_err(|e| Fail::config(e.to_string()))?;
    outln!("KodAct {}", kodact_core::VERSION);
    let mut checks = kodact_core::doctor::checks(&cwd);
    if probe {
        checks.push(kodact_core::doctor::probe(&cwd).await);
    }
    for c in &checks {
        let mark = match (c.ok, c.note) {
            (false, _) => term::red("FAIL"),
            (true, true) => term::paint("33", "note"),
            (true, false) => term::paint("32", "ok  "),
        };
        outln!("{mark} {:<12} {}", c.name, c.detail);
    }
    Ok(if checks.iter().all(|c| c.ok) { exit::OK } else { exit::CONFIG })
}

fn run_config(action: Option<ConfigAction>) -> Result<i32, Fail> {
    let cwd = std::env::current_dir().map_err(|e| Fail::config(e.to_string()))?;
    let opts = kodact_config::SettingsOptions::new(&cwd);
    let settings = kodact_config::load_settings(&opts);
    for e in &settings.errors {
        eprintln!("{} {e}", term::yellow("kodact: warning: invalid settings file skipped:"));
    }
    let pretty = |v: &Value| serde_json::to_string_pretty(&kodact_config::redact(v)).unwrap_or_default();
    match action {
        None | Some(ConfigAction::List { origin: false }) => outln!("{}", pretty(&settings.merged)),
        Some(ConfigAction::List { origin: true }) => {
            let mut out = serde_json::Map::new();
            for (k, v) in settings.merged.as_object().into_iter().flatten() {
                let layer = settings.layers.iter().rev().find(|l| l.value.get(k).is_some());
                out.insert(
                    k.clone(),
                    json!({
                        "value": kodact_config::redact(&json!({k.as_str(): v}))[k.as_str()].clone(),
                        "source": layer.map(|l| l.source.as_str()),
                        "file": layer.and_then(|l| l.path.as_ref()).map(|p| p.display().to_string()),
                    }),
                );
            }
            outln!("{}", serde_json::to_string_pretty(&Value::Object(out)).unwrap_or_default());
        }
        Some(ConfigAction::Get { key }) => {
            let ptr = if key.starts_with('/') { key.clone() } else { format!("/{}", key.replace('.', "/")) };
            match settings.get(&ptr) {
                Some(v) => {
                    let last = ptr.rsplit('/').next().unwrap_or("");
                    let shown = kodact_config::redact(&json!({last: v}))[last].clone();
                    outln!(
                        "{}",
                        if shown.is_string() { shown.as_str().unwrap_or("").to_string() } else { pretty(&shown) }
                    );
                }
                None => {
                    eprintln!("kodact: {key} is not set");
                    return Ok(exit::FAILED);
                }
            }
        }
        Some(ConfigAction::Paths) => {
            outln!("config  {}", kodact_config::config_dir().display());
            outln!("state   {}", kodact_config::state_dir().display());
            outln!("cache   {}", kodact_config::cache_dir().display());
            for l in &settings.layers {
                if let Some(p) = &l.path {
                    outln!("{:<7} {}", l.source.as_str().trim_end_matches("Settings"), p.display());
                }
            }
        }
    }
    Ok(exit::OK)
}

/// Settings made before KodAct was renamed (from ForgeCLI) keep working: each
/// `FORGE_*` variable stands for its `KODACT_*` name when that one is unset.
/// Runs first, while the process has one thread, before anything reads a setting.
fn adopt_forgecli_env() {
    let legacy: Vec<(String, std::ffi::OsString)> = std::env::vars_os()
        .filter_map(|(k, v)| {
            let k = k.into_string().ok()?;
            let rest = k.strip_prefix("FORGE_")?;
            Some((format!("KODACT_{rest}"), v))
        })
        .collect();
    for (name, value) in legacy {
        if std::env::var_os(&name).is_none() {
            std::env::set_var(name, value);
        }
    }
}

fn main() {
    adopt_forgecli_env();
    // clap prints help and version and exits (0) or reports usage errors (2) before anything else starts.
    let cli = Cli::parse();
    term::init(cli.opts.color);
    init_tracing(&cli.opts);
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("kodact: cannot start the async runtime: {e}");
            std::process::exit(exit::FAILED);
        }
    };
    let result = rt.block_on(async move {
        match cli.command {
            Some(Command::Doctor { probe }) => run_doctor(probe).await,
            Some(Command::Config { action }) => run_config(action),
            Some(Command::Completion { shell }) => {
                let mut script = vec![];
                clap_complete::generate(shell, &mut Cli::command(), "kodact", &mut script);
                outln!("{}", String::from_utf8_lossy(&script).trim_end());
                Ok(exit::OK)
            }
            Some(Command::Mcp { action }) => mcp_cmd::run(action, &cli.opts).await,
            Some(Command::Relay { port }) => relay::run(port).await,
            None if cli.opts.print => run_print(cli.prompt, cli.opts).await,
            None => {
                let t = term::get();
                if cli.opts.no_input {
                    Err(Fail::usage("interactive mode needs input, but --no-input is set")
                        .with_hint("Use print mode: kodact -p \"<prompt>\""))
                } else if !t.stdin_tty {
                    Err(Fail::usage("interactive mode needs a terminal on stdin").with_hint(
                        "For scripts and pipes use print mode: kodact -p \"<prompt>\", or: echo \"<prompt>\" | kodact -p",
                    ))
                } else if use_tui(&cli.opts, t) {
                    tui::run(cli.prompt, cli.opts).await
                } else {
                    repl::run(cli.prompt, cli.opts).await
                }
            }
        }
    });
    let code = match result {
        Ok(code) => code,
        Err(f) => {
            eprintln!("{} {}", term::red("kodact:"), f.message);
            if let Some(h) = &f.hint {
                eprintln!("  {} {h}", term::dim("hint:"));
            }
            f.code
        }
    };
    rt.shutdown_timeout(std::time::Duration::from_millis(200));
    std::process::exit(code);
}
