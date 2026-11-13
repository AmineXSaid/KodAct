//! The session task: it owns the [`Driver`] and runs what the UI sends it,
//! one input at a time. Engine events, questions for the person and command
//! answers go back to the UI as [`UiEvent`]s. The loop mirrors the line REPL
//! (`repl.rs`): scheduled tasks fire and finished subtasks are handed back
//! while the session is idle.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use kodact_core::commands::picker::picker;
use kodact_core::commands::{parse, Builtin, Catalog, Exec, Invocation, Surface};
use kodact_core::view::SessionView;
use kodact_core::{Driver, Flow};
use kodact_engine::{EngineEvent, EventSink, PermissionAnswer, PermissionPrompt, PermissionPrompter, TurnResult};
use kodact_types::MessageContent;
use tokio::sync::{mpsc, oneshot};

use super::app::{CommandInfo, StatusView, UiEvent};

/// From the UI to the session task.
#[derive(Debug, Clone, PartialEq)]
pub enum ToSession {
    Input(String),
    /// The picker for a command's second step (`/rewind 2`).
    Picker(String),
    Exit,
}

/// Sends every engine event to the UI. The channel is unbounded, so the engine never waits.
pub struct TuiSink(pub mpsc::UnboundedSender<UiEvent>);

impl EventSink for TuiSink {
    fn emit(&self, event: EngineEvent) {
        let _ = self.0.send(UiEvent::Engine(event));
    }
}

/// Asks the person through a dialog and waits for the answer.
pub struct TuiPrompter(pub mpsc::UnboundedSender<UiEvent>);

#[async_trait::async_trait]
impl PermissionPrompter for TuiPrompter {
    async fn ask(&self, prompt: PermissionPrompt) -> PermissionAnswer {
        let gone = || PermissionAnswer::Deny { message: "The UI closed.".into(), interrupt: true };
        let (reply, answer) = oneshot::channel();
        if self.0.send(UiEvent::Ask { prompt, reply }).is_err() {
            return gone();
        }
        answer.await.unwrap_or_else(|_| gone())
    }
}

/// What the status line shows, read from the driver.
pub fn status(d: &Driver) -> StatusView {
    let model = d.handle().model();
    let mode = d.handle().permissions.read().unwrap().mode.as_str().to_string();
    let window = d.engine.context_window(&model);
    let used = d.engine.projected_context_tokens();
    let context_pct = (window > 0).then(|| ((used * 100 / window).min(100)) as u8);
    let cost_unknown = !d.engine.state.unpriced.is_empty() || !d.engine.has_price(&model);
    StatusView {
        model,
        mode,
        cwd: d.info.cwd.display().to_string(),
        branch: kodact_git::current_branch(&d.info.cwd).unwrap_or_default(),
        cost: d.engine.state.total_cost_usd,
        cost_unknown,
        context_pct,
        context_used: used > 0,
        window,
        context_tokens: used,
        compact_at: d.engine.autocompact_at(&d.handle().model()),
        window_guessed: d.engine.cfg.autocompact_window.is_none()
            && kodact_api::models::limits_source(&d.handle().model()) == kodact_api::models::LimitsSource::Guessed,
        session_tokens: d.engine.state.total_usage.context_tokens(),
    }
}

/// The `/` menu's rows: every command the TUI can run.
pub fn commands(d: &Driver) -> Vec<CommandInfo> {
    d.catalog
        .catalog_json(Surface::Tui)
        .iter()
        .map(|c| CommandInfo {
            name: c["name"].as_str().unwrap_or("").to_string(),
            args: c["argumentHint"].as_str().unwrap_or("").to_string(),
            description: c["description"].as_str().unwrap_or("").to_string(),
        })
        .collect()
}

/// What a finished turn shows. A model turn's text and its errors already
/// arrived as engine events, so only local answers, blocked prompts and
/// interrupts are shown here.
pub fn reply_for(r: &TurnResult) -> Option<UiEvent> {
    if let Some(b) = &r.prompt_blocked {
        return Some(UiEvent::Reply { text: format!("Prompt blocked by a hook: {b}"), is_error: true });
    }
    if r.num_turns == 0 && r.stop_reason.is_none() {
        return Some(UiEvent::Reply { text: r.result.clone().unwrap_or_default(), is_error: r.is_error });
    }
    if r.stop_reason.as_deref() == Some("interrupted") {
        return Some(UiEvent::Reply { text: "Interrupted · What should KodAct do instead?".into(), is_error: false });
    }
    // A model turn's errors (API errors, limits) arrived as error notices already.
    None
}

/// The `n`th latest answer's text (1 is the latest).
pub fn answer_text(d: &Driver, n: usize) -> Option<String> {
    d.engine
        .state
        .messages
        .iter()
        .rev()
        .filter(|m| m.role == kodact_types::Role::Assistant)
        .map(|m| m.content.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("\n"))
        .filter(|t| !t.trim().is_empty())
        .nth(n.checked_sub(1)?)
}

/// Commands the UI answers itself, since they need the terminal: `/copy`,
/// `/keybindings`, `/terminal-setup`. `None` for everything else.
fn ui_command(d: &Driver, text: &str) -> Option<Vec<UiEvent>> {
    let Invocation::Builtin { spec, args } = parse(text, &d.catalog) else { return None };
    let reply = |text: String, is_error: bool| UiEvent::Reply { text, is_error };
    Some(match spec.id {
        Builtin::Keybindings => vec![reply(super::app::keys_text(&super::keys::Keymap::load().0), false)],
        Builtin::TerminalSetup => vec![reply(super::app::terminal_setup_text(super::keyboard_protocol()), false)],
        Builtin::Copy => {
            let n = if args.is_empty() { Some(1) } else { args.parse::<usize>().ok().filter(|n| *n > 0) };
            let Some(n) = n else {
                return Some(vec![reply("Usage: /copy [n]  (1 is the latest answer)".into(), true)]);
            };
            match answer_text(d, n) {
                Some(t) => {
                    let which = if n == 1 { "the latest answer".to_string() } else { format!("answer {n} back") };
                    let note = format!(
                        "Copied {which} ({} characters) to the clipboard. Terminals without clipboard \
                         access (OSC 52) ignore it.",
                        t.chars().count()
                    );
                    vec![UiEvent::Copy(t), reply(note, false)]
                }
                None => vec![reply(format!("There is no answer {n} back to copy."), true)],
            }
        }
        _ => return None,
    })
}

/// The session as JSON for the `statusLine` command's stdin.
pub fn status_json(d: &Driver) -> serde_json::Value {
    let s = status(d);
    serde_json::json!({
        "session_id": d.info.session_id,
        "cwd": s.cwd,
        "model": {"id": s.model, "display_name": s.model},
        "workspace": {"current_dir": s.cwd},
        "cost": {"total_cost_usd": s.cost},
        "context": {"used_percentage": s.context_pct},
        "permission_mode": s.mode,
        "version": kodact_core::VERSION,
    })
}

/// The `statusLine.command` setting, unless hooks are turned off.
fn status_command(d: &Driver) -> Option<String> {
    if d.info.settings.get("/disableAllHooks").and_then(serde_json::Value::as_bool) == Some(true) {
        return None;
    }
    d.info.settings.str("/statusLine/command").map(str::to_string).filter(|c| !c.trim().is_empty())
}

/// Run the status line command (at most 3 s) and send its first output line.
fn refresh_status_line(
    shell: kodact_platform::shell::ShellChoice,
    cmd: String,
    input: serde_json::Value,
    cwd: std::path::PathBuf,
    ui: mpsc::UnboundedSender<UiEvent>,
) {
    tokio::spawn(async move {
        let line = run_status_command(&shell, &cmd, &input, &cwd).await;
        let _ = ui.send(UiEvent::StatusLine(line));
    });
}

pub async fn run_status_command(
    shell: &kodact_platform::shell::ShellChoice,
    cmd: &str,
    input: &serde_json::Value,
    cwd: &std::path::Path,
) -> Option<String> {
    use tokio::io::AsyncWriteExt;
    let shell = shell.as_ref().ok()?;
    // Windows: the script file must outlive the process.
    let (mut std_cmd, _script) = shell.command(&shell.script(cmd, None)).ok()?;
    kodact_platform::process::no_window(&mut std_cmd);
    let mut child = tokio::process::Command::from(std_cmd)
        .current_dir(cwd)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(input.to_string().as_bytes()).await;
    }
    let out = tokio::time::timeout(Duration::from_secs(3), child.wait_with_output()).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let first = text.lines().next()?.trim_end();
    // Control characters would break the screen; colours aren't kept.
    let clean: String = strip_escapes(first).chars().filter(|c| !c.is_control()).collect();
    Some(clean).filter(|s| !s.trim().is_empty())
}

/// `s` without ANSI escape sequences.
fn strip_escapes(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if c.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Run `work` (a turn, holding the driver) to its end. Meanwhile immediate commands from the
/// UI are answered from the session view, each on its own task so the turn and the spinner
/// keep going; any other input waits in `queue` for the turn to end (C17).
/// `text` is a command that puts another conversation on screen: `/clear`, `/resume`
/// or `/rewind`, by any of their names.
fn replaces_conversation(text: &str) -> bool {
    matches!(kodact_core::commands::builtin_of(text), Some(Builtin::Clear | Builtin::Resume | Builtin::Rewind))
}

/// The screen starts over with `replay`, the session's conversation as it is now.
fn reset_event(driver: &Driver, replay: Vec<kodact_types::Message>, echo: Option<String>) -> UiEvent {
    UiEvent::Reset { replay, recent: kodact_core::commands::recent_sessions(driver, 3), echo }
}

async fn while_busy<F: Future>(
    work: F,
    view: &SessionView,
    rx: &mut mpsc::UnboundedReceiver<ToSession>,
    ui: &mpsc::UnboundedSender<UiEvent>,
    queue: &mut VecDeque<ToSession>,
) -> F::Output {
    tokio::pin!(work);
    let mut open = true;
    loop {
        tokio::select! {
            out = &mut work => return out,
            msg = rx.recv(), if open => match msg {
                Some(ToSession::Input(text)) if kodact_core::commands::immediate(&text, &Catalog::default()) => {
                    answer_immediate(view.clone(), text, ui.clone());
                }
                Some(m) => queue.push_back(m),
                None => {
                    open = false;
                    queue.push_back(ToSession::Exit);
                }
            },
        }
    }
}

/// Answer an immediate command from the view and send the reply.
fn answer_immediate(view: SessionView, text: String, ui: mpsc::UnboundedSender<UiEvent>) {
    // A bare `/context` is its grid, mid-turn too.
    if let Some(screen) = kodact_core::commands::screens::view_screen(&view, &text) {
        let _ = ui.send(UiEvent::Screen(screen));
        return;
    }
    tokio::spawn(async move {
        let cancel = tokio_util::sync::CancellationToken::new();
        if let Some(Exec::Local { text, is_error }) =
            kodact_core::commands::execute_immediate(&view, &text, &cancel).await
        {
            let _ = ui.send(UiEvent::Reply { text, is_error });
        }
    });
}

/// Run the session until the UI says exit, `/exit` runs, or the UI is gone.
pub async fn run(mut driver: Driver, mut rx: mpsc::UnboundedReceiver<ToSession>, ui: mpsc::UnboundedSender<UiEvent>) {
    let send = ui.clone();
    let mut report = move |r: &TurnResult| {
        if let Some(ev) = reply_for(r) {
            let _ = send.send(ev);
        }
    };
    // A conversation loaded at start (--resume, --continue) is shown, not only remembered.
    let loaded = driver.engine.shown_messages();
    if !loaded.is_empty() {
        let _ = ui.send(reset_event(&driver, loaded, None));
    }
    let mut menu = commands(&driver);
    let _ = ui.send(UiEvent::Commands(menu.clone()));
    let skill_names = |d: &Driver| -> Vec<String> { d.catalog.skills.iter().map(|s| s.name.clone()).collect() };
    let mut skills = skill_names(&driver);
    let _ = ui.send(UiEvent::Skills(skills.clone()));
    let mut theme = driver.info.settings.str("/theme").map(str::to_string);
    if let Some(t) = &theme {
        let _ = ui.send(UiEvent::Theme(t.clone()));
    }
    let mut look = driver.info.settings.str("/tuiStyle").map(str::to_string);
    if let Some(l) = &look {
        let _ = ui.send(UiEvent::Look(l.clone()));
    }
    let mut had_status_command = false;
    // Changes each time a subtask finishes (C20).
    let mut finished = driver.subtasks.watch();
    let far = Duration::from_secs(365 * 86_400);
    let view = driver.view();
    // Inputs that came in during a turn, in order.
    let mut queue: VecDeque<ToSession> = VecDeque::new();
    // The endpoint's models, asked in the background so the first screen and
    // `/model` never wait on a slow gateway. It also brings the context window
    // the endpoint reports, which the context meter uses.
    let mut listing = Some(prefetch_models(&driver));
    'session: loop {
        // Subtasks that finished during the last turn are handed back before the next input.
        let _ = *finished.borrow_and_update();
        driver.deliver_subtasks();
        // What immediate commands that ended after the turn left (/btw's cost, /mcp's refresh).
        driver.sync_view();
        // /clear, /resume, /branch, /cd and the reloads (/reload-skills, and /hooks add or
        // /agents create, which reload) can change the commands; a reload keeps the session id.
        let now_menu = commands(&driver);
        if now_menu != menu {
            menu = now_menu;
            let _ = ui.send(UiEvent::Commands(menu.clone()));
        }
        let now_skills = skill_names(&driver);
        if now_skills != skills {
            skills = now_skills;
            let _ = ui.send(UiEvent::Skills(skills.clone()));
        }
        let now = driver.info.settings.str("/theme").map(str::to_string);
        if now != theme {
            theme = now;
            let _ = ui.send(UiEvent::Theme(theme.clone().unwrap_or_else(|| "auto".into())));
        }
        let now = driver.info.settings.str("/tuiStyle").map(str::to_string);
        if now != look {
            look = now;
            let _ = ui.send(UiEvent::Look(look.clone().unwrap_or_default()));
        }
        match status_command(&driver) {
            Some(cmd) => {
                had_status_command = true;
                refresh_status_line(
                    driver.engine.tool_ctx().shell.clone(),
                    cmd,
                    status_json(&driver),
                    driver.info.cwd.clone(),
                    ui.clone(),
                );
            }
            None if had_status_command => {
                had_status_command = false;
                let _ = ui.send(UiEvent::StatusLine(None));
            }
            None => {}
        }
        let _ = ui.send(UiEvent::Status(status(&driver)));
        let _ = ui.send(UiEvent::Idle);
        let wait = driver.next_wait();
        let msg = match queue.pop_front() {
            Some(m) => Some(m),
            None => loop {
                tokio::select! {
                    msg = rx.recv() => break msg,
                    // A scheduled task fires while idle; the UI shows the session busy until it ends.
                    _ = tokio::time::sleep(wait.unwrap_or(far)), if wait.is_some() => {
                        if driver.task_due() {
                            let _ = ui.send(UiEvent::Busy);
                            while_busy(driver.run_due(&mut report), &view, &mut rx, &ui, &mut queue).await;
                        }
                        continue 'session;
                    }
                    // One finished while idle: the next pass hands it back.
                    _ = finished.changed() => continue 'session,
                    // An immediate command answered after its turn left work: the next pass applies it.
                    _ = view.effect_recorded() => continue 'session,
                    // The background model listing came back: the meter takes the window it
                    // reports. Still idle, so no new pass (that would say Idle again).
                    got = async { listing.as_mut().unwrap().await }, if listing.is_some() => {
                        listing = None;
                        if let Some(why) = driver.accept_models(got.unwrap_or(None)) {
                            tracing::debug!(%why, "model list");
                        }
                        let _ = ui.send(UiEvent::Status(status(&driver)));
                    }
                }
            },
        };
        match msg {
            Some(ToSession::Picker(text)) => match ready_picker(&mut driver, &mut listing, &text).await {
                Some(p) => {
                    let _ = ui.send(UiEvent::Picker(p));
                }
                None => {
                    let _ = ui.send(UiEvent::Reply { text: "Nothing to choose from.".into(), is_error: true });
                }
            },
            Some(ToSession::Input(text)) => {
                // A command typed without its choice opens a picker instead, and one with a
                // screen (/diff, /context, /hooks, /agents) opens that.
                ready_models(&mut driver, &mut listing, &text).await;
                if let Some(p) = picker(&driver, &text) {
                    let _ = ui.send(UiEvent::Picker(p));
                } else if let Some(screen) = kodact_core::commands::screens::screen(&driver, &text) {
                    let _ = ui.send(UiEvent::Screen(screen));
                } else if let Some(events) = ui_command(&driver, &text) {
                    for e in events {
                        let _ = ui.send(e);
                    }
                } else if replaces_conversation(&text) {
                    // /clear, /resume and /rewind put another conversation on screen: the
                    // command's answer is held until the screen has started over, so it
                    // is written under the conversation it left.
                    let before = (driver.info.session_id.clone(), driver.engine.shown_messages());
                    let mut held: Vec<UiEvent> = vec![];
                    let mut hold = |r: &TurnResult| held.extend(reply_for(r));
                    let input = driver.input(MessageContent::Text(text.clone()), &mut hold);
                    let flow = while_busy(input, &view, &mut rx, &ui, &mut queue).await;
                    let failed = held.iter().any(|e| matches!(e, UiEvent::Reply { is_error: true, .. }));
                    let now = driver.engine.shown_messages();
                    let cleared = kodact_core::commands::builtin_of(&text) == Some(Builtin::Clear);
                    if !failed && (cleared || before != (driver.info.session_id.clone(), now.clone())) {
                        let _ = ui.send(reset_event(&driver, now, Some(text)));
                    }
                    for e in held {
                        let _ = ui.send(e);
                    }
                    if flow == Flow::Exit {
                        let _ = ui.send(UiEvent::Exit);
                        break;
                    }
                } else {
                    let input = driver.input(MessageContent::Text(text), &mut report);
                    if while_busy(input, &view, &mut rx, &ui, &mut queue).await == Flow::Exit {
                        let _ = ui.send(UiEvent::Exit);
                        break;
                    }
                }
            }
            Some(ToSession::Exit) | None => break,
        }
    }
    driver.shutdown("prompt_input_exit").await;
}

/// Await the MCP connections; when they take more than a moment, say which
/// servers the start is waiting for, so a slow server never looks like a hung
/// terminal. The line is cleared once they answer.
async fn connecting<T>(lo: &kodact_core::LaunchOptions, connect: impl std::future::Future<Output = T>) -> T {
    use std::io::Write;
    let names: Vec<String> = kodact_core::resolve_mcp(lo).servers.into_iter().map(|s| s.name).collect();
    tokio::pin!(connect);
    if names.is_empty() {
        return connect.await;
    }
    tokio::select! {
        done = &mut connect => return done,
        _ = tokio::time::sleep(Duration::from_millis(300)) => {}
    }
    let mut err = std::io::stderr();
    let _ = write!(err, "Connecting to MCP servers: {}…", names.join(", "));
    let _ = err.flush();
    let done = connect.await;
    let _ = write!(err, "\r\x1b[2K");
    let _ = err.flush();
    done
}

/// Build the session for the UI, as the line REPL does: launch options, MCP,
/// then a driver whose events and questions go to `ui`.
pub async fn build(
    o: &crate::args::Opts,
    ui: mpsc::UnboundedSender<UiEvent>,
) -> Result<(Driver, Vec<String>), crate::exit::Fail> {
    let mut lo = crate::launch_options(o)?;
    let (mcp, mcp_warnings) = connecting(&lo, kodact_core::connect_mcp(&lo)).await;
    lo.mcp = Some(mcp.clone());
    let sink: Arc<dyn EventSink> = Arc::new(TuiSink(ui.clone()));
    let prompter: Arc<dyn PermissionPrompter> = Arc::new(TuiPrompter(ui));
    let rebuild = (lo.clone(), sink.clone(), prompter.clone());
    let session = match kodact_core::build_session(lo, sink, prompter) {
        Ok(s) => s,
        Err(e) => {
            mcp.shutdown().await;
            return Err(crate::exit::Fail::from(e));
        }
    };
    let mut session = session;
    session.learn_window().await;
    let warnings: Vec<String> = session.warnings.iter().chain(&mcp_warnings).cloned().collect();
    let mut driver = Driver::new(session, Surface::Tui, Some(mcp));
    driver.set_rebuild(rebuild.0, rebuild.1, rebuild.2);
    Ok((driver, warnings))
}

/// The background listing of the endpoint's models.
type Listing = tokio::sync::oneshot::Receiver<Option<Result<Vec<String>, kodact_api::ApiError>>>;

/// Ask the endpoint for its models in the background.
fn prefetch_models(driver: &kodact_core::Driver) -> Listing {
    let provider = driver.engine.provider();
    let (tx, rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = tx.send(provider.list_models().await);
    });
    rx
}

/// `/model` and the agents wizard offer the endpoint's own models: only the
/// pickers need the list (`/model <name>` doesn't). The background listing is
/// awaited when it is still on its way; otherwise it is asked (once, or again
/// after a failure has cooled down).
async fn ready_models(driver: &mut kodact_core::Driver, listing: &mut Option<Listing>, text: &str) {
    let t = text.trim();
    if t != "/model" && !t.starts_with("/agents") {
        return;
    }
    let why = match listing.take() {
        Some(rx) => driver.accept_models(rx.await.unwrap_or(None)),
        None => driver.load_models().await,
    };
    if let Some(why) = why {
        tracing::debug!(%why, "model list");
    }
}

/// The picker for `text`, with the model list ready.
async fn ready_picker(
    driver: &mut kodact_core::Driver,
    listing: &mut Option<Listing>,
    text: &str,
) -> Option<kodact_core::commands::picker::Picker> {
    ready_models(driver, listing, text).await;
    picker(driver, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodact_api::{MockProvider, MockTurn};
    use kodact_config::SettingSource;
    use kodact_core::LaunchOptions;
    use kodact_types::{ContentBlock, Delta, StreamEvent};
    use serde_json::{json, Value};

    /// The command catalog row as the stream-json `initialize` response has it.
    fn row(c: &CommandInfo) -> Value {
        json!({"name": c.name, "argumentHint": c.args, "description": c.description})
    }

    struct T {
        _dir: tempfile::TempDir,
        p: Arc<MockProvider>,
        to: mpsc::UnboundedSender<ToSession>,
        ui: mpsc::UnboundedReceiver<UiEvent>,
        task: tokio::task::JoinHandle<()>,
    }

    /// A session on a mock provider, running on its own task (as `driver_with` in kodact-core's tests).
    fn start(f: impl FnOnce(&mut LaunchOptions)) -> T {
        let dir = tempfile::tempdir().unwrap();
        let proj = dir.path().join("proj");
        std::fs::create_dir_all(proj.join(".kodact")).unwrap();
        let proj = proj.canonicalize().unwrap();
        let p = Arc::new(MockProvider::new(vec![]));
        let mut o = LaunchOptions {
            cwd: proj,
            provider: Some(p.clone()),
            store_root: Some(dir.path().join("store")),
            setting_sources: Some(vec![SettingSource::Project, SettingSource::Local]),
            ..Default::default()
        };
        f(&mut o);
        let (ui_tx, ui) = mpsc::unbounded_channel();
        let sink: Arc<dyn EventSink> = Arc::new(TuiSink(ui_tx.clone()));
        let prompter: Arc<dyn PermissionPrompter> = Arc::new(TuiPrompter(ui_tx.clone()));
        let s = kodact_core::build_session(o.clone(), sink.clone(), prompter.clone()).unwrap();
        let mut d = Driver::new(s, Surface::Tui, None);
        // Never write the real user settings from a test.
        d.info.user_settings = dir.path().join("home/settings.json");
        d.set_rebuild(o, sink, prompter);
        let (to, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run(d, rx, ui_tx));
        T { _dir: dir, p, to, ui, task }
    }

    /// Events up to and including the next `Idle` (or `Exit`).
    async fn until_idle(ui: &mut mpsc::UnboundedReceiver<UiEvent>) -> Vec<UiEvent> {
        let mut out = vec![];
        loop {
            let ev =
                tokio::time::timeout(Duration::from_secs(10), ui.recv()).await.expect("timed out").expect("closed");
            let end = matches!(ev, UiEvent::Idle | UiEvent::Exit);
            out.push(ev);
            if end {
                return out;
            }
        }
    }

    fn streamed_text(evs: &[UiEvent]) -> String {
        evs.iter()
            .filter_map(|e| match e {
                UiEvent::Engine(EngineEvent::Stream {
                    event: StreamEvent::ContentBlockDelta { delta: Delta::TextDelta { text }, .. },
                    ..
                }) => Some(text.as_str()),
                _ => None,
            })
            .collect()
    }

    /// The status line named the branch the session started on for as long as it ran:
    /// after `/cd` to a folder outside git it still said `main`, and a branch switched
    /// under it never showed. Each status snapshot reads the branch where the session is.
    #[tokio::test]
    async fn the_status_names_the_branch_where_the_session_is_now() {
        let mut t = start(|_| {});
        let proj = t._dir.path().join("proj").canonicalize().unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git").args(args).current_dir(&proj).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        git(&["init", "-q", "-b", "first"]);
        git(&["-c", "user.email=t@example.com", "-c", "user.name=t", "commit", "-q", "--allow-empty", "-m", "start"]);
        let branch = |evs: &[UiEvent]| {
            evs.iter().rev().find_map(|e| match e {
                UiEvent::Status(s) => Some(s.branch.clone()),
                _ => None,
            })
        };
        until_idle(&mut t.ui).await;
        t.to.send(ToSession::Input("/status".into())).unwrap();
        assert_eq!(branch(&until_idle(&mut t.ui).await).as_deref(), Some("first"));
        git(&["checkout", "-q", "-b", "second"]);
        t.to.send(ToSession::Input("/status".into())).unwrap();
        assert_eq!(branch(&until_idle(&mut t.ui).await).as_deref(), Some("second"), "a branch switched under it");
        let outside = t._dir.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        t.to.send(ToSession::Input(format!("/cd {}", outside.display()))).unwrap();
        assert_eq!(branch(&until_idle(&mut t.ui).await).as_deref(), Some(""), "outside git there is no branch");
        t.to.send(ToSession::Exit).unwrap();
        t.task.await.unwrap();
    }

    /// `/clear` left the old conversation on screen, and `/resume` and `/rewind` never
    /// showed the one they moved to: the UI was told the answer, not that the
    /// conversation had changed.
    #[tokio::test]
    async fn clear_resume_and_rewind_start_the_screen_over() {
        let mut t = start(|_| {});
        until_idle(&mut t.ui).await;
        let said = |m: &kodact_types::Message| -> String {
            m.content.iter().filter_map(ContentBlock::as_text).collect::<Vec<_>>().join(" ")
        };
        // The Reset of a command: what it replays, and whether it came before the answer.
        type Seen = (Vec<String>, Vec<(String, String)>, bool);
        let reset = |evs: &[UiEvent]| -> Option<Seen> {
            let at = evs.iter().position(|e| matches!(e, UiEvent::Reset { .. }))?;
            let reply = evs.iter().position(|e| matches!(e, UiEvent::Reply { .. }));
            match &evs[at] {
                UiEvent::Reset { replay, recent, .. } => {
                    Some((replay.iter().map(said).collect(), recent.clone(), reply.is_some_and(|r| at < r)))
                }
                _ => None,
            }
        };
        t.p.push(MockTurn::text("first answer"));
        t.to.send(ToSession::Input("first question".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(reset(&evs).is_none(), "a turn does not start the screen over");

        // /clear: nothing to replay, and the conversation it left is the newest to resume.
        t.to.send(ToSession::Input("/clear".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        let (replay, recent, before_reply) = reset(&evs).expect("/clear starts the screen over");
        assert!(replay.is_empty(), "{replay:?}");
        assert_eq!(recent.first().map(|r| r.0.as_str()), Some("first question"));
        assert!(before_reply, "the answer is written on the new screen");
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Reset { echo: Some(c), .. } if c == "/clear")));
        // The answer names the conversation it left: `/resume <id> brings it back`.
        let first_id = evs
            .iter()
            .find_map(|e| match e {
                UiEvent::Reply { text, .. } => {
                    text.split("/resume ").nth(1)?.split_whitespace().next().map(str::to_string)
                }
                _ => None,
            })
            .expect("the id of the conversation it left");

        // Its other names do the same; a command that changes nothing does not.
        t.to.send(ToSession::Input("/new".into())).unwrap();
        assert!(reset(&until_idle(&mut t.ui).await).is_some(), "/new");
        t.to.send(ToSession::Input("/status".into())).unwrap();
        assert!(reset(&until_idle(&mut t.ui).await).is_none(), "/status");
        t.to.send(ToSession::Input("/resume no-such-conversation".into())).unwrap();
        assert!(reset(&until_idle(&mut t.ui).await).is_none(), "a /resume that failed");

        // /resume: the conversation it moves to is replayed.
        t.p.push(MockTurn::text("second answer"));
        t.to.send(ToSession::Input("second question".into())).unwrap();
        until_idle(&mut t.ui).await;
        t.to.send(ToSession::Input(format!("/resume {first_id}"))).unwrap();
        let evs = until_idle(&mut t.ui).await;
        let (replay, _, before_reply) = reset(&evs).expect("/resume starts the screen over");
        assert_eq!(replay, ["first question", "first answer"]);
        assert!(before_reply);

        // /rewind: what is left of the conversation is replayed.
        t.p.push(MockTurn::text("third answer"));
        t.to.send(ToSession::Input("third question".into())).unwrap();
        until_idle(&mut t.ui).await;
        t.to.send(ToSession::Input("/rewind 2 conversation".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        let (replay, _, _) = reset(&evs).expect("/rewind starts the screen over");
        assert_eq!(replay, ["first question", "first answer"]);

        t.to.send(ToSession::Exit).unwrap();
        t.task.await.unwrap();
    }

    #[tokio::test]
    async fn turns_commands_questions_and_exit_go_through_the_channels() {
        let mut t = start(|_| {});
        let first = until_idle(&mut t.ui).await;
        let cmds = first.iter().find_map(|e| match e {
            UiEvent::Commands(c) => Some(c.clone()),
            _ => None,
        });
        let cmds = cmds.expect("the catalog comes first");
        assert!(cmds.iter().any(|c| row(c)
            == json!({"name": "compact", "argumentHint": "[instructions]",
            "description": "Free context by summarizing the conversation so far"})));
        assert!(first.iter().any(|e| matches!(e, UiEvent::Status(s) if s.mode == "default" && !s.model.is_empty())));

        // A text turn: the text arrives as engine events, then Idle; nothing else to show.
        t.p.push(MockTurn::text("Hello there"));
        t.to.send(ToSession::Input("hi".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert_eq!(streamed_text(&evs), "Hello there");
        assert!(!evs.iter().any(|e| matches!(e, UiEvent::Reply { .. })));
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Status(s) if s.cost >= 0.0)));

        // A local command answers with a Reply.
        t.to.send(ToSession::Input("/status".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        let reply = evs.iter().find_map(|e| match e {
            UiEvent::Reply { text, is_error: false } => Some(text.clone()),
            _ => None,
        });
        assert!(reply.expect("a reply").contains("Model"), "status lists the model");
        t.to.send(ToSession::Input("/nope".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(evs
            .iter()
            .any(|e| matches!(e, UiEvent::Reply { text, is_error: true } if text.contains("Unknown command"))));

        // A permission prompt arrives as Ask; the oneshot answer reaches the tool.
        t.p.push(MockTurn::tool("Bash", json!({"command": "touch marker && echo kodact-tui-ok"})));
        t.p.push(MockTurn::text("done"));
        t.to.send(ToSession::Input("run it".into())).unwrap();
        // Queued while the turn runs: the session takes it after Idle.
        t.p.push(MockTurn::text("second answer"));
        t.to.send(ToSession::Input("and then".into())).unwrap();
        let mut asked = false;
        let mut result = None;
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(10), t.ui.recv()).await.unwrap().unwrap();
            match ev {
                UiEvent::Ask { prompt, reply } => {
                    assert_eq!(prompt.tool_name, "Bash");
                    asked = true;
                    reply.send(PermissionAnswer::Allow { updated_input: None, updated_permissions: vec![] }).unwrap();
                }
                UiEvent::Engine(EngineEvent::User { message, .. }) => {
                    for b in &message.content {
                        if let ContentBlock::ToolResult { content, .. } = b {
                            result = Some(content.to_text());
                        }
                    }
                }
                UiEvent::Idle => break,
                _ => {}
            }
        }
        assert!(asked, "the Bash call asked first");
        assert!(result.unwrap_or_default().contains("kodact-tui-ok"), "the allowed command ran");
        let evs = until_idle(&mut t.ui).await;
        assert_eq!(streamed_text(&evs), "second answer", "the queued input ran after Idle");

        // A command without its choice opens a picker; a second step is asked for by text.
        t.to.send(ToSession::Input("/model".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Picker(p) if p.choices.iter().any(|c| c.current))));
        t.to.send(ToSession::Picker("/rewind 1".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Picker(p) if p.choices.len() >= 4)));

        // The UI answers /copy and /keybindings itself.
        t.to.send(ToSession::Input("/copy".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Copy(c) if c == "second answer")));
        t.to.send(ToSession::Input("/copy 2".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Copy(c) if c == "done")));
        t.to.send(ToSession::Input("/copy 99".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Reply { is_error: true, .. })));
        t.to.send(ToSession::Input("/keybindings".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Reply { text, .. } if text.contains("Shift+Tab"))));

        // /theme is saved and applied; a status line command shows its first line.
        t.to.send(ToSession::Input("/theme light".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Theme(n) if n == "light")));
        t.to.send(ToSession::Input(
            "/statusline read x; echo \"$x\" | grep -o '\"total_cost_usd\":[0-9.]*'; echo more".into(),
        ))
        .unwrap();
        until_idle(&mut t.ui).await;
        let line = loop {
            match tokio::time::timeout(Duration::from_secs(10), t.ui.recv()).await.unwrap().unwrap() {
                UiEvent::StatusLine(l) => break l,
                _ => continue,
            }
        };
        assert!(line.unwrap_or_default().starts_with("\"total_cost_usd\":"));
        t.to.send(ToSession::Input("/statusline off".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(evs.iter().any(|e| matches!(e, UiEvent::StatusLine(None))));

        // /exit ends the session with Exit.
        t.to.send(ToSession::Input("/exit".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(matches!(evs.last(), Some(UiEvent::Exit)));
        tokio::time::timeout(Duration::from_secs(10), t.task).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn immediate_commands_answer_while_a_turn_runs() {
        let mut t = start(|_| {});
        until_idle(&mut t.ui).await;
        t.p.push(MockTurn::text("a slow answer").with_delay(Duration::from_millis(100)));
        t.p.push(MockTurn::text("the next one"));
        t.to.send(ToSession::Input("take your time".into())).unwrap();
        t.to.send(ToSession::Input("/status".into())).unwrap();
        t.to.send(ToSession::Input("then this".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        let reply = evs.iter().position(|e| matches!(e, UiEvent::Reply { text, .. } if text.contains("Model:")));
        let last_stream = evs.iter().rposition(|e| matches!(e, UiEvent::Engine(EngineEvent::Stream { .. })));
        assert!(reply.unwrap() < last_stream.unwrap(), "/status was answered while the turn streamed");
        assert_eq!(streamed_text(&evs), "a slow answer", "the plain prompt waited");
        let evs = until_idle(&mut t.ui).await;
        assert_eq!(streamed_text(&evs), "the next one");
        t.to.send(ToSession::Exit).unwrap();
        tokio::time::timeout(Duration::from_secs(10), t.task).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn bare_diff_and_context_open_screens() {
        let mut t = start(|_| {});
        until_idle(&mut t.ui).await;
        let screen = |evs: &[UiEvent]| {
            evs.iter().find_map(|e| match e {
                UiEvent::Screen(s) => Some(s.clone()),
                _ => None,
            })
        };
        t.to.send(ToSession::Input("/diff".into())).unwrap();
        let s = screen(&until_idle(&mut t.ui).await).expect("a /diff screen");
        assert!(s.title.contains("not a git repository"), "{}", s.title);
        t.to.send(ToSession::Input("/context".into())).unwrap();
        let s = screen(&until_idle(&mut t.ui).await).expect("a /context screen");
        let text = kodact_core::commands::screens::plain(&s);
        assert_eq!(s.rows.iter().take_while(|r| r.spans.len() == 10).count(), 10, "a 10x10 grid:\n{text}");
        assert!(text.contains("Context: about") && text.contains("System prompt") && text.contains("Free"), "{text}");
        // With an argument it is the text answer.
        t.to.send(ToSession::Input("/context all".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        assert!(screen(&evs).is_none());
        assert!(evs.iter().any(|e| matches!(e, UiEvent::Reply { text, .. } if text.contains("Tools:"))));
        t.to.send(ToSession::Exit).unwrap();
        tokio::time::timeout(Duration::from_secs(10), t.task).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_reload_that_adds_a_skill_updates_the_menu() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("proj")).unwrap();
        let proj = dir.path().join("proj").canonicalize().unwrap();
        let mut t = start(|o| o.cwd = proj.clone());
        until_idle(&mut t.ui).await;
        std::fs::create_dir_all(proj.join(".kodact/skills/lint")).unwrap();
        std::fs::write(proj.join(".kodact/skills/lint/SKILL.md"), "---\ndescription: Lint it\n---\nRun make lint.")
            .unwrap();
        t.to.send(ToSession::Input("/reload-skills".into())).unwrap();
        let evs = until_idle(&mut t.ui).await;
        let menu = evs.iter().find_map(|e| match e {
            UiEvent::Commands(c) => Some(c.clone()),
            _ => None,
        });
        assert!(menu.expect("a new menu").iter().any(|c| c.name == "lint"));
        t.to.send(ToSession::Exit).unwrap();
        tokio::time::timeout(Duration::from_secs(10), t.task).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_closed_ui_denies_questions_and_ends_the_session() {
        let (tx, rx) = mpsc::unbounded_channel();
        drop(rx);
        let p = TuiPrompter(tx);
        let a = p
            .ask(PermissionPrompt {
                tool_name: "Bash".into(),
                tool_use_id: "t".into(),
                input: json!({}),
                reason: String::new(),
                suggestions: vec![],
                blocked_path: None,
            })
            .await;
        assert_eq!(a, PermissionAnswer::Deny { message: "The UI closed.".into(), interrupt: true });

        let mut t = start(|_| {});
        until_idle(&mut t.ui).await;
        drop(t.to);
        tokio::time::timeout(Duration::from_secs(10), t.task).await.unwrap().unwrap();
    }

    #[test]
    fn replies_for_local_answers_failures_and_blocked_prompts() {
        let base = TurnResult {
            subtype: kodact_types::sdk::ResultSubtype::Success,
            is_error: false,
            result: Some("ok".into()),
            stop_reason: None,
            num_turns: 0,
            duration_ms: 0,
            duration_api_ms: 0,
            usage: Default::default(),
            total_cost_usd: 0.0,
            model_usage: Default::default(),
            permission_denials: vec![],
            errors: vec![],
            structured_output: None,
            prompt_blocked: None,
            fatal: false,
            auth_failed: false,
            tool_calls: 0,
            verification: None,
            finish: None,
            missing: vec![],
        };
        assert!(matches!(reply_for(&base), Some(UiEvent::Reply { text, is_error: false }) if text == "ok"));
        let model = TurnResult { num_turns: 1, stop_reason: Some("end_turn".into()), ..base.clone() };
        assert!(reply_for(&model).is_none(), "streamed already");
        let failed = TurnResult { is_error: true, errors: vec!["overloaded".into()], ..model.clone() };
        assert!(reply_for(&failed).is_none(), "shown once, as the engine's error notice");
        let stopped = TurnResult { is_error: true, stop_reason: Some("interrupted".into()), ..model.clone() };
        assert!(
            matches!(reply_for(&stopped), Some(UiEvent::Reply { text, is_error: false }) if text.starts_with("Interrupted"))
        );
        let blocked = TurnResult { prompt_blocked: Some("no secrets".into()), ..model };
        assert!(matches!(reply_for(&blocked), Some(UiEvent::Reply { is_error: true, .. })));
    }
}
