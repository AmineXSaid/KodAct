//! `/subtask` (contract C20): forked background sub-agents.
//!
//! A subtask starts from the whole conversation so far: the same system prompt, tools and
//! messages, so its first request reads the main conversation's prompt cache. It runs on its
//! own tokio task while the person keeps going, never asks for permission, and is handed back
//! the next time the session is idle: its spend joins the session's, the person gets a notice,
//! a host gets `system/subtask`, and its report rides along with the next prompt (in `-p`, one
//! more turn).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use kodact_engine::{
    Engine, EngineEvent, EventSink, PermissionAnswer, PermissionPrompt, PermissionPrompter, TurnResult,
};
use kodact_types::{ContentBlock, MessageContent};
use serde_json::Value;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

/// Subtasks that may run at once.
pub const MAX_RUNNING: usize = 8;
/// How long running subtasks get to wind down when the session ends.
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(3);

const FORK_NOTE: &str = concat!(
    "You are now a background subtask, forked from the conversation above. The user has moved on: they won't ",
    "see your messages and can't answer questions, and nobody can approve a permission request (any that would ",
    "ask is denied). Work only on the task below, using the conversation for context. Don't start other agents, ",
    "schedule tasks or enter plan mode. End with a short report for the main conversation: what you did or found, ",
    "the files you changed, what you couldn't do and why, and what is left open. That report is all it will see."
);

/// The prompt of the turn that hands reports back in `-p`, once the input has ended.
pub(crate) const CONTINUE_PROMPT: &str = concat!(
    "The background subtasks the user started have finished; their reports are in the reminder above. Give the ",
    "user the outcome: what was found or changed, and anything still open. Do more work only if the conversation ",
    "calls for it."
);

/// The fork's prompt. Sub-agents get no reminders, so the note travels in it.
pub(crate) fn prompt(task: &str) -> String {
    format!("<system-reminder>\n{FORK_NOTE}\n</system-reminder>\n\nTask: {task}")
}

/// For the main conversation's next prompt, when a subtask starts.
pub(crate) fn started_note(id: &str, task: &str) -> String {
    format!(
        "The user started background subtask {id}, on a copy of this conversation, to work on: {task}\nIts report \
         will reach you when it finishes. Don't do that task yourself, and avoid editing the files it is likely to \
         change meanwhile."
    )
}

/// For the main conversation's next prompt, when a subtask is handed back.
pub(crate) fn report_note(id: &str, task: &str, o: &Outcome) -> String {
    format!("Background subtask {id} ({task}) {}. Its report:\n\n{}", o.how(), o.report)
}

/// How a subtask ended.
#[derive(Debug, Clone)]
pub struct Outcome {
    /// `completed`, `interrupted` or `error`.
    pub status: &'static str,
    /// Its final report, or what went wrong.
    pub report: String,
    /// Its spend (`{costUsd, usage, modelUsage}`), for `Engine::record_subagent_usage`.
    pub usage: Value,
    pub denials: usize,
    pub duration: Duration,
}

impl Outcome {
    fn of(r: &TurnResult, duration: Duration) -> Self {
        let status = kodact_agents::status_of(r);
        let text = r.result.clone().unwrap_or_default();
        let report = if status == "error" && text.is_empty() {
            r.errors.join("; ")
        } else if text.trim().is_empty() {
            "(it ended without a report)".to_string()
        } else {
            text
        };
        Outcome { status, report, usage: kodact_agents::usage_of(r), denials: r.permission_denials.len(), duration }
    }

    /// Its task ended without a result: it panicked or was aborted.
    fn lost() -> Self {
        let report = "The subtask stopped unexpectedly.".to_string();
        Outcome { status: "error", report, usage: Value::Null, denials: 0, duration: Duration::ZERO }
    }

    pub fn how(&self) -> &'static str {
        match self.status {
            "completed" => "has finished",
            "interrupted" => "was stopped before it finished",
            _ => "failed",
        }
    }
}

/// Nobody watches a subtask: whatever would ask is denied, with a reason the model can act on
/// and report.
pub struct SubtaskPrompter;

#[async_trait::async_trait]
impl PermissionPrompter for SubtaskPrompter {
    async fn ask(&self, p: PermissionPrompt) -> PermissionAnswer {
        let message = match p.tool_name.as_str() {
            "AskUserQuestion" => {
                "Nobody can answer questions in a background subtask. Decide, and list your assumptions in the report."
                    .to_string()
            }
            "EnterPlanMode" | "ExitPlanMode" => "Plan mode isn't available in a background subtask.".to_string(),
            name => format!(
                "Permission to use {name} was denied: this is a background subtask and nobody can approve it ({}). \
                 Don't retry it. Do what you can without it, and say in your report what still needs approval.",
                p.reason
            ),
        };
        PermissionAnswer::Deny { message, interrupt: false }
    }
}

/// A subtask's events: tool calls are counted (for `/tasks`), notices reach the person tagged
/// with its id, and the rest stays in its transcript.
pub struct SubtaskSink {
    id: String,
    parent: Arc<dyn EventSink>,
    calls: Arc<AtomicUsize>,
}

impl SubtaskSink {
    pub fn new(id: &str, parent: Arc<dyn EventSink>, calls: Arc<AtomicUsize>) -> Self {
        SubtaskSink { id: id.to_string(), parent, calls }
    }
}

impl EventSink for SubtaskSink {
    fn emit(&self, event: EngineEvent) {
        match event {
            EngineEvent::Assistant { message, .. } => {
                let n = message.content.iter().filter(|b| matches!(b, ContentBlock::ToolUse { .. })).count();
                self.calls.fetch_add(n, Ordering::Relaxed);
            }
            EngineEvent::Notice { level, text } => {
                self.parent.emit(EngineEvent::Notice { level, text: format!("[{}] {text}", self.id) })
            }
            _ => {}
        }
    }
}

/// One subtask.
pub struct Subtask {
    pub id: String,
    pub task: String,
    pub started: Instant,
    /// Its sidechain transcript (`None` without session persistence).
    pub transcript: Option<PathBuf>,
    /// The conversation it was forked from is gone (`/clear`, `/resume`): no report to the model.
    pub(crate) orphaned: bool,
    calls: Arc<AtomicUsize>,
    stop: CancellationToken,
    outcome: Arc<Mutex<Option<Outcome>>>,
    join: tokio::task::JoinHandle<()>,
}

impl Subtask {
    pub fn is_running(&self) -> bool {
        self.outcome.lock().unwrap().is_none()
    }

    /// Tool calls so far.
    pub fn tool_calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }

    /// A row for the session view: it follows this subtask (running state, tool calls) and can stop it.
    pub fn row(&self) -> SubtaskRow {
        SubtaskRow {
            id: self.id.clone(),
            task: self.task.clone(),
            started: self.started,
            calls: self.calls.clone(),
            stop: self.stop.clone(),
            outcome: self.outcome.clone(),
        }
    }
}

/// A subtask as the session view sees it (`/tasks` mid-turn). Cheap to clone.
#[derive(Clone)]
pub struct SubtaskRow {
    pub id: String,
    pub task: String,
    pub started: Instant,
    calls: Arc<AtomicUsize>,
    stop: CancellationToken,
    outcome: Arc<Mutex<Option<Outcome>>>,
}

impl SubtaskRow {
    pub fn is_running(&self) -> bool {
        self.outcome.lock().unwrap().is_none()
    }

    pub fn tool_calls(&self) -> usize {
        self.calls.load(Ordering::Relaxed)
    }
}

/// `/tasks stop <id>` for a subtask: what happened, or `None` when there is no such subtask.
pub fn stop_row(rows: &[SubtaskRow], finished: &[FinishedSubtask], id: &str) -> Option<String> {
    if finished.iter().any(|f| f.id == id) {
        return Some(format!("{id} has already finished and been reported."));
    }
    let t = rows.iter().find(|t| t.id == id)?;
    Some(if t.is_running() {
        t.stop.cancel();
        format!("Stopping {id}. It ends after its current step; what it did so far is handed back.")
    } else {
        format!("{id} has already finished; its report is handed back with your next prompt.")
    })
}

/// A subtask that was handed back, for `/tasks`.
#[derive(Debug, Clone)]
pub struct FinishedSubtask {
    pub id: String,
    pub task: String,
    /// `completed`, `interrupted` or `error`.
    pub status: &'static str,
    pub duration: Duration,
    pub tool_calls: usize,
}

/// Handed-back subtasks `/tasks` keeps listing.
const KEEP_FINISHED: usize = 20;

/// The session's subtasks: running, or finished and not yet handed back.
pub struct Subtasks {
    list: Vec<Subtask>,
    /// Handed back already, newest last (at most [`KEEP_FINISHED`]).
    finished: Vec<FinishedSubtask>,
    count: u32,
    /// Bumped each time one finishes. Front ends hold receivers: move this registry, never rebuild it.
    changed: Arc<watch::Sender<u64>>,
}

impl Default for Subtasks {
    fn default() -> Self {
        Subtasks { list: vec![], finished: vec![], count: 0, changed: Arc::new(watch::channel(0).0) }
    }
}

impl Subtasks {
    /// Changes each time a subtask finishes. Mark it seen (`borrow_and_update`) before looking at
    /// the subtasks, then wait on `changed()`: an end after the mark still wakes the wait.
    pub fn watch(&self) -> watch::Receiver<u64> {
        self.changed.subscribe()
    }

    pub fn running(&self) -> usize {
        self.list.iter().filter(|t| t.is_running()).count()
    }

    /// Running, or finished and not yet handed back.
    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Subtask> {
        self.list.iter()
    }

    /// Subtasks already handed back, oldest first.
    pub fn finished(&self) -> &[FinishedSubtask] {
        &self.finished
    }

    pub(crate) fn next_id(&mut self) -> String {
        self.count += 1;
        format!("subtask_{}", self.count)
    }

    /// Run `child` on `prompt` on its own tokio task. Needs a tokio runtime.
    pub(crate) fn spawn(&mut self, id: String, task: &str, mut child: Engine, prompt: String, calls: Arc<AtomicUsize>) {
        let stop = CancellationToken::new();
        let outcome = Arc::new(Mutex::new(None));
        let transcript = child.transcript().path().map(Path::to_path_buf);
        let done = Finished { outcome: outcome.clone(), changed: self.changed.clone() };
        let token = stop.clone();
        let join = tokio::spawn(async move {
            let started = Instant::now();
            let result = run(&mut child, prompt, &token).await;
            // Its background shells (its own) can't be read by anyone now.
            child.tool_ctx().shells.stop_all();
            done.set(Outcome::of(&result, started.elapsed()));
        });
        let task = task.to_string();
        self.list.push(Subtask {
            id,
            task,
            started: Instant::now(),
            transcript,
            orphaned: false,
            calls,
            stop,
            outcome,
            join,
        });
    }

    /// `/tasks stop <id>`.
    pub fn stop(&self, id: &str) -> Option<String> {
        stop_row(&self.rows(), &self.finished, id)
    }

    /// Rows for the session view, oldest first.
    pub fn rows(&self) -> Vec<SubtaskRow> {
        self.list.iter().map(Subtask::row).collect()
    }

    /// The conversation changed under them (`/clear`, `/resume`): stop the running ones, and
    /// pass no report to the new conversation. Their spend is still counted.
    pub(crate) fn orphan_all(&mut self) {
        for t in &mut self.list {
            t.orphaned = true;
            t.stop.cancel();
        }
    }

    /// Remove the finished ones, oldest first.
    pub(crate) fn take_finished(&mut self) -> Vec<(Subtask, Outcome)> {
        let (done, running): (Vec<Subtask>, Vec<Subtask>) =
            std::mem::take(&mut self.list).into_iter().partition(|t| !t.is_running());
        self.list = running;
        let out: Vec<(Subtask, Outcome)> = done
            .into_iter()
            .map(|t| {
                let o = t.outcome.lock().unwrap().clone().unwrap_or_else(Outcome::lost);
                (t, o)
            })
            .collect();
        for (t, o) in &out {
            self.finished.push(FinishedSubtask {
                id: t.id.clone(),
                task: t.task.clone(),
                status: o.status,
                duration: o.duration,
                tool_calls: t.tool_calls(),
            });
        }
        let extra = self.finished.len().saturating_sub(KEEP_FINISHED);
        self.finished.drain(..extra);
        out
    }

    /// Session end: stop them all, give them `grace` to wind down, abort the rest.
    pub(crate) async fn shutdown(&self, grace: Duration) {
        if self.list.is_empty() {
            return;
        }
        for t in &self.list {
            t.stop.cancel();
        }
        let mut w = self.watch();
        let _ = tokio::time::timeout(grace, async {
            loop {
                let _ = *w.borrow_and_update();
                if self.running() == 0 || w.changed().await.is_err() {
                    return;
                }
            }
        })
        .await;
        for t in &self.list {
            t.join.abort();
        }
    }
}

/// Records how a subtask ended and wakes every waiter, also when its task panics or is
/// aborted (the future is dropped with this guard in it).
struct Finished {
    outcome: Arc<Mutex<Option<Outcome>>>,
    changed: Arc<watch::Sender<u64>>,
}

impl Finished {
    fn set(self, o: Outcome) {
        *self.outcome.lock().unwrap() = Some(o);
        // Dropping `self` wakes the waiters, after the outcome is in place.
    }
}

impl Drop for Finished {
    fn drop(&mut self) {
        if let Ok(mut o) = self.outcome.lock() {
            if o.is_none() {
                *o = Some(Outcome::lost());
            }
        }
        self.changed.send_modify(|n| *n = n.wrapping_add(1));
    }
}

/// The child's one turn. A stop interrupts it and lets the turn wind down, so its transcript
/// and spend stay whole (aborting would lose the spend).
async fn run(child: &mut Engine, prompt: String, stop: &CancellationToken) -> TurnResult {
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
                _ = tokio::time::sleep(Duration::from_millis(100)) => {}
            }
        },
    }
}
