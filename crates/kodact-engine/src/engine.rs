use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use futures::StreamExt;
use kodact_api::models::{model_info, ModelInfo};
use kodact_api::{ApiError, MessageAccumulator, Provider};
use kodact_hooks::{Exit2Effect, HookEvent, HookRunner};
use kodact_permissions::Engine as PermEngine;
use kodact_session::{FileHistory, Transcript};
use kodact_tools::{ToolContext, ToolRegistry};
use kodact_types::sdk::{PermissionDenial, ResultSubtype};
use kodact_types::{
    ApiMessage, ContentBlock, Message, MessageContent, MessagesRequest, StopReason, StreamEvent, SystemBlock, Usage,
};
use serde_json::{json, Map, Value};
use tokio_util::sync::CancellationToken;

use crate::events::{EngineEvent, EventSink, NoticeLevel, PermissionPrompter};
use crate::request::{apply_cache_breakpoints, normalize};
use crate::snapshot::{EngineSnapshot, RequestParts, TurnProgress};
use crate::{INTERRUPT_MARKER, INTERRUPT_MARKER_TOOLS};

/// Sent with every prompt while plan mode is on.
const PLAN_MODE_REMINDER: &str = "Plan mode is active. The user wants a plan before any change: explore with \
read-only tools only (no edits, no writes, no commands that change anything), design the approach, and then \
present it with the ExitPlanMode tool (the steps, the files to change, and how you will verify the result). \
Do not change anything until the user approves the plan. If you need a choice from the user first, ask it.";

/// USD per million tokens (settings `modelPricing` override).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pricing {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

impl Pricing {
    fn cost(&self, u: &Usage) -> f64 {
        (u.input_tokens as f64 * self.input
            + u.output_tokens as f64 * self.output
            + u.cache_read_input_tokens as f64 * self.cache_read
            + u.cache_creation_input_tokens as f64 * self.cache_write)
            / 1_000_000.0
    }

    fn from_model(m: &ModelInfo) -> Self {
        Pricing {
            input: m.input_price,
            output: m.output_price,
            cache_read: m.cache_read_price,
            cache_write: m.cache_write_price(),
        }
    }
}

/// A wall-clock limit for the run (`--max-time`), counted from when it started.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeLimit {
    pub started: Instant,
    pub total: Duration,
}

impl TimeLimit {
    pub fn starting_now(total: Duration) -> Self {
        TimeLimit { started: Instant::now(), total }
    }

    pub fn remaining(&self) -> Duration {
        self.total.saturating_sub(self.started.elapsed())
    }
}

/// The note that ends a turn cut by `--max-time`.
const TIME_UP_MARKER: &str = "[The run reached its time limit (--max-time) and stopped here]";

/// A spawned task stopped when its owner goes (the `--max-time` watch of one turn).
struct AbortOnDrop(tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The smallest reply budget a request is sized down to; with less room than this, it goes as
/// it is, and a refusal for length compacts the conversation and tries again.
const MIN_REPLY: u32 = 4_096;

/// Times a turn's request is sent again with half the reply budget, after refusals for length
/// whose prompt fits the window: 32,000 → 16,000 → 8,000 → 4,096, then the conversation is
/// compacted.
const MAX_REPLY_SHRINKS: u32 = 3;

/// Compactions one turn makes because the conversation no longer fits. More than one: a long
/// `-p` run is one turn, and it fills a small window again after each compaction. Each needs the
/// conversation to have grown since the last, so one that didn't make room isn't repeated.
const MAX_LENGTH_COMPACTIONS: u32 = 8;

/// A reply budget that fits the window: `cap`, but no more than what `window` has left after a
/// `prompt` of that many tokens, less a margin for the parts of the prompt that are estimated (a
/// fiftieth of the window, at least 1,024). Endpoints that count the requested output against the
/// window (vLLM, LiteLLM and gateways in front of them) refuse a request whose prompt and cap add
/// up to more than the window, even when the prompt alone fits: in 090446d, 7 summary calls asked
/// for 32,000 tokens on top of a 68,000-token prompt in a 100,000 window, and one run ended on it.
/// With room to spare (a large window, or a short conversation) this is `cap`, unchanged.
pub(crate) fn fit_reply(cap: u32, window: u64, prompt: u64) -> u32 {
    let margin = (window / 50).max(1_024);
    let room = window.saturating_sub(prompt).saturating_sub(margin);
    let fitted = room.max(u64::from(cap.min(MIN_REPLY)));
    u32::try_from(fitted.min(u64::from(cap))).unwrap_or(cap)
}

/// Characters of the person's requests a compaction carries on: a tenth of the model's window
/// (at about four characters a token), at least 2,000. Sized from the window, so a large model
/// keeps every request whole and a small one keeps the task and the latest requests.
pub(crate) fn request_budget(window: u64) -> usize {
    (usize::try_from(window).unwrap_or(usize::MAX).saturating_mul(4) / 10).max(2_000)
}

/// "about 12 minutes", for time-limit notes.
fn about(left: Duration) -> String {
    match left.as_secs() {
        s if s < 60 => "less than a minute".into(),
        s if s < 90 => "about a minute".into(),
        s => format!("about {} minutes", (s + 30) / 60),
    }
}

/// Unattended runs: how many times one turn is reminded of an announced next step, or of open todos.
const FINISH_REMINDERS: u32 = 2;

/// The longest a turn waits, before it ends, for commands that outlived their Bash timeout; and
/// how many times one turn waits.
const BACKGROUND_WAIT: Duration = Duration::from_secs(600);
const BACKGROUND_WAITS: u32 = 2;

/// The last line of a reply when it announces a next step instead of ending ("Let me check the
/// logs."), cut to 160 characters. "Let me know..." is an ending, not a step.
pub(crate) fn announced_next_step(text: &str) -> Option<String> {
    let last = text.trim().lines().rev().find(|l| !l.trim().is_empty())?.trim();
    let lower = last.to_lowercase().replace('\u{2019}', "'").replace(['*', '_', '`'], "");
    let lower = lower.trim_start_matches(|c: char| !c.is_alphanumeric());
    const STEPS: [&str; 10] = [
        "let me ",
        "let's ",
        "i'll ",
        "i will ",
        "i'm going to ",
        "i am going to ",
        "now i'll ",
        "now let me ",
        "next, i",
        "next i'll ",
    ];
    let step = STEPS.iter().any(|s| lower.starts_with(s)) && !lower.starts_with("let me know");
    (step || lower.ends_with(':')).then(|| last.chars().take(160).collect())
}

fn next_step_reminder(line: &str) -> String {
    format!(
        "<system-reminder>\nYour last message ends by announcing a next step (\"{line}\") but makes no tool call, so \
         the run would end here with that step undone. Do it now with a tool call. If the task is in fact complete, say \
         so and state what you verified.\n</system-reminder>"
    )
}

/// For a one-shot run that is not unattended ([`EngineConfig::one_shot`]).
fn one_shot_todo_reminder(open: &[String]) -> String {
    format!(
        "<system-reminder>\nThis run ends with your reply and takes no further input, and todo items are not \
         completed:\n{}\nFinish what you can now. If an item is blocked, needs an answer from the person, or no \
         longer applies, update the list with TodoWrite and say plainly what is left and why.\n</system-reminder>",
        open.iter().map(|t| format!("- {t}")).collect::<Vec<_>>().join("\n")
    )
}

fn todo_reminder(open: &[String]) -> String {
    format!(
        "<system-reminder>\nThe run is about to end with todo items not completed:\n{}\nFinish them now. If one no \
         longer applies, update the list with TodoWrite and say why.\n</system-reminder>",
        open.iter().map(|t| format!("- {t}")).collect::<Vec<_>>().join("\n")
    )
}

/// Unattended runs (GOALS pillar 2): the first wait before a failed model call is tried again.
const API_RETRY_FIRST_WAIT: Duration = Duration::from_secs(5);
/// The longest wait between two tries.
const API_RETRY_MAX_WAIT: Duration = kodact_api::MAX_RETRY_WAIT;
/// Rate limits waited out in a row before a fallback model, when one is set, takes the turn.
const FALLBACK_AFTER_THROTTLES: u32 = 3;
/// Without `--max-time`, how many times a failed call is tried again.
const API_RETRY_ATTEMPTS: u32 = 10;
/// With `--max-time`, the most tries, and the time kept after a wait for the model to wrap up.
const API_RETRY_MAX_ATTEMPTS: u32 = 30;
const API_RETRY_RESERVE: Duration = Duration::from_secs(60);
/// Retries of a `-p` run whose endpoint never answered and can't be reached.
const FIRST_CONTACT_RETRIES: u32 = 3;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// KodAct's own nudges to the model: the verification reminder, the loop guard, the
    /// act, attempt, next-step and todo reminders, the budget and time wrap-up, and the
    /// background-command warning. Off, the model works on its own between tool results, and
    /// only what the person and the CLI did is told to it (plan mode, a mode switch, attached
    /// files, a subtask's result). On by default; `reminders: false` turns them off.
    pub reminders: bool,
    pub model: String,
    pub fallback_models: Vec<String>,
    /// Output cap per request (default 32,000, capped by the model).
    pub max_output_tokens: u32,
    pub effort: Option<String>,
    /// `None` = model default, `Some(0)` = off, `Some(n)` = on / budget.
    pub max_thinking_tokens: Option<u32>,
    pub max_turns: Option<u32>,
    pub max_budget_usd: Option<f64>,
    /// `--max-time`: the model is told the limit and warned near the end; the host enforces it.
    pub time_limit: Option<TimeLimit>,
    /// `--autonomous`: nobody answers; a turn that ends without any tool call gets one
    /// reminder to make a real attempt first, and a model call that fails for a reason that
    /// passes (rate limit, overload, server error, lost connection) is tried again.
    pub autonomous: bool,
    /// The first wait before such a retry; each next one doubles, up to a minute.
    pub api_retry_wait: Duration,
    /// `--json-schema`: structured output for the final answer.
    pub json_schema: Option<Value>,
    pub pricing: HashMap<String, Pricing>,
    /// Text prepended (as a system reminder) to the first user message of a new session.
    pub initial_context: Option<String>,
    pub metadata_user_id: Option<String>,
    /// Context window used for compaction thresholds (`--autocompact`); default: the model's window.
    pub autocompact_window: Option<u64>,
    /// Summarize automatically when the window is nearly full (contract C9).
    pub auto_compact: bool,
    /// Runs as a sub-agent: SubagentStop instead of Stop, no session hooks.
    pub is_subagent: bool,
    /// The verification loop; `None` turns it off. Never runs in sub-agents.
    pub verify: Option<crate::verify::VerifyConfig>,
    /// Tell the model, with an edit's result, when the file it wrote no longer parses
    /// (`verification.syntaxCheck`, on with the verification loop). Sub-agents keep it.
    pub edit_diagnostics: bool,
    /// Ask a language server for the errors after an edit (`verification.languageServer`, off
    /// by default): types, imports and broken callers, which the parse check can't see. Only
    /// the main agent asks; `None` is off.
    pub language_servers: Option<crate::lsp::LspConfig>,
    /// Run the edited file through its formatter when it was formatted before the edit
    /// (`verification.format`, off by default); `None` is off.
    pub formatters: Option<crate::format::FormatConfig>,
    /// The run ends with this turn and takes no further input (`-p` with its prompt given up
    /// front): a turn about to end with todo items open is sent back once, since a script gets
    /// what the turn left. Not for a session with a person or a host at the other end, where
    /// stopping to ask is right.
    pub one_shot: bool,
    /// After a `max_tokens` stop, raise the output cap to [`ESCALATED_OUTPUT_TOKENS`]
    /// for the rest of the turn (off when the user set the cap).
    pub escalate_output: bool,
    /// Start in fast mode (`fastMode` setting).
    pub fast: bool,
    /// Progress nudges (`nudges` in settings, `KODACT_NUDGES`): each off until measured, and
    /// all off with `reminders`. Never in sub-agents.
    pub nudges: crate::progress::NudgeConfig,
}

/// The output cap after a `max_tokens` stop (capped by the model).
pub const ESCALATED_OUTPUT_TOKENS: u32 = 64_000;
/// Times one turn may continue a reply cut off by `max_tokens`.
const MAX_CONTINUATIONS: u32 = 3;
/// Reasoning-only cut-offs in a row, with no tool call between them, that get [`ACT_PROMPT`]. A
/// tool call resets the count; past it the model is not making progress and the turn may end.
const MAX_REASONING_CUTOFFS: u32 = 3;
const CONTINUE_PROMPT: &str = "<system-reminder>\nYour reply was cut off by the output token limit. Continue \
                               exactly where it stopped: no apology, no recap, and do not repeat what you already \
                               wrote.\n</system-reminder>";
/// For a reply cut off while it was all reasoning (no text, no tool call). Asking it to "continue
/// exactly" only starts another reasoning pass up to the same limit; on a hard task that repeats
/// until the run ends with nothing done. This asks it to act on what it has instead.
const ACT_PROMPT: &str = "<system-reminder>\nYour reply reached the output token limit while you were still \
                          reasoning, before you answered or took any action. Do not restart the analysis. Act on \
                          what you have now: take the next concrete step (run a command, make an edit, or give your \
                          answer), then continue from its result. If you were drafting code or text, write it to its \
                          file now, in parts if it is long, instead of drafting it again.\n</system-reminder>";
/// What happened, as the two reminders above and below say it.
const ACT_CUTOFF: &str = "Your reply reached the output token limit while you were still reasoning, before you \
                          answered or took any action.";
const EMPTY_REPLY: &str = "Your reply ended without an answer or an action: it had no text and no tool call.";
/// How much of a reply's reasoning an act reminder hands back, where the provider doesn't keep it:
/// how it began (the reading of the task) and where it stopped (the latest step).
const REASONING_HEAD: usize = 2_000;
const REASONING_TAIL: usize = 6_000;
/// For a reply that ended normally with no text and no tool call (only reasoning, or nothing).
/// Taken as the end of the turn, it leaves the work undone and the person with an empty answer:
/// seen on Terminal-Bench gpt2-codegolf, where such a reply ended unattended runs that had only
/// started (06eba3b, ce92502).
const EMPTY_REPLY_PROMPT: &str = "<system-reminder>\nYour reply ended without an answer or an action: it had \
                                  no text and no tool call. Do not restart the analysis. Act on what you have now: \
                                  take the next concrete step (run a command, make an edit, or give your answer), \
                                  then continue from its result.\n</system-reminder>";

/// Added to the second summary attempt when the first returned no summary.
const SUMMARY_RETRY: &str = "Your previous attempt returned no summary. Write the summary now, from what you \
                             already know; do not analyze further.";

/// The error of a turn that ended with no answer (its last replies were all reasoning).
const NO_ANSWER: &str = "The model stopped without an answer: its last replies had only reasoning, with no \
                         text and no tool call.";

/// What `compact_inner` returns when the model's reply held no summary.
const EMPTY_SUMMARY: &str = "the model returned an empty summary";

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            reminders: true,
            model: kodact_api::models::DEFAULT_MODEL.into(),
            fallback_models: vec![],
            max_output_tokens: 32_000,
            effort: None,
            max_thinking_tokens: None,
            max_turns: None,
            max_budget_usd: None,
            time_limit: None,
            autonomous: false,
            api_retry_wait: API_RETRY_FIRST_WAIT,
            json_schema: None,
            pricing: HashMap::new(),
            initial_context: None,
            metadata_user_id: None,
            autocompact_window: None,
            auto_compact: true,
            is_subagent: false,
            verify: None,
            edit_diagnostics: false,
            language_servers: None,
            formatters: None,
            one_shot: false,
            escalate_output: true,
            fast: false,
            nudges: Default::default(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error(
        "--max-budget-usd needs pricing for model {0}, which KodAct does not know. Add it under \"modelPricing\" in settings (input/output/cacheRead/cacheWrite USD per million tokens) or remove the budget."
    )]
    UnknownPricing(String),
}

/// The beta flag fast mode needs.
/// Sent once per turn when an unattended run ends without having used a tool (`--autonomous`).
const ATTEMPT_REMINDER: &str = "<system-reminder>\nThis run is unattended, and you are about to finish \
without having used any tool. Make a real attempt first: look at the environment, try an approach, run it and \
fix what fails. Difficulty is not a reason to stop: start with the smallest piece that works and build from \
there. Only conclude that the task can't be done after trying. If you declined for safety or policy reasons, \
keep your answer as it is.\n</system-reminder>";

pub const FAST_MODE_BETA: &str = "fast-mode-2026-02-01";
/// Fast mode's price over the model's standard rates (input and output alike).
pub const FAST_PRICE_MULTIPLIER: f64 = 2.0;

/// Settings a host may change while the engine runs.
#[derive(Debug, Clone)]
pub struct Runtime {
    pub model: String,
    pub max_thinking_tokens: Option<u32>,
    pub effort: Option<String>,
    /// Fast mode (`/fast`): faster output at a higher price, where the model supports it.
    pub fast: bool,
}

/// A cloneable handle for controlling a running engine (interrupt, mode, model).
#[derive(Clone)]
pub struct EngineHandle {
    cancel: Arc<Mutex<CancellationToken>>,
    pub permissions: Arc<RwLock<PermEngine>>,
    pub runtime: Arc<RwLock<Runtime>>,
    transcript: Arc<Transcript>,
    /// The engine's state as last published, replaced whole (C17, immediate commands).
    snapshot: Arc<RwLock<Arc<EngineSnapshot>>>,
    /// Task agents running in this engine's turn, by their Task call's `tool_use_id`:
    /// a host stops one, or all, without ending the turn (`stop_task`).
    agents: Arc<Mutex<HashMap<String, (u64, CancellationToken)>>>,
    /// Messages a host sent while the turn ran (a user line with `priority: "next"`): the
    /// model gets them at its next step, between tool calls, as Claude Code does.
    steers: Arc<Mutex<Vec<Steer>>>,
    /// A turn is running: a steered message can still reach it.
    turn_running: Arc<std::sync::atomic::AtomicBool>,
    /// The model and effort to go back to when the turn ends, while a skill's
    /// `model`/`effort` holds for it.
    turn_restore: Arc<Mutex<Option<ModelAndEffort>>>,
    /// Skills loaded in this conversation (name, text), oldest first: re-attached after compaction.
    skills_loaded: Arc<Mutex<Vec<(String, String)>>>,
}

/// A model and its effort, as a turn found them.
type ModelAndEffort = (String, Option<String>);

/// After compaction, each loaded skill keeps at most this much of its text, and all of them together this much
/// (the reference CLI's 5,000 and 25,000 tokens, at about four characters a token).
const SKILL_REATTACH_CHARS: usize = 20_000;
const SKILLS_REATTACH_CHARS: usize = 100_000;

/// What the model is told about a message sent while it works. More work is added to the
/// task list and done after the current step, so neither the new request nor the work in
/// hand is dropped; a correction or a stop applies at once.
pub const STEER_REMINDER: &str = "<system-reminder>\nThe user sent the message below while you were working. \
    If it corrects, redirects or stops the current work, follow it now. If it asks for more work, \
    add it to your task list (TodoWrite), finish the step you are on, then do it: do not drop \
    what you were doing, and do not forget the new request. If it is a question, answer it \
    briefly and carry on.\n</system-reminder>";

/// A message sent while a turn runs, waiting for the model's next step.
#[derive(Clone, Debug)]
pub struct Steer {
    /// The host's id for it, echoed when it is delivered (`PromptAccepted`).
    pub uuid: String,
    pub content: MessageContent,
}

/// A running Task agent's place in [`EngineHandle`]'s registry; dropping it removes the entry.
pub struct AgentRegistration {
    agents: Arc<Mutex<HashMap<String, (u64, CancellationToken)>>>,
    id: String,
    serial: u64,
    token: CancellationToken,
}

impl AgentRegistration {
    /// Cancelled when the user stops this agent ([`EngineHandle::stop_agent`] or `stop_agents`).
    pub fn token(&self) -> CancellationToken {
        self.token.clone()
    }
}

impl Drop for AgentRegistration {
    fn drop(&mut self) {
        let mut agents = self.agents.lock().unwrap_or_else(|e| e.into_inner());
        // A later registration under the same id is not this one's to remove.
        if agents.get(&self.id).is_some_and(|(serial, _)| *serial == self.serial) {
            agents.remove(&self.id);
        }
    }
}

impl EngineHandle {
    /// Register a running Task agent under its Task call's `tool_use_id`. The returned
    /// token fires when the user stops it; the entry goes when the registration drops.
    pub fn register_agent(&self, id: &str) -> AgentRegistration {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let token = CancellationToken::new();
        self.agents.lock().unwrap().insert(id.to_string(), (serial, token.clone()));
        AgentRegistration { agents: self.agents.clone(), id: id.to_string(), serial, token }
    }

    /// Stop one running Task agent; the turn goes on. False when no agent runs under `id`.
    pub fn stop_agent(&self, id: &str) -> bool {
        match self.agents.lock().unwrap().get(id) {
            Some((_, t)) => {
                t.cancel();
                true
            }
            None => false,
        }
    }

    /// Stop every running Task agent; the turn goes on. Returns how many were running.
    pub fn stop_agents(&self) -> usize {
        let agents = self.agents.lock().unwrap();
        agents.values().for_each(|(_, t)| t.cancel());
        agents.len()
    }

    /// The `tool_use_id`s of the Task agents running now.
    pub fn running_agents(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.agents.lock().unwrap().keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Queue a message for the running turn's next step. False when no turn runs:
    /// the host sends it as a turn of its own instead.
    pub fn steer(&self, steer: Steer) -> bool {
        let mut steers = self.steers.lock().unwrap();
        if !self.turn_running() {
            return false;
        }
        steers.push(steer);
        true
    }

    /// Take back a message not yet delivered. False when it was (or never was queued).
    pub fn cancel_steer(&self, uuid: &str) -> bool {
        let mut steers = self.steers.lock().unwrap();
        let before = steers.len();
        steers.retain(|s| s.uuid != uuid);
        steers.len() != before
    }

    /// The messages waiting, in the order sent; the queue is left empty.
    pub fn take_steers(&self) -> Vec<Steer> {
        std::mem::take(&mut *self.steers.lock().unwrap())
    }

    pub fn turn_running(&self) -> bool {
        self.turn_running.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Whether a turn runs; set by the turn itself and cleared by its host loop, which
    /// then takes what is left ([`EngineHandle::take_steers`]) as the next turn.
    pub fn set_turn_running(&self, on: bool) {
        // Under the queue's lock, so a message is either queued before this or refused.
        let _steers = self.steers.lock().unwrap();
        self.turn_running.store(on, std::sync::atomic::Ordering::SeqCst);
    }

    /// Interrupt the running turn (contract C3).
    pub fn interrupt(&self) {
        self.cancel.lock().unwrap().cancel();
    }

    /// Start interruptible work outside a turn (a `!command`): Ctrl-C and a
    /// host's `interrupt` cancel the returned token until the next turn starts.
    pub fn new_token(&self) -> CancellationToken {
        let t = CancellationToken::new();
        *self.cancel.lock().unwrap() = t.clone();
        t
    }

    /// The current turn's token. [`EngineHandle::interrupt`] cancels it until the
    /// next turn starts, so work between turns (a goal check) can watch it too.
    pub fn turn_token(&self) -> CancellationToken {
        self.cancel.lock().unwrap().clone()
    }

    pub fn set_permission_mode(&self, mode: kodact_permissions::PermissionMode) {
        self.permissions.write().unwrap().mode = mode;
        self.transcript.append_system("permission_mode", json!({"mode": mode.as_str()}));
    }

    pub fn set_model(&self, model: &str) {
        self.runtime.write().unwrap().model = kodact_api::resolve_model(model);
    }

    pub fn set_max_thinking_tokens(&self, n: Option<u32>) {
        self.runtime.write().unwrap().max_thinking_tokens = n;
    }

    pub fn set_effort(&self, effort: Option<String>) {
        self.runtime.write().unwrap().effort = effort;
    }

    /// Run the rest of this turn on `model` and/or at `effort` (a skill's
    /// frontmatter); both go back to what they were when the turn ends.
    pub fn override_for_turn(&self, model: Option<&str>, effort: Option<&str>) {
        if model.is_none() && effort.is_none() {
            return;
        }
        let mut rt = self.runtime.write().unwrap();
        self.turn_restore.lock().unwrap().get_or_insert_with(|| (rt.model.clone(), rt.effort.clone()));
        if let Some(m) = model {
            rt.model = kodact_api::resolve_model(m);
        }
        if let Some(e) = effort {
            rt.effort = Some(e.to_string());
        }
    }

    /// A skill was loaded with `text`: remembered, so compaction can put it back.
    pub fn record_skill(&self, name: &str, text: &str) {
        let mut list = self.skills_loaded.lock().unwrap();
        list.retain(|(n, _)| n != name);
        list.push((name.to_string(), text.to_string()));
    }

    /// The loaded skills, most recent first, within the re-attach limits: for the
    /// message that starts a compacted conversation.
    pub fn skills_context(&self) -> Option<String> {
        let list = self.skills_loaded.lock().unwrap();
        let mut out = String::new();
        for (name, text) in list.iter().rev() {
            let room = SKILLS_REATTACH_CHARS.saturating_sub(out.len());
            if room < 200 {
                break;
            }
            let body = kodact_tools::truncate_middle(text, SKILL_REATTACH_CHARS.min(room));
            out.push_str(&format!("\n\n### The {name} skill\n\n{body}"));
        }
        (!out.is_empty()).then(|| {
            format!(
                "Skills loaded earlier in this conversation, attached again after it was compacted; they still \
                 apply:{out}"
            )
        })
    }

    /// `/clear`: the loaded skills are forgotten with the conversation.
    pub fn forget_skills(&self) {
        self.skills_loaded.lock().unwrap().clear();
    }

    /// The turn ended: a skill's grants, removed tools, model and effort go.
    pub fn end_turn_overrides(&self) {
        self.permissions.write().unwrap().clear_turn();
        if let Some((model, effort)) = self.turn_restore.lock().unwrap().take() {
            let mut rt = self.runtime.write().unwrap();
            rt.model = model;
            rt.effort = effort;
        }
    }

    pub fn set_fast(&self, on: bool) {
        self.runtime.write().unwrap().fast = on;
    }

    pub fn runtime(&self) -> Runtime {
        self.runtime.read().unwrap().clone()
    }

    pub fn model(&self) -> String {
        self.runtime.read().unwrap().model.clone()
    }

    /// The engine's state as last published ([`Engine::publish`]). The lock is held only to
    /// clone the `Arc`.
    pub fn snapshot(&self) -> Arc<EngineSnapshot> {
        self.snapshot.read().unwrap().clone()
    }
}

/// The parts an engine is assembled from (built by `kodact-core`).
pub struct EngineParts {
    pub provider: Arc<dyn Provider>,
    pub tools: ToolRegistry,
    pub tool_ctx: ToolContext,
    pub permissions: PermEngine,
    pub hooks: HookRunner,
    pub prompter: Arc<dyn PermissionPrompter>,
    pub sink: Arc<dyn EventSink>,
    pub transcript: Arc<Transcript>,
    pub history: Arc<FileHistory>,
    pub system: Vec<SystemBlock>,
}

/// Persists an accepted `PermissionUpdate` (SDK shape) to settings.
pub type PermissionUpdateHandler = Box<dyn Fn(&Value) + Send + Sync>;

/// State shared with concurrently running tool pipelines.
pub(crate) struct Shared {
    pub provider: Arc<dyn Provider>,
    pub tools: ToolRegistry,
    pub tool_ctx: ToolContext,
    pub permissions: Arc<RwLock<PermEngine>>,
    pub hooks: HookRunner,
    pub prompter: Arc<dyn PermissionPrompter>,
    /// Serializes permission prompts (contract C2).
    pub prompt_lock: tokio::sync::Mutex<()>,
    pub sink: Arc<dyn EventSink>,
    pub transcript: Arc<Transcript>,
    pub history: Arc<FileHistory>,
    /// Persists accepted permission updates (`updatedPermissions`) to settings.
    pub on_permission_update: Mutex<Option<PermissionUpdateHandler>>,
    /// Injection notes already given (tool and source), so the same file or
    /// page isn't flagged again on every read.
    pub injection_noted: Mutex<HashSet<String>>,
}

/// Conversation state that persists across turns.
#[derive(Debug, Default, Clone)]
pub struct TurnState {
    pub messages: Vec<Message>,
    /// Transcript uuid of each message (same index as `messages`).
    pub uuids: Vec<String>,
    /// Uuids of synthetic user messages (reminders, interrupt markers, summaries).
    pub meta: HashSet<String>,
    pub microcompacted: HashSet<String>,
    pub total_usage: Usage,
    pub total_cost_usd: f64,
    pub model_usage: Map<String, Value>,
    /// The context size the last response reported (0: the server reports none).
    pub context_tokens: u64,
    /// How many of `messages` `context_tokens` covers; later ones are estimated.
    pub counted: usize,
    /// Models used without a known price (their cost counts as 0).
    pub unpriced: std::collections::BTreeSet<String>,
    /// The model the conversation's thinking blocks came from. Another
    /// model (or another provider) can't verify their signatures, so they
    /// are dropped when the model changes; the words and tool calls stay.
    pub thinking_model: Option<String>,
}

/// Outcome of one [`Engine::submit`].
#[derive(Debug, Clone)]
pub struct TurnResult {
    pub subtype: ResultSubtype,
    pub is_error: bool,
    pub result: Option<String>,
    pub stop_reason: Option<String>,
    pub num_turns: u32,
    pub duration_ms: u64,
    pub duration_api_ms: u64,
    pub usage: Usage,
    pub total_cost_usd: f64,
    pub model_usage: Map<String, Value>,
    pub permission_denials: Vec<PermissionDenial>,
    pub errors: Vec<String>,
    pub structured_output: Option<Value>,
    /// The user's prompt was blocked by a UserPromptSubmit hook (shown to the user, not the model).
    pub prompt_blocked: Option<String>,
    /// The turn failed in a way the next turn would too: credentials, billing,
    /// a missing model, or a conversation too long even after compaction.
    pub fatal: bool,
    /// The endpoint refused the credentials (401/403) or none were set: print
    /// mode exits with the configuration status.
    pub auth_failed: bool,
    /// Tool calls the model made in this turn.
    pub tool_calls: u32,
    /// Where the turn's changes stand: checked, failed, or not run (`None` with the
    /// verification loop off).
    pub verification: Option<crate::verify::Verified>,
}

pub struct Engine {
    pub cfg: EngineConfig,
    pub(crate) shared: Arc<Shared>,
    pub state: TurnState,
    handle: EngineHandle,
    system: Vec<SystemBlock>,
    session_started: bool,
    /// Memory and environment context, re-attached after compaction.
    session_context: Option<String>,
    /// The person's requests from the parts of the conversation compaction replaced, oldest
    /// first: each later compaction carries them on, word for word.
    carried_requests: Vec<String>,
    /// The run reached its `--max-time` limit: the turn that was cut ends as the limit, not as
    /// an interruption.
    time_up: Arc<std::sync::atomic::AtomicBool>,
    verify: crate::verify::Tracker,
    guard: crate::stuck::LoopGuard,
    /// This turn's progress, for the pacing nudges.
    progress: crate::progress::Progress,
    /// What this turn's request names: its files and commands.
    named: crate::named::Named,
    /// The named files on disk, watched for progress.
    watched: crate::named::Watched,
    /// Nudges silenced for the rest of the run (`nudges.backOff`).
    back_off: crate::progress::BackOff,
    /// The shell-edit suggestion was given this turn.
    shell_edit_told: bool,
    /// Shell commands run this turn, for the constraint report.
    ran_commands: Vec<String>,
    /// The constraint report was given this turn.
    constraints_told: bool,
    /// The latest version that passed a check, saved with the Checkpoint tool
    /// (`nudges.passingSnapshots`): its checkpoint label, the check, and the file writes then.
    passing: Option<(String, String, usize)>,
    /// The model was told this turn where the passing version is.
    passing_told: bool,
    /// The limits this turn's request states (`nudges.limitVerdicts`, `nudges.staleDeliverables`).
    limits: crate::limits::Limits,
    /// A measurement that met every limit for its row, with the files the request asks for
    /// unchanged since: (step, command, line, row).
    unsaved: Option<(u32, String, String, Option<String>)>,
    /// Stale-deliverable notes given this turn, and whether the end-of-turn one was.
    stale_told: u32,
    stale_end_told: bool,
    /// Size-limit notes given this turn, per file (`nudges.constraints`).
    size_told: HashMap<String, u32>,
    /// The session's language servers, started on first use ([`EngineConfig::language_servers`]).
    lsp: Option<Arc<crate::lsp::Manager>>,
    /// This turn's raised output cap, after a `max_tokens` stop.
    output_cap: Option<u32>,
    /// The most reply a request asks for after a refusal for length whose prompt fit the window
    /// (`shrink_reply`); cleared when the turn starts and when a compaction makes room.
    reply_shrink: Option<u32>,
    /// The reasoning level of this turn's requests, when it adapts (`nudges.adaptiveEffort`).
    effort_loop: Option<crate::effort_loop::EffortLoop>,
    /// Tokens the last reply spent thinking, for judging its step once its tools have run.
    last_thought: u64,
    /// After `/clear`: attach the memory context to the next prompt again.
    reattach_context: bool,
    /// Notes for the model, attached to the next prompt (`/add-dir`, ...).
    reminders: Vec<String>,
    /// SessionStart's `source`: startup, resume or clear.
    start_source: String,
    /// The last prompt went out in plan mode: when the mode changes, the
    /// model is told plan mode is over.
    planning: bool,
}

/// A person's prompt in the conversation: a point `/rewind` can go back to.
#[derive(Debug, Clone, PartialEq)]
pub struct PromptPoint {
    /// Index in `TurnState::messages`.
    pub index: usize,
    /// Transcript uuid (the file-history turn id).
    pub uuid: String,
    pub text: String,
    /// Files changed at or after this prompt.
    pub changed_files: Vec<std::path::PathBuf>,
}

/// What a compaction did.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactInfo {
    pub trigger: String,
    pub pre_tokens: u64,
    pub summary: String,
}

/// Per-turn accounting.
#[derive(Default)]
pub(crate) struct TurnAcc {
    pub api_calls: u32,
    pub tool_calls: u32,
    pub api_ms: u64,
    pub usage: Usage,
    pub denials: Vec<PermissionDenial>,
    pub errors: Vec<String>,
    pub stop: Option<String>,
}

enum StreamOutcome {
    Done(ApiMessage),
    Interrupted(Option<ApiMessage>),
    Failed(ApiError),
    /// The reply was stopped while it streamed: it was one passage repeated ([`crate::repeat`]).
    Repeating(crate::repeat::Repeating),
}

impl Engine {
    pub fn new(cfg: EngineConfig, parts: EngineParts) -> Result<Self, EngineError> {
        if cfg.max_budget_usd.is_some() {
            for m in std::iter::once(&cfg.model).chain(cfg.fallback_models.iter()) {
                if model_info(m).is_none() && !cfg.pricing.contains_key(m) {
                    return Err(EngineError::UnknownPricing(m.clone()));
                }
            }
        }
        let runtime = Runtime {
            model: cfg.model.clone(),
            max_thinking_tokens: cfg.max_thinking_tokens,
            effort: cfg.effort.clone(),
            fast: cfg.fast,
        };
        let permissions = Arc::new(RwLock::new(parts.permissions));
        let handle = EngineHandle {
            cancel: Arc::new(Mutex::new(CancellationToken::new())),
            permissions: permissions.clone(),
            runtime: Arc::new(RwLock::new(runtime)),
            transcript: parts.transcript.clone(),
            snapshot: Default::default(),
            agents: Default::default(),
            steers: Default::default(),
            turn_running: Default::default(),
            turn_restore: Default::default(),
            skills_loaded: Default::default(),
        };
        let shared = Arc::new(Shared {
            provider: parts.provider,
            tools: parts.tools,
            tool_ctx: parts.tool_ctx,
            permissions,
            hooks: parts.hooks,
            prompter: parts.prompter,
            prompt_lock: tokio::sync::Mutex::new(()),
            sink: parts.sink,
            transcript: parts.transcript,
            history: parts.history,
            on_permission_update: Mutex::new(None),
            injection_noted: Mutex::new(HashSet::new()),
        });
        let session_context = cfg.initial_context.clone();
        Ok(Engine {
            cfg,
            shared,
            state: TurnState::default(),
            handle,
            system: parts.system,
            session_started: false,
            session_context,
            verify: Default::default(),
            guard: Default::default(),
            progress: Default::default(),
            named: Default::default(),
            watched: Default::default(),
            back_off: Default::default(),
            shell_edit_told: false,
            ran_commands: vec![],
            constraints_told: false,
            passing: None,
            passing_told: false,
            limits: Default::default(),
            unsaved: None,
            stale_told: 0,
            stale_end_told: false,
            size_told: HashMap::new(),
            output_cap: None,
            reply_shrink: None,
            effort_loop: None,
            last_thought: 0,
            reattach_context: false,
            carried_requests: vec![],
            lsp: None,
            time_up: Default::default(),
            reminders: vec![],
            start_source: "startup".into(),
            planning: false,
        })
    }

    pub fn handle(&self) -> EngineHandle {
        self.handle.clone()
    }

    pub fn tools(&self) -> &ToolRegistry {
        &self.shared.tools
    }

    /// The model provider, for side requests (titles, summaries) outside the conversation.
    pub fn provider(&self) -> Arc<dyn Provider> {
        self.shared.provider.clone()
    }

    /// Add a user message to the conversation without running a turn
    /// (`!command` output when the model shouldn't answer it).
    pub fn add_user_note(&mut self, text: impl Into<String>) {
        self.push_user(Message::user_text(text.into()), false, None, true);
    }

    /// The session goes on in a rebuilt engine (`/reload-*`): no SessionStart.
    pub fn skip_session_start(&mut self) {
        self.session_started = true;
    }

    /// Attach the memory and environment context to the next prompt again
    /// (the session moved to another directory).
    pub fn reattach_context(&mut self) {
        self.reattach_context = self.session_context.is_some();
    }

    /// What SessionStart hooks see as the `source` (`startup`, `resume`, `clear`).
    pub fn set_start_source(&mut self, source: &str) {
        self.start_source = source.to_string();
    }

    /// Notes still waiting for the next prompt (a session switch carries them over).
    pub fn take_reminders(&mut self) -> Vec<String> {
        std::mem::take(&mut self.reminders)
    }

    /// Start from another engine's conversation (a forked sub-agent, `/subtask`): its messages,
    /// cleared tool results and context size. Not its spend, which stays with the session that
    /// made it, and not its transcript chain or task list, unlike [`Engine::restore`].
    pub fn seed(&mut self, from: &TurnState) {
        self.state.messages = from.messages.clone();
        self.state.uuids = from.uuids.clone();
        self.state.meta = from.meta.clone();
        self.state.microcompacted = from.microcompacted.clone();
        // Automatic compaction measures this before the first request.
        self.state.context_tokens = from.context_tokens;
        self.state.counted = from.counted;
        self.session_started = !self.state.messages.is_empty();
    }

    /// Give the model the messages sent while the turn ran, each as a user message of its
    /// own (said to come mid-task), and tell the host each one landed.
    fn deliver_steers(&mut self) {
        for steer in self.handle.take_steers() {
            let mut blocks = steer.content.clone().into_blocks();
            blocks.insert(0, ContentBlock::text(STEER_REMINDER));
            let msg = Message::user(blocks);
            self.push_user(msg, false, None, false);
            // The host's own id and words, so it can tell which of its messages landed.
            self.emit(EngineEvent::PromptAccepted {
                message: Message::user(steer.content.into_blocks()),
                uuid: steer.uuid,
            });
        }
    }

    /// Tell the model something with the next prompt (a system reminder).
    pub fn remind(&mut self, text: impl Into<String>) {
        self.reminders.push(text.into());
    }

    /// The context the next request will carry: the last size the server reported plus an
    /// estimate for messages added since. With no reported size (some servers send no usage),
    /// an estimate of the whole request.
    pub fn projected_context_tokens(&self) -> u64 {
        let msgs = &self.state.messages;
        if self.state.context_tokens == 0 {
            if msgs.is_empty() {
                return 0;
            }
            let system: usize = self.system.iter().map(|b| b.text.len()).sum();
            let tools: usize =
                self.shared.tools.specs().iter().map(|t| t.description.len() + t.input_schema.to_string().len()).sum();
            return self.estimate_sent(msgs) + ((system + tools) / 4) as u64;
        }
        let from = self.state.counted.min(msgs.len());
        self.state.context_tokens + self.estimate_sent(&msgs[from..])
    }

    /// The reply budget of the next request to `model`: the output cap, but no more than the
    /// room the model's context window has left after the prompt ([`fit_reply`]). The window is
    /// the model's own (its limits, `modelLimits`, `KODACT_CONTEXT_WINDOW`, what the endpoint
    /// reports), never the `--autocompact` window, which may be set lower to compact early.
    fn reply_budget(&self, model: &str) -> u32 {
        let cap = self.output_cap.unwrap_or(self.cfg.max_output_tokens);
        let window = kodact_api::models::model_info_or_default(model).context_window;
        let fitted = fit_reply(cap, window, self.projected_context_tokens());
        self.reply_shrink.map_or(fitted, |s| fitted.min(s))
    }

    /// The window compaction is measured against, for `model`.
    pub fn context_window(&self, model: &str) -> u64 {
        self.compact_window(model)
    }

    /// The context size at which automatic compaction runs, for `model`.
    pub fn autocompact_at(&self, model: &str) -> u64 {
        kodact_compact::autocompact_threshold(self.compact_window(model), self.cfg.max_output_tokens)
    }

    /// The model provider's short name (`messages`, `openai`, `mock`).
    pub fn provider_name(&self) -> &str {
        self.shared.provider.name()
    }

    pub fn tool_ctx(&self) -> &ToolContext {
        &self.shared.tool_ctx
    }

    pub fn transcript(&self) -> &Arc<Transcript> {
        &self.shared.transcript
    }

    pub fn history(&self) -> &Arc<FileHistory> {
        &self.shared.history
    }

    pub fn hooks(&self) -> &HookRunner {
        &self.shared.hooks
    }

    /// The prompter tools of this engine ask through (share it with sub-agents).
    pub fn prompter(&self) -> Arc<dyn PermissionPrompter> {
        self.shared.prompter.clone()
    }

    pub fn system(&self) -> &[SystemBlock] {
        &self.system
    }

    pub fn set_system(&mut self, system: Vec<SystemBlock>) {
        self.system = system;
    }

    pub fn set_permission_update_handler(&self, f: PermissionUpdateHandler) {
        *self.shared.on_permission_update.lock().unwrap() = Some(f);
    }

    /// Load a resumed conversation (contract C5).
    pub fn restore(&mut self, loaded: &kodact_session::LoadedSession) {
        self.state.messages = loaded.messages.iter().map(|e| e.message.clone()).collect();
        self.state.uuids = loaded.messages.iter().map(|e| e.uuid.clone()).collect();
        self.state.meta = loaded.messages.iter().filter(|e| e.is_meta).map(|e| e.uuid.clone()).collect();
        self.state.microcompacted = loaded.microcompacted.clone();
        self.state.thinking_model = loaded.model.clone();
        // A resumed session still gets SessionStart (source "resume") with its next
        // prompt; a sub-agent's context is already in its messages.
        if self.cfg.is_subagent {
            self.session_started = !self.state.messages.is_empty();
        }
        self.shared.transcript.continue_from(loaded);
        self.carried_requests = loaded.requests.clone().unwrap_or_default();
        if loaded.cut_off && !self.cfg.is_subagent {
            self.recover_cut_off_turn();
        }
        // The plan survives a resume: the last TodoWrite list becomes the current one.
        let last_todos = self.state.messages.iter().rev().flat_map(|m| m.tool_uses().collect::<Vec<_>>()).find_map(
            |(_, name, input)| {
                (name == "TodoWrite").then(|| input.get("todos").and_then(Value::as_array).cloned()).flatten()
            },
        );
        if let Some(todos) = last_todos.or_else(|| loaded.todos.clone()) {
            *self.shared.tool_ctx.todos.lock().unwrap() = todos;
        }
    }

    /// The resumed conversation's last turn never ended: KodAct was killed or crashed in it
    /// (GOALS pillar 2). The model is told with the next prompt which calls have no result,
    /// so it checks what they did instead of trusting or blindly repeating them.
    fn recover_cut_off_turn(&mut self) {
        let answered: HashSet<&str> = self
            .state
            .messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();
        let pending: Vec<String> = self
            .state
            .messages
            .iter()
            .flat_map(|m| m.tool_uses().collect::<Vec<_>>())
            .filter(|(id, _, _)| !answered.contains(id))
            .map(|(_, name, _)| name.to_string())
            .collect();
        let mut text = String::from(
            "The previous turn of this conversation did not end: KodAct was stopped or crashed while it was \
             running, so the work it was doing may be half done.",
        );
        if !pending.is_empty() {
            text.push_str(&format!(
                " These tool calls were under way and returned nothing: {}. Each may have run fully, partly \
                 or not at all.",
                pending.join(", ")
            ));
        }
        text.push_str(
            " Before continuing, look at the current state (the files concerned, `git status` in a repository, \
             any process that should be running) and carry on from what is really there; don't assume a step \
             was done, and don't repeat one that was.",
        );
        self.reminders.push(text);
        self.notice(
            NoticeLevel::Warning,
            "The last turn of this session was cut off (KodAct was stopped or crashed mid-turn). The next prompt \
             continues from the state it left.",
        );
        self.system_event("turn_recovered", json!({"pending": pending}));
    }

    /// The model's notes (the Notes tool), for re-attaching after compaction (GOALS pillar 5):
    /// in full within a share of the window (a twentieth of its tokens, half of what the
    /// carried requests get), otherwise their end, which is the latest, with the way to read
    /// the rest.
    pub(crate) fn notes_context(&self, window: u64) -> Option<String> {
        let notes = self.shared.tool_ctx.notes_text();
        if notes.trim().is_empty() {
            return None;
        }
        // About four characters to a token.
        let room = usize::try_from(window / 20 * 4).unwrap_or(usize::MAX).max(2_000);
        let total = notes.chars().count();
        let body = if total > room {
            let tail: String = notes.chars().skip(total - room).collect();
            let tail = tail.split_once('\n').map(|(_, rest)| rest.to_string()).unwrap_or(tail);
            format!("(the earlier notes are left out here; `Notes` with action `read` returns all of them)\n{tail}")
        } else {
            notes
        };
        Some(format!("Your notes (the Notes tool) at the time of compaction. Keep them up to date:\n{body}"))
    }

    /// The current task list, for re-attaching after compaction (GOALS pillar 5).
    fn plan_context(&self) -> Option<String> {
        let todos = self.shared.tool_ctx.todos.lock().unwrap().clone();
        if todos.is_empty() {
            return None;
        }
        let lines: Vec<String> = todos
            .iter()
            .map(|t| {
                format!(
                    "- [{}] {}",
                    t.get("status").and_then(Value::as_str).unwrap_or("pending"),
                    t.get("content").and_then(Value::as_str).unwrap_or_default()
                )
            })
            .collect();
        Some(format!(
            "Your task list (TodoWrite) at the time of compaction. Keep it up to date as you continue:\n{}",
            lines.join("\n")
        ))
    }

    pub fn emit(&self, ev: EngineEvent) {
        self.shared.sink.emit(ev);
    }

    fn notice(&self, level: NoticeLevel, text: impl Into<String>) {
        self.emit(EngineEvent::Notice { level, text: text.into() });
    }

    fn mode_str(&self) -> &'static str {
        self.shared.permissions.read().unwrap().mode.as_str()
    }

    /// KodAct knows `model`'s price (built in, or `modelPricing`).
    pub fn has_price(&self, model: &str) -> bool {
        self.pricing_for(model).is_some()
    }

    fn pricing_for(&self, model: &str) -> Option<Pricing> {
        self.cfg.pricing.get(model).copied().or_else(|| model_info(model).map(Pricing::from_model))
    }

    /// Record a message in state and transcript; returns its uuid.
    pub(crate) fn push_user(
        &mut self,
        msg: Message,
        is_meta: bool,
        tool_use_result: Option<Value>,
        emit: bool,
    ) -> String {
        let extra = match &tool_use_result {
            Some(v) => json!({"toolUseResult": v}),
            None => json!({}),
        };
        let uuid = self.shared.transcript.append_user(&msg, is_meta, extra);
        if emit {
            self.emit(EngineEvent::User {
                message: msg.clone(),
                uuid: uuid.clone(),
                tool_use_result,
                is_meta,
                parent_tool_use_id: None,
            });
        }
        if is_meta {
            self.state.meta.insert(uuid.clone());
        }
        self.state.messages.push(msg);
        self.state.uuids.push(uuid.clone());
        uuid
    }

    fn push_assistant(&mut self, msg: ApiMessage) -> String {
        let uuid = self.shared.transcript.append_assistant(&msg);
        self.emit(EngineEvent::Assistant { message: msg.clone(), uuid: uuid.clone(), parent_tool_use_id: None });
        self.state.messages.push(msg.to_message());
        self.state.uuids.push(uuid.clone());
        uuid
    }

    /// The conversation goes on with the current model: when it is another
    /// one than the model that wrote the thinking blocks so far, those blocks
    /// are dropped (another model or provider rejects their signatures),
    /// and everything else is kept, so the new model sees the whole chat.
    pub fn adopt_model(&mut self) {
        let model = self.handle.model();
        if self.state.thinking_model.as_deref().is_some_and(|m| !same_model(m, &model)) {
            for m in &mut self.state.messages {
                m.content
                    .retain(|b| !matches!(b, ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }));
                // What the last model attached (Gemini's thought signatures) is
                // that model's: another one refuses it. Calls without one get
                // the wire's placeholder where the new model needs it.
                for b in &mut m.content {
                    if let ContentBlock::ToolUse { provider, .. } | ContentBlock::Text { provider, .. } = b {
                        *provider = None;
                    }
                }
            }
            // A message left empty stays in place (its uuid lines up with
            // it); requests skip empty messages.
        }
        self.state.thinking_model = Some(model);
    }

    /// Tell the host about something (`system/<subtype>`) without recording it.
    pub fn announce(&self, subtype: &str, data: Value) {
        self.emit(EngineEvent::System { subtype: subtype.into(), data });
    }

    /// TodoWrite items not marked completed, as "[status] content".
    fn unfinished_todos(&self) -> Vec<String> {
        self.shared
            .tool_ctx
            .todos
            .lock()
            .unwrap()
            .iter()
            .filter_map(|t| {
                let status = t.get("status").and_then(Value::as_str).unwrap_or("pending");
                let content = t.get("content").and_then(Value::as_str).unwrap_or("");
                (status != "completed" && !content.is_empty()).then(|| format!("[{status}] {content}"))
            })
            .collect()
    }

    /// How long to wait before trying a failed model call again, or `None` to give up, in every
    /// run. A stream that broke before its first event counts as a lost connection; an endpoint
    /// that says it is down for maintenance is waited out; and a "model not found" after the
    /// model has already answered in this run is the endpoint losing it for a moment, not a
    /// wrong name. A spent quota is not waited for (`is_retryable` says no).
    pub(crate) fn api_retry_wait(&self, e: &ApiError, retries: u32) -> Option<Duration> {
        let answered = || self.state.messages.iter().any(|m| m.role == kodact_types::Role::Assistant);
        let passes = e.is_retryable()
            || e.is_outage()
            || (e.is_model_missing() && answered())
            || matches!(e, ApiError::Parse(m) if m.starts_with("stream ended"));
        // Every run, attended or not: an outage or a 429 is waited out (Esc stops the wait). An
        // endpoint that can't be reached before it ever answered is a setting to fix, not an
        // outage to wait for, in a session with a person at the prompt: said at once. A script
        // (`-p`, `--autonomous`) has nobody to fix it at once and is often one of many runs, so
        // a network that drops for a moment (a name lookup that fails, a VPN reconnecting) is
        // waited out there; a wrong URL still ends the run after the retries.
        let unattended = self.cfg.autonomous || self.cfg.one_shot;
        let unreached = !answered() && matches!(e, ApiError::Network { .. });
        if !passes || (!unattended && unreached) {
            return None;
        }
        // A `-p` run that never reached the endpoint: a few short tries (about half a minute),
        // enough for a network that drops for a moment, not minutes on a mistyped URL.
        if unreached && !self.cfg.autonomous && retries >= FIRST_CONTACT_RETRIES {
            return None;
        }
        // KODACT_MAX_RETRIES, the person's own cap on retries, holds here too (0: none).
        if let Some(max) = std::env::var("KODACT_MAX_RETRIES").ok().and_then(|v| v.trim().parse::<u32>().ok()) {
            if retries >= max {
                return None;
            }
        }
        // The endpoint asks for longer than a retry sleeps. Sooner, it refuses again: ten tries a
        // minute apart would spend ten minutes to show the same error. So the wait is taken in
        // full or not at all. A person at the prompt is told how long and decides; an unattended
        // run waits when the wait fits its time (`--max-time`), or, with no limit set, the time
        // its retries would have taken anyway.
        if let Some(asked) = e.long_wait() {
            let fits = match &self.cfg.time_limit {
                Some(t) => t.remaining() > asked + API_RETRY_RESERVE,
                None => self.cfg.autonomous && asked <= API_RETRY_MAX_WAIT * API_RETRY_ATTEMPTS,
            };
            return fits.then_some(asked);
        }
        let backoff = self.cfg.api_retry_wait.saturating_mul(1 << retries.min(6));
        let retry_after = match e {
            ApiError::Http { retry_after: Some(d), .. } => *d,
            _ => Duration::ZERO,
        };
        // Spread the retries of runs that failed together (they share the endpoint's limits).
        let nanos =
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().subsec_nanos();
        let jitter = backoff.mul_f64(f64::from(nanos % 1000) / 4000.0);
        let wait = (backoff + jitter).max(retry_after).min(API_RETRY_MAX_WAIT);
        match &self.cfg.time_limit {
            Some(t) => (retries < API_RETRY_MAX_ATTEMPTS && t.remaining() > wait + API_RETRY_RESERVE).then_some(wait),
            None => (retries < API_RETRY_ATTEMPTS).then_some(wait),
        }
    }

    fn system_event(&self, subtype: &str, data: Value) {
        self.shared.transcript.append_system(subtype, data.clone());
        self.emit(EngineEvent::System { subtype: subtype.into(), data });
    }

    /// The tools the model is offered: all of them but those a running skill
    /// took away for this turn (`disallowed-tools`).
    fn offered_tools(&self) -> Vec<kodact_types::ToolSpec> {
        let perms = self.shared.permissions.read().unwrap();
        let mut specs = self.shared.tools.specs();
        if !perms.turn_removed.is_empty() {
            specs.retain(|s| !perms.removed_this_turn(&s.name));
        }
        specs
    }

    fn build_request(&self, model: &str) -> MessagesRequest {
        let mut rt = self.handle.runtime.read().unwrap().clone();
        if let Some(l) = &self.effort_loop {
            rt.effort = Some(l.level().to_string());
        }
        crate::snapshot::assemble(
            model,
            &rt,
            RequestParts {
                messages: &self.state.messages,
                microcompacted: &self.state.microcompacted,
                system: &self.system,
                tools: self.offered_tools(),
                max_output_tokens: self.reply_budget(model),
                json_schema: self.cfg.json_schema.as_ref(),
                metadata_user_id: self.cfg.metadata_user_id.as_deref(),
                provider_name: self.shared.provider.name(),
            },
        )
    }

    /// Publish the state for readers outside the turn ([`EngineHandle::snapshot`]): the driver
    /// calls this while idle; the engine itself after each model call, each tool batch and at
    /// the end of a turn.
    pub fn publish(&self) {
        self.publish_with(None);
    }

    fn publish_with(&self, turn: Option<TurnProgress>) {
        // Nobody reads a sub-agent's snapshot (its caller reports for it): skip the copy.
        if self.cfg.is_subagent {
            return;
        }
        let snap = EngineSnapshot {
            messages: Arc::new(self.state.messages.clone()),
            microcompacted: Arc::new(self.state.microcompacted.clone()),
            system: Arc::new(self.system.clone()),
            tools: Arc::new(self.shared.tools.specs()),
            total_usage: self.state.total_usage.clone(),
            total_cost_usd: self.state.total_cost_usd,
            model_usage: self.state.model_usage.clone(),
            unpriced: self.state.unpriced.iter().cloned().collect(),
            context_tokens: self.projected_context_tokens(),
            turn,
            provider_name: self.shared.provider.name().to_string(),
            max_output_tokens: self.cfg.max_output_tokens,
            json_schema: self.cfg.json_schema.clone(),
            metadata_user_id: self.cfg.metadata_user_id.clone(),
            autocompact_window: self.cfg.autocompact_window,
            auto_compact: self.cfg.auto_compact,
        };
        *self.handle.snapshot.write().unwrap() = Arc::new(snap);
    }

    fn publish_turn(&self, started: Instant, turn: &TurnAcc) {
        self.publish_with(Some(TurnProgress {
            started,
            api_calls: turn.api_calls,
            api_ms: turn.api_ms,
            tool_calls: turn.tool_calls,
        }));
    }

    /// Stream one model response, emitting events. Keeps a usable partial on interrupt.
    /// One model call of the turn. `watch`: stop a reply that repeats itself as it streams.
    async fn stream_once(&self, req: MessagesRequest, cancel: &CancellationToken, watch: bool) -> StreamOutcome {
        self.stream_inner(req, cancel, true, watch).await
    }

    async fn stream_inner(
        &self,
        req: MessagesRequest,
        cancel: &CancellationToken,
        emit: bool,
        watch: bool,
    ) -> StreamOutcome {
        let mut stream = match self.shared.provider.stream(req, cancel.clone()).await {
            Ok(s) => s,
            Err(ApiError::Cancelled) => return StreamOutcome::Interrupted(None),
            Err(e) => return StreamOutcome::Failed(e),
        };
        let mut acc = MessageAccumulator::new();
        let mut stopped: HashSet<usize> = HashSet::new();
        let mut watcher = crate::repeat::Watcher::default();
        loop {
            let next = tokio::select! {
                n = stream.next() => n,
                _ = cancel.cancelled() => Some(Err(ApiError::Cancelled)),
            };
            match next {
                Some(Ok(ev)) => {
                    if let StreamEvent::ContentBlockStop { index } = &ev {
                        stopped.insert(*index);
                    }
                    if watch {
                        let repeating = match &ev {
                            StreamEvent::ContentBlockStart { .. } => {
                                watcher.reset();
                                None
                            }
                            StreamEvent::ContentBlockDelta {
                                delta: kodact_types::Delta::TextDelta { text }, ..
                            } => watcher.push(text, false),
                            StreamEvent::ContentBlockDelta {
                                delta: kodact_types::Delta::ThinkingDelta { thinking },
                                ..
                            } => watcher.push(thinking, true),
                            _ => None,
                        };
                        // Dropping the stream ends the request.
                        if let Some(r) = repeating {
                            return StreamOutcome::Repeating(r);
                        }
                    }
                    if let Err(e) = acc.push(&ev) {
                        return StreamOutcome::Failed(e);
                    }
                    if emit {
                        self.emit(EngineEvent::Stream { event: ev, parent_tool_use_id: None });
                    }
                }
                Some(Err(ApiError::Cancelled)) => {
                    let partial = acc.snapshot().cloned().map(|mut m| {
                        // Keep text and finished tool calls only (contract C3).
                        let mut i = 0;
                        m.content.retain(|b| {
                            let keep = match b {
                                ContentBlock::ToolUse { .. } => stopped.contains(&i),
                                ContentBlock::Text { text, .. } => !text.is_empty(),
                                ContentBlock::Thinking { signature, .. } => {
                                    !signature.is_empty() && stopped.contains(&i)
                                }
                                _ => stopped.contains(&i),
                            };
                            i += 1;
                            keep
                        });
                        m.stop_reason = None;
                        m
                    });
                    return StreamOutcome::Interrupted(partial.filter(|m| !m.content.is_empty()));
                }
                Some(Err(e)) => return StreamOutcome::Failed(e),
                None => break,
            }
        }
        match acc.finish() {
            Ok(m) => StreamOutcome::Done(m),
            Err(e) => StreamOutcome::Failed(e),
        }
    }

    /// Requests for `model` go out in fast mode: it's on and the model offers it.
    fn sends_fast(&self, model: &str) -> bool {
        crate::snapshot::sends_fast(&self.handle.runtime(), model, self.shared.provider.name())
    }

    /// Take the conversation's size from a reply in it, which is pushed right after: what its
    /// request held, plus what the next request carries of the reply. Its reasoning counts only
    /// when the provider sends reasoning back. Over chat completions it is gone after the reply,
    /// and counting it compacted conversations at a quarter of the window (regex-chess, 9ef0955:
    /// 24k tokens in the request plus a 32k reply that was all reasoning, counted as 56k).
    fn measure_context(&mut self, model: &str, reply: &ApiMessage) {
        let u = &reply.usage;
        let input = u.input_tokens + u.cache_read_input_tokens + u.cache_creation_input_tokens;
        // A server that reports no input tokens reports no usable size: 0 makes
        // `projected_context_tokens` estimate the whole request instead.
        self.state.context_tokens = if input == 0 {
            0
        } else if self.shared.provider.resends_reasoning(model) {
            input + u.output_tokens
        } else {
            // The reply's text and calls, as their share of its output (by characters). With no
            // reasoning in it, that's all of the output, as before.
            let msg = [reply.to_message()];
            let all = kodact_compact::estimate_sent_tokens(&msg, true);
            let sent = kodact_compact::estimate_sent_tokens(&msg, false);
            // Reasoning the endpoint counted but never sent isn't in the reply's text either:
            // leave it out before sharing the rest by characters.
            let shown =
                if all == sent { u.output_tokens.saturating_sub(u.reasoning_output_tokens) } else { u.output_tokens };
            let carried = if all == 0 { shown } else { (shown as u128 * sent as u128 / all as u128) as u64 };
            input + carried
        };
        self.state.counted = self.state.messages.len() + 1;
    }

    /// Estimate `msgs` as a request carries them: without reasoning when the provider leaves it out.
    fn estimate_sent(&self, msgs: &[Message]) -> u64 {
        kodact_compact::estimate_sent_tokens(msgs, self.shared.provider.resends_reasoning(&self.handle.model()))
    }

    /// Count a request's tokens and cost toward the session. The conversation's size is
    /// [`Self::measure_context`]'s: a summary or a side question isn't the conversation.
    fn record_usage(&mut self, model: &str, usage: &Usage, turn: &mut TurnAcc) {
        turn.usage.add(usage);
        self.state.total_usage.add(usage);
        let pricing = self.pricing_for(model);
        if pricing.is_none() && (usage.input_tokens > 0 || usage.output_tokens > 0) {
            self.state.unpriced.insert(model.to_string());
        }
        let mut cost = pricing.map(|p| p.cost(usage)).unwrap_or(0.0);
        if self.sends_fast(model) {
            cost *= FAST_PRICE_MULTIPLIER;
        }
        self.state.total_cost_usd += cost;
        let window = self.compact_window(model);
        let e = self.state.model_usage.entry(model.to_string()).or_insert_with(|| {
            json!({"inputTokens": 0, "outputTokens": 0, "cacheReadInputTokens": 0, "cacheCreationInputTokens": 0,
                   "webSearchRequests": 0, "costUSD": 0.0, "contextWindow": window})
        });
        let add = |e: &mut Value, k: &str, n: u64| e[k] = json!(e[k].as_u64().unwrap_or(0) + n);
        add(e, "inputTokens", usage.input_tokens);
        add(e, "outputTokens", usage.output_tokens);
        add(e, "cacheReadInputTokens", usage.cache_read_input_tokens);
        add(e, "cacheCreationInputTokens", usage.cache_creation_input_tokens);
        if let Some(n) =
            usage.server_tool_use.as_ref().and_then(|s| s.get("web_search_requests")).and_then(Value::as_u64)
        {
            add(e, "webSearchRequests", n);
        }
        e["costUSD"] = json!(e["costUSD"].as_f64().unwrap_or(0.0) + cost);
    }

    fn compact_window(&self, model: &str) -> u64 {
        self.cfg.autocompact_window.unwrap_or_else(|| kodact_api::models::model_info_or_default(model).context_window)
    }

    /// Micro-compact and, if the window is nearly full, summarize (contract C9).
    async fn maybe_compact(
        &mut self,
        model: &str,
        continue_work: bool,
        cancel: &CancellationToken,
        turn: &mut TurnAcc,
    ) {
        let window = self.compact_window(model);
        let max_out = self.cfg.max_output_tokens;
        // What the next request will hold: the last reported size plus everything added since
        // (a batch of large tool results can push it over the window in one step).
        let used = self.projected_context_tokens();
        if used > kodact_compact::micro_threshold(window, max_out) {
            let ids = kodact_compact::micro_candidates(&self.state.messages, &self.state.microcompacted);
            if !ids.is_empty() {
                self.shared.transcript.append_system("microcompact", json!({"toolUseIds": ids}));
                self.shared.tool_ctx.files.forget_views(&ids);
                // The cleared results are gone from the model's view: re-running one isn't a loop.
                self.guard.forget_results();
                self.state.microcompacted.extend(ids);
            }
        }
        if self.cfg.auto_compact
            && used >= kodact_compact::autocompact_threshold(window, max_out)
            && !self.state.messages.is_empty()
        {
            if let Err(e) = self.compact_for_room(false, continue_work, model, cancel, turn).await {
                self.notice(NoticeLevel::Warning, format!("Automatic compaction failed: {e}"));
            }
        }
    }

    /// Summarize the conversation now (`/compact [instructions]`).
    pub async fn compact(&mut self, instructions: Option<&str>) -> Result<CompactInfo, String> {
        self.adopt_model();
        let cancel = self.new_turn_token();
        let model = self.handle.model();
        let mut turn = TurnAcc::default();
        self.compact_inner(instructions, "manual", false, &model, &cancel, &mut turn).await
    }

    /// Compact to make room: the conversation is nearly full, or (`overflowed`) no longer fits.
    /// A failed summary is never sent again unchanged: it gets one corrected try, then KodAct's
    /// own note stands in. An automatic attempt used to be repeated, the same, before every later
    /// request (path-tracing-reverse, 9ef0955: seven empty summaries in a row, 140k tokens, about
    /// half of the run's 30 minutes). Nearly full, only an empty summary gets this (an API error
    /// may pass, and the conversation still fits); once it no longer fits, any failure but an
    /// interruption does, since the run can't go on without it.
    async fn compact_for_room(
        &mut self,
        overflowed: bool,
        continue_work: bool,
        model: &str,
        cancel: &CancellationToken,
        turn: &mut TurnAcc,
    ) -> Result<CompactInfo, String> {
        let again = |r: &Result<CompactInfo, String>| matches!(r, Err(e) if e != "interrupted" && (overflowed || e == EMPTY_SUMMARY));
        let mut compacted = self.compact_inner(None, "auto", continue_work, model, cancel, turn).await;
        if again(&compacted) {
            compacted = self.compact_inner(Some(SUMMARY_RETRY), "auto", continue_work, model, cancel, turn).await;
        }
        if again(&compacted) {
            compacted = Ok(self.compact_without_model("auto", continue_work, model));
        }
        compacted
    }

    async fn compact_inner(
        &mut self,
        instructions: Option<&str>,
        trigger: &str,
        continue_work: bool,
        model: &str,
        cancel: &CancellationToken,
        turn: &mut TurnAcc,
    ) -> Result<CompactInfo, String> {
        if self.state.messages.is_empty() {
            return Err("nothing to compact yet".into());
        }
        tracing::debug!(trigger, context_tokens = self.state.context_tokens, "compacting");
        let o = self
            .shared
            .hooks
            .run(
                HookEvent::PreCompact,
                Some(trigger),
                self.mode_str(),
                json!({"trigger": trigger, "custom_instructions": instructions.unwrap_or("")}),
                cancel,
            )
            .await;
        for m in o.user_messages.iter().chain(o.blocked.iter()) {
            self.notice(NoticeLevel::Warning, m.clone());
        }
        let mut extra = instructions.map(str::to_string).unwrap_or_default();
        for c in &o.additional_context {
            extra.push('\n');
            extra.push_str(c);
        }
        let mut req = self.build_request(model);
        let ask = ContentBlock::text(kodact_compact::summary_instruction(
            Some(extra.as_str()).filter(|e| !e.trim().is_empty()),
        ));
        match req.messages.last_mut() {
            Some(last) if last.role == kodact_types::Role::User => last.content.push(ask),
            _ => req.messages.push(Message::user(vec![ask])),
        }
        req.tool_choice = Some(json!({"type": "none"}));
        req.output_config = req.output_config.map(|mut oc| {
            if let Some(o) = oc.as_object_mut() {
                o.remove("format");
            }
            oc
        });
        // The summary gets a reply's whole output budget: a model that reasons spends its
        // reasoning from the same budget before writing the summary. At 20k, path-tracing-reverse
        // (9ef0955) reasoned to the limit and wrote nothing seven times in a row, and five of the
        // eight summaries that did come back that run used 18k to 20k.
        let sent = req.max_tokens;
        let t0 = Instant::now();
        let outcome = self.stream_inner(req, cancel, false, false).await;
        turn.api_ms += t0.elapsed().as_millis() as u64;
        let msg = match outcome {
            StreamOutcome::Done(m) => m,
            StreamOutcome::Interrupted(_) | StreamOutcome::Repeating(_) => return Err("interrupted".into()),
            StreamOutcome::Failed(e) => {
                let reason = e.to_string();
                self.record_compact_failure(trigger, &reason, None);
                // The next attempt, sized to the window it states or with half the reply.
                if !self.learn_from_refusal(model, &e, false) {
                    self.shrink_reply(model, &e, sent);
                }
                return Err(reason);
            }
        };
        self.record_usage(model, &msg.usage.clone(), turn);
        let summary = kodact_compact::extract_summary(&msg.to_message().text());
        if summary.is_empty() {
            self.record_compact_failure(trigger, "empty_summary", Some(&msg));
            return Err(EMPTY_SUMMARY.into());
        }
        Ok(self.install_summary(trigger, summary, continue_work, model))
    }

    /// The person's requests carried through compactions, oldest first.
    pub fn carried_requests(&self) -> &[String] {
        &self.carried_requests
    }

    /// Replace the conversation with `summary` (a full compaction). The person's requests go
    /// with it word for word, whatever the summary kept: a summary that lost the task left a
    /// model rebuilding it from the last step it was on (video-processing, 090446d). The
    /// requests are recorded (`system/requests`) so a resumed session carries them on too.
    fn install_summary(&mut self, trigger: &str, summary: String, continue_work: bool, model: &str) -> CompactInfo {
        let pre_tokens = self.projected_context_tokens();
        let mut requests = std::mem::take(&mut self.carried_requests);
        requests.extend(self.prompt_points().into_iter().map(|p| p.text));
        self.shared.transcript.append_compact_boundary(json!({"trigger": trigger, "preTokens": pre_tokens}));
        self.emit(EngineEvent::System {
            subtype: "compact_boundary".into(),
            data: json!({"compact_metadata": {"trigger": trigger, "pre_tokens": pre_tokens}}),
        });
        self.state.messages.clear();
        self.state.uuids.clear();
        self.state.microcompacted.clear();
        let context: Vec<String> = self
            .session_context
            .iter()
            .cloned()
            .chain(self.plan_context())
            .chain(self.notes_context(self.compact_window(model)))
            .chain(self.handle.skills_context())
            .collect();
        let context = (!context.is_empty()).then(|| context.join("\n\n"));
        let block = kodact_compact::requests_block(&requests, request_budget(self.compact_window(model)));
        let first = kodact_compact::summary_message(&summary, context.as_deref(), block.as_deref(), continue_work);
        self.push_user(first, true, None, false);
        if !requests.is_empty() {
            self.shared.transcript.append_system("requests", json!({"requests": requests}));
        }
        self.carried_requests = requests;
        let todos = self.shared.tool_ctx.todos.lock().unwrap().clone();
        if !todos.is_empty() {
            self.shared.transcript.append_system("todos", json!({"todos": todos}));
        }
        self.state.context_tokens = kodact_compact::estimate_tokens(&self.state.messages);
        self.state.counted = self.state.messages.len();
        self.reply_shrink = None;
        self.forget_seen_results();
        CompactInfo { trigger: trigger.into(), pre_tokens, summary }
    }

    /// A refusal for length that states the model's window: use it from now on, for every request
    /// to `model` (reply budgets, compaction). `measured` is set when the refused request carried
    /// the whole conversation, so the prompt size it states is the conversation's. True when the
    /// window KodAct used was wrong, so the request is worth sending again, sized to fit, before
    /// anything is compacted. Any window is taken as stated, small or large.
    fn learn_from_refusal(&mut self, model: &str, e: &ApiError, measured: bool) -> bool {
        let reported = e.reported_limits();
        if measured {
            if let Some(p) = reported.prompt {
                self.state.context_tokens = p;
                self.state.counted = self.state.messages.len();
            }
        }
        let Some(window) = reported.window else { return false };
        if !kodact_api::models::reported_window(model, window) {
            return false;
        }
        self.system_event(
            "context_window_learned",
            json!({"model": model, "window": window, "prompt": reported.prompt}),
        );
        self.notice(
            NoticeLevel::Warning,
            format!("{model}'s endpoint gives its context window as {window} tokens: using it from now on."),
        );
        // Sending it again only helps when the prompt itself fits, with room for a reply.
        reported.prompt.is_none_or(|p| p.saturating_add(u64::from(MIN_REPLY)) <= window)
    }

    /// Count a step for the adaptive level, and say when it changes.
    fn effort_step(&mut self, step: crate::effort_loop::Step) {
        let Some(l) = self.effort_loop.as_mut() else { return };
        if let Some((from, to, why)) = l.record(step) {
            self.system_event(
                "nudge",
                json!({"kind": crate::progress::Nudge::AdaptiveEffort.as_str(), "from": from, "to": to, "why": why}),
            );
            self.notice(NoticeLevel::Info, format!("Reasoning effort {from} → {to}: {why}."));
        }
    }

    /// A refusal for length whose prompt fits the model's window, with room for a reply: the reply
    /// asked for was what didn't fit. The prompt was larger than KodAct's estimate (a tool result
    /// that tokenizes densely), or the server states only a lower bound: vLLM's "your prompt
    /// contains at least N input tokens" is the window less the reply asked for, plus one,
    /// whatever the prompt's size, so a budget sized to N is refused again by the same margin. The
    /// next request asks for half the reply (at least [`MIN_REPLY`]), before anything is compacted:
    /// path-tracing (2026-11-10) asked 28,537 tokens on a prompt "at least 71,464" in a 100,000
    /// window, and the run ended there. True when it shrank, so the request is worth sending again.
    fn shrink_reply(&mut self, model: &str, e: &ApiError, sent: u32) -> bool {
        if !e.is_prompt_too_long() || sent <= MIN_REPLY {
            return false;
        }
        let reported = e.reported_limits();
        let window = reported.window.unwrap_or_else(|| kodact_api::models::model_info_or_default(model).context_window);
        let prompt = reported.prompt.unwrap_or_else(|| self.projected_context_tokens());
        if prompt.saturating_add(u64::from(MIN_REPLY)) > window {
            return false;
        }
        let to = (sent / 2).max(MIN_REPLY);
        self.reply_shrink = Some(self.reply_shrink.map_or(to, |s| s.min(to)));
        self.system_event(
            "reply_shrunk",
            json!({"model": model, "from": sent, "to": to, "window": window, "prompt": prompt}),
        );
        true
    }

    /// A summary attempt that failed, recorded as `system/compact_failed`: why, and what the reply
    /// looked like. The attempt isn't part of the conversation, so without this a failure can't be
    /// told apart afterwards (a reply that reasoned until its limit and wrote nothing, or an error).
    fn record_compact_failure(&self, trigger: &str, reason: &str, reply: Option<&ApiMessage>) {
        let mut data = json!({"trigger": trigger, "reason": reason});
        if let Some(m) = reply {
            let msg = m.to_message();
            let reasoned = msg
                .content
                .iter()
                .any(|b| matches!(b, ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }));
            data["stop_reason"] = json!(m.stop_reason);
            data["output_tokens"] = json!(m.usage.output_tokens);
            data["reasoning_only"] = json!(reasoned && msg.text().trim().is_empty());
        }
        self.system_event("compact_failed", data);
    }

    /// Compaction without the model, for when two summary attempts failed: a note of what KodAct
    /// itself knows (the person's requests, the files changed, the task list, the last thing the
    /// model said) stands in for the model's summary. Less detail, but the run goes on, instead of
    /// ending or paying for another attempt before every request.
    fn compact_without_model(&mut self, trigger: &str, continue_work: bool, model: &str) -> CompactInfo {
        let clip = |t: &str, max: usize| {
            let t = t.trim();
            if t.chars().count() > max {
                format!("{}…", t.chars().take(max).collect::<String>())
            } else {
                t.to_string()
            }
        };
        let points = self.prompt_points();
        // The person's requests come with every summary, this note included (install_summary).
        let mut note = String::from(
            "The conversation could not be summarized, so this note was written without the model: earlier \
             messages are gone. Re-read any file you need before you change it.\n",
        );
        let changed = points.first().map(|p| p.changed_files.clone()).unwrap_or_default();
        if !changed.is_empty() {
            note.push_str("\nFiles changed in this session:\n");
            for f in &changed {
                note.push_str(&format!("- {}\n", f.display()));
            }
        }
        let todos: Vec<String> = self
            .shared
            .tool_ctx
            .todos
            .lock()
            .unwrap()
            .iter()
            .filter_map(|t| {
                let status = t.get("status").and_then(Value::as_str).unwrap_or("pending");
                let content = t.get("content").and_then(Value::as_str).unwrap_or("");
                (!content.is_empty()).then(|| format!("[{status}] {content}"))
            })
            .collect();
        if !todos.is_empty() {
            note.push_str("\nTask list:\n");
            for t in &todos {
                note.push_str(&format!("- {t}\n"));
            }
        }
        let last = self.state.messages.iter().rev().find_map(|m| {
            let t = m.text();
            (m.role == kodact_types::Role::Assistant && !t.trim().is_empty()).then_some(t)
        });
        if let Some(t) = last {
            note.push_str(&format!("\nThe last thing you wrote before this:\n{}\n", clip(&t, 2_000)));
        }
        self.system_event("compact_fallback", json!({"trigger": trigger}));
        self.install_summary(trigger, note, continue_work, model)
    }

    /// The person's prompts, oldest first: user messages with text that are
    /// neither tool results nor only system reminders.
    pub fn prompt_points(&self) -> Vec<PromptPoint> {
        let mut out = vec![];
        for (i, m) in self.state.messages.iter().enumerate() {
            if m.role != kodact_types::Role::User
                || m.content.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. }))
            {
                continue;
            }
            let uuid = self.state.uuids.get(i).cloned().unwrap_or_default();
            if self.state.meta.contains(&uuid) {
                continue;
            }
            let Some(text) =
                m.content.iter().filter_map(|b| b.as_text()).find(|t| !t.trim_start().starts_with("<system-reminder>"))
            else {
                continue;
            };
            out.push(PromptPoint {
                index: i,
                changed_files: self
                    .checkpoint_turn(i)
                    .map(|t| self.shared.history.changed_since(t))
                    .unwrap_or_default(),
                uuid,
                text: text.trim().to_string(),
            });
        }
        out
    }

    /// The conversation as a person would read it back, oldest first: every
    /// message but the ones the harness added for the model alone (meta
    /// messages), with system reminders left out of the person's prompts. A UI
    /// redraws from this when the conversation is replaced (`/resume`,
    /// `/rewind`) or was loaded at start (`--resume`).
    pub fn shown_messages(&self) -> Vec<Message> {
        let reminder = |b: &ContentBlock| b.as_text().is_some_and(|t| t.trim_start().starts_with("<system-reminder>"));
        let mut out = vec![];
        for (i, m) in self.state.messages.iter().enumerate() {
            if self.state.uuids.get(i).is_some_and(|u| self.state.meta.contains(u)) {
                continue;
            }
            let content: Vec<ContentBlock> = if m.role == kodact_types::Role::User {
                m.content.iter().filter(|b| !reminder(b)).cloned().collect()
            } else {
                m.content.clone()
            };
            if !content.is_empty() {
                out.push(Message { role: m.role, content });
            }
        }
        out
    }

    /// `/rewind`: drop the conversation from the message at `index` on (a
    /// prompt point). The transcript branches from the message before it.
    pub fn rewind_conversation(&mut self, index: usize) -> Result<(), String> {
        if index >= self.state.messages.len() {
            return Err("no such message".into());
        }
        let to = self.state.uuids.get(index).cloned();
        self.state.messages.truncate(index);
        self.state.uuids.truncate(index);
        self.state.context_tokens = self.estimate_sent(&self.state.messages);
        self.state.counted = self.state.messages.len();
        self.forget_seen_results();
        self.shared.transcript.set_leaf(self.state.uuids.last().cloned());
        self.shared.transcript.append_system("rewind", json!({"to": to}));
        self.settle_after_rewrite();
        Ok(())
    }

    /// After the conversation was cut or rewritten: keep what the transcript
    /// would rebuild on resume in step with memory. Cleared tool results that
    /// are still in the conversation are recorded again on the new chain, and
    /// the task list is the last TodoWrite's that remains.
    fn settle_after_rewrite(&mut self) {
        let present: HashSet<String> = self
            .state
            .messages
            .iter()
            .flat_map(|m| m.tool_uses().map(|(id, _, _)| id.to_string()).collect::<Vec<_>>())
            .collect();
        self.state.microcompacted.retain(|id| present.contains(id));
        if !self.state.microcompacted.is_empty() {
            let mut ids: Vec<&String> = self.state.microcompacted.iter().collect();
            ids.sort();
            self.shared.transcript.append_system("microcompact", json!({"toolUseIds": ids}));
        }
        let todos = self
            .state
            .messages
            .iter()
            .rev()
            .flat_map(|m| m.tool_uses().collect::<Vec<_>>())
            .find_map(|(_, name, input)| {
                (name == "TodoWrite").then(|| input.get("todos").and_then(Value::as_array).cloned()).flatten()
            })
            .unwrap_or_default();
        *self.shared.tool_ctx.todos.lock().unwrap() = todos;
    }

    /// `/rewind` with code: restore files to how they were before the prompt at `index`.
    pub fn rewind_code(&self, index: usize) -> Result<kodact_session::RewindPlan, String> {
        if index >= self.state.uuids.len() {
            return Err("no such message".into());
        }
        match self.checkpoint_turn(index) {
            Some(turn) => self.shared.history.rewind(turn, false),
            None => Ok(kodact_session::RewindPlan::default()),
        }
    }

    /// The first checkpointed user turn at or after message `index`: a message
    /// that started no turn of its own (a `!command` note) rewinds the turns after it.
    fn checkpoint_turn(&self, index: usize) -> Option<&String> {
        self.state.uuids.iter().skip(index).find(|u| self.shared.history.knows(u))
    }

    /// `/rewind` summarize: replace messages `from..to` (prompt boundaries, or
    /// the end) with a summary, keeping the messages around them.
    pub async fn summarize_range(
        &mut self,
        from: usize,
        to: usize,
        instructions: Option<&str>,
    ) -> Result<CompactInfo, String> {
        let len = self.state.messages.len();
        if from >= to || to > len {
            return Err("nothing to summarize there".into());
        }
        let model = self.handle.model();
        let cancel = self.new_turn_token();
        let mut req = self.build_request(&model);
        let mut slice = normalize(&self.state.messages[from..to], &self.state.microcompacted);
        let ask =
            ContentBlock::text(kodact_compact::summary_instruction(instructions.filter(|i| !i.trim().is_empty())));
        match slice.last_mut() {
            Some(last) if last.role == kodact_types::Role::User => last.content.push(ask),
            _ => slice.push(Message::user(vec![ask])),
        }
        apply_cache_breakpoints(&mut slice);
        req.messages = slice;
        req.tool_choice = Some(json!({"type": "none"}));
        req.output_config = req.output_config.map(|mut oc| {
            if let Some(o) = oc.as_object_mut() {
                o.remove("format");
            }
            oc
        });
        // A reply's whole output budget, as for a full compaction.
        let msg = match self.stream_inner(req, &cancel, false, false).await {
            StreamOutcome::Done(m) => m,
            StreamOutcome::Interrupted(_) | StreamOutcome::Repeating(_) => return Err("interrupted".into()),
            StreamOutcome::Failed(e) => {
                let reason = e.to_string();
                self.record_compact_failure("partial", &reason, None);
                self.learn_from_refusal(&model, &e, false);
                return Err(reason);
            }
        };
        self.record_usage(&model, &msg.usage.clone(), &mut TurnAcc::default());
        let summary = kodact_compact::extract_summary(&msg.to_message().text());
        if summary.is_empty() {
            self.record_compact_failure("partial", "empty_summary", Some(&msg));
            return Err(EMPTY_SUMMARY.into());
        }
        let pre_tokens = self.estimate_sent(&self.state.messages);
        let which = if to == len { "from this point on" } else { "before this point" };
        let note = Message::user_text(format!(
            "<system-reminder>\nThe conversation {which} was summarized at the user's request.\n</system-reminder>\n\n{summary}"
        ));
        let before: Vec<(String, Message)> =
            self.state.uuids[..from].iter().cloned().zip(self.state.messages[..from].iter().cloned()).collect();
        let after: Vec<(String, Message)> =
            self.state.uuids[to..].iter().cloned().zip(self.state.messages[to..].iter().cloned()).collect();
        let t = &self.shared.transcript;
        if to == len {
            // Branch from the last kept message: nothing to copy.
            t.set_leaf(before.last().map(|(u, _)| u.clone()));
        } else {
            t.append_compact_boundary(json!({"trigger": "partial", "preTokens": pre_tokens}));
            for (u, m) in &before {
                t.rewrite(u, m, self.state.meta.contains(u));
            }
        }
        let note_uuid = t.append_user(&note, true, json!({}));
        for (u, m) in &after {
            t.rewrite(u, m, self.state.meta.contains(u));
        }
        self.state.meta.insert(note_uuid.clone());
        let (mut uuids, mut messages): (Vec<String>, Vec<Message>) = before.into_iter().unzip();
        uuids.push(note_uuid);
        messages.push(note);
        let (au, am): (Vec<String>, Vec<Message>) = after.into_iter().unzip();
        uuids.extend(au);
        messages.extend(am);
        self.state.uuids = uuids;
        self.state.messages = messages;
        self.state.context_tokens = self.estimate_sent(&self.state.messages);
        self.state.counted = self.state.messages.len();
        self.forget_seen_results();
        self.settle_after_rewrite();
        Ok(CompactInfo { trigger: "partial".into(), pre_tokens, summary })
    }

    /// `/clear`: forget the conversation (the transcript keeps it before a boundary), the task
    /// list and what was read. Memory files come back with the next prompt.
    pub fn clear(&mut self) {
        self.shared
            .transcript
            .append_compact_boundary(json!({"trigger": "clear", "preTokens": self.state.context_tokens}));
        self.state.messages.clear();
        self.state.uuids.clear();
        self.state.microcompacted.clear();
        self.state.context_tokens = 0;
        self.state.counted = 0;
        self.shared.tool_ctx.todos.lock().unwrap().clear();
        self.carried_requests.clear();
        self.forget_seen_results();
        self.handle.forget_skills();
        self.reattach_context = self.session_context.is_some();
    }

    /// A result for something answered without the model (a local slash command).
    pub fn local_result(&self, text: impl Into<String>, is_error: bool) -> TurnResult {
        TurnResult {
            subtype: if is_error { ResultSubtype::ErrorDuringExecution } else { ResultSubtype::Success },
            is_error,
            result: Some(text.into()),
            stop_reason: None,
            num_turns: 0,
            duration_ms: 0,
            duration_api_ms: 0,
            usage: Usage::default(),
            total_cost_usd: self.state.total_cost_usd,
            model_usage: self.state.model_usage.clone(),
            permission_denials: vec![],
            errors: vec![],
            structured_output: None,
            prompt_blocked: None,
            fatal: false,
            auth_failed: false,
            tool_calls: 0,
            verification: None,
        }
    }

    /// Add a sub-agent's spend (`{costUsd, usage, modelUsage}`) to this session: a Task call's as
    /// its result arrives, a subtask's when it is handed back. Budgets include both.
    pub fn record_subagent_usage(&mut self, sub: &Value) {
        let cost = sub.get("costUsd").and_then(Value::as_f64).unwrap_or(0.0);
        self.state.total_cost_usd += cost;
        if let Some(usage) = sub.get("usage").and_then(|u| serde_json::from_value::<Usage>(u.clone()).ok()) {
            self.state.total_usage.add(&usage);
        }
        if let Some(Value::Object(per_model)) = sub.get("modelUsage") {
            for (model, mu) in per_model {
                let e = self.state.model_usage.entry(model.clone()).or_insert_with(|| json!({}));
                for (k, v) in mu.as_object().into_iter().flatten() {
                    let merged = match (e.get(k), v) {
                        (Some(a), b) if a.is_u64() && b.is_u64() => {
                            json!(a.as_u64().unwrap_or(0) + b.as_u64().unwrap_or(0))
                        }
                        (Some(a), b) if a.is_number() && b.is_number() => {
                            json!(a.as_f64().unwrap_or(0.0) + b.as_f64().unwrap_or(0.0))
                        }
                        (_, b) => b.clone(),
                    };
                    e[k] = merged;
                }
            }
        }
    }

    /// A fresh cancellation token for this turn.
    /// `/btw`: answer a side question with the conversation as context, but no
    /// tools, and without adding to the conversation. Earlier side exchanges
    /// Count a request made beside the conversation (a goal check, a title)
    /// toward the session's cost, without changing the context size.
    pub fn record_side_usage(&mut self, model: &str, usage: &Usage) {
        self.record_usage(model, usage, &mut TurnAcc::default());
    }

    /// ride along. The request keeps the conversation's system prompt and
    /// tools, so the cached prefix is reused; its cost counts toward the session.
    pub async fn side_question(&mut self, question: &str, earlier: &[(String, String)]) -> Result<String, String> {
        let model = self.handle.model();
        self.publish();
        let req = crate::snapshot::side_question_request(
            &self.handle.snapshot(),
            &self.handle.runtime(),
            question,
            earlier,
            None,
        );
        let cancel = self.new_turn_token();
        let msg = kodact_api::complete(self.shared.provider.as_ref(), req, &cancel).await.map_err(|e| match e {
            ApiError::Cancelled => "interrupted".to_string(),
            e => e.describe(),
        })?;
        self.record_usage(&model, &msg.usage, &mut TurnAcc::default());
        let text = msg.to_message().text();
        if text.trim().is_empty() {
            return Err("the model gave no answer".into());
        }
        Ok(text.trim().to_string())
    }

    /// The note that ends a cut turn in the conversation: the time limit, or an interruption.
    fn stop_marker(&self, tools: bool) -> &'static str {
        match (self.time_up.load(std::sync::atomic::Ordering::SeqCst), tools) {
            (true, _) => TIME_UP_MARKER,
            (false, false) => INTERRUPT_MARKER,
            (false, true) => INTERRUPT_MARKER_TOOLS,
        }
    }

    fn new_turn_token(&self) -> CancellationToken {
        let t = CancellationToken::new();
        *self.handle.cancel.lock().unwrap() = t.clone();
        t
    }

    /// Run one user turn to completion. While it runs, a host may send messages for its
    /// next step ([`EngineHandle::steer`]); any it ended before reaching are left for the
    /// host to run next ([`EngineHandle::take_steers`]).
    pub async fn submit(&mut self, prompt: MessageContent) -> TurnResult {
        // The transcript says when a turn is open, so a resume can tell a turn the process
        // died in from one that ended (GOALS pillar 2). Written to the file only: hosts get
        // the turn's `result`.
        if self.cfg.is_subagent {
            self.shared.transcript.append_system("turn_start", json!({}));
            let result = self.run_turn(prompt).await;
            self.shared.transcript.append_system("turn_end", json!({"stop_reason": result.stop_reason}));
            return result;
        }
        self.handle.set_turn_running(true);
        self.shared.transcript.append_system("turn_start", json!({}));
        let mut result = self.run_turn(prompt).await;
        self.shared.transcript.append_system("turn_end", json!({"stop_reason": result.stop_reason}));
        // A turn a hook blocked before it began did nothing to report on.
        if result.prompt_blocked.is_none() {
            result.verification = self.verification_state().await;
        }
        self.handle.set_turn_running(false);
        self.handle.end_turn_overrides();
        result
    }

    async fn run_turn(&mut self, prompt: MessageContent) -> TurnResult {
        self.adopt_model();
        let started = Instant::now();
        let cancel = self.new_turn_token();
        // `--max-time` stops the run at its limit: the turn is cancelled as by an interrupt, a
        // model call or a tool in flight included, and `finish` reports the limit. A turn that
        // starts after the limit ends at once.
        let _deadline = self.cfg.time_limit.map(|t| {
            let left = t.remaining();
            if left.is_zero() {
                self.time_up.store(true, std::sync::atomic::Ordering::SeqCst);
                cancel.cancel();
            }
            let (cancel, up) = (cancel.clone(), self.time_up.clone());
            AbortOnDrop(tokio::spawn(async move {
                tokio::time::sleep(left).await;
                up.store(true, std::sync::atomic::Ordering::SeqCst);
                cancel.cancel();
            }))
        });
        let mut turn = TurnAcc::default();
        let mut blocks = prompt.into_blocks();
        // Hooks see the prompt as typed: reminders that ride along with it (`@path` attachments)
        // are left out, unless they are all there is.
        let texts: Vec<&str> = blocks.iter().filter_map(|b| b.as_text()).collect();
        let typed: Vec<&str> =
            texts.iter().copied().filter(|t| !t.trim_start().starts_with("<system-reminder>")).collect();
        let prompt_text = if typed.is_empty() { texts.join("\n") } else { typed.join("\n") };

        // Sub-agents get their context but no session hooks.
        if !self.session_started && self.cfg.is_subagent {
            self.session_started = true;
            if let Some(c) = self.cfg.initial_context.take() {
                blocks.insert(0, ContentBlock::text(format!("<system-reminder>\n{c}\n</system-reminder>")));
            }
        }
        // SessionStart on the first turn of the process.
        if !self.session_started {
            self.session_started = true;
            let o = self
                .shared
                .hooks
                .run(
                    HookEvent::SessionStart,
                    Some(&self.start_source),
                    self.mode_str(),
                    json!({"source": self.start_source}),
                    &cancel,
                )
                .await;
            for m in &o.user_messages {
                self.notice(NoticeLevel::Warning, m.clone());
            }
            let mut context = vec![];
            // A resumed conversation already starts with the memory context.
            if let Some(c) = self.cfg.initial_context.take().filter(|_| self.state.messages.is_empty()) {
                context.push(c);
            }
            // A time-limited run (GOALS pillar 5): the model plans for the deadline from the start.
            if let Some(t) = self.cfg.time_limit {
                context.push(format!(
                    "This run has a time limit (--max-time): {} left. Plan the work to fit: make the essential \
                     change first, check it, and leave time to report. The run is stopped when time is up, and \
                     unfinished work is lost.",
                    about(t.remaining())
                ));
            }
            context.extend(o.additional_context);
            if !context.is_empty() {
                blocks.insert(
                    0,
                    ContentBlock::text(format!("<system-reminder>\n{}\n</system-reminder>", context.join("\n\n"))),
                );
            }
        }

        if std::mem::take(&mut self.reattach_context) {
            if let Some(c) = &self.session_context {
                blocks.insert(0, ContentBlock::text(format!("<system-reminder>\n{c}\n</system-reminder>")));
            }
        }
        // Background commands that exited between turns.
        if let Some(text) = self.background_exits().await {
            blocks.insert(0, ContentBlock::text(text));
        }
        // The permission mode can change between prompts (a host's
        // `set_permission_mode`, shift+tab): tell the model while it plans,
        // and once when planning ends, as Claude Code does.
        if !self.cfg.is_subagent {
            let planning = self.mode_str() == "plan";
            if planning {
                self.reminders.push(PLAN_MODE_REMINDER.to_string());
            } else if self.planning {
                self.reminders.push(format!(
                    "Plan mode is off (the permission mode is now {}). You may change files and run commands again.",
                    self.mode_str()
                ));
            }
            self.planning = planning;
        }
        if !self.reminders.is_empty() && !self.cfg.is_subagent {
            let notes = std::mem::take(&mut self.reminders).join("\n\n");
            blocks.insert(0, ContentBlock::text(format!("<system-reminder>\n{notes}\n</system-reminder>")));
        }

        // UserPromptSubmit (contract C11: exit 2 blocks and erases the prompt). Not for sub-agents.
        let o = if self.cfg.is_subagent {
            kodact_hooks::HookOutcome::default()
        } else {
            self.shared
                .hooks
                .run(HookEvent::UserPromptSubmit, None, self.mode_str(), json!({"prompt": prompt_text}), &cancel)
                .await
        };
        for m in &o.user_messages {
            self.notice(NoticeLevel::Warning, m.clone());
        }
        if let Some(reason) = o.blocked.clone().or_else(|| o.stop.clone()) {
            return self.finish(
                started,
                turn,
                ResultSubtype::Success,
                None,
                Some("prompt_blocked".into()),
                Some(reason),
            );
        }
        if !o.additional_context.is_empty() {
            blocks.push(ContentBlock::text(format!(
                "<system-reminder>\n{}\n</system-reminder>",
                o.additional_context.join("\n")
            )));
        }

        let model0 = self.handle.model();
        self.maybe_compact(&model0, false, &cancel, &mut turn).await;
        let user_msg = Message::user(blocks);
        let user_uuid = self.push_user(user_msg.clone(), false, None, false);
        self.emit(EngineEvent::PromptAccepted { message: user_msg, uuid: user_uuid.clone() });
        // Sub-agents write into the caller's checkpoint turn, so rewinding the caller undoes them too.
        if !self.cfg.is_subagent {
            self.shared.history.begin_turn(&user_uuid);
        }
        self.verify = Default::default();
        self.guard = Default::default();
        self.progress = crate::progress::Progress::new(self.state.total_cost_usd);
        self.named = crate::named::Named::from_request(&prompt_text, &self.shared.tool_ctx.project_dir);
        self.watched = crate::named::Watched::at_start(&self.named, &self.shared.tool_ctx.project_dir);
        self.shell_edit_told = false;
        self.ran_commands.clear();
        self.constraints_told = false;
        self.passing_told = false;
        let n = &self.cfg.nudges;
        self.limits = if n.limit_verdicts || n.stale_deliverables {
            crate::limits::Limits::from_request(&prompt_text)
        } else {
            Default::default()
        };
        self.unsaved = None;
        self.stale_told = 0;
        self.stale_end_told = false;
        self.size_told.clear();
        self.output_cap = None;
        self.reply_shrink = None;
        // Each turn starts at the person's level (`high` without one) and adapts from there.
        self.effort_loop = None;
        self.last_thought = 0;
        if self.cfg.reminders && !self.cfg.is_subagent && self.cfg.nudges.adaptive_effort {
            let start = self.handle.runtime.read().unwrap().effort.clone().unwrap_or_else(|| "high".into());
            self.effort_loop = crate::effort_loop::EffortLoop::new(&start);
        }
        if self.verifying() {
            self.verify.fingerprint = self.worktree_fingerprint().await;
        }

        let mut model;
        // Set once a fallback took over; it then holds for the rest of the turn (contract C6).
        let mut fallback: Option<String> = None;
        let mut fallbacks = self.cfg.fallback_models.clone().into_iter();
        let mut stop_hook_active = false;
        // Compactions for length this turn, and the conversation's length right after the last.
        let mut length_compactions = 0u32;
        let mut compacted_len: Option<usize> = None;
        // A refusal that gave the model's real window: sent again once, sized to it.
        let mut learned_window = false;
        let mut reply_shrinks = 0u32;
        let mut last_text: Option<String> = None;
        let mut continuations = 0;
        let mut reasoning_cutoffs = 0;
        let mut continuing = false;
        let mut budget_warned = false;
        let mut attempt_reminded = false;
        let mut next_step_reminders = 0u32;
        let mut todo_reminders = 0u32;
        let mut background_waits = 0u32;
        let mut background_reminded = false;
        let mut api_retries = 0u32;

        loop {
            if cancel.is_cancelled() {
                self.push_user(Message::user_text(self.stop_marker(false)), true, None, true);
                return self.finish(started, turn, ResultSubtype::Success, last_text, Some("interrupted".into()), None);
            }
            if let Some(max) = self.cfg.max_turns {
                if turn.api_calls >= max {
                    let text = format!("Reached maximum number of turns ({max})");
                    self.notice(NoticeLevel::Error, text.clone());
                    turn.errors.push(text);
                    return self.finish(
                        started,
                        turn,
                        ResultSubtype::ErrorMaxTurns,
                        last_text,
                        Some("max_turns".into()),
                        None,
                    );
                }
            }
            // A host may switch models between calls; a fallback wins for this turn.
            model = fallback.clone().unwrap_or_else(|| self.handle.model());
            if turn.api_calls > 0 {
                self.maybe_compact(&model, true, &cancel, &mut turn).await;
            }
            let req = self.build_request(&model);
            let sent = req.max_tokens;
            tracing::debug!(
                model = %req.model,
                messages = req.messages.len(),
                tools = req.tools.len(),
                max_tokens = req.max_tokens,
                output_config = ?req.output_config,
                speed = ?req.speed,
                context_tokens = self.state.context_tokens,
                "model request"
            );
            let t0 = Instant::now();
            // Watched while this stretch still has reminders to act left: past them, a reply
            // runs to its end and the turn ends as it would have.
            let watch = self.cfg.reminders && reasoning_cutoffs < MAX_REASONING_CUTOFFS;
            let outcome = self.stream_once(req, &cancel, watch).await;
            turn.api_ms += t0.elapsed().as_millis() as u64;
            match &outcome {
                StreamOutcome::Done(m) => tracing::debug!(
                    ms = t0.elapsed().as_millis() as u64,
                    stop_reason = ?m.stop_reason,
                    input_tokens = m.usage.input_tokens,
                    output_tokens = m.usage.output_tokens,
                    cache_read = m.usage.cache_read_input_tokens,
                    tool_calls = m.content.iter().filter(|b| matches!(b, ContentBlock::ToolUse { .. })).count(),
                    "model response"
                ),
                StreamOutcome::Interrupted(_) => tracing::debug!("model response interrupted"),
                StreamOutcome::Failed(e) => tracing::debug!(error = %e, "model request failed"),
                StreamOutcome::Repeating(r) => tracing::debug!(copies = r.copies, "model response repeating; stopped"),
            }
            let mut msg = match outcome {
                StreamOutcome::Done(m) => m,
                // Nothing of the reply is kept: it was one passage over and over. The model is
                // told, within the same budget as replies that were all reasoning.
                StreamOutcome::Repeating(r) => {
                    turn.api_calls += 1;
                    reasoning_cutoffs += 1;
                    self.system_event(
                        "act_reminder",
                        json!({"kind": "repeating", "copies": r.copies, "reasoning": r.reasoning}),
                    );
                    self.notice(
                        NoticeLevel::Warning,
                        "The reply was repeating itself: stopped, and the model was told.",
                    );
                    self.push_user(Message::user_text(r.text()), true, None, true);
                    continue;
                }
                StreamOutcome::Interrupted(partial) => {
                    let mut marker = self.stop_marker(false);
                    if let Some(p) = partial {
                        let ids: Vec<String> = p.to_message().tool_uses().map(|(id, _, _)| id.to_string()).collect();
                        self.push_assistant(p);
                        for id in ids {
                            self.push_user(
                                Message::user(vec![ContentBlock::tool_result(id, kodact_tools::INTERRUPTED, true)]),
                                false,
                                None,
                                true,
                            );
                            marker = self.stop_marker(true);
                        }
                    }
                    self.push_user(Message::user_text(marker), true, None, true);
                    return self.finish(
                        started,
                        turn,
                        ResultSubtype::Success,
                        last_text,
                        Some("interrupted".into()),
                        None,
                    );
                }
                StreamOutcome::Failed(e) if !learned_window && self.learn_from_refusal(&model, &e, true) => {
                    learned_window = true;
                    continue;
                }
                StreamOutcome::Failed(e)
                    if reply_shrinks < MAX_REPLY_SHRINKS && self.shrink_reply(&model, &e, sent) =>
                {
                    reply_shrinks += 1;
                    continue;
                }
                StreamOutcome::Failed(e)
                    if e.is_prompt_too_long()
                        && length_compactions < MAX_LENGTH_COMPACTIONS
                        && compacted_len.is_none_or(|n| self.state.messages.len() > n) =>
                {
                    length_compactions += 1;
                    self.notice(
                        NoticeLevel::Warning,
                        "The conversation no longer fits the context window; compacting.",
                    );
                    // Here a failed summary would end the run (password-recovery, 4e06b66: one
                    // came back empty, the next attempt worked).
                    let compacted = self.compact_for_room(true, true, &model, &cancel, &mut turn).await;
                    match compacted {
                        Ok(_) => {
                            compacted_len = Some(self.state.messages.len());
                            reply_shrinks = 0;
                            continue;
                        }
                        Err(err) => {
                            let text = format!("API Error: {e} (compaction failed: {err})");
                            self.notice(NoticeLevel::Error, text.clone());
                            turn.errors.push(text.clone());
                            let mut r = self.finish(
                                started,
                                turn,
                                ResultSubtype::ErrorDuringExecution,
                                Some(text),
                                Some("api_error".into()),
                                None,
                            );
                            r.fatal = true;
                            return r;
                        }
                    }
                }
                StreamOutcome::Failed(e) => {
                    // A fallback model is for when this one can't serve the turn: overloaded, gone,
                    // or limited in a way a short wait doesn't lift (a throttle that has been
                    // waited out several times in a row, a wait longer than a retry sleeps, a spent
                    // quota). With no fallback set, each of these goes on as before.
                    let limited = matches!(&e, ApiError::Http { status: 429, .. })
                        && api_retries >= FALLBACK_AFTER_THROTTLES
                        || e.long_wait().is_some()
                        || e.is_quota();
                    let switch = e.is_overloaded() || matches!(&e, ApiError::Http { status: 404, .. }) || limited;
                    if switch {
                        if let Some(next) = fallbacks.next() {
                            self.system_event(
                                "model_fallback",
                                json!({"from": model, "to": next, "reason": e.to_string()}),
                            );
                            self.notice(
                                NoticeLevel::Warning,
                                format!("{model} unavailable ({e}); switching to {next} for this turn"),
                            );
                            fallback = Some(next);
                            continue;
                        }
                    }
                    if let Some(wait) = self.api_retry_wait(&e, api_retries) {
                        api_retries += 1;
                        self.notice(
                            NoticeLevel::Warning,
                            format!(
                                "API Error: {e}. Retrying in {}s (attempt {api_retries}).",
                                wait.as_secs_f64().ceil() as u64
                            ),
                        );
                        self.system_event(
                            "api_retry",
                            json!({"attempt": api_retries, "retry_delay_ms": wait.as_millis() as u64, "error": e.to_string()}),
                        );
                        tokio::select! {
                            _ = cancel.cancelled() => {}
                            _ = tokio::time::sleep(wait) => {}
                        }
                        continue;
                    }
                    let text = format!("API Error: {}", e.describe());
                    self.notice(NoticeLevel::Error, text.clone());
                    turn.errors.push(text.clone());
                    let mut r = self.finish(
                        started,
                        turn,
                        ResultSubtype::ErrorDuringExecution,
                        Some(text),
                        Some("api_error".into()),
                        None,
                    );
                    // Still too long after compacting: the next turn would fail the same way.
                    r.fatal = e.is_unrecoverable() || (e.is_prompt_too_long() && length_compactions > 0);
                    r.auth_failed = e.is_auth_failure();
                    return r;
                }
            };
            turn.api_calls += 1;
            // Each run of failures gets the full retry budget.
            api_retries = 0;
            self.record_usage(&model, &msg.usage.clone(), &mut turn);
            self.measure_context(&model, &msg);
            // A call the model wrote out as text is the call it meant (small and local models
            // do this): run it instead of ending the turn on a line of JSON.
            // Not where the answer itself is asked as JSON (`--json-schema`).
            if self.cfg.json_schema.is_none()
                && msg.stop_reason != Some(StopReason::MaxTokens)
                && msg.stop_reason != Some(StopReason::Refusal)
            {
                let promoted =
                    crate::textcall::promote(&mut msg.content, &|name| self.shared.tools.get(name).is_some());
                if !promoted.is_empty() {
                    msg.stop_reason = Some(StopReason::ToolUse);
                    self.system_event("tool_call_from_text", json!({"tools": promoted}));
                }
            }
            let text = msg.to_message().text();
            let next_step = announced_next_step(&text);
            // Read before `text` moves into `last_text`: a cut-off reply with no text was all reasoning.
            let reply_has_text = !text.is_empty();
            // The rest of an answer cut off by `max_tokens`: its text so far is in `last_text`.
            let was_continuation = continuing;
            if continuing {
                // A reply continued after `max_tokens` is one answer.
                last_text = Some(format!("{}{text}", last_text.take().unwrap_or_default()));
            } else if !text.is_empty() {
                last_text = Some(text);
            }
            let stop_reason = msg.stop_reason;
            self.settle_invalid_tool_inputs(&mut msg.content, stop_reason);
            let tool_uses: Vec<(String, String, Value)> =
                msg.to_message().tool_uses().map(|(a, b, c)| (a.to_string(), b.to_string(), c.clone())).collect();
            // A reply that was all reasoning: what an act reminder hands back where it isn't kept.
            let reasoning =
                if tool_uses.is_empty() && !reply_has_text { reasoning_of(&msg.content) } else { String::new() };
            // How much this reply thought, for the adaptive level (about four characters a token).
            self.last_thought = msg
                .content
                .iter()
                .map(|b| match b {
                    ContentBlock::Thinking { thinking, .. } => thinking.len() as u64 / 4,
                    _ => 0,
                })
                .sum();
            if tool_uses.is_empty() && !reply_has_text {
                self.effort_step(crate::effort_loop::Step::AllThinking);
            }
            self.push_assistant(msg);
            self.publish_turn(started, &turn);

            // Budget (contract C7): checked after every call.
            if let Some(budget) = self.cfg.max_budget_usd {
                if self.state.total_cost_usd >= budget {
                    for (id, _, _) in &tool_uses {
                        self.push_user(
                            Message::user(vec![ContentBlock::tool_result(
                                id.clone(),
                                "Not run: the session's budget is used up.",
                                true,
                            )]),
                            true,
                            None,
                            false,
                        );
                    }
                    let text = format!("Reached maximum budget (${budget})");
                    self.notice(NoticeLevel::Error, text.clone());
                    turn.errors.push(text);
                    return self.finish(
                        started,
                        turn,
                        ResultSubtype::ErrorMaxBudgetUsd,
                        last_text,
                        Some("max_budget".into()),
                        None,
                    );
                }
            }

            // Output cut off (GOALS pillar 2): a larger cap for the rest of the turn; a cut-off tool
            // call is answered with an error by exec; cut-off text is continued.
            continuing = false;
            if stop_reason == Some(StopReason::MaxTokens) {
                // Only output that needs the room gets a larger cap: text or a tool call cut off. A
                // reply that was all reasoning is asked to act on what it has (ACT_PROMPT), and twice
                // the room would let its next pass reason twice as long before it acts (path-tracing
                // on a 100,000-token reasoning model, 2026-11-10: three replies of 32,000 tokens of
                // reasoning each, minutes apiece, in a 30-minute run).
                let cut_output = reply_has_text || !tool_uses.is_empty();
                if self.cfg.escalate_output
                    && cut_output
                    && self.output_cap.is_none()
                    && self.cfg.max_output_tokens < ESCALATED_OUTPUT_TOKENS
                {
                    self.output_cap = Some(ESCALATED_OUTPUT_TOKENS);
                    self.system_event(
                        "output_limit",
                        json!({"from": self.cfg.max_output_tokens, "to": ESCALATED_OUTPUT_TOKENS}),
                    );
                }
                if tool_uses.is_empty() && reply_has_text && continuations < MAX_CONTINUATIONS {
                    continuations += 1;
                    continuing = true;
                    self.push_user(Message::user_text(CONTINUE_PROMPT), true, None, true);
                    continue;
                }
                // All reasoning, nothing to continue: the next reply is a fresh step, not the rest of
                // this one, so `continuing` stays false. Counted per stretch without a tool call, not
                // per run: a long task can hit this many times, each time after real work.
                if self.cfg.reminders
                    && tool_uses.is_empty()
                    && !reply_has_text
                    && reasoning_cutoffs < MAX_REASONING_CUTOFFS
                {
                    reasoning_cutoffs += 1;
                    let (text, kept) = self.act_reminder(ACT_PROMPT, ACT_CUTOFF, &model, &reasoning);
                    self.system_event("act_reminder", json!({"kind": "reasoning_cutoff", "reasoning_chars": kept}));
                    self.push_user(Message::user_text(text), true, None, true);
                    continue;
                }
            }

            if !tool_uses.is_empty() {
                // The model acted: a later reasoning-only cut-off starts a new stretch.
                reasoning_cutoffs = 0;
                // A call the model was already told would fail the same way is answered, not run.
                let refused: Vec<Option<crate::stuck::Refused>> = tool_uses
                    .iter()
                    .map(|(_, name, input)| {
                        let files_only = self.depends_on_files_only(name);
                        self.guard.would_repeat(name, input, files_only).filter(|_| self.cfg.reminders)
                    })
                    .collect();
                let to_run: Vec<(String, String, Value)> =
                    tool_uses.iter().zip(&refused).filter(|(_, r)| r.is_none()).map(|(c, _)| c.clone()).collect();
                // Calls that ran: a call answered without running is not work done (a goal's
                // "turns without using a tool" count them as none).
                turn.tool_calls += to_run.len() as u32;
                let writes_before = self.shared.history.writes_len();
                // Shell commands that may write: the files they name, as they are now.
                let shell_before: Vec<Option<crate::shell_edit::Before>> =
                    tool_uses.iter().map(|(_, name, input)| self.shell_edit_before(name, input)).collect();
                let mut ran = crate::exec::run_tools(&self.shared, &to_run, &cancel, self.mode_str()).await.into_iter();
                let writes_now = self.shared.history.writes_len();
                let mut results: Vec<crate::exec::CallResult> = tool_uses
                    .iter()
                    .zip(&refused)
                    .map(|((id, ..), r)| match r {
                        Some(r) => {
                            self.system_event("loop_guard", r.record());
                            let mut c = crate::exec::CallResult::plain(id, kodact_tools::ToolOutput::error(r.text()));
                            c.writes_after = writes_now;
                            c
                        }
                        None => ran.next().unwrap_or_else(|| {
                            crate::exec::CallResult::plain(
                                id,
                                kodact_tools::ToolOutput::error(kodact_tools::INTERRUPTED),
                            )
                        }),
                    })
                    .collect();
                if self.verifying() {
                    self.track_verification(&tool_uses, &results).await;
                }
                let stuck: Vec<crate::stuck::Stuck> = tool_uses
                    .iter()
                    .zip(&results)
                    .zip(&refused)
                    .filter(|((_, r), refused)| r.denial.is_none() && refused.is_none())
                    .map(|(pair, _)| pair)
                    .filter_map(|((_, name, input), r)| {
                        // Could the call have changed files? Then a command run after it may give a
                        // new result, and the guard doesn't call a repeat of it a loop.
                        let may_change = match name.as_str() {
                            "Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "Task" | "Checkpoint" => {
                                !r.output.is_error
                            }
                            "Bash" => !self.shared.tools.get("Bash").map(|t| t.is_read_only(input)).unwrap_or(true),
                            _ => false,
                        };
                        self.guard.observe(name, input, &r.output.text_content(), r.output.is_error, may_change)
                    })
                    .collect();
                if self.cfg.edit_diagnostics || self.cfg.language_servers.is_some() || self.cfg.formatters.is_some() {
                    self.diagnose_edits(&tool_uses, &mut results, &model).await;
                }
                self.shell_edits_parse(&tool_uses, &mut results, &shell_before).await;
                let changed_named = self.watched.changed();
                let named_changed = !changed_named.is_empty();
                self.size_limits_told(&tool_uses, &mut results, &changed_named);
                if named_changed {
                    self.unsaved = None;
                }
                self.judge_limits(&tool_uses, &mut results, turn.api_calls, named_changed);
                let made_progress = self.made_progress(&tool_uses, &results, writes_before, named_changed);
                // Long thinking: a quarter of the reply budget, at least 2,000 tokens.
                let long = self.last_thought >= u64::from(sent / 4).max(2_000);
                self.effort_step(if made_progress {
                    crate::effort_loop::Step::Progress
                } else if long {
                    crate::effort_loop::Step::LongNoProgress
                } else {
                    crate::effort_loop::Step::Other
                });
                if made_progress {
                    self.progress_for_back_off();
                }
                for ((_, name, input), r) in tool_uses.iter().zip(&results) {
                    if name == "Bash" && r.denial.is_none() {
                        if let Some(c) = input.get("command").and_then(Value::as_str) {
                            self.ran_commands.push(c.to_string());
                        }
                    }
                }
                let mut interrupted = cancel.is_cancelled();
                for r in results {
                    if let Some(d) = r.denial {
                        turn.denials.push(d);
                    }
                    if r.interrupt_turn {
                        interrupted = true;
                    }
                    if let Some(s) = r.stop {
                        turn.stop = Some(s);
                    }
                    if let Some(sub) = r.output.structured.as_ref().and_then(|s| s.get("subagentUsage")) {
                        self.record_subagent_usage(sub);
                    }
                    // A request a tool made itself (the Advisor): priced here, custom pricing included.
                    if let Some(side) = r.output.structured.as_ref().and_then(|s| s.get("sideUsage")) {
                        let model = side["model"].as_str().unwrap_or_default().to_string();
                        if let Ok(u) = serde_json::from_value::<Usage>(side["usage"].clone()) {
                            self.record_side_usage(&model, &u);
                        }
                    }
                    let block = ContentBlock::ToolResult {
                        tool_use_id: r.id.clone(),
                        content: r.output.content.clone(),
                        is_error: if r.output.is_error { Some(true) } else { None },
                        cache_control: None,
                    };
                    self.push_user(Message::user(vec![block]), false, r.output.structured.clone(), true);
                }
                // Background commands that exited meanwhile, told with the results.
                if let Some(text) = self.background_exits().await {
                    self.push_user(Message::user_text(text), true, None, true);
                }
                self.publish_turn(started, &turn);
                if let (Some(s), false, true) = (stuck.first(), interrupted, self.cfg.reminders) {
                    self.system_event("loop_guard", s.record());
                    self.push_user(Message::user_text(s.text()), true, None, true);
                }
                // Pacing by progress: a long stretch with nothing made. The loop guard speaks
                // first; one reminder per step is enough.
                if !interrupted {
                    if let Some(text) = self.progress_nudge(made_progress, stuck.is_empty()) {
                        self.push_user(Message::user_text(text), true, None, true);
                    }
                }
                if !interrupted {
                    if let Some(text) = self.stale_note(turn.api_calls, false) {
                        self.push_user(Message::user_text(text), true, None, true);
                    }
                }
                if self.cfg.reminders && !budget_warned && !interrupted {
                    if let Some(text) = self.budget_warning(turn.api_calls, started) {
                        budget_warned = true;
                        self.push_user(Message::user_text(text), true, None, true);
                    }
                }
                if interrupted {
                    cancel.cancel();
                    self.push_user(Message::user_text(self.stop_marker(true)), true, None, true);
                    return self.finish(
                        started,
                        turn,
                        ResultSubtype::Success,
                        last_text,
                        Some("interrupted".into()),
                        None,
                    );
                }
                // What you sent while it worked: the model reads it now, at its next step.
                if !self.cfg.is_subagent {
                    self.deliver_steers();
                }
                if let Some(reason) = turn.stop.clone() {
                    self.notice(NoticeLevel::Info, format!("Stopped by hook: {reason}"));
                    return self.finish(
                        started,
                        turn,
                        ResultSubtype::Success,
                        last_text,
                        Some("hook_stopped".into()),
                        None,
                    );
                }
                continue;
            }
            if stop_reason == Some(StopReason::PauseTurn) {
                continue;
            }

            // Commands that outlived their Bash timeout went on in the background: the turn doesn't
            // end while they run, so their result isn't lost (unattended, the run's end would stop
            // them). It waits for them, then goes on with how they ended.
            if background_waits < BACKGROUND_WAITS && stop_reason != Some(StopReason::Refusal) {
                if let Some(text) = self.wait_for_background(&cancel).await {
                    background_waits += 1;
                    self.push_user(Message::user_text(text), true, None, true);
                    continue;
                }
            }

            // Unattended runs (GOALS pillar 5): no finishing without an attempt.
            if self.cfg.reminders
                && self.cfg.autonomous
                && !self.cfg.is_subagent
                && !attempt_reminded
                && turn.tool_calls == 0
                && stop_reason != Some(StopReason::Refusal)
            {
                attempt_reminded = true;
                self.system_event("attempt_reminder", json!({}));
                self.push_user(Message::user_text(ATTEMPT_REMINDER), true, None, true);
                continue;
            }

            // Unattended runs: a reply that announces its next step ("Let me check...") and makes
            // no tool call would end the run with that step undone.
            if self.cfg.reminders
                && self.cfg.autonomous
                && !self.cfg.is_subagent
                && stop_reason != Some(StopReason::Refusal)
            {
                if let (Some(line), true) = (next_step.as_deref(), next_step_reminders < FINISH_REMINDERS) {
                    next_step_reminders += 1;
                    self.system_event("next_step_reminder", json!({"line": line}));
                    self.push_user(Message::user_text(next_step_reminder(line)), true, None, true);
                    continue;
                }
                let open = self.unfinished_todos();
                if !open.is_empty() && todo_reminders < FINISH_REMINDERS {
                    todo_reminders += 1;
                    self.system_event("todo_reminder", json!({"open": open}));
                    self.push_user(Message::user_text(todo_reminder(&open)), true, None, true);
                    continue;
                }
            }

            // A one-shot run that isn't unattended: sent back once on open todos. Unattended
            // runs have their own, firmer reminders above.
            if self.cfg.one_shot
                && !self.cfg.autonomous
                && !self.cfg.is_subagent
                && self.cfg.reminders
                && todo_reminders == 0
                && stop_reason != Some(StopReason::Refusal)
            {
                let open = self.unfinished_todos();
                if !open.is_empty() {
                    todo_reminders += 1;
                    self.system_event("todo_reminder", json!({"open": open, "one_shot": true}));
                    self.push_user(Message::user_text(one_shot_todo_reminder(&open)), true, None, true);
                    continue;
                }
            }

            // What the request names and isn't there (`nudges.constraints`): said once a turn.
            if self.cfg.reminders
                && self.cfg.nudges.constraints
                && !self.cfg.is_subagent
                && !self.constraints_told
                && stop_reason != Some(StopReason::Refusal)
            {
                self.constraints_told = true;
                let report = crate::named::report(&self.named, &self.shared.tool_ctx.project_dir, &self.ran_commands);
                if let Some((text, record)) = report.filter(|_| self.may_nudge(crate::progress::Nudge::Constraints)) {
                    self.system_event("nudge", record);
                    self.push_user(Message::user_text(text), true, None, true);
                    continue;
                }
            }

            // A result that met every limit was printed and never written: said once at the end.
            if stop_reason != Some(StopReason::Refusal) {
                if let Some(text) = self.stale_note(turn.api_calls, true) {
                    self.push_user(Message::user_text(text), true, None, true);
                    continue;
                }
            }

            // A version that passed is saved and the files now fail: said once a turn. Restoring
            // it is the model's choice (or the person's), never KodAct's.
            if self.cfg.reminders && !self.passing_told && stop_reason != Some(StopReason::Refusal) {
                if let Some(note) =
                    self.passing_note().filter(|_| self.may_nudge(crate::progress::Nudge::PassingSnapshot))
                {
                    self.passing_told = true;
                    let (label, ..) = self.passing.clone().unwrap_or_default();
                    self.system_event(
                        "nudge",
                        json!({"kind": crate::progress::Nudge::PassingSnapshot.as_str(), "label": label}),
                    );
                    self.push_user(
                        Message::user_text(format!(
                            "<system-reminder>\n{note} If you can't fix the failure, put that version back rather \
                             than finish on a failing one, or say why the failing one is right.\n</system-reminder>"
                        )),
                        true,
                        None,
                        true,
                    );
                    continue;
                }
            }

            // Verification loop (GOALS pillar 3): no finishing on unchecked changes.
            if self.cfg.reminders && stop_reason != Some(StopReason::Refusal) {
                if let Some(text) = self.verification_reminder().await {
                    self.push_user(Message::user_text(text), true, None, true);
                    continue;
                }
            }

            // A reply that ended with no text and no tool call answers nothing, whatever ended it: ask
            // for the next step or the answer instead of ending the turn. Same budget as the
            // reasoning cut-offs: three in a row with no tool call between them end the turn.
            if self.cfg.reminders
                && !reply_has_text
                && !was_continuation
                && stop_reason != Some(StopReason::Refusal)
                && reasoning_cutoffs < MAX_REASONING_CUTOFFS
            {
                reasoning_cutoffs += 1;
                let (text, kept) = self.act_reminder(EMPTY_REPLY_PROMPT, EMPTY_REPLY, &model, &reasoning);
                self.system_event("act_reminder", json!({"kind": "empty_reply", "reasoning_chars": kept}));
                self.push_user(Message::user_text(text), true, None, true);
                continue;
            }

            // Unattended: what still runs in the background is stopped when the session ends,
            // right after this turn, so a service the task needs would be gone before anything
            // checks it (pypi-server, 9ef0955). Said once, with what is still running.
            if self.cfg.reminders
                && self.cfg.autonomous
                && !self.cfg.is_subagent
                && !background_reminded
                && stop_reason != Some(StopReason::Refusal)
            {
                if let Some(text) = self.background_reminder() {
                    background_reminded = true;
                    self.push_user(Message::user_text(text), true, None, true);
                    continue;
                }
            }

            // Stop / SubagentStop hook (contract C11: exit 2 makes the model keep going).
            let stop_event = if self.cfg.is_subagent { HookEvent::SubagentStop } else { HookEvent::Stop };
            let o = self
                .shared
                .hooks
                .run(stop_event, None, self.mode_str(), json!({"stop_hook_active": stop_hook_active}), &cancel)
                .await;
            for m in &o.user_messages {
                self.notice(NoticeLevel::Warning, m.clone());
            }
            if let (Some(feedback), None) = (o.blocked.clone(), o.stop.clone()) {
                if stop_event.exit2_effect() == Exit2Effect::BlockToModel {
                    stop_hook_active = true;
                    self.push_user(Message::user_text(format!("Stop hook feedback:\n{feedback}")), true, None, true);
                    continue;
                }
            }
            // `continue: false` from a Stop hook: the run stops here, nothing follows it.
            if let Some(reason) = o.stop.clone() {
                self.notice(NoticeLevel::Info, format!("Stopped by hook: {reason}"));
                turn.stop = Some(reason);
                return self.finish(
                    started,
                    turn,
                    ResultSubtype::Success,
                    last_text,
                    Some("hook_stopped".into()),
                    None,
                );
            }
            let reason = match stop_reason {
                Some(StopReason::EndTurn) | None => "end_turn",
                Some(StopReason::MaxTokens) => "max_tokens",
                Some(StopReason::StopSequence) => "stop_sequence",
                Some(StopReason::Refusal) => "refusal",
                Some(StopReason::ModelContextWindowExceeded) => "model_context_window_exceeded",
                Some(StopReason::ToolUse) => "tool_use",
                Some(StopReason::PauseTurn) => "pause_turn",
                Some(StopReason::Other) => "other",
            };
            // Ended with no answer: the last replies were all reasoning, and the reminders to act
            // are used up. "Success" would tell a script the work was done (write-compressor,
            // 9ef0955: four replies cut off while reasoning, nothing written, reported as success).
            if !reply_has_text && !was_continuation && stop_reason != Some(StopReason::Refusal) {
                self.notice(NoticeLevel::Error, NO_ANSWER);
                turn.errors.push(NO_ANSWER.into());
                return self.finish(
                    started,
                    turn,
                    ResultSubtype::ErrorDuringExecution,
                    None,
                    Some(reason.into()),
                    None,
                );
            }
            return self.finish(started, turn, ResultSubtype::Success, last_text, Some(reason.into()), None);
        }
    }

    /// Tool calls whose arguments weren't valid JSON (GOALS pillar 4). A call cut off by
    /// `max_tokens` stays marked as cut off. One the model ended itself was written invalid:
    /// it is repaired when a small, safe fix makes it an object (a missing comma, quote or
    /// brace, raw newlines in a string), and otherwise marked invalid, so the model is told
    /// what was wrong instead of being told to write less.
    fn settle_invalid_tool_inputs(&self, content: &mut [ContentBlock], stop_reason: Option<StopReason>) {
        if stop_reason == Some(StopReason::MaxTokens) {
            return;
        }
        for block in content.iter_mut() {
            let ContentBlock::ToolUse { name, input, .. } = block else { continue };
            let Some(problem) = input.get(kodact_api::TRUNCATED_INPUT).and_then(Value::as_str).map(str::to_string)
            else {
                continue;
            };
            let raw = input.get(kodact_api::RAW_INPUT).and_then(Value::as_str).unwrap_or_default().to_string();
            if let Some(fixed) = kodact_api::repair_json(&raw) {
                self.system_event("tool_input_repaired", json!({"tool": name, "problem": problem}));
                *input = Value::Object(fixed);
            } else {
                let problem = problem.replacen("incomplete JSON", "invalid JSON", 1);
                *input = json!({ kodact_api::INVALID_INPUT: problem, kodact_api::RAW_INPUT: raw });
            }
        }
    }

    /// A heads-up when this run's turn, spending or time limit is close (GOALS pillar 5), so the model
    /// wraps up with a report instead of being cut off mid-change.
    fn budget_warning(&self, calls: u32, started: Instant) -> Option<String> {
        let pace = self.cfg.nudges.pace && !self.cfg.is_subagent;
        let turns_left = self.cfg.max_turns.filter(|m| *m >= 6).map(|m| m.saturating_sub(calls)).filter(|l| *l <= 3);
        let money_left = self
            .cfg
            .max_budget_usd
            .map(|b| b - self.state.total_cost_usd)
            .filter(|left| self.cfg.max_budget_usd.map(|b| *left <= b * 0.15).unwrap_or(false));
        // With `nudges.pace`: the steps the time left holds at this turn's pace so far.
        let steps = self
            .cfg
            .time_limit
            .filter(|_| pace)
            .and_then(|t| crate::progress::steps_left(t.remaining(), started.elapsed(), calls));
        let time_left = self
            .cfg
            .time_limit
            .map(|t| (t.remaining(), t.total))
            .filter(|(left, total)| crate::progress::time_warning_due(*left, *total, steps.map(|(n, _)| n)))
            .map(|(left, _)| left);
        let what = match (turns_left, money_left, time_left) {
            (Some(t), _, _) => {
                format!("{t} model call{} left in this run (--max-turns)", if t == 1 { "" } else { "s" })
            }
            (None, Some(m), _) => format!("${m:.2} of the run's budget left (--max-budget-usd)"),
            (None, None, Some(left)) => match steps {
                Some((n, per)) => format!(
                    "{} of the run's time left (--max-time); at this run's pace so far (about {} s a step), that \
                     is about {n} more step{}",
                    about(left),
                    per.as_secs().max(1),
                    if n == 1 { "" } else { "s" }
                ),
                None => format!("{} of the run's time left (--max-time)", about(left)),
            },
            (None, None, None) => return None,
        };
        // The files the request asks for that still don't exist: only what is on disk counts.
        let missing = if pace { self.watched.still_missing() } else { vec![] };
        let first = if missing.is_empty() {
            String::new()
        } else {
            format!(
                " These files the request asks for don't exist yet: {}. Only what is in them when the run ends \
                 counts: write your best version to them first, then improve it.",
                missing.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ")
            )
        };
        if pace && (steps.is_some() || !missing.is_empty()) {
            self.system_event(
                "nudge",
                json!({"kind": "pace", "steps_left": steps.map(|(n, _)| n), "missing": missing}),
            );
        }
        Some(format!(
            "<system-reminder>\nYou have {what}.{first} Wrap up: if the version now in place tested worse than one \
             you had before, put the better one back first. Then finish the most important change, check it, and \
             end with what is done, what was verified and what remains.\n</system-reminder>"
        ))
    }

    /// Background commands that exited since the model last heard of them: a note for it, and
    /// any check among them recorded for the verification loop.
    async fn background_exits(&mut self) -> Option<String> {
        let exited = self.shared.tool_ctx.shells.take_exited();
        if exited.is_empty() {
            return None;
        }
        let shells: Vec<Value> = exited
            .iter()
            .map(|sh| {
                let code = match sh.status() {
                    kodact_tools::shells::ShellStatus::Completed(code) => code,
                    _ => None,
                };
                json!({"id": sh.id(), "command": sh.command, "exit_code": code, "seconds": sh.runtime().as_secs()})
            })
            .collect();
        self.system_event("background_exit", json!({"shells": shells}));
        let mut refresh = false;
        for sh in &exited {
            if let Some(i) = self.verify.pending.iter().position(|(id, ..)| id == sh.id()) {
                let (_, command, writes_after) = self.verify.pending.remove(i);
                let passed = sh.status() == kodact_tools::shells::ShellStatus::Completed(Some(0));
                self.verify.write_mark = writes_after;
                self.verify.last_check = Some((command, !passed));
                refresh = true;
            }
        }
        if refresh {
            self.verify.fingerprint = self.worktree_fingerprint().await;
        }
        kodact_tools::builtin::exit_notice(&exited)
    }

    /// Before the turn ends: wait for commands that outlived their Bash timeout and still run,
    /// up to [`BACKGROUND_WAIT`] (and a minute short of the run's time limit), then say how they
    /// ended. `None` when nothing runs, no time is left, or the wait was interrupted.
    async fn wait_for_background(&mut self, cancel: &CancellationToken) -> Option<String> {
        let shells = self.shared.tool_ctx.shells.clone();
        let running = shells.awaited_running();
        if running.is_empty() {
            return None;
        }
        let mut limit = BACKGROUND_WAIT;
        if let Some(t) = self.cfg.time_limit {
            limit = limit.min(t.remaining().saturating_sub(Duration::from_secs(60)));
        }
        if limit < Duration::from_secs(5) {
            return None;
        }
        let ids: Vec<&str> = running.iter().map(|sh| sh.id()).collect();
        self.notice(NoticeLevel::Info, format!("Waiting for {} to finish (up to {})", ids.join(", "), about(limit)));
        self.system_event("background_wait", json!({"shells": ids, "limit_ms": limit.as_millis() as u64}));
        let started = std::time::Instant::now();
        let all = async {
            for sh in &running {
                sh.wait().await;
            }
        };
        tokio::select! {
            _ = all => {}
            _ = tokio::time::sleep(limit) => {}
            _ = cancel.cancelled() => return None,
        }
        let still: Vec<_> =
            running.iter().filter(|sh| sh.status() == kodact_tools::shells::ShellStatus::Running).cloned().collect();
        let notes: Vec<String> =
            [self.background_exits().await, kodact_tools::builtin::still_running_notice(&still, started.elapsed())]
                .into_iter()
                .flatten()
                .collect();
        (!notes.is_empty()).then(|| notes.join("\n"))
    }

    /// Commands started in the background that still run as an unattended turn ends, as a
    /// reminder that they will be stopped (`None` when there are none). Commands a Bash call moved
    /// to the background at its timeout aren't listed: the turn waits for those.
    fn background_reminder(&self) -> Option<String> {
        let running: Vec<_> = self
            .shared
            .tool_ctx
            .shells
            .list()
            .into_iter()
            .filter(|sh| sh.status() == kodact_tools::shells::ShellStatus::Running && !sh.is_awaited())
            .collect();
        if running.is_empty() {
            return None;
        }
        let ids: Vec<&str> = running.iter().map(|sh| sh.id()).collect();
        self.system_event("background_reminder", json!({"shells": ids}));
        let list = running
            .iter()
            .take(5)
            .map(|sh| {
                let c = sh.command.trim();
                let c = if c.chars().count() > 200 {
                    format!("{}…", c.chars().take(200).collect::<String>())
                } else {
                    c.to_string()
                };
                format!("`{c}` ({})", sh.id())
            })
            .collect::<Vec<_>>()
            .join(", ");
        Some(format!(
            "<system-reminder>\nStill running in the background: {list}. Background commands are stopped when the \
             session ends, which in an unattended run is right after you finish. If one of them must keep running \
             after that (a server or service the task needs), stop it with KillShell and start it again detached \
             from the session, for example `nohup <command> > /tmp/<name>.log 2>&1 &`, then check that it answers. \
             If none needs to keep running, finish.\n</system-reminder>"
        ))
    }

    /// The reminder after a reply that was all reasoning, and how many characters of that
    /// reasoning it hands back. A provider that sends reasoning back keeps it in the next request,
    /// and `plain` applies. One that doesn't (chat completions) drops it, and "do not restart the
    /// analysis" asks for what the model no longer has: write-compressor (9ef0955) began the same
    /// analysis of the decoder again in three cut-off replies in a row and wrote nothing. There,
    /// the reminder carries how the reasoning began and where it stopped.
    fn act_reminder(&self, plain: &str, what: &str, model: &str, reasoning: &str) -> (String, usize) {
        let reasoning = reasoning.trim();
        if reasoning.is_empty() || self.shared.provider.resends_reasoning(model) {
            return (plain.to_string(), 0);
        }
        let n = reasoning.chars().count();
        let (shown, kept) = if n <= REASONING_HEAD + REASONING_TAIL {
            (format!("Here it is:\n\n<reasoning>\n{reasoning}\n</reasoning>"), n)
        } else {
            let head: String = reasoning.chars().take(REASONING_HEAD).collect();
            let tail: String = reasoning.chars().skip(n - REASONING_TAIL).collect();
            let gap = n - REASONING_HEAD - REASONING_TAIL;
            (
                format!(
                    "Here is how it began and where it stopped ({gap} characters in between left out):\n\n\
                     <reasoning_start>\n{head}\n</reasoning_start>\n\n<reasoning_end>\n{tail}\n</reasoning_end>"
                ),
                REASONING_HEAD + REASONING_TAIL,
            )
        };
        let text = format!(
            "<system-reminder>\n{what} Your reasoning is not kept from one reply to the next. {shown}\n\n\
             Go on from where it stopped instead of starting over, and act now: take the next concrete step \
             (run a command, make an edit, or give your answer), then continue from its result.\n</system-reminder>"
        );
        (text, kept)
    }

    /// Is this tool's result decided by the call and the files alone? Then a call that failed
    /// the same way three times, with no file changed since, fails again. A name no tool has is
    /// one of them: sending it again never finds a tool.
    fn depends_on_files_only(&self, name: &str) -> bool {
        matches!(name, "Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "Read" | "Glob" | "Grep" | "LS")
            || self.shared.tools.get(name).is_none()
    }

    /// The session's language servers, when they are on and this is the main agent. Rebuilt
    /// when the setting changed mid-session.
    fn language_servers(&mut self) -> Option<Arc<crate::lsp::Manager>> {
        let cfg = self.cfg.language_servers.clone().filter(|_| !self.cfg.is_subagent);
        match (&self.lsp, cfg) {
            (_, None) => self.lsp = None,
            (Some(m), Some(cfg)) if *m.config() == cfg => {}
            (_, Some(cfg)) => {
                let root = self.shared.tool_ctx.project_dir.clone();
                self.lsp = Some(Arc::new(crate::lsp::Manager::new(cfg, root)));
            }
        }
        self.lsp.clone()
    }

    /// Diagnostics after edits (GOALS pillar 3), told with the edit's result: a file it left
    /// unparseable ([`crate::diagnose`]), or, when that is on, what a language server reports
    /// ([`crate::lsp`]). Nothing is added when neither finds an error.
    async fn diagnose_edits(
        &mut self,
        calls: &[(String, String, Value)],
        results: &mut [crate::exec::CallResult],
        model: &str,
    ) {
        let root = self.shared.tool_ctx.project_dir.clone();
        let servers = self.language_servers();
        // Room for a server's errors: a share of the model's window (a fiftieth of its tokens,
        // in characters), so a long list doesn't crowd a small window and isn't cut short on a
        // large one.
        let room = usize::try_from(self.context_window(model) / 50).unwrap_or(usize::MAX).clamp(1_500, 20_000);
        for ((_, name, input), r) in calls.iter().zip(results.iter_mut()) {
            // A shell command that rewrote a file the server already knows: checked as an edit
            // of it would be.
            if name == "Bash" && r.denial.is_none() {
                let read_only = self.shared.tools.get("Bash").map(|t| t.is_read_only(input)).unwrap_or(true);
                if let (false, Some(servers)) = (read_only, &servers) {
                    for report in servers.check_changed().await {
                        self.system_event(
                            "diagnostics",
                            json!({"server": report.server, "errors": report.problems.len(), "after": "Bash"}),
                        );
                        let text = report.text(&root, room);
                        match &mut r.output.content {
                            kodact_types::ToolResultContent::Text(s) => s.push_str(&text),
                            kodact_types::ToolResultContent::Blocks(b) => b.push(ContentBlock::text(text.trim_start())),
                        }
                    }
                }
                continue;
            }
            if !matches!(name.as_str(), "Edit" | "MultiEdit" | "Write") || r.output.is_error || r.denial.is_some() {
                continue;
            }
            let Some(raw) = input.get("file_path").and_then(Value::as_str) else { continue };
            let path = kodact_tools::expand_path(raw, &root);
            let shown = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
            // The file before the edit, when the tool gave it: what the formatter and the
            // language server compare the edit with.
            let st = r.output.structured.as_ref();
            let created = st.and_then(|s| s.get("type")).and_then(Value::as_str) == Some("create");
            let original = st.and_then(|s| s.get("originalFile")).and_then(Value::as_str).map(str::to_string);
            let mut formatted = String::new();
            if let Some(cfg) = &self.cfg.formatters {
                let done = crate::format::format(cfg, &root, &path, original.as_deref(), created).await;
                if let Some(f) = done.filter(|f| std::fs::write(&path, &f.text).is_ok()) {
                    // Still the model's own change: no new read is asked for before the next edit.
                    self.shared.tool_ctx.files.record_write(&path);
                    self.system_event("formatted", json!({"file": shown, "formatter": f.formatter, "lines": f.lines}));
                    formatted = f.note(&shown);
                }
            }
            let parse = if self.cfg.edit_diagnostics { crate::diagnose::check(&path).await } else { None };
            let found = match (parse, &servers) {
                (Some(d), _) => {
                    self.system_event("diagnostics", json!({"file": shown, "parser": d.parser}));
                    Some(d.text(&shown))
                }
                // A file that parses: what a compiler would say of it, beyond what it said
                // before the edit.
                (None, Some(servers)) => {
                    let baseline = if created { Some("") } else { original.as_deref() };
                    servers.check(&path, baseline).await.map(|report| {
                        self.system_event(
                            "diagnostics",
                            json!({"file": shown, "server": report.server, "errors": report.problems.len()}),
                        );
                        report.text(&root, room)
                    })
                }
                (None, None) => None,
            };
            let text = format!("{formatted}{}", found.unwrap_or_default());
            if text.is_empty() {
                continue;
            }
            match &mut r.output.content {
                kodact_types::ToolResultContent::Text(s) => s.push_str(&text),
                kodact_types::ToolResultContent::Blocks(b) => b.push(ContentBlock::text(text.trim_start())),
            }
        }
    }

    fn verifying(&self) -> bool {
        self.cfg.verify.is_some() && !self.cfg.is_subagent
    }

    /// The conversation was rewritten (compaction, rewind, `/clear`): the model no longer sees
    /// earlier tool results. Forget them where KodAct tracks them, so re-reading a file is neither
    /// refused as unread-before-write nor called a loop by the guard.
    fn forget_seen_results(&mut self) {
        self.shared.tool_ctx.files.forget_all_views();
        self.guard = Default::default();
    }

    /// The verification loop's view of the files: the worktree's files in a git repository,
    /// plus the files written this turn (so a shell rewrite of one is seen outside git too).
    async fn worktree_fingerprint(&self) -> Option<u64> {
        let dir = self.shared.tool_ctx.project_dir.clone();
        let written = self.shared.history.writes_since(0);
        tokio::task::spawn_blocking(move || kodact_git::worktree_fingerprint(&dir, &written)).await.ok().flatten()
    }

    /// Note checks and possible shell writes among the calls just run.
    /// The project's check commands for this turn: the configured or detected ones, and with
    /// `nudges.namedChecks`, the checks the person's request names (first: they asked for them).
    fn check_commands(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        if self.cfg.reminders && self.cfg.nudges.named_checks {
            let root = &self.shared.tool_ctx.project_dir;
            out.extend(self.named.commands.iter().filter(|c| crate::verify::named_check(c, root)).cloned());
        }
        for c in self.cfg.verify.iter().flat_map(|v| v.commands.iter()) {
            if !out.contains(c) {
                out.push(c.clone());
            }
        }
        out
    }

    async fn track_verification(&mut self, calls: &[(String, String, Value)], results: &[crate::exec::CallResult]) {
        let Some(mut vc) = self.cfg.verify.clone() else { return };
        vc.commands = self.check_commands();
        let mut refresh = false;
        for ((_, name, input), r) in calls.iter().zip(results) {
            // A file read back: plain data read after its last change has been checked.
            if r.denial.is_none() && !r.output.is_error {
                if name == "Read" {
                    if let Some(p) = input.get("file_path").and_then(Value::as_str) {
                        self.verify.read_back.push((p.to_string(), r.writes_after));
                    }
                } else if name == "Bash" {
                    let command = input.get("command").and_then(Value::as_str).unwrap_or_default();
                    for f in crate::verify::files_viewed(command) {
                        self.verify.read_back.push((f, r.writes_after));
                    }
                }
            }
            if name != "Bash"
                || r.denial.is_some()
                || input.get("run_in_background").and_then(Value::as_bool) == Some(true)
            {
                continue;
            }
            let command = input.get("command").and_then(Value::as_str).unwrap_or_default();
            let changed = self.shared.history.writes_since(0);
            // A command that outlived its timeout runs on in the background: a check counts when
            // it exits (`background_exits`).
            if let Some(id) =
                r.output.structured.as_ref().and_then(|s| s.get("backgroundTaskId")).and_then(Value::as_str)
            {
                if crate::verify::is_check(command, &vc.commands, &changed) {
                    self.verify.pending.push((id.to_string(), command.to_string(), r.writes_after));
                } else if !self.shared.tools.get("Bash").map(|t| t.is_read_only(input)).unwrap_or(true) {
                    self.verify.shell_may_have_changed = true;
                }
                continue;
            }
            // Running the program a passing build made: it has been seen to work, or not.
            if self.verify.built.as_ref().is_some_and(|(_, a)| crate::verify::runs_artifact(command, a)) {
                self.verify.built = None;
            }
            if crate::verify::is_check(command, &vc.commands, &changed) {
                if !r.output.is_error {
                    self.save_passing(command).await;
                }
                self.verify.write_mark = r.writes_after;
                self.verify.shell_may_have_changed = false;
                self.verify.last_check = Some((command.to_string(), r.output.is_error));
                // A passing build is remembered until its program runs; any other passing check
                // (tests, a run of the program) stands on its own.
                if !r.output.is_error {
                    // `gcc -o app app.c && ./app` builds and runs it in one go.
                    self.verify.built = crate::verify::built_artifact(command)
                        .filter(|a| !crate::verify::runs_artifact(command, a))
                        .map(|a| (command.to_string(), a));
                }
                refresh = true;
            } else if !self.shared.tools.get("Bash").map(|t| t.is_read_only(input)).unwrap_or(true) {
                self.verify.shell_may_have_changed = true;
            }
        }
        if refresh {
            self.verify.fingerprint = self.worktree_fingerprint().await;
        }
    }

    /// Where the turn's changes stand now (`None` when the verification loop is off): what the
    /// reminder is decided on, and what the result reports.
    async fn verification_state(&self) -> Option<crate::verify::Verified> {
        use crate::verify::Verified;
        if !self.verifying() {
            return None;
        }
        let writes = self.shared.history.writes_len();
        let root = &self.shared.tool_ctx.project_dir;
        let history = &self.shared.history;
        let read_since_change = |p: &std::path::Path| {
            crate::verify::is_data_file(p) && {
                let last = history.last_write_len(p);
                self.verify.read_back.iter().any(|(arg, at)| *at >= last && crate::verify::names_file(p, arg))
            }
        };
        let files: Vec<String> = history
            .writes_since(self.verify.write_mark)
            .iter()
            .filter(|p| !read_since_change(p))
            .map(|p| p.strip_prefix(root).unwrap_or(p).display().to_string())
            .collect();
        let shell = files.is_empty()
            && self.verify.shell_may_have_changed
            && self.verify.fingerprint.is_some()
            && self.worktree_fingerprint().await != self.verify.fingerprint;
        Some(if !files.is_empty() || shell {
            Verified::Unchecked { files, shell }
        } else {
            match &self.verify.last_check {
                Some((command, true)) => Verified::Failed { command: command.clone() },
                Some((command, false)) => Verified::Passed { command: command.clone() },
                None if writes > 0 => Verified::ReadBack,
                None => Verified::NoChanges,
            }
        })
    }

    /// Did this step make progress? A file the request names changed on disk (by any tool, a
    /// script's output included), or a check passed. When the request asks for no new file,
    /// any file written counts too, and when it names no file at all, so does a shell command
    /// that changed something. Helper scripts and scratch files written while the files the
    /// request asks for don't exist yet are not progress. A failed or refused call made nothing.
    fn made_progress(
        &mut self,
        calls: &[(String, String, Value)],
        results: &[crate::exec::CallResult],
        writes_before: usize,
        named_changed: bool,
    ) -> bool {
        if named_changed {
            return true;
        }
        let any_write = !self.watched.asks_for_new();
        if any_write && self.shared.history.writes_len() > writes_before {
            return true;
        }
        let configured = self.check_commands();
        let changed = self.shared.history.writes_since(0);
        calls.iter().zip(results).any(|((_, name, input), r)| {
            if r.denial.is_some() || r.output.is_error {
                return false;
            }
            match name.as_str() {
                "Write" | "Edit" | "MultiEdit" | "NotebookEdit" => any_write,
                "Bash" => {
                    let command = input.get("command").and_then(Value::as_str).unwrap_or_default();
                    crate::verify::is_check(command, &configured, &changed)
                        || (!self.named.names_files()
                            && !self.shared.tools.get("Bash").map(|t| t.is_read_only(input)).unwrap_or(true))
                }
                _ => false,
            }
        })
    }

    /// Shell output held against the limits the request states (`nudges.limitVerdicts`), and a
    /// result meeting all of them noted while the files the request asks for are unchanged
    /// (`nudges.staleDeliverables`).
    fn judge_limits(
        &mut self,
        calls: &[(String, String, Value)],
        results: &mut [crate::exec::CallResult],
        step: u32,
        named_changed: bool,
    ) {
        let (verdicts_on, stale_on) = (self.cfg.nudges.limit_verdicts, self.cfg.nudges.stale_deliverables);
        if !self.cfg.reminders || self.cfg.is_subagent || !(verdicts_on || stale_on) || self.limits.is_empty() {
            return;
        }
        let limits = std::mem::take(&mut self.limits);
        for ((_, name, input), r) in calls.iter().zip(results.iter_mut()) {
            if name != "Bash" || r.denial.is_some() {
                continue;
            }
            let command = input.get("command").and_then(Value::as_str).unwrap_or_default();
            let measured = crate::limits::measurements(&r.output.text_content());
            let judged: Vec<(crate::limits::Verdict, &crate::limits::Measured)> =
                measured.iter().filter_map(|m| limits.judge(m, command).map(|v| (v, m))).collect();
            if judged.is_empty() {
                continue;
            }
            // A command that names a file the request asks for measured that file: nothing unsaved.
            if stale_on
                && self.named.names_files()
                && !named_changed
                && self.unsaved.is_none()
                && !self.watched.named_in(command)
            {
                if let Some((v, _)) = judged.iter().find(|(v, _)| v.all_met()) {
                    self.unsaved = Some((step, command.to_string(), v.line.clone(), v.scope.clone()));
                }
            }
            if !verdicts_on || !self.may_nudge(crate::progress::Nudge::LimitVerdict) {
                continue;
            }
            let scoped: Vec<(Option<String>, &crate::limits::Measured)> =
                judged.iter().map(|(v, m)| (v.scope.clone(), *m)).collect();
            let globals = limits.global_counts(&scoped);
            let verdicts: Vec<crate::limits::Verdict> = judged.iter().map(|(v, _)| v.clone()).collect();
            self.system_event(
                "nudge",
                json!({
                    "kind": crate::progress::Nudge::LimitVerdict.as_str(),
                    "lines": verdicts.len(),
                    "met": verdicts.iter().filter(|v| v.all_met()).count(),
                    "across": globals.iter().map(|(g, _)| g.name.clone()).collect::<Vec<_>>(),
                }),
            );
            let text = crate::limits::text(&verdicts, &globals);
            match &mut r.output.content {
                kodact_types::ToolResultContent::Text(s) => s.push_str(&text),
                kodact_types::ToolResultContent::Blocks(blocks) => blocks.push(ContentBlock::text(text.trim_start())),
            }
        }
        self.limits = limits;
    }

    /// A file the request states a size limit for went over it in this step (`nudges.constraints`):
    /// said with the result of the call that wrote it, while the model can still act on it, not
    /// only before the turn ends (a run cut by its time limit never gets there). Three times a
    /// turn per file at most.
    fn size_limits_told(
        &mut self,
        calls: &[(String, String, Value)],
        results: &mut [crate::exec::CallResult],
        changed: &[String],
    ) {
        /// Notes per file a turn: one for each of the first few writes that leave it too large.
        const MAX_PER_FILE: u32 = 3;
        if !self.cfg.reminders || self.cfg.is_subagent || !self.cfg.nudges.constraints || changed.is_empty() {
            return;
        }
        let project = self.shared.tool_ctx.project_dir.clone();
        for (file, size, max, said) in self.named.over_limit(&project) {
            if !changed.contains(&file) || self.size_told.get(&file).copied().unwrap_or(0) >= MAX_PER_FILE {
                continue;
            }
            if !self.may_nudge(crate::progress::Nudge::SizeLimit) {
                continue;
            }
            *self.size_told.entry(file.clone()).or_default() += 1;
            self.system_event(
                "nudge",
                json!({"kind": crate::progress::Nudge::SizeLimit.as_str(), "file": file, "size": size, "limit": max}),
            );
            // The call that wrote it: a file tool on that path, else a shell command naming it,
            // else the step's last call (a script it ran wrote the file).
            let path = if std::path::Path::new(&file).is_absolute() {
                std::path::PathBuf::from(&file)
            } else {
                project.join(&file)
            };
            let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| file.clone());
            let wrote = calls.iter().position(|(_, tool, input)| {
                matches!(tool.as_str(), "Write" | "Edit" | "MultiEdit" | "NotebookEdit")
                    && ["file_path", "notebook_path"].iter().filter_map(|k| input.get(*k).and_then(Value::as_str)).any(
                        |p| {
                            let p = std::path::Path::new(p);
                            (if p.is_absolute() { p.to_path_buf() } else { project.join(p) }) == path
                        },
                    )
            });
            let named_by = || {
                calls.iter().rposition(|(_, tool, input)| {
                    tool == "Bash" && input.get("command").and_then(Value::as_str).is_some_and(|c| c.contains(&name))
                })
            };
            let Some(at) = wrote.or_else(named_by).or(calls.len().checked_sub(1)) else { continue };
            let text = format!(
                "\n\n<system-reminder>\n`{file}` is now {size} bytes; the request says {said} ({max} bytes at most): \
                 {} bytes over.\n</system-reminder>",
                size - max
            );
            match &mut results[at].output.content {
                kodact_types::ToolResultContent::Text(s) => s.push_str(&text),
                kodact_types::ToolResultContent::Blocks(blocks) => blocks.push(ContentBlock::text(text.trim_start())),
            }
        }
    }

    /// A result met every limit for its row some steps ago and the files the request asks for
    /// haven't changed since (`nudges.staleDeliverables`): twice a turn at most, and once more
    /// when the model would finish.
    fn stale_note(&mut self, step: u32, at_end: bool) -> Option<String> {
        /// Steps after such a result before it is called unsaved: time to write it.
        const STALE_AFTER: u32 = 3;
        if !self.cfg.reminders || self.cfg.is_subagent || !self.cfg.nudges.stale_deliverables {
            return None;
        }
        let (at, command, line, scope) = self.unsaved.clone()?;
        let due = if at_end { !self.stale_end_told } else { self.stale_told < 2 && step >= at + STALE_AFTER };
        if !due || !self.may_nudge(crate::progress::Nudge::StaleDeliverable) {
            return None;
        }
        if at_end {
            self.stale_end_told = true;
        } else {
            self.stale_told += 1;
            // Said again only after as many more steps.
            self.unsaved = Some((step, command.clone(), line.clone(), scope.clone()));
        }
        let files = self.watched.names_for(scope.as_deref());
        self.system_event(
            "nudge",
            json!({
                "kind": crate::progress::Nudge::StaleDeliverable.as_str(),
                "steps": step.saturating_sub(at),
                "files": files,
                "at_end": at_end,
            }),
        );
        let row = scope.map(|s| format!(" for {s}")).unwrap_or_default();
        let files = files.iter().map(|f| format!("`{f}`")).collect::<Vec<_>>().join(", ");
        Some(format!(
            "<system-reminder>\n`{command}` printed a result that meets every limit the request states{row}:\n               {line}\n{files} have not changed since. If it is the best result so far, write it to the file now, so              the file holds it whatever happens next. Ignore this if the file already holds it.\n</system-reminder>"
        ))
    }

    /// Before a shell command that may write runs: the files it names (`nudges.shellEdit`).
    fn shell_edit_before(&self, name: &str, input: &Value) -> Option<crate::shell_edit::Before> {
        if name != "Bash" || !self.cfg.reminders || self.cfg.is_subagent || !self.cfg.nudges.shell_edit {
            return None;
        }
        if self.shared.tools.get("Bash").map(|t| t.is_read_only(input)).unwrap_or(true)
            || input.get("run_in_background").and_then(Value::as_bool) == Some(true)
        {
            return None;
        }
        let command = input.get("command").and_then(Value::as_str)?;
        Some(crate::shell_edit::Before::note(command, &self.shared.tool_ctx.project_dir)).filter(|b| !b.is_empty())
    }

    /// After shell commands that changed files: a changed file that no longer parses is told
    /// with the command's result, and the first time in a turn, that Edit suits careful changes.
    async fn shell_edits_parse(
        &mut self,
        calls: &[(String, String, Value)],
        results: &mut [crate::exec::CallResult],
        before: &[Option<crate::shell_edit::Before>],
    ) {
        let root = self.shared.tool_ctx.project_dir.clone();
        for ((_, r), b) in calls.iter().zip(results.iter_mut()).zip(before) {
            let Some(b) = b else { continue };
            if r.denial.is_some() {
                continue;
            }
            for path in b.changed() {
                let Some(d) = crate::diagnose::check(&path).await else { continue };
                let shown = path.strip_prefix(&root).unwrap_or(&path).display().to_string();
                let mut text = d.text(&shown);
                if !self.shell_edit_told && self.may_nudge(crate::progress::Nudge::ShellEdit) {
                    self.shell_edit_told = true;
                    self.system_event(
                        "nudge",
                        json!({"kind": crate::progress::Nudge::ShellEdit.as_str(), "file": shown, "parser": d.parser}),
                    );
                    text.push_str(&crate::shell_edit::suggestion(&shown));
                }
                match &mut r.output.content {
                    kodact_types::ToolResultContent::Text(s) => s.push_str(&text),
                    kodact_types::ToolResultContent::Blocks(blocks) => {
                        blocks.push(ContentBlock::text(text.trim_start()))
                    }
                }
            }
        }
    }

    /// After a check passed with changes since the last saved passing version: save this one
    /// with the Checkpoint tool (`nudges.passingSnapshots`). Saving only: KodAct never
    /// restores it by itself.
    async fn save_passing(&mut self, command: &str) {
        if !self.cfg.reminders || self.cfg.is_subagent || !self.cfg.nudges.passing_snapshots {
            return;
        }
        let writes = self.shared.history.writes_len();
        if writes == 0 || self.passing.as_ref().is_some_and(|(_, _, at)| *at == writes) {
            return;
        }
        let Some(tool) = self.shared.tools.get("Checkpoint") else { return };
        let ctx = &self.shared.tool_ctx;
        let Some(label) = (1..=99)
            .map(|n| format!("passing-{n}"))
            .find(|l| tool.validate(&json!({"action": "save", "label": l}), ctx).is_ok())
        else {
            return;
        };
        let out = tool.call(json!({"action": "save", "label": label}), ctx).await;
        if out.is_error {
            return;
        }
        self.system_event("passing_snapshot", json!({"label": label, "command": command}));
        self.passing = Some((label, command.to_string(), writes));
    }

    /// Where the passing version is, when the files have changed since and the last check
    /// failed: for the model before it finishes, and for the result of a run cut by its limit.
    fn passing_note(&self) -> Option<String> {
        let (label, passed, at) = self.passing.as_ref()?;
        let (failed, true) = self.verify.last_check.as_ref()? else { return None };
        (self.shared.history.writes_len() > *at).then(|| {
            format!(
                "The version that passed `{passed}` is saved as checkpoint `{label}`; the files now fail `{failed}`. \
                 `Checkpoint` with action `restore` and label `{label}` brings that version back."
            )
        })
    }

    /// May this nudge be given now? With `nudges.backOff`, not once it has twice been followed
    /// by progress in this run. Giving it is noted, for the count.
    fn may_nudge(&mut self, kind: crate::progress::Nudge) -> bool {
        if !self.cfg.nudges.back_off {
            return true;
        }
        if !self.back_off.allows(kind) {
            return false;
        }
        self.back_off.nudged(kind);
        true
    }

    /// Progress was made this step: count it for back-off, and log the nudges it silences.
    fn progress_for_back_off(&mut self) {
        if !self.cfg.nudges.back_off {
            return;
        }
        for kind in self.back_off.progress() {
            self.system_event("nudge_silenced", json!({"kind": kind.as_str()}));
        }
    }

    /// The run's budgets, for the pacing nudges.
    fn run_budget(&self) -> crate::progress::Budget {
        crate::progress::Budget {
            max_calls: self.cfg.max_turns,
            time: self.cfg.time_limit.map(|t| t.total),
            usd: self.cfg.max_budget_usd,
        }
    }

    /// Count this step for the pacing nudges, and give one when it is due (`may_nudge`: no
    /// other reminder was given this step).
    fn progress_nudge(&mut self, made: bool, may_nudge: bool) -> Option<String> {
        let n = &self.cfg.nudges;
        if !self.cfg.reminders || self.cfg.is_subagent || !(n.progress || n.progress_early) {
            return None;
        }
        let cost = self.state.total_cost_usd;
        if made {
            self.progress.made(cost);
            return None;
        }
        self.progress.step();
        if !may_nudge {
            return None;
        }
        let budget = self.run_budget();
        let (kind, stretch) = self.progress.due(&budget, cost, n.progress_early, n.progress)?;
        if !self.may_nudge(kind) {
            return None;
        }
        self.system_event("nudge", crate::progress::record(kind, stretch));
        Some(crate::progress::text(kind, stretch, budget.exists()))
    }

    /// The reminder to send before the turn may end, if one is due.
    async fn verification_reminder(&mut self) -> Option<String> {
        let mut vc = self.cfg.verify.clone()?;
        vc.commands = self.check_commands();
        if self.cfg.is_subagent || self.verify.reminders >= vc.max_reminders {
            return None;
        }
        let writes = self.shared.history.writes_len();
        let changed_this_turn = writes > 0 || self.verify.shell_may_have_changed;
        if !changed_this_turn || self.shared.tools.get("Bash").is_none() {
            return None;
        }
        // Checks that can't run make a reminder a wasted call.
        let probe = vc.commands.first().cloned().unwrap_or_else(|| "true".into());
        let req = kodact_permissions::Request::new("Bash", kodact_permissions::Subject::Command(probe), false);
        if matches!(self.shared.permissions.read().unwrap().decide(&req), kodact_permissions::Decision::Deny { .. }) {
            return None;
        }
        // One reminder per set of changes: after one, another comes only once something changed
        // since. Without this, a re-check the loop doesn't recognize brought the same reminder
        // back: 6 of the 8 trials reminded in 090446d got it twice for files that hadn't changed.
        if let Some((at, fingerprint)) = self.verify.reminded_at {
            let shell_changed = self.verify.shell_may_have_changed
                && fingerprint.is_some()
                && self.worktree_fingerprint().await != fingerprint;
            if writes == at && !shell_changed {
                return None;
            }
        }
        let reminder = match self.verification_state().await? {
            crate::verify::Verified::Unchecked { files, shell } => crate::verify::Reminder::Unchecked { files, shell },
            // The passing-version note already said the files fail this check, and what to do.
            crate::verify::Verified::Failed { .. } if self.passing_told => return None,
            crate::verify::Verified::Failed { command } if writes > 0 => {
                crate::verify::Reminder::LastCheckFailed { command }
            }
            crate::verify::Verified::Passed { .. } if writes > 0 => match self.verify.built.clone() {
                Some((command, artifact)) => crate::verify::Reminder::BuiltNotRun { command, artifact },
                None => return None,
            },
            _ => return None,
        };
        self.verify.reminders += 1;
        self.verify.reminded_at = Some((writes, self.worktree_fingerprint().await));
        self.system_event("verification", reminder.record());
        self.notice(NoticeLevel::Info, "Changes not verified yet: asking the model to run the project's checks.");
        Some(reminder.text(&vc.commands))
    }

    fn finish(
        &mut self,
        started: Instant,
        mut turn: TurnAcc,
        subtype: ResultSubtype,
        result: Option<String>,
        stop_reason: Option<String>,
        prompt_blocked: Option<String>,
    ) -> TurnResult {
        // A turn cut by `--max-time` ends as the limit: an error a script can tell from success.
        let (subtype, stop_reason) = if stop_reason.as_deref() == Some("interrupted")
            && self.time_up.load(std::sync::atomic::Ordering::SeqCst)
        {
            let text = "Reached the time limit (--max-time): the run stopped; the work done so far is left as it is.";
            self.notice(NoticeLevel::Error, text.to_string());
            turn.errors.push(text.to_string());
            (ResultSubtype::ErrorDuringExecution, Some("max_time".to_string()))
        } else {
            (subtype, stop_reason)
        };
        // A run cut short (a limit, an error) with a passing version saved and failing files:
        // the result says where that version is. It is reported, never put back silently.
        if subtype != ResultSubtype::Success {
            if let Some(note) = self.passing_note() {
                self.notice(NoticeLevel::Warning, note.clone());
                turn.errors.push(note);
            }
        }
        let structured_output = if self.cfg.json_schema.is_some() && subtype == ResultSubtype::Success {
            result.as_deref().and_then(|r| serde_json::from_str::<Value>(r.trim()).ok())
        } else {
            None
        };
        let is_error = subtype != ResultSubtype::Success;
        self.publish();
        TurnResult {
            subtype,
            is_error,
            result,
            stop_reason,
            num_turns: turn.api_calls,
            duration_ms: started.elapsed().as_millis() as u64,
            duration_api_ms: turn.api_ms,
            usage: turn.usage,
            total_cost_usd: self.state.total_cost_usd,
            model_usage: self.state.model_usage.clone(),
            permission_denials: turn.denials,
            errors: turn.errors,
            structured_output,
            prompt_blocked,
            fatal: false,
            auth_failed: false,
            tool_calls: turn.tool_calls,
            verification: None,
        }
    }

    /// End the session: SessionEnd hooks, background shells.
    pub async fn end_session(&self, reason: &str) {
        let c = CancellationToken::new();
        let _ = tokio::time::timeout(
            Duration::from_secs(70),
            self.shared.hooks.run(HookEvent::SessionEnd, None, self.mode_str(), json!({"reason": reason}), &c),
        )
        .await;
        self.shared.tool_ctx.shells.kill_all();
    }
}

/// The reasoning a reply's content holds, in order.
fn reasoning_of(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Two names for one model: equal, or the same once aliases resolve, or one
/// a dated id of the other ("claude-x" and "claude-x-20250101").
fn same_model(a: &str, b: &str) -> bool {
    let (a, b) = (kodact_api::resolve_model(a), kodact_api::resolve_model(b));
    a == b || a.starts_with(&b) || b.starts_with(&a)
}
