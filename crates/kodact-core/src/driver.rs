//! The session driver: what every front end (print mode, stream-json, the line
//! REPL, the TUI) runs inputs through. It owns the engine and the command
//! catalog. A message is one of:
//! - a slash command, handled by [`crate::commands::execute`];
//! - `!command`, run in the shell (shell mode);
//! - a turn for the model.
//!
//! After a model turn, an active `/goal` is checked, and the driver keeps
//! starting turns until it is met (contract C18). Every turn's result goes
//! to the front end as it finishes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use kodact_config::LoadedSettings;
use kodact_engine::{Engine, EngineHandle, EventSink, NoticeLevel, PermissionPrompter, TurnResult};
use kodact_session::{Entry, FileHistory, LoadedSession};
use kodact_types::sdk::{InitInfo, ResultSubtype};
use kodact_types::{ContentBlock, MessageContent};

use crate::commands::{self, Catalog, Surface};
use crate::goal::{self, Goal, Status, Verdict};
use crate::{build_session, LaunchOptions, PromptSpec, Resume, Session};

/// Facts about the session for `/status`, `/doctor` and friends.
pub struct SessionInfo {
    pub session_id: String,
    pub init: InitInfo,
    pub settings: LoadedSettings,
    pub warnings: Vec<String>,
    pub cwd: PathBuf,
    /// The user settings file commands save defaults to.
    pub user_settings: PathBuf,
}

/// Activity totals across this process's inputs (for `/usage`).
#[derive(Debug, Default, Clone)]
pub struct Activity {
    pub turns: u32,
    pub api_time: Duration,
    pub prompts: u32,
}

/// The session a front end is talking to. It stays valid when the driver
/// switches sessions (`/clear`, `/resume`, `/branch`, `/cd`), so Ctrl-C and
/// SDK control requests always reach the current engine.
#[derive(Clone)]
pub struct Live(Arc<RwLock<(EngineHandle, Arc<FileHistory>, String)>>);

impl Live {
    fn new(engine: &Engine, session_id: &str) -> Self {
        Live(Arc::new(RwLock::new((engine.handle(), engine.history().clone(), session_id.to_string()))))
    }

    fn set(&self, engine: &Engine, session_id: &str) {
        *self.0.write().unwrap() = (engine.handle(), engine.history().clone(), session_id.to_string());
    }

    pub fn handle(&self) -> EngineHandle {
        self.0.read().unwrap().0.clone()
    }

    pub fn history(&self) -> Arc<FileHistory> {
        self.0.read().unwrap().1.clone()
    }

    pub fn session_id(&self) -> String {
        self.0.read().unwrap().2.clone()
    }
}

/// What the driver needs to build another session (switching sessions).
#[derive(Clone)]
pub struct Rebuild {
    pub opts: LaunchOptions,
    pub sink: Arc<dyn EventSink>,
    pub prompter: Arc<dyn PermissionPrompter>,
}

/// Where a session switch goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Switch {
    /// A new, empty session (`/clear`).
    New,
    /// A saved session (`/resume`).
    Resume(String),
    /// A copy of this conversation under a new id (`/branch`).
    Branch,
    /// A copy of this conversation in another directory (`/cd`).
    Cd(PathBuf),
    /// This session again, with commands, skills, plugins and settings re-read.
    Reload,
}

/// `/btw` exchanges kept as context for the next side question.
pub const MAX_SIDE_QUESTIONS: usize = 20;

pub struct Driver {
    pub engine: Engine,
    pub catalog: Catalog,
    pub info: SessionInfo,
    pub prompt: PromptSpec,
    pub surface: Surface,
    pub started: Instant,
    pub activity: Activity,
    /// The session's `/goal`, while one is set (and the last one, after it ends).
    pub goal: Option<Goal>,
    /// Earlier `/btw` questions and answers, oldest first.
    pub side_questions: Vec<(String, String)>,
    live: Live,
    /// What immediate commands read while a turn holds the driver (C17).
    view: crate::view::SessionView,
    rebuild: Option<Rebuild>,
    /// Scheduled prompts (`None` when KODACT_DISABLE_CRON is set).
    pub scheduler: Option<crate::schedule_tools::SharedScheduler>,
    /// The self-paced `/loop` iteration in progress, if any.
    pub(crate) self_paced: Option<SelfPaced>,
    /// The id of the pending wakeup when it is a fallback check.
    pub(crate) fallback_pending: Option<String>,
    /// The advisor model, while one is set (`/advisor`).
    pub advisor: crate::advisor::AdvisorCell,
    /// Builds sub-agents for the Task tool and `/subtask` (C20).
    pub(crate) agent_rt: Arc<kodact_agents::AgentRuntime>,
    /// Background subtasks: running, or finished and not yet handed back.
    pub subtasks: crate::subtask::Subtasks,
    /// The models the endpoint lists, once asked (`/model`); empty when it lists none.
    models: Option<Vec<String>>,
    /// When the last listing failed: it isn't asked again for a while, so a
    /// slow or broken endpoint doesn't hold up every `/model`.
    models_failed: Option<Instant>,
}

/// How long a failed model listing is not asked again.
const MODELS_RETRY: Duration = Duration::from_secs(30);

/// Book-keeping for a self-paced loop: did the iteration reschedule or stop?
#[derive(Debug, Clone)]
pub(crate) struct SelfPaced {
    /// The `/loop` input, for the fallback wakeup.
    pub input: String,
    /// The scheduler's wakeup counter when the iteration started.
    pub mark: u64,
    /// This iteration is the fallback check: if it doesn't reschedule, the loop ends.
    pub fallback: bool,
}

/// What to do after an input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Continue,
    /// `/exit`: end the session.
    Exit,
}

/// Receives each turn's result as it finishes (one input can run several
/// turns, when a goal is set). A local command's answer is a result with
/// `num_turns == 0` and no stop reason.
pub type Report<'a> = dyn FnMut(&TurnResult) + Send + 'a;

/// Longest `!command` output kept, in characters.
const SHELL_OUTPUT_CHARS: usize = 30_000;
const SHELL_TIMEOUT: Duration = Duration::from_secs(120);
/// The longest [`Driver::next_wait`].
const MAX_WAIT: Duration = Duration::from_secs(60);

impl Driver {
    pub fn new(s: Session, surface: Surface, mcp: Option<Arc<kodact_mcp::McpManager>>) -> Self {
        let cwd = s.engine.tool_ctx().project_dir.clone();
        let cost = s.engine.state.total_cost_usd;
        let goal = s.resumed.as_ref().and_then(|r| r.goal.as_ref()).and_then(|g| Goal::restore(g, cost));
        let live = Live::new(&s.engine, &s.session_id);
        let view = crate::view::SessionView::new(view_state_of(&s.engine, &s.session_id, &s.init, surface));
        let mut d = Driver {
            engine: s.engine,
            catalog: Catalog {
                commands: s.commands,
                skills: s.skills,
                styles: s.styles,
                plugins: s.plugins,
                agents: s.agents,
                mcp,
            },
            info: SessionInfo {
                session_id: s.session_id,
                init: s.init,
                settings: s.settings,
                warnings: s.warnings,
                cwd,
                user_settings: kodact_config::kodact_home().join("settings.json"),
            },
            prompt: s.prompt,
            surface,
            started: Instant::now(),
            activity: Activity::default(),
            goal,
            side_questions: vec![],
            live,
            view,
            rebuild: None,
            scheduler: s.scheduler,
            self_paced: None,
            fallback_pending: None,
            advisor: s.advisor,
            agent_rt: s.agent_rt,
            subtasks: Default::default(),
            models: None,
            models_failed: None,
        };
        d.sync_view();
        d
    }

    /// The read-only session view, for commands that run while a turn is in progress (C17).
    /// It stays valid across session switches.
    pub fn view(&self) -> crate::view::SessionView {
        self.view.clone()
    }

    /// Apply what immediate commands left for the driver ([`crate::view::Effect`]), then
    /// publish the engine's and the driver's state into the view. Call it while idle.
    pub fn sync_view(&mut self) {
        let mut mcp = false;
        for e in self.view.take_effects() {
            match e {
                crate::view::Effect::RefreshMcp => mcp = true,
                crate::view::Effect::SideUsage { model, usage } => self.engine.record_side_usage(&model, &usage),
                crate::view::Effect::SideQuestion { question, answer } => {
                    self.side_questions.push((question, answer));
                    let extra = self.side_questions.len().saturating_sub(MAX_SIDE_QUESTIONS);
                    self.side_questions.drain(..extra);
                }
            }
        }
        if mcp {
            self.refresh_mcp();
        }
        self.engine.publish();
        self.publish_view();
    }

    /// Publish the driver's part of the view (the engine publishes its own during a turn).
    fn publish_view(&self) {
        let hooks = self.engine.hooks();
        let state = crate::view::ViewState {
            session_id: self.info.session_id.clone(),
            cwd: self.info.cwd.clone(),
            init: self.info.init.clone(),
            settings: self.info.settings.clone(),
            warnings: self.info.warnings.clone(),
            surface: self.surface,
            can_switch: self.can_switch(),
            handle: self.engine.handle(),
            transcript: self.engine.transcript().clone(),
            tool_ctx: self.engine.tool_ctx().clone(),
            provider: self.engine.provider(),
            hooks_disabled: hooks.disabled,
            hook_count: hooks.config.events.values().map(|m| m.iter().map(|x| x.hooks.len()).sum::<usize>()).sum(),
            started: self.started,
            activity: self.activity.clone(),
            side_questions: self.side_questions.clone(),
            skills: self.catalog.skills.iter().map(|k| (k.name.clone(), k.description.clone())).collect(),
            memory: kodact_config::load_memory(&self.info.cwd),
            subtasks: self.subtasks.rows(),
            finished_subtasks: self.subtasks.finished().to_vec(),
            scheduler: self.scheduler.clone(),
            mcp: self.catalog.mcp.clone(),
        };
        self.view.publish(state);
    }

    pub fn handle(&self) -> EngineHandle {
        self.engine.handle()
    }

    /// Ask the endpoint which models it offers (once per session). `/model`
    /// lists only these: the provider's own catalogue, never a built-in one.
    /// Returns why the listing failed, when it did.
    pub async fn load_models(&mut self) -> Option<String> {
        if self.models.is_some() {
            return None;
        }
        if self.models_failed.is_some_and(|at| at.elapsed() < MODELS_RETRY) {
            return Some("the endpoint's model list failed a moment ago".into());
        }
        let listing = self.engine.provider().list_models().await;
        self.accept_models(listing)
    }

    /// Take a model listing made elsewhere (the UI asks in the background at
    /// startup). Returns why it failed, when it did.
    pub fn accept_models(&mut self, listing: Option<Result<Vec<String>, kodact_api::ApiError>>) -> Option<String> {
        let (list, failure) = match listing {
            None => (vec![], None),
            Some(Ok(ids)) => (ids, None),
            Some(Err(e)) => (vec![], Some(format!("could not list the endpoint's models: {}", e.describe()))),
        };
        // A failed listing is asked again, after a pause.
        if failure.is_none() {
            self.models = Some(list);
            self.models_failed = None;
        } else {
            self.models_failed = Some(Instant::now());
        }
        failure
    }

    /// The models to choose from: the endpoint's list (after [`Driver::load_models`]),
    /// with the current model first when the list leaves it out.
    pub fn model_choices(&self) -> Vec<String> {
        let current = self.engine.handle().model();
        let mut out = self.models.clone().unwrap_or_default();
        if !out.contains(&current) {
            out.insert(0, current);
        }
        out
    }

    /// The current session, for Ctrl-C handlers and SDK control requests.
    pub fn live(&self) -> Live {
        self.live.clone()
    }

    /// Let the driver build other sessions: `opts` as the session was launched.
    pub fn set_rebuild(
        &mut self,
        opts: LaunchOptions,
        sink: Arc<dyn EventSink>,
        prompter: Arc<dyn PermissionPrompter>,
    ) {
        self.rebuild = Some(Rebuild { opts, sink, prompter });
        self.publish_view();
    }

    /// May the permission mode become `bypassPermissions` now (an SDK host's
    /// `set_permission_mode`)? Only when the session was launched in it or with
    /// `--allow-dangerously-skip-permissions`, and managed settings allow it.
    pub fn bypass_allowed(&self) -> bool {
        let disabled = self
            .info
            .settings
            .managed()
            .and_then(|m| m.pointer("/permissions/disableBypassPermissionsMode"))
            .and_then(serde_json::Value::as_str)
            == Some("disable");
        let launched = self.rebuild.as_ref().is_some_and(|r| {
            r.opts.dangerously_skip_permissions
                || r.opts.allow_dangerously_skip_permissions
                || r.opts.permission_mode.as_deref() == Some("bypassPermissions")
        });
        let now = self.engine.handle().permissions.read().unwrap().mode
            == kodact_permissions::PermissionMode::BypassPermissions;
        !disabled && (launched || now)
    }

    pub fn can_switch(&self) -> bool {
        self.rebuild.is_some()
    }

    /// `--allowedTools` and `--disallowedTools` as launched.
    pub(crate) fn launch_rules(&self) -> Vec<String> {
        self.rebuild
            .as_ref()
            .map(|r| r.opts.allowed_tools.iter().chain(&r.opts.disallowed_tools).cloned().collect())
            .unwrap_or_default()
    }

    /// Where sessions are stored, when the launch options set it.
    pub(crate) fn rebuild_store(&self) -> Option<PathBuf> {
        self.rebuild.as_ref().and_then(|r| r.opts.store_root.clone())
    }

    /// The conversation in memory, as a loaded session.
    pub fn snapshot(&self) -> LoadedSession {
        let st = &self.engine.state;
        let t = self.engine.transcript();
        LoadedSession {
            session_id: self.info.session_id.clone(),
            cwd: self.info.cwd.clone(),
            messages: st
                .uuids
                .iter()
                .zip(&st.messages)
                .map(|(u, m)| Entry { uuid: u.clone(), message: m.clone(), is_meta: st.meta.contains(u) })
                .collect(),
            // Directories added by /add-dir and by permission answers alike.
            additional_dirs: self.engine.tool_ctx().working_dirs.read().unwrap().iter().skip(1).cloned().collect(),
            microcompacted: st.microcompacted.clone(),
            title: t.title(),
            last_uuid: t.last_uuid(),
            permission_mode: Some(self.engine.handle().permissions.read().unwrap().mode.as_str().to_string()),
            model: Some(self.engine.handle().model()),
            worktree: None,
            todos: Some(self.engine.tool_ctx().todos.lock().unwrap().clone()).filter(|t| !t.is_empty()),
            requests: Some(self.engine.carried_requests().to_vec()).filter(|r| !r.is_empty()),
            goal: self.goal.as_ref().map(Goal::record),
            schedule: self.scheduler.as_ref().map(|s| s.lock().unwrap().record()),
            cut_off: false,
        }
    }

    /// Replace the session: a new one, a saved one, a branch of this one, or
    /// this one rebuilt. MCP connections, the running model and mode, the cost
    /// so far and the front end's [`Live`] carry over.
    pub async fn switch(&mut self, to: Switch) -> Result<(), String> {
        let rb = self.rebuild.clone().ok_or("this front end can't switch sessions")?;
        let mut opts = rb.opts.clone();
        opts.cwd = self.info.cwd.clone();
        opts.worktree = None;
        opts.mcp = self.catalog.mcp.clone();
        opts.session_id = None;
        opts.fork_session = false;
        let rt = self.engine.handle().runtime();
        opts.model = Some(rt.model.clone());
        opts.effort = rt.effort.clone();
        let mode = self.engine.handle().permissions.read().unwrap().mode;
        opts.permission_mode = Some(mode.as_str().to_string());
        let (source, ends) = match &to {
            Switch::New => {
                opts.resume = Resume::New;
                ("clear", Some("clear"))
            }
            Switch::Resume(id) => {
                opts.resume = Resume::Id(id.clone());
                // A session from another directory (a worktree) resumes in its own directory.
                let store = kodact_session::SessionStore::new(
                    opts.store_root.clone().unwrap_or_else(|| kodact_session::kodact_home().join("projects")),
                );
                if let Some(dir) = store
                    .find(id)
                    .ok()
                    .and_then(|p| LoadedSession::load(&p, None).ok())
                    .map(|l| l.cwd)
                    .filter(|d| d.is_dir())
                {
                    opts.cwd = dir;
                }
                ("resume", Some("resume"))
            }
            Switch::Branch => {
                opts.resume = Resume::Loaded(Box::new(self.snapshot()));
                opts.fork_session = true;
                ("resume", Some("other"))
            }
            Switch::Cd(dir) => {
                opts.resume = Resume::Loaded(Box::new(self.snapshot()));
                opts.fork_session = true;
                opts.cwd = dir.clone();
                ("resume", Some("other"))
            }
            Switch::Reload => {
                opts.resume = Resume::Loaded(Box::new(self.snapshot()));
                // Nothing ends: background shells keep running in the rebuilt session.
                opts.shells = Some(self.engine.tool_ctx().shells.clone());
                ("resume", None)
            }
        };
        let continues = matches!(to, Switch::Branch | Switch::Cd(_) | Switch::Reload);
        let s = build_session(opts, rb.sink.clone(), rb.prompter.clone()).map_err(|e| e.to_string())?;
        if let Some(reason) = ends {
            self.engine.end_session(reason).await;
        }
        let mut next = Driver::new(s, self.surface, self.catalog.mcp.clone());
        next.engine.set_start_source(source);
        // Carry over what the person set in this process.
        let h = next.engine.handle();
        h.set_fast(rt.fast);
        h.set_max_thinking_tokens(rt.max_thinking_tokens);
        next.engine.tool_ctx().set_sandbox(self.engine.tool_ctx().sandbox_policy().map(|p| (*p).clone()));
        {
            let old = self.engine.handle().permissions.read().unwrap().added.clone();
            let mut perm = h.permissions.write().unwrap();
            for (b, list) in [
                (kodact_permissions::Behavior::Allow, old.allow),
                (kodact_permissions::Behavior::Ask, old.ask),
                (kodact_permissions::Behavior::Deny, old.deny),
            ] {
                for rule in list {
                    perm.add_rule(b, rule);
                }
            }
        }
        if continues {
            // The same conversation goes on: keep the goal, scheduled tasks and loop state
            // as they are in memory (restoring from records would reset them).
            next.goal = self.goal.clone();
            if let (Some(old), Some(new)) = (&self.scheduler, &next.scheduler) {
                let tasks = old.lock().unwrap().clone();
                *new.lock().unwrap() = tasks;
            }
            next.self_paced = self.self_paced.take();
            next.fallback_pending = self.fallback_pending.take();
            next.side_questions = std::mem::take(&mut self.side_questions);
            if let Some(g) = &next.goal {
                next.engine.transcript().set_goal(g.record());
            }
            next.save_schedule();
        }
        if matches!(to, Switch::Reload) {
            next.engine.skip_session_start();
        }
        if matches!(to, Switch::Cd(_)) {
            // The new directory's KODACT.md and friends reach the model with the next prompt.
            next.engine.reattach_context();
        }
        let old = &self.engine.state;
        let st = &mut next.engine.state;
        st.total_cost_usd += old.total_cost_usd;
        st.total_usage.add(&old.total_usage);
        for (model, usage) in &old.model_usage {
            let e = st.model_usage.entry(model.clone()).or_insert_with(|| serde_json::json!({}));
            for (k, v) in usage.as_object().into_iter().flatten() {
                let sum = e.get(k).and_then(serde_json::Value::as_f64).unwrap_or(0.0) + v.as_f64().unwrap_or(0.0);
                e[k] = if v.is_u64() { serde_json::json!(sum as u64) } else { serde_json::json!(sum) };
            }
        }
        if next.info.cwd == self.info.cwd {
            next.prompt.output_style = self.prompt.output_style.clone();
            next.info.init.output_style = self.info.init.output_style.clone();
        }
        *next.advisor.write().unwrap() = self.advisor.read().unwrap().clone();
        next.prompt.replace = self.prompt.replace.clone();
        next.prompt.append = self.prompt.append.clone();
        next.rebuild_system();
        next.info.user_settings = self.info.user_settings.clone();
        next.activity = self.activity.clone();
        next.started = self.started;
        // Subtasks follow the conversation (C20). A branch, a move or a reload continues it: they
        // carry on and report there, and notes still due for the next prompt come along. A new or
        // resumed conversation is another one: they stop, and their reports aren't passed on.
        // Move the registry, never rebuild it: front ends hold receivers of its watch channel.
        if continues {
            for note in self.engine.take_reminders() {
                next.engine.remind(note);
            }
        } else {
            self.subtasks.orphan_all();
        }
        next.subtasks = std::mem::take(&mut self.subtasks);
        next.rebuild = self.rebuild.take();
        next.live = self.live.clone();
        next.live.set(&next.engine, &next.info.session_id);
        // The view too: front ends hold it. Effects still waiting apply to the new session.
        next.view = self.view.clone();
        *self = next;
        self.sync_view();
        Ok(())
    }

    /// Run one input. `report` gets each turn's result as it finishes.
    pub async fn input(&mut self, content: MessageContent, report: &mut Report<'_>) -> Flow {
        // Subtasks that finished since the last input are handed back first: their reports ride along with it.
        self.deliver_subtasks();
        self.sync_view();
        // Shell mode is for a person at the keyboard: on print and stream surfaces the
        // prompt usually comes from a program, so `!...` is an ordinary prompt there.
        let shell = shell_command(&content).filter(|_| matches!(self.surface, Surface::Repl | Surface::Tui));
        let (result, engine_turn) = if let Some(cmd) = shell {
            self.shell(&cmd).await
        } else {
            match commands::command_text(&content) {
                None => (self.submit_attached(content).await, true),
                Some(text) => match commands::execute(self, &text).await {
                    // A loop's prompt attaches its @ mentions now, as each scheduled run will.
                    commands::Exec::Submit(prompt) if text.trim_start().starts_with("/loop") => {
                        (self.submit_attached(prompt).await, true)
                    }
                    commands::Exec::Submit(prompt) => (self.submit(prompt).await, true),
                    commands::Exec::Local { text, is_error } => (self.engine.local_result(text, is_error), false),
                    commands::Exec::Exit => return Flow::Exit,
                },
            }
        };
        self.record(&result, true);
        report(&result);
        if result.num_turns > 0 {
            // A prompt that reached the model resumes a paused goal.
            if let Some(g) = self.goal.as_mut().filter(|g| g.is_active()) {
                g.paused = None;
                g.idle = 0;
            }
        }
        self.after_turn(result, engine_turn, report).await;
        self.sync_view();
        Flow::Continue
    }

    /// Submit a prompt with its `@path` mentions attached. The model sees those files, so it may
    /// Edit them without a Read; that is undone, for files not read before, if a hook erases
    /// the prompt.
    async fn submit_attached(&mut self, content: MessageContent) -> kodact_engine::TurnResult {
        let (content, read) = self.attach_mentions(content);
        let files = self.engine.tool_ctx().files.clone();
        let fresh: Vec<PathBuf> = read.into_iter().filter(|p| files.check_writable(p).is_err()).collect();
        for p in &fresh {
            files.record_read(p);
        }
        let result = self.submit(content).await;
        if result.prompt_blocked.is_some() {
            for p in &fresh {
                files.forget(p);
            }
        }
        result
    }

    /// A prompt's `@path` mentions, attached (docs/CLI.md, "`@` mentions"): the text stays as
    /// typed, and the contents follow in a system reminder of their own. Only a message that is
    /// a single piece of text is scanned. Also returns the files attached whole or in part.
    fn attach_mentions(&self, content: MessageContent) -> (MessageContent, Vec<PathBuf>) {
        use kodact_agents::attach;
        use kodact_permissions::{Decision, Reason, Request, Subject};
        let Some(text) = commands::command_text_any(&content) else { return (content, vec![]) };
        // An attachment is a read: the Read tool's decision on the path, without prompting.
        let perm = self.engine.handle().permissions.read().unwrap().clone();
        let allowed = |path: &Path| -> Result<(), String> {
            let req = Request::new("Read", Subject::Path { path: path.to_path_buf(), write: false }, true);
            match perm.decide(&req) {
                Decision::Allow { .. } => Ok(()),
                Decision::Ask { reason: Reason::OutsideWorkingDirs(_), .. }
                | Decision::Deny { reason: Reason::OutsideWorkingDirs(_) } => {
                    Err("outside the working directories".into())
                }
                Decision::Deny { .. } => Err("blocked by a permission rule".into()),
                Decision::Ask { .. } => Err("reading it needs permission".into()),
            }
        };
        let atts = attach::at_mentions(&text, &self.info.cwd, &allowed);
        let Some(note) = attach::reminder(&atts) else { return (content, vec![]) };
        let read = atts.into_iter().filter(|a| a.kind == attach::Kind::File).map(|a| a.path).collect();
        (MessageContent::Blocks(vec![ContentBlock::text(text), ContentBlock::text(note)]), read)
    }

    /// After a turn: check an active goal, then settle a self-paced loop iteration.
    async fn after_turn(&mut self, result: TurnResult, engine_turn: bool, report: &mut Report<'_>) {
        let mut interrupted = result.stop_reason.as_deref() == Some("interrupted");
        // A turn that failed or was blocked before its first model call still counts for the goal.
        if engine_turn && (result.num_turns > 0 || result.is_error || result.prompt_blocked.is_some() || interrupted) {
            interrupted |= self.pursue_goal(result, report).await;
        }
        if self.self_paced.is_some() {
            self.after_loop_iteration(interrupted);
        }
    }

    /// Real time until the next scheduled task is due.
    ///
    /// Capped at a minute: a timer stops while the machine sleeps, so long
    /// waits are re-checked against the wall clock.
    pub fn next_wait(&self) -> Option<Duration> {
        self.scheduler.as_ref().and_then(|s| s.lock().unwrap().next_wait()).map(|w| w.min(MAX_WAIT))
    }

    /// Is a scheduled task due now?
    pub fn task_due(&self) -> bool {
        self.scheduler.as_ref().and_then(|s| s.lock().unwrap().next_wait()) == Some(Duration::ZERO)
    }

    /// Are scheduled tasks waiting, or subtasks out? (`-p` keeps running while they are.)
    pub fn has_pending(&self) -> bool {
        self.scheduler.as_ref().is_some_and(|s| !s.lock().unwrap().tasks.is_empty()) || !self.subtasks.is_empty()
    }

    pub(crate) fn save_schedule(&self) {
        if let Some(s) = &self.scheduler {
            self.engine.transcript().set_schedule(s.lock().unwrap().record());
        }
    }

    /// Run the next due scheduled task, if any, as a turn. Call it only while idle.
    pub async fn run_due(&mut self, report: &mut Report<'_>) -> bool {
        let Some(task) = self.scheduler.as_ref().and_then(|s| s.lock().unwrap().take_due()) else { return false };
        self.deliver_subtasks();
        self.save_schedule();
        self.engine.announce(
            "scheduled_task",
            serde_json::json!({"id": task.id, "prompt": task.prompt, "schedule": task.describe()}),
        );
        self.notice(NoticeLevel::Info, format!("Running scheduled task {}: {}", task.id, task.prompt));
        let is_fallback = self.fallback_pending.as_deref() == Some(task.id.as_str());
        if is_fallback {
            self.fallback_pending = None;
        }
        let (result, engine_turn) = if task.prompt.trim_start().starts_with("/loop") {
            match commands::execute(self, task.prompt.trim()).await {
                commands::Exec::Submit(p) => (self.submit(p).await, true),
                commands::Exec::Local { text, is_error } => (self.engine.local_result(text, is_error), false),
                commands::Exec::Exit => return true,
            }
        } else {
            let prompt = commands::scheduled_prompt(self, &task.prompt).await;
            // A plain scheduled prompt attaches its @ mentions, as typed prompts do; a custom
            // command attached its own already.
            if task.prompt.trim_start().starts_with('/') {
                (self.submit(prompt).await, true)
            } else {
                (self.submit_attached(prompt).await, true)
            }
        };
        if let Some(it) = self.self_paced.as_mut() {
            it.fallback = is_fallback;
        }
        self.record(&result, false);
        report(&result);
        self.after_turn(result, engine_turn, report).await;
        self.sync_view();
        true
    }

    /// `/subtask <task>`: fork this conversation into a background sub-agent (C20). Call only
    /// while idle. Returns its id.
    pub(crate) fn start_subtask(&mut self, task: &str) -> Result<String, String> {
        use crate::subtask::{self, SubtaskPrompter, SubtaskSink};
        if self.subtasks.running() >= subtask::MAX_RUNNING {
            return Err(format!(
                "{} subtasks are already running. Stop one with /tasks stop <id>, or wait for one to finish.",
                subtask::MAX_RUNNING
            ));
        }
        let spent = self.engine.state.total_cost_usd;
        let budget = match self.engine.cfg.max_budget_usd {
            Some(b) if spent >= b => return Err("the session's budget is used up.".into()),
            b => b.map(|b| b - spent),
        };
        // The main conversation's settings as they are now, so the fork's requests match its prefix.
        let rt = self.engine.handle().runtime();
        let mut cfg = self.engine.cfg.clone();
        cfg.model = rt.model;
        cfg.effort = rt.effort;
        cfg.max_thinking_tokens = rt.max_thinking_tokens;
        cfg.fast = rt.fast;
        cfg.max_budget_usd = budget;
        cfg.json_schema = None;
        cfg.verify = None;
        let id = self.subtasks.next_id();
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let child = self.agent_rt.child(kodact_agents::ChildSpec {
            cfg,
            tools: kodact_agents::fork_tools(self.engine.tools()),
            system: self.engine.system().to_vec(),
            prompter: Arc::new(SubtaskPrompter),
            sink: Arc::new(SubtaskSink::new(&id, self.agent_rt.sink.clone(), calls.clone())),
            seed: Some(&self.engine.state),
            checkpoint_turn: Some(format!("{id}-{}", self.info.session_id)),
            dir: None,
        })?;
        child.engine.transcript().append_meta(serde_json::json!({
            "forkOf": {"sessionId": self.info.session_id, "leafUuid": self.engine.transcript().last_uuid()},
        }));
        // Its edits get a checkpoint turn of their own, in order after the prompts so far: a
        // rewind to an earlier prompt undoes them, one to a later prompt doesn't.
        self.engine.history().add_turn(&format!("{id}-{}", self.info.session_id));
        self.subtasks.spawn(id.clone(), task, child.engine, subtask::prompt(task), calls);
        self.engine.remind(subtask::started_note(&id, task));
        self.engine.announce("subtask", serde_json::json!({"id": id, "status": "started", "task": task}));
        Ok(id)
    }

    /// Hand back finished subtasks (C20): each one's spend joins the session's, the person gets a
    /// notice and a host `system/subtask`, and its report rides along with the next prompt.
    /// Returns how many reports the model will see. Call only while idle.
    pub fn deliver_subtasks(&mut self) -> usize {
        let mut reports = 0;
        for (t, o) in self.subtasks.take_finished() {
            self.engine.record_subagent_usage(&o.usage);
            let cost = o.usage.get("costUsd").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
            let mut text = format!(
                "Subtask {} {} ({} tool calls, {}): {}\n{}",
                t.id,
                o.how(),
                t.tool_calls(),
                commands::duration(o.duration),
                t.task,
                o.report
            );
            if t.orphaned {
                text.push_str("\n(The conversation it was forked from was closed, so this report isn't passed on.)");
            }
            let level = if o.status == "completed" || t.orphaned { NoticeLevel::Info } else { NoticeLevel::Warning };
            self.notice(level, text);
            self.engine.announce(
                "subtask",
                serde_json::json!({
                    "id": t.id, "status": o.status, "task": t.task, "result": o.report,
                    "num_tool_calls": t.tool_calls(), "duration_ms": o.duration.as_millis() as u64,
                    "total_cost_usd": cost, "permission_denials": o.denials,
                    "transcript_path": t.transcript, "handed_back": !t.orphaned,
                }),
            );
            if !t.orphaned {
                self.engine.remind(crate::subtask::report_note(&t.id, &t.task, &o));
                reports += 1;
            }
        }
        reports
    }

    /// After the input ended (`-p`): hand back finished subtasks and run one more turn, so the
    /// model uses their reports. False when there was nothing to hand back.
    pub async fn continue_with_subtasks(&mut self, report: &mut Report<'_>) -> bool {
        if self.deliver_subtasks() == 0 {
            return false;
        }
        let result = self.submit(MessageContent::Text(crate::subtask::CONTINUE_PROMPT.into())).await;
        self.record(&result, false);
        report(&result);
        self.after_turn(result, true, report).await;
        true
    }

    /// After a self-paced iteration: it continues if the model called
    /// ScheduleWakeup. If it did neither that nor stop, one fallback check
    /// comes 20 minutes later; a second miss ends the loop.
    fn after_loop_iteration(&mut self, interrupted: bool) {
        let (Some(it), Some(sched)) = (self.self_paced.take(), self.scheduler.clone()) else { return };
        let mut s = sched.lock().unwrap();
        // Ctrl-C ends a self-paced loop.
        if interrupted {
            s.stop_wakeups();
            drop(s);
            self.save_schedule();
            self.notice(NoticeLevel::Info, "The loop is stopped.");
            return;
        }
        if s.wakeups != it.mark {
            if s.stopped {
                drop(s);
                self.notice(NoticeLevel::Info, "The loop is finished.");
            }
            return;
        }
        if it.fallback {
            drop(s);
            self.notice(
                NoticeLevel::Warning,
                "The loop ended: two iterations in a row neither rescheduled nor stopped.",
            );
            return;
        }
        let secs = crate::schedule::FALLBACK_WAKEUP.as_secs();
        let input = format!("/loop {}", it.input);
        let fallback = s.wakeup(secs, &input, "fallback check: the last iteration didn't reschedule").ok();
        drop(s);
        if let Some(task) = fallback {
            self.save_schedule();
            self.fallback_pending = Some(task.id);
        }
    }

    fn record(&mut self, r: &TurnResult, prompt: bool) {
        if r.num_turns > 0 {
            self.activity.prompts += u32::from(prompt);
            self.activity.turns += r.num_turns;
            self.activity.api_time += Duration::from_millis(r.duration_api_ms);
        }
        // Between the turns of one input (a goal), immediate commands see the totals so far.
        self.publish_view();
    }

    fn notice(&self, level: NoticeLevel, text: impl Into<String>) {
        self.engine.emit(kodact_engine::EngineEvent::Notice { level, text: text.into() });
    }

    /// Record the goal and tell the host (`system/goal`).
    pub(crate) fn goal_changed(&self) {
        if let Some(g) = &self.goal {
            let rec = g.record();
            self.engine.transcript().set_goal(rec.clone());
            self.engine.announce("goal", rec);
        }
    }

    /// Keep working on an active goal: check each finished turn, and start
    /// another while the goal isn't met. True when a turn it ran, or the
    /// check, was interrupted.
    async fn pursue_goal(&mut self, mut last: TurnResult, report: &mut Report<'_>) -> bool {
        let max_turns = self.engine.cfg.max_turns;
        let mut used = last.num_turns;
        let mut ran_any = false;
        loop {
            let Some(g) = self.goal.as_ref() else { return false };
            if !g.is_active() || g.paused.is_some() {
                return false;
            }
            let condition = g.condition.clone();
            // Errors and interrupts stop the loop; some end the goal.
            if last.stop_reason.as_deref() == Some("interrupted") {
                self.pause("interrupted", "Goal paused: the turn was interrupted. Send a message to continue.");
                return ran_any;
            }
            if last.prompt_blocked.is_some() {
                self.pause("prompt blocked", "Goal paused: a hook blocked the prompt.");
                return false;
            }
            if last.stop_reason.as_deref() == Some("hook_stopped") {
                self.pause("hook stopped", "Goal paused: a hook stopped the run. Send a message to continue.");
                return false;
            }
            if last.is_error {
                let why = last.errors.first().cloned().or_else(|| last.result.clone()).unwrap_or_default();
                if last.fatal {
                    if let Some(g) = self.goal.as_mut() {
                        g.status = Status::Cleared;
                        g.last_reason = Some(why.clone());
                    }
                    self.goal_changed();
                    self.notice(
                        NoticeLevel::Error,
                        format!("Goal cleared after an error that won't go away by itself ({why}). Run /goal again to continue."),
                    );
                } else if matches!(last.subtype, ResultSubtype::ErrorMaxTurns | ResultSubtype::ErrorMaxBudgetUsd) {
                    self.pause("limit", "Goal paused: a turn or budget limit was reached.");
                } else {
                    self.pause("error", format!("Goal paused after an error: {why}. Send a message to continue."));
                }
                return false;
            }
            if let Some(g) = self.goal.as_mut() {
                g.idle = if last.tool_calls == 0 { g.idle + 1 } else { 0 };
            }

            // The turn that just ended is not interrupted, so its token is still
            // the one Ctrl-C and a host's `interrupt` cancel until the next turn starts.
            let cancel = self.engine.handle().turn_token();
            let transcript = goal::evaluator_transcript(&self.engine.state.messages);
            let user = format!("Goal: {condition}\n\nTranscript:\n{transcript}");
            let model = commands::goal_model(self);
            let answer = commands::side_request_on(self, &model, goal::EVALUATOR_PROMPT, user, 800, &cancel).await;
            if cancel.is_cancelled() {
                self.pause("interrupted", "Goal paused: interrupted. Send a message to continue.");
                return true;
            }
            let (verdict, reason) = match answer {
                Ok(text) => goal::parse_verdict(&text),
                Err(e) => {
                    self.pause("check failed", format!("Goal paused: the goal check failed ({e})."));
                    return false;
                }
            };
            let g = self.goal.as_mut().expect("goal checked above");
            g.checks += 1;
            g.last_reason = Some(reason.clone());
            match verdict {
                Verdict::Met => {
                    g.status = Status::Achieved;
                    self.goal_changed();
                    self.notice(NoticeLevel::Info, format!("Goal achieved: {reason}"));
                    return false;
                }
                Verdict::Impossible => {
                    g.status = Status::Failed(reason.clone());
                    self.goal_changed();
                    self.notice(NoticeLevel::Warning, format!("Goal can't be met: {reason}"));
                    return false;
                }
                Verdict::NotMet => {
                    if g.idle >= goal::MAX_IDLE_TURNS {
                        self.pause(
                            "no progress",
                            format!(
                                "Goal paused: {} turns in a row without using a tool. The goal stays set; send a \
                                 message to continue.",
                                goal::MAX_IDLE_TURNS
                            ),
                        );
                        return false;
                    }
                    if max_turns.is_some_and(|m| used >= m) {
                        self.pause("limit", "Goal paused: --max-turns reached.");
                        return false;
                    }
                    if let Some(budget) = self.engine.cfg.max_budget_usd {
                        if self.engine.state.total_cost_usd >= budget {
                            self.pause("limit", "Goal paused: --max-budget-usd reached.");
                            return false;
                        }
                    }
                    self.engine.announce(
                        "goal",
                        serde_json::json!({"condition": condition, "status": "active", "reason": reason}),
                    );
                    // An interrupt between the check and the next turn would be lost
                    // when `submit` starts a new turn token.
                    if cancel.is_cancelled() {
                        self.pause("interrupted", "Goal paused: interrupted. Send a message to continue.");
                        return true;
                    }
                    // --max-turns counts across the whole goal loop, not per turn.
                    self.engine.cfg.max_turns = max_turns.map(|m| m - used);
                    let next = self.submit(MessageContent::Text(goal::continue_prompt(&condition, &reason))).await;
                    self.engine.cfg.max_turns = max_turns;
                    ran_any = true;
                    used += next.num_turns;
                    self.record(&next, false);
                    report(&next);
                    last = next;
                }
            }
        }
    }

    fn pause(&mut self, why: &str, text: impl Into<String>) {
        let text = text.into();
        if let Some(g) = self.goal.as_mut() {
            g.paused = Some(why.to_string());
            // Hosts follow the goal through `system/goal`: say it stopped.
            self.engine.announce(
                "goal",
                serde_json::json!({"condition": g.condition, "status": "active", "paused": why, "reason": text}),
            );
        }
        self.notice(NoticeLevel::Warning, text);
    }

    /// `!command`: run it in the shell, as the user, and put the command and
    /// its output in the conversation. The model answers it unless
    /// `respondToBashCommands` is false. The bool says whether a model turn ran.
    async fn shell(&mut self, cmd: &str) -> (TurnResult, bool) {
        let ctx = self.engine.tool_ctx().clone();
        let cancel = self.engine.handle().new_token();
        let run = kodact_tools::shells::run_command(
            &ctx.shell,
            cmd,
            &ctx.shell_cwd(),
            &ctx.env,
            SHELL_TIMEOUT,
            &cancel,
            None,
        )
        .await;
        let (code, output) = match run {
            Ok(r) if r.interrupted => return (self.engine.local_result("Interrupted.", true), false),
            Ok(r) => {
                let mut text = r.stdout;
                if !r.stderr.trim().is_empty() {
                    if !text.is_empty() && !text.ends_with('\n') {
                        text.push('\n');
                    }
                    text.push_str(&r.stderr);
                }
                if r.timed_out {
                    text.push_str(&format!("\n(timed out after {}s)", SHELL_TIMEOUT.as_secs()));
                }
                (r.code.unwrap_or(-1), text)
            }
            Err(e) => (-1, e.to_string()),
        };
        let output = kodact_tools::truncate_middle(output.trim_end(), SHELL_OUTPUT_CHARS);
        // The output can't close its wrapper early and pass as the user's own words.
        let safe = |t: &str| {
            t.replace("</shell-output>", "<\\/shell-output>").replace("</shell-command>", "<\\/shell-command>")
        };
        let note = format!(
            "The user ran a shell command:\n<shell-command>{}</shell-command>\n<shell-output exit-code=\"{code}\">\n{}\n</shell-output>",
            safe(cmd),
            safe(&output)
        );
        if self.info.settings.bool("/respondToBashCommands") == Some(false) {
            self.engine.add_user_note(note);
            return (self.engine.local_result(output, code != 0), false);
        }
        if !output.is_empty() {
            self.notice(NoticeLevel::Info, format!("$ {cmd}\n{output}"));
        }
        (self.submit(MessageContent::Text(note)).await, true)
    }

    /// Rebuild the system prompt from [`Driver::prompt`] after changing it.
    /// Start a model turn. A model set from outside the driver (an SDK host's
    /// `set_model`) first reaches the system prompt, which names the model.
    async fn submit(&mut self, content: MessageContent) -> kodact_engine::TurnResult {
        let model = self.engine.handle().model();
        if self.prompt.env.model != model {
            self.prompt.set_model(&model);
            self.rebuild_system();
            self.info.init.model = model;
        }
        self.engine.submit(content).await
    }

    pub fn rebuild_system(&mut self) {
        self.engine.set_system(self.prompt.build().0);
    }

    /// After `/mcp` (or an `mcp_toggle` / `mcp_reconnect` control request)
    /// changed a server: its instructions leave or rejoin the system prompt,
    /// and the `system/init` facts follow the servers.
    pub fn refresh_mcp(&mut self) {
        let Some(m) = self.catalog.mcp.clone() else { return };
        let instructions = m.instructions();
        if instructions != self.prompt.mcp_instructions {
            self.prompt.mcp_instructions = instructions;
            self.rebuild_system();
        }
        self.info.init.mcp_servers =
            m.status().into_iter().map(|(name, status)| kodact_types::sdk::McpServerStatus { name, status }).collect();
        self.info.init.tools = self.engine.tools().names();
    }

    /// An SDK host's `initialize` `systemPrompt` / `appendSystemPrompt`.
    pub fn set_system_prompt(&mut self, replace: Option<String>, append: Option<String>) {
        if replace.is_some() {
            self.prompt.replace = replace;
        }
        if append.is_some() {
            self.prompt.append = append;
        }
        self.rebuild_system();
    }

    pub async fn shutdown(&self, reason: &str) {
        let running = self.subtasks.running();
        if running > 0 {
            self.notice(NoticeLevel::Info, format!("Stopping {running} running subtask(s)."));
        }
        // Before the session ends: they use its MCP connections.
        self.subtasks.shutdown(crate::subtask::SHUTDOWN_GRACE).await;
        self.engine.end_session(reason).await;
        if let Some(m) = &self.catalog.mcp {
            m.shutdown().await;
        }
    }
}

/// The command in a `!command` message.
fn shell_command(content: &MessageContent) -> Option<String> {
    let text = commands::command_text_any(content)?;
    let cmd = text.trim_start().strip_prefix('!')?.trim();
    (!cmd.is_empty()).then(|| cmd.to_string())
}

/// The first view, before the driver exists: it is published again at once.
fn view_state_of(engine: &Engine, session_id: &str, init: &InitInfo, surface: Surface) -> crate::view::ViewState {
    crate::view::ViewState {
        session_id: session_id.to_string(),
        cwd: engine.tool_ctx().project_dir.clone(),
        init: init.clone(),
        settings: LoadedSettings::default(),
        warnings: vec![],
        surface,
        can_switch: false,
        handle: engine.handle(),
        transcript: engine.transcript().clone(),
        tool_ctx: engine.tool_ctx().clone(),
        provider: engine.provider(),
        hooks_disabled: false,
        hook_count: 0,
        started: Instant::now(),
        activity: Activity::default(),
        side_questions: vec![],
        skills: vec![],
        memory: vec![],
        subtasks: vec![],
        finished_subtasks: vec![],
        scheduler: None,
        mcp: None,
    }
}
