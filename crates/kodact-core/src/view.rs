//! A read-only view of the session, for commands that run while a turn is in
//! progress (contract C17, "Immediate commands": `/status`, `/usage`,
//! `/tasks`, `/context`, `/mcp`, `/btw`).
//!
//! A turn holds the [`Driver`](crate::Driver) mutably, so these commands read
//! a [`SessionView`] instead. It has two parts:
//! - the driver's ([`ViewState`]): session facts, activity, side questions,
//!   subtask rows and the shared handles (permissions, runtime, scheduler,
//!   background shells, MCP), published by the driver when idle, between the
//!   turns of one input and after a session switch;
//! - the engine's ([`kodact_engine::EngineSnapshot`]), published by the engine
//!   after each model call and tool batch and at the end of a turn, reached
//!   through the engine handle in [`ViewState`].
//!
//! Both are `Arc`s replaced whole: a reader clones the `Arc` and drops the lock
//! at once, so no guard is held for long and never across `.await`.
//!
//! What a command can't do through the view (record `/btw`'s cost and its
//! exchange, refresh what the session derives from MCP servers) is recorded
//! as an [`Effect`]. The driver applies effects when it is next free
//! ([`crate::Driver::sync_view`]): at the end of the turn, or at once when idle.

use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use kodact_api::Provider;
use kodact_config::{LoadedSettings, MemoryFile};
use kodact_engine::{EngineHandle, EngineSnapshot};
use kodact_session::Transcript;
use kodact_tools::ToolContext;
use kodact_types::sdk::InitInfo;
use kodact_types::Usage;

use crate::commands::Surface;
use crate::driver::Activity;
use crate::schedule_tools::SharedScheduler;
use crate::subtask::{FinishedSubtask, SubtaskRow};

/// The driver's part of the view, replaced whole on each publish.
pub struct ViewState {
    pub session_id: String,
    pub cwd: std::path::PathBuf,
    pub init: InitInfo,
    pub settings: LoadedSettings,
    pub warnings: Vec<String>,
    pub surface: Surface,
    /// The session can switch (`/reload-plugins` is available).
    pub can_switch: bool,
    /// The engine's controls (runtime, permissions) and its published snapshot.
    pub handle: EngineHandle,
    pub transcript: Arc<Transcript>,
    /// Working directories, the sandbox policy and background shells.
    pub tool_ctx: ToolContext,
    pub provider: Arc<dyn Provider>,
    pub hooks_disabled: bool,
    pub hook_count: usize,
    pub started: Instant,
    pub activity: Activity,
    pub side_questions: Vec<(String, String)>,
    /// Skill names and descriptions (`/context all`).
    pub skills: Vec<(String, String)>,
    pub memory: Vec<MemoryFile>,
    pub subtasks: Vec<SubtaskRow>,
    pub finished_subtasks: Vec<FinishedSubtask>,
    pub scheduler: Option<SharedScheduler>,
    pub mcp: Option<Arc<kodact_mcp::McpManager>>,
}

/// Something a command run from the view leaves for the driver to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    /// An MCP server changed: refresh its instructions and the `system/init` facts.
    RefreshMcp,
    /// A side request's spend, priced and counted by the engine.
    SideUsage { model: String, usage: Usage },
    /// A `/btw` exchange, kept as context for the next one.
    SideQuestion { question: String, answer: String },
}

struct Inner {
    state: RwLock<Arc<ViewState>>,
    effects: Mutex<Vec<Effect>>,
    recorded: tokio::sync::Notify,
}

/// The session as immediate commands see it. Cheap to clone; it stays valid
/// across session switches (the driver publishes the new session into it).
#[derive(Clone)]
pub struct SessionView(Arc<Inner>);

impl SessionView {
    pub(crate) fn new(state: ViewState) -> Self {
        SessionView(Arc::new(Inner {
            state: RwLock::new(Arc::new(state)),
            effects: Mutex::new(vec![]),
            recorded: tokio::sync::Notify::new(),
        }))
    }

    pub(crate) fn publish(&self, state: ViewState) {
        *self.0.state.write().unwrap() = Arc::new(state);
    }

    /// The driver's part, as last published.
    pub fn state(&self) -> Arc<ViewState> {
        self.0.state.read().unwrap().clone()
    }

    /// The engine's part, as last published.
    pub fn engine(&self) -> Arc<EngineSnapshot> {
        self.state().handle.snapshot()
    }

    /// Leave `e` for the driver.
    pub fn record(&self, e: Effect) {
        self.0.effects.lock().unwrap().push(e);
        self.0.recorded.notify_one();
    }

    /// `/btw` exchanges so far: those the driver has, then those still waiting for it.
    pub fn side_questions(&self) -> Vec<(String, String)> {
        let mut out = self.state().side_questions.clone();
        out.extend(self.0.effects.lock().unwrap().iter().filter_map(|e| match e {
            Effect::SideQuestion { question, answer } => Some((question.clone(), answer.clone())),
            _ => None,
        }));
        let extra = out.len().saturating_sub(crate::driver::MAX_SIDE_QUESTIONS);
        out.drain(..extra);
        out
    }

    pub(crate) fn take_effects(&self) -> Vec<Effect> {
        std::mem::take(&mut *self.0.effects.lock().unwrap())
    }

    /// A local answer's result, in the shape the driver reports one (`num_turns: 0`).
    pub fn local_result(&self, text: impl Into<String>, is_error: bool) -> kodact_engine::TurnResult {
        let snap = self.engine();
        kodact_engine::TurnResult {
            subtype: if is_error {
                kodact_types::sdk::ResultSubtype::ErrorDuringExecution
            } else {
                kodact_types::sdk::ResultSubtype::Success
            },
            is_error,
            result: Some(text.into()),
            stop_reason: None,
            num_turns: 0,
            duration_ms: 0,
            duration_api_ms: 0,
            usage: Usage::default(),
            total_cost_usd: snap.total_cost_usd,
            model_usage: snap.model_usage.clone(),
            permission_denials: vec![],
            errors: vec![],
            structured_output: None,
            prompt_blocked: None,
            fatal: false,
            auth_failed: false,
            tool_calls: 0,
            verification: None,
            missing: vec![],
        }
    }

    /// Wakes when an effect has been recorded (a front end's idle loop then calls
    /// [`crate::Driver::sync_view`]).
    pub async fn effect_recorded(&self) {
        self.0.recorded.notified().await;
    }
}
