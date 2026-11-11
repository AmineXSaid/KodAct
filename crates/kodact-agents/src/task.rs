use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Instant;

use kodact_api::Provider;
use kodact_engine::{
    Engine, EngineConfig, EngineHandle, EngineParts, EnvInfo, EventSink, ForwardSink, PermissionPrompter, TurnResult,
    TurnState,
};
use kodact_hooks::HookRunner;
use kodact_permissions::Subject;
use kodact_session::{FileHistory, SessionStore, Transcript};
use kodact_tools::{Tool, ToolContext, ToolOutput, ToolRegistry};
use kodact_types::{ContentBlock, MessageContent};
use serde_json::{json, Value};

use crate::definitions::AgentDef;

/// What a sub-agent inherits from the session that runs it (set once the main engine exists).
pub struct ParentLink {
    pub handle: EngineHandle,
    pub prompter: Arc<dyn PermissionPrompter>,
    pub history: Arc<FileHistory>,
}

/// Everything the Task tool needs to start sub-agents.
pub struct AgentRuntime {
    pub provider: Arc<dyn Provider>,
    pub agents: Vec<AgentDef>,
    pub project_dir: PathBuf,
    pub working_dirs: Arc<RwLock<Vec<PathBuf>>>,
    pub env: Arc<std::collections::HashMap<String, String>>,
    /// The session's shell sandbox, inherited by sub-agents.
    /// The parent session's sandbox, shared so `/sandbox` reaches sub-agents too.
    pub sandbox: kodact_tools::SandboxCell,
    /// The session's shell (Bash tool), the same for sub-agents.
    pub shell: kodact_tools::shells::ShellChoice,
    /// Edit may name lines instead of retyping them (`edit.lineRanges`), as in the session.
    pub line_edits: bool,
    /// Task agents that may run at once (`maxParallelAgents`); more wait their turn.
    pub agent_slots: Arc<tokio::sync::Semaphore>,
    /// Tools beyond the built-ins (MCP servers' tools), offered to sub-agents too.
    pub extra_tools: Vec<Arc<dyn Tool>>,
    /// Where sub-agent transcripts go (`None` = not persisted).
    pub store: Option<SessionStore>,
    pub session_id: String,
    pub hooks: HookRunner,
    /// The main session's sink; sub-agent messages are forwarded with `parent_tool_use_id`.
    pub sink: Arc<dyn EventSink>,
    /// Engine settings shared with the main session (pricing, output cap, compaction).
    pub base: EngineConfig,
    /// Memory files, given to sub-agents as context.
    pub memory_context: Option<String>,
    pub parent: OnceLock<ParentLink>,
}

/// What a child engine is built from, besides what it shares with the session.
pub struct ChildSpec<'a> {
    /// Engine settings (`is_subagent` is forced on).
    pub cfg: EngineConfig,
    pub tools: ToolRegistry,
    pub system: Vec<kodact_types::SystemBlock>,
    pub prompter: Arc<dyn PermissionPrompter>,
    pub sink: Arc<dyn EventSink>,
    /// A fork starts from this conversation; `None` starts it empty.
    pub seed: Option<&'a TurnState>,
    /// Checkpoint its edits under this turn of the session's file history
    /// (added with `FileHistory::add_turn`), not the user's current turn: a
    /// background subtask's edits aren't part of whatever prompt runs meanwhile.
    pub checkpoint_turn: Option<String>,
    /// Run in this directory instead of the session's (an isolated worktree): it is the
    /// child's project and its only working directory besides the session's added ones.
    pub dir: Option<PathBuf>,
}

/// Snapshots files under one fixed turn of the session's file history.
struct TurnCheckpointer {
    history: Arc<FileHistory>,
    turn: String,
}

impl kodact_tools::Checkpointer for TurnCheckpointer {
    fn before_write(&self, path: &std::path::Path) {
        self.history.snapshot_in(&self.turn, path);
    }
}

/// A child engine and its id (its transcript's session id).
pub struct Child {
    pub id: String,
    pub engine: Engine,
}

impl AgentRuntime {
    /// Settings for a fresh sub-agent on `model`: the session's base settings and the parent's
    /// current thinking budget and effort, without the run's limits.
    pub fn child_config(&self, model: &str) -> EngineConfig {
        let mut cfg = self.base.clone();
        cfg.model = model.to_string();
        cfg.fallback_models = vec![];
        cfg.max_budget_usd = None;
        cfg.max_turns = None;
        cfg.json_schema = None;
        cfg.is_subagent = true;
        cfg.initial_context = self.memory_context.clone();
        if let Some(p) = self.parent.get() {
            let rt = p.handle.runtime();
            cfg.max_thinking_tokens = rt.max_thinking_tokens;
            cfg.effort = rt.effort;
        }
        cfg
    }

    /// Build a child engine (the Task tool's agents, `/subtask`). It shares the session's
    /// provider, hooks, sandbox, working directories and file history, starts with a copy of the
    /// parent's permission rules and mode, and gets its own transcript, tool context (Bash cwd,
    /// todos, read state, shells) and conversation.
    pub fn child(&self, spec: ChildSpec<'_>) -> Result<Child, String> {
        let parent = self.parent.get().ok_or("Sub-agents are not available in this session.")?;
        let id = uuid::Uuid::new_v4().to_string();
        let transcript = match &self.store {
            Some(store) => Transcript::create(store, &self.project_dir, &id, None, true),
            None => Transcript::create(
                &SessionStore::new(PathBuf::from("/nonexistent")),
                &self.project_dir,
                &id,
                None,
                false,
            ),
        }
        .map_err(|e| e.to_string())?;
        let mut tool_ctx = ToolContext::new(spec.dir.as_deref().unwrap_or(&self.project_dir));
        if spec.dir.is_some() {
            let extra: Vec<PathBuf> = self.working_dirs.read().unwrap().iter().skip(1).cloned().collect();
            tool_ctx.working_dirs.write().unwrap().extend(extra);
        } else {
            tool_ctx.working_dirs = self.working_dirs.clone();
        }
        tool_ctx.env = self.env.clone();
        tool_ctx.sandbox = self.sandbox.clone();
        tool_ctx.shell = self.shell.clone();
        tool_ctx.session_id = self.session_id.clone();
        if let Some(turn) = spec.checkpoint_turn {
            tool_ctx.checkpointer = Some(Arc::new(TurnCheckpointer { history: parent.history.clone(), turn }));
        }
        let mut cfg = spec.cfg;
        cfg.is_subagent = true;
        let mut permissions = parent.handle.permissions.read().unwrap().clone();
        if let Some(dir) = &spec.dir {
            // Its edits belong in its own tree: the session's is outside its working directories.
            let extra: Vec<PathBuf> = permissions.working_dirs.iter().skip(1).cloned().collect();
            let fresh = kodact_permissions::Engine::new(permissions.mode, permissions.rules.clone(), dir, &extra);
            permissions.cwd = fresh.cwd;
            permissions.working_dirs = fresh.working_dirs;
        }
        let parts = EngineParts {
            provider: self.provider.clone(),
            tools: spec.tools,
            tool_ctx,
            permissions,
            hooks: self.hooks.clone(),
            prompter: spec.prompter,
            sink: spec.sink,
            transcript: Arc::new(transcript),
            history: parent.history.clone(),
            system: spec.system,
        };
        let mut engine = Engine::new(cfg, parts).map_err(|e| e.to_string())?;
        if let Some(seed) = spec.seed {
            engine.seed(seed);
        }
        Ok(Child { id, engine })
    }
}

/// Run the child's turn. When the user stops it, interrupt it and let the turn wind down, so
/// its transcript and spend stay whole (dropping the turn would lose the spend).
async fn run_until_stopped(
    child: &mut Engine,
    prompt: String,
    stop: &tokio_util::sync::CancellationToken,
) -> TurnResult {
    let handle = child.handle();
    let turn = child.submit(MessageContent::Text(prompt));
    tokio::pin!(turn);
    tokio::select! {
        r = &mut turn => r,
        _ = stop.cancelled() => loop {
            // `submit` makes a fresh cancel token when it starts: an interrupt before that is
            // lost, so repeat it until the turn ends.
            handle.interrupt();
            tokio::select! {
                r = &mut turn => break r,
                _ = tokio::time::sleep(std::time::Duration::from_millis(100)) => {}
            }
        },
    }
}

/// What the parent model sees for an agent the user stopped: an error (its task is not done)
/// that begins with [`kodact_tools::AGENT_STOPPED`], with the agent's last text when it had any.
fn stopped_output(agent: &str, partial: &str, structured: Value) -> ToolOutput {
    let mut text = format!(
        "{}: the user stopped the {agent} agent before it finished. Its task is not done; don't start it \
         again unless the user asks. Carry on with the rest of the request without it.",
        kodact_tools::AGENT_STOPPED
    );
    if !partial.trim().is_empty() {
        text.push_str(&format!("\n\nThe agent's last message before the stop:\n{partial}"));
    }
    ToolOutput::error(text).with_structured(structured)
}

/// How a child's turn ended: `completed`, `interrupted` or `error`.
pub fn status_of(r: &TurnResult) -> &'static str {
    if r.is_error {
        "error"
    } else if r.stop_reason.as_deref() == Some("interrupted") {
        "interrupted"
    } else {
        "completed"
    }
}

/// A child's spend, as `Engine::record_subagent_usage` takes it (`subagentUsage`).
pub fn usage_of(r: &TurnResult) -> Value {
    json!({"costUsd": r.total_cost_usd, "usage": r.usage, "modelUsage": r.model_usage})
}

/// Tools a fork lists but may not run: they would reach the person, change the session's
/// schedule, or start agents that ask through the person's prompter.
const NOT_IN_FORKS: &[(&str, &str)] = &[
    ("Task", "Background subtasks can't start other agents."),
    (
        "AskUserQuestion",
        "Nobody can answer questions in a background subtask: decide, and list your assumptions in the report.",
    ),
    ("EnterPlanMode", "Plan mode isn't available in a background subtask."),
    ("ExitPlanMode", "Plan mode isn't available in a background subtask."),
    ("CronCreate", "Background subtasks can't schedule tasks."),
    ("CronDelete", "Background subtasks can't change scheduled tasks."),
    ("ScheduleWakeup", "Background subtasks can't schedule tasks."),
];

/// A fork's tools: the parent's, in the same order and with the same text, so the request
/// prefix matches and the parent's prompt cache is read; refused tools stay listed.
pub fn fork_tools(parent: &ToolRegistry) -> ToolRegistry {
    let mut reg = parent.clone();
    for &(name, why) in NOT_IN_FORKS {
        reg.wrap(name, |inner| Arc::new(Unavailable { inner, why }));
    }
    reg
}

struct Unavailable {
    inner: Arc<dyn Tool>,
    why: &'static str,
}

#[async_trait::async_trait]
impl Tool for Unavailable {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn description(&self) -> String {
        self.inner.description()
    }

    fn input_schema(&self) -> Value {
        self.inner.input_schema()
    }

    fn is_enabled(&self) -> bool {
        self.inner.is_enabled()
    }

    /// Refused before any permission check.
    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn validate(&self, _input: &Value, _ctx: &ToolContext) -> Result<(), String> {
        Err(self.why.to_string())
    }

    async fn call(&self, _input: Value, _ctx: &ToolContext) -> ToolOutput {
        ToolOutput::error(self.why)
    }
}

/// A sub-agent's own git worktree (`isolation: "worktree"`): a copy of HEAD on a new branch
/// under `.kodact/worktrees/`, where its edits can't collide with the session's or another
/// agent's.
struct Isolated {
    repo: PathBuf,
    path: PathBuf,
    branch: String,
    /// Where the agent starts: the session's directory, inside the copy.
    cwd: PathBuf,
    base: Option<String>,
}

impl Isolated {
    fn create(project_dir: &std::path::Path) -> Result<Self, String> {
        // One at a time: agents started together would race for git's locks.
        static CREATING: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = CREATING.lock().unwrap_or_else(|e| e.into_inner());
        let repo = kodact_git::repo_root(project_dir).ok_or("not a git repository")?;
        let name = format!("agent-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]);
        let path = repo.join(".kodact").join("worktrees").join(&name);
        let branch = format!("kodact/{name}");
        kodact_git::add_worktree(&repo, &path, &branch)?;
        let _ = kodact_git::exclude(&repo, ".kodact/worktrees/");
        let rel = project_dir.strip_prefix(&repo).unwrap_or(std::path::Path::new(""));
        let cwd = if path.join(rel).is_dir() { path.join(rel) } else { path.clone() };
        let base = kodact_git::head(&path);
        Ok(Isolated { repo, path, branch, cwd, base })
    }

    /// Remove the copy when the agent left nothing in it; otherwise keep it and say where the
    /// work is. Nothing is merged or deleted on the agent's behalf.
    fn finish(&self) -> Option<String> {
        let committed = kodact_git::head(&self.path) != self.base;
        let dirty = kodact_git::is_dirty(&self.path);
        if !committed && !dirty {
            let _ = kodact_git::remove_worktree(&self.repo, &self.path);
            let _ = kodact_git::delete_branch(&self.repo, &self.branch);
            return None;
        }
        let state = match (committed, dirty) {
            (true, false) => "committed on that branch",
            (true, true) => "partly committed on that branch, partly uncommitted in that directory",
            _ => "uncommitted in that directory",
        };
        Some(format!(
            "\n\n[The agent worked in its own worktree: {} (branch {}). Its changes are {state}; they are not \
             in your working tree. Review them (`git -C {} status`, `git diff HEAD...{}`) and bring over what \
             you want (commit there, then `git merge` or `git cherry-pick`; or copy the files). Remove the \
             worktree with `git worktree remove` once done.]",
            self.path.display(),
            self.branch,
            self.path.display(),
            self.branch,
        ))
    }
}

pub struct TaskTool {
    pub rt: Arc<AgentRuntime>,
}

impl TaskTool {
    fn agent(&self, name: &str) -> Option<&AgentDef> {
        let n = if name.trim().is_empty() { "general-purpose" } else { name.trim() };
        self.rt.agents.iter().find(|a| a.name == n || a.name.eq_ignore_ascii_case(n))
    }

    fn registry_for(&self, agent: &AgentDef) -> ToolRegistry {
        let mut reg = ToolRegistry::new();
        kodact_tools::builtin::register_core(&mut reg);
        kodact_tools::builtin::set_shell(&mut reg, &self.rt.shell);
        kodact_tools::builtin::set_line_edits(&mut reg, self.rt.line_edits);
        for t in &self.rt.extra_tools {
            reg.register(t.clone());
        }
        if let Some(allowed) = &agent.tools {
            // `mcp__server` in an agent's list covers every tool of that server.
            reg.retain(|n| allowed.iter().any(|a| a == n || n.starts_with(&format!("{a}__"))));
        }
        // No nested sub-agents, and only the main session talks to the person.
        reg.retain(|n| !matches!(n, "Task" | "AskUserQuestion" | "EnterPlanMode" | "ExitPlanMode"));
        reg
    }
}

#[async_trait::async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "Task"
    }

    fn description(&self) -> String {
        let mut s = String::from(
            "Run a sub-agent on a task in its own context, and get back only its final report.\n\n\
             Use it for:\n\
             - open-ended searches across many files, where you only need the conclusion;\n\
             - investigations or side work whose details would crowd your context;\n\
             - independent pieces of work that can run in parallel (several Task calls in one message run \
               concurrently).\n\n\
             The sub-agent cannot see this conversation. Give it a complete, self-contained prompt: the goal, \
             what you already know, constraints, and exactly what to report back. Its report is returned to \
             you, not shown to the user, so relay what matters. Read a specific known file yourself instead \
             of starting an agent.\n\nAvailable agents (`subagent_type`):\n",
        );
        for a in &self.rt.agents {
            let tools = match &a.tools {
                Some(t) => t.join(", "),
                None => "all tools".into(),
            };
            s.push_str(&format!("- {}: {} (Tools: {tools})\n", a.name, a.description));
        }
        s
    }

    fn input_schema(&self) -> Value {
        let names: Vec<&str> = self.rt.agents.iter().map(|a| a.name.as_str()).collect();
        json!({
            "type": "object",
            "properties": {
                "description": {"type": "string", "description": "A short (3-5 word) description of the task"},
                "prompt": {"type": "string", "description": "The complete, self-contained task for the agent"},
                "subagent_type": {"type": "string", "enum": names, "description": "Which agent to run (default general-purpose)"},
                "isolation": {"type": "string", "enum": ["worktree"], "description": "\"worktree\": the agent works in its own git worktree, a copy of the last commit on a new branch. Use it when agents running in parallel change files, so they can't overwrite each other. Uncommitted changes are not in the copy. The result names the branch and the directory when the agent changed anything; merge it yourself"}
            },
            "required": ["description", "prompt"],
            "additionalProperties": false
        })
    }

    /// Launching an agent needs no prompt: every tool it calls is checked on its own.
    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    fn permission_subject(&self, input: &Value, _ctx: &ToolContext) -> Subject {
        Subject::Name(input.get("subagent_type").and_then(Value::as_str).unwrap_or("general-purpose").to_string())
    }

    fn validate(&self, input: &Value, ctx: &ToolContext) -> Result<(), String> {
        kodact_tools::validate_required(&self.input_schema(), input)?;
        let name = input.get("subagent_type").and_then(Value::as_str).unwrap_or("");
        if self.agent(name).is_none() {
            let names: Vec<&str> = self.rt.agents.iter().map(|a| a.name.as_str()).collect();
            return Err(format!("Unknown subagent_type {name:?}. Available: {}", names.join(", ")));
        }
        if input.get("isolation").is_some() && kodact_git::repo_root(&self.rt.project_dir).is_none() {
            return Err("isolation \"worktree\" needs a git repository, and this project is not one. Run the \
                        agent without it."
                .into());
        }
        let _ = ctx;
        Ok(())
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let Some(parent) = self.rt.parent.get() else {
            return ToolOutput::error("Sub-agents are not available in this session.");
        };
        let agent = self.agent(input.get("subagent_type").and_then(Value::as_str).unwrap_or("")).cloned().unwrap();
        let prompt = input.get("prompt").and_then(Value::as_str).unwrap_or("").to_string();
        let started = Instant::now();
        // The user can stop this agent alone (a host's `stop_task`); the parent's turn goes on.
        let registration = parent.handle.register_agent(&ctx.tool_use_id);
        let stop = registration.token();
        // At most `maxParallelAgents` run at once; the others wait here, without spending anything.
        let _slot = tokio::select! {
            _ = ctx.cancel.cancelled() => return ToolOutput::error(kodact_tools::INTERRUPTED),
            _ = stop.cancelled() => return stopped_output(&agent.name, "", json!({
                "agentType": agent.name,
                "status": "stopped",
                "totalToolUseCount": 0,
                "totalDurationMs": started.elapsed().as_millis() as u64,
            })),
            p = self.rt.agent_slots.clone().acquire_owned() => match p {
                Ok(p) => p,
                Err(_) => return ToolOutput::error("Sub-agents are not available in this session."),
            },
        };

        let model = match &agent.model {
            Some(m) => kodact_api::resolve_model(m),
            None => parent.handle.model(),
        };
        let isolated = match input.get("isolation").and_then(Value::as_str) {
            Some(_) => match Isolated::create(&self.rt.project_dir) {
                Ok(w) => Some(w),
                Err(e) => return ToolOutput::error(format!("Could not create the agent's worktree: {e}")),
            },
            None => None,
        };
        let dirs: Vec<PathBuf> = self.rt.working_dirs.read().unwrap().iter().skip(1).cloned().collect();
        let agent_dir = isolated.as_ref().map(|w| w.cwd.clone());
        let mut env = EnvInfo::collect(agent_dir.as_deref().unwrap_or(&self.rt.project_dir), &dirs, &model);
        env.shell = kodact_platform::shell::env_line(&self.rt.shell);
        let mut sys = kodact_types::SystemBlock::text(format!("{}\n\n{}", agent.prompt, env.render()));
        sys.cache_control = Some(kodact_types::CacheControl::ephemeral());
        let mut cfg = self.rt.child_config(&model);
        if agent.effort.is_some() {
            cfg.effort = agent.effort.clone();
        }
        let spec = ChildSpec {
            cfg,
            tools: self.registry_for(&agent),
            system: vec![sys],
            prompter: parent.prompter.clone(),
            sink: Arc::new(ForwardSink { parent: self.rt.sink.clone(), parent_tool_use_id: ctx.tool_use_id.clone() }),
            seed: None,
            checkpoint_turn: None,
            dir: agent_dir,
        };
        let Child { id: child_id, engine: mut child } = match self.rt.child(spec) {
            Ok(c) => c,
            Err(e) => {
                if let Some(w) = &isolated {
                    w.finish();
                }
                return ToolOutput::error(format!("Could not start the sub-agent: {e}"));
            }
        };
        // Interrupting the caller interrupts the agent.
        let handle = child.handle();
        let cancel = ctx.cancel.clone();
        let watcher = tokio::spawn(async move {
            cancel.cancelled().await;
            handle.interrupt();
        });
        let result = run_until_stopped(&mut child, prompt, &stop).await;
        watcher.abort();
        drop(registration);
        // Stopped by the user alone, not by an interrupt of the whole turn.
        let stopped =
            stop.is_cancelled() && !ctx.cancel.is_cancelled() && result.stop_reason.as_deref() == Some("interrupted");

        let tool_uses = child
            .state
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
            .count();
        let mut structured = json!({
            "agentType": agent.name,
            "agentId": child_id,
            "status": if stopped { "stopped" } else { status_of(&result) },
            "totalToolUseCount": tool_uses,
            "totalDurationMs": started.elapsed().as_millis() as u64,
            "subagentUsage": usage_of(&result),
        });
        let mut text = result.result.clone().unwrap_or_default();
        // Whatever way it ended, the parent is told where its changes are (or that the copy is gone).
        if let Some(w) = &isolated {
            let kept = w.finish();
            structured["worktree"] = json!({"path": w.path, "branch": w.branch, "kept": kept.is_some()});
            if let Some(note) = kept {
                text.push_str(&note);
            }
        }
        if stopped {
            return stopped_output(&agent.name, &text, structured);
        }
        if result.is_error {
            let detail = if text.is_empty() { result.errors.join("; ") } else { text };
            return ToolOutput::error(format!(
                "FAILED: the {} agent did not finish its task. {detail}\n\
                 Nothing it was asked to do can be counted as done, and it wrote no report. Run it again (after \
                 the cause is fixed), do the work yourself, or tell the user it failed; never describe its task \
                 as completed.",
                agent.name
            ))
            .with_structured(structured);
        }
        if result.stop_reason.as_deref() == Some("interrupted") {
            return ToolOutput::error(kodact_tools::INTERRUPTED).with_structured(structured);
        }
        let text = if text.trim().is_empty() { "(the agent finished without a report)".to_string() } else { text };
        ToolOutput::text(text).with_structured(structured)
    }
}
