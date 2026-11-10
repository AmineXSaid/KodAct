//! Tool execution (contract C2): batches, per-tool pipelines, serialized prompts.

use std::path::Path;
use std::sync::Arc;

use kodact_hooks::HookEvent;
use kodact_permissions::{Decision, PermissionMode, Request, Rule, Subject, Suggestion};
use kodact_tools::{Checkpointer, ToolOutput, INTERRUPTED};
use kodact_types::sdk::PermissionDenial;
use kodact_types::ContentBlock;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::engine::Shared;
use crate::events::{EngineEvent, NoticeLevel, PermissionAnswer, PermissionPrompt};

/// Adapter: the session's file history is the tools' checkpointer (contract C4).
pub struct HistoryCheckpointer(pub Arc<kodact_session::FileHistory>);

impl Checkpointer for HistoryCheckpointer {
    fn before_write(&self, path: &Path) {
        self.0.snapshot(path);
    }
}

/// Characters of tool output kept in a result when the tool sets no limit of its own.
const GENERIC_OUTPUT_BUDGET: usize = 50_000;

pub(crate) struct CallResult {
    pub id: String,
    pub output: ToolOutput,
    pub denial: Option<PermissionDenial>,
    /// The person denied with "interrupt": stop the whole turn.
    pub interrupt_turn: bool,
    /// A hook asked to stop the run (`continue: false`).
    pub stop: Option<String>,
    /// File-history writes this turn once the call's batch finished (verification loop).
    pub writes_after: usize,
}

impl CallResult {
    pub(crate) fn plain(id: &str, output: ToolOutput) -> Self {
        CallResult { id: id.to_string(), output, denial: None, interrupt_turn: false, stop: None, writes_after: 0 }
    }
}

/// Split calls into batches: runs of concurrency-safe calls, or single unsafe calls.
pub(crate) fn batches(shared: &Shared, calls: &[(String, String, Value)]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = vec![];
    let mut run: Vec<usize> = vec![];
    for (i, (_, name, input)) in calls.iter().enumerate() {
        let safe = shared.tools.get(name).map(|t| t.is_concurrency_safe(input)).unwrap_or(true);
        if safe {
            run.push(i);
        } else {
            if !run.is_empty() {
                out.push(std::mem::take(&mut run));
            }
            out.push(vec![i]);
        }
    }
    if !run.is_empty() {
        out.push(run);
    }
    out
}

/// Run every call of one assistant message; results come back in call order.
pub(crate) async fn run_tools(
    shared: &Shared,
    calls: &[(String, String, Value)],
    cancel: &CancellationToken,
    mode: &str,
) -> Vec<CallResult> {
    let mut slots: Vec<Option<CallResult>> = (0..calls.len()).map(|_| None).collect();
    for batch in batches(shared, calls) {
        let futs = batch.iter().map(|&i| {
            let (id, name, input) = &calls[i];
            async move { (i, run_one(shared, id, name, input.clone(), cancel, mode).await) }
        });
        let done = futures::future::join_all(futs).await;
        let writes = shared.history.writes_len();
        for (i, mut r) in done {
            r.writes_after = writes;
            slots[i] = Some(r);
        }
    }
    slots
        .into_iter()
        .enumerate()
        .map(|(i, r)| r.unwrap_or_else(|| CallResult::plain(&calls[i].0, ToolOutput::error(INTERRUPTED))))
        .collect()
}

fn denial(name: &str, id: &str, input: &Value) -> Option<PermissionDenial> {
    Some(PermissionDenial { tool_name: name.into(), tool_use_id: id.into(), tool_input: input.clone() })
}

fn blocked_path(subject: &Subject) -> Option<String> {
    match subject {
        Subject::Path { path, .. } => Some(path.display().to_string()),
        _ => None,
    }
}

/// One call's pipeline: validate → rules → PreToolUse → (prompt) → execute → PostToolUse.
async fn run_one(
    shared: &Shared,
    id: &str,
    name: &str,
    mut input: Value,
    cancel: &CancellationToken,
    mode: &str,
) -> CallResult {
    if cancel.is_cancelled() {
        return CallResult::plain(id, ToolOutput::error(INTERRUPTED));
    }
    if input.get(kodact_api::TRUNCATED_INPUT).is_some() {
        return CallResult::plain(
            id,
            ToolOutput::error(
                "<tool_use_error>This call was cut off by the output token limit before its input was complete, \
                 so it did not run. Make the change in smaller steps: write a large file in parts (Write the \
                 first part, then Edit to add the rest), or split a big edit into several Edit calls.</tool_use_error>",
            ),
        );
    }
    if let Some(problem) = input.get(kodact_api::INVALID_INPUT).and_then(Value::as_str) {
        let raw = input.get(kodact_api::RAW_INPUT).and_then(Value::as_str).unwrap_or_default();
        let tail: String = {
            let chars: Vec<char> = raw.chars().collect();
            chars[chars.len().saturating_sub(160)..].iter().collect()
        };
        return CallResult::plain(
            id,
            ToolOutput::error(format!(
                "<tool_use_error>The arguments of this call were not valid JSON ({problem}), so it did not run. \
                 They ended with:\n{tail}\nSend the call again as valid JSON: quote every key and string, put a \
                 comma between fields, and escape quotes, backslashes and newlines inside strings. For a long \
                 script, Write it to a file first and then run the file.</tool_use_error>"
            )),
        );
    }
    let Some(tool) = shared.tools.get(name) else {
        return CallResult::plain(id, ToolOutput::error(format!("Error: No such tool available: {name}")));
    };
    let call_cancel = cancel.child_token();
    let mut ctx = shared.tool_ctx.for_call(id, call_cancel.clone());
    if ctx.checkpointer.is_none() {
        ctx.checkpointer = Some(Arc::new(HistoryCheckpointer(shared.history.clone())));
    }
    if let Err(e) = tool.validate(&input, &ctx) {
        return CallResult::plain(id, ToolOutput::error(format!("<tool_use_error>{e}</tool_use_error>")));
    }

    // 1. Rules and mode.
    let perm = shared.permissions.read().unwrap().clone();
    let subject = tool.permission_subject(&input, &ctx);
    let req = Request {
        tool: name,
        subject: subject.clone(),
        read_only: tool.is_read_only(&input),
        sandboxed: tool.sandboxed(&input, &ctx),
    };
    let mut decision = perm.decide(&req);
    tracing::debug!(tool = name, decision = ?decision, mode = perm.mode.as_str(), "permission check");
    // Questions for the person always reach them (contract C8): only deny rules and
    // bypassPermissions (which approves plans) skip the prompt.
    if tool.needs_user() {
        decision = match decision {
            d @ Decision::Deny { reason: kodact_permissions::Reason::Rule { .. } } => d,
            _ if perm.mode == PermissionMode::BypassPermissions => {
                Decision::Allow { reason: kodact_permissions::Reason::Mode(perm.mode) }
            }
            _ if perm.mode == PermissionMode::DontAsk => {
                Decision::Deny { reason: kodact_permissions::Reason::Mode(perm.mode) }
            }
            _ => Decision::Ask { reason: kodact_permissions::Reason::Default, suggestions: vec![] },
        };
    }
    let rule_denied = matches!(decision, Decision::Deny { .. });

    // 2. PreToolUse hooks.
    let mut stop = None;
    if !rule_denied {
        let o = shared
            .hooks
            .run(
                HookEvent::PreToolUse,
                Some(name),
                mode,
                json!({"tool_name": name, "tool_input": input, "tool_use_id": id}),
                cancel,
            )
            .await;
        for m in &o.user_messages {
            shared.sink.emit(EngineEvent::Notice { level: NoticeLevel::Warning, text: m.clone() });
        }
        stop = o.stop.clone();
        if let Some(reason) = o.blocked {
            return CallResult {
                id: id.into(),
                output: ToolOutput::error(reason),
                denial: denial(name, id, &input),
                interrupt_turn: false,
                stop,
                writes_after: 0,
            };
        }
        if let Some((d, reason)) = o.permission {
            let reason = kodact_permissions::Reason::Rule {
                behavior: kodact_permissions::Behavior::Ask,
                rule: format!("PreToolUse hook: {reason}"),
            };
            decision = match d.as_str() {
                "deny" => Decision::Deny { reason },
                "allow" => Decision::Allow { reason },
                "ask" => Decision::Ask { reason, suggestions: perm.suggest(&req) },
                _ => decision,
            };
        }
        if let Some(u) = o.updated_input {
            input = u;
            if let Err(e) = tool.validate(&input, &ctx) {
                return CallResult::plain(id, ToolOutput::error(format!("<tool_use_error>{e}</tool_use_error>")));
            }
        }
    }

    // 3. Prompt if needed, one at a time.
    match decision {
        Decision::Deny { reason } => {
            return CallResult {
                id: id.into(),
                output: ToolOutput::error(format!("Permission to use {name} has been denied: {reason}.")),
                denial: denial(name, id, &input),
                interrupt_turn: false,
                stop,
                writes_after: 0,
            };
        }
        Decision::Ask { reason, suggestions } => {
            let answer = {
                let _guard = tokio::select! {
                    g = shared.prompt_lock.lock() => g,
                    _ = cancel.cancelled() => return CallResult::plain(id, ToolOutput::error(INTERRUPTED)),
                };
                let prompt = PermissionPrompt {
                    tool_name: name.into(),
                    tool_use_id: id.into(),
                    input: input.clone(),
                    reason: reason.to_string(),
                    suggestions,
                    blocked_path: blocked_path(&subject),
                };
                tokio::select! {
                    a = shared.prompter.ask(prompt) => a,
                    _ = cancel.cancelled() => return CallResult::plain(id, ToolOutput::error(INTERRUPTED)),
                }
            };
            match answer {
                PermissionAnswer::Allow { updated_input, updated_permissions } => {
                    if let Some(u) = updated_input.filter(Value::is_object) {
                        input = u;
                    }
                    for upd in &updated_permissions {
                        apply_permission_update(shared, upd);
                    }
                }
                PermissionAnswer::Deny { message, interrupt } => {
                    let text = if message.is_empty() {
                        format!(
                            "The user rejected this {name} call; nothing was run. Don't run it again or reach the same \
                             result another way (another command, shell or tool); ask what they want instead."
                        )
                    } else {
                        message
                    };
                    return CallResult {
                        id: id.into(),
                        output: ToolOutput::error(text),
                        denial: denial(name, id, &input),
                        interrupt_turn: interrupt,
                        stop,
                        writes_after: 0,
                    };
                }
            }
        }
        Decision::Allow { .. } => {}
    }

    // 4. Execute (abort on interrupt even if the tool does not cooperate).
    let t0 = std::time::Instant::now();
    let output = tokio::select! {
        o = tool.call(input.clone(), &ctx) => o,
        _ = call_cancel.cancelled() => ToolOutput::error(INTERRUPTED),
    };
    tracing::debug!(tool = name, id, ms = t0.elapsed().as_millis() as u64, is_error = output.is_error, "tool call");
    if call_cancel.is_cancelled() {
        return CallResult { id: id.into(), output, denial: None, interrupt_turn: false, stop, writes_after: 0 };
    }

    // Plan mode follows the plan tools: EnterPlanMode turns it on; an approved plan turns it off,
    // unless the approval already chose a mode.
    if !output.is_error {
        let mut perm = shared.permissions.write().unwrap();
        let next = match name {
            "EnterPlanMode" => Some(PermissionMode::Plan),
            "ExitPlanMode" if perm.mode == PermissionMode::Plan => Some(PermissionMode::Default),
            _ => None,
        };
        if let Some(m) = next.filter(|m| *m != perm.mode) {
            perm.mode = m;
            shared.transcript.append_system("permission_mode", json!({"mode": m.as_str()}));
            drop(perm);
            announce_mode(shared, m);
        }
    }

    // 5. PostToolUse / PostToolUseFailure.
    let event = if output.is_error { HookEvent::PostToolUseFailure } else { HookEvent::PostToolUse };
    let mut output = output;
    if shared.hooks.enabled_for(event) {
        let response = output.structured.clone().unwrap_or_else(|| json!(output.text_content()));
        let o = shared
            .hooks
            .run(
                event,
                Some(name),
                mode,
                json!({"tool_name": name, "tool_input": input, "tool_response": response, "tool_use_id": id}),
                cancel,
            )
            .await;
        for m in &o.user_messages {
            shared.sink.emit(EngineEvent::Notice { level: NoticeLevel::Warning, text: m.clone() });
        }
        let mut extra = vec![];
        if let Some(fb) = o.blocked {
            extra.push(format!("{} hook feedback:\n{fb}", event.as_str()));
        }
        extra.extend(o.additional_context);
        if !extra.is_empty() {
            output = append_text(output, &extra.join("\n\n"));
        }
        if o.stop.is_some() {
            stop = o.stop;
        }
    }
    // 6. Output budget for tools without their own (GOALS pillar 1): long text is saved, not resent.
    if !matches!(name, "Read" | "Bash") {
        if let kodact_types::ToolResultContent::Text(t) = &output.content {
            if t.len() > GENERIC_OUTPUT_BUDGET {
                let ctx = shared.tool_ctx.for_call(id, cancel.clone());
                output.content = kodact_types::ToolResultContent::Text(kodact_tools::fit_output(
                    &ctx,
                    t,
                    GENERIC_OUTPUT_BUDGET,
                    "output",
                ));
            }
        }
    }
    // 7. Output that carries instructions for an agent is marked as data (OWASP LLM01).
    if !output.is_error && !matches!(name, "Write" | "Edit" | "MultiEdit" | "NotebookEdit" | "TodoWrite") {
        if let Some(why) = kodact_tools::injection::suspicious(&output.text_content()) {
            // Once per source: re-reading the same file or page isn't news.
            let source = ["file_path", "path", "url", "pattern", "command", "query"]
                .iter()
                .find_map(|k| input.get(*k).and_then(Value::as_str))
                .unwrap_or("");
            let key = format!("{name}\u{0}{source}\u{0}{why}");
            if shared.injection_noted.lock().unwrap().insert(key) {
                output = append_text(output, &kodact_tools::injection::note(name, &why));
            }
        }
    }
    CallResult { id: id.into(), output, denial: None, interrupt_turn: false, stop, writes_after: 0 }
}

fn append_text(mut out: ToolOutput, text: &str) -> ToolOutput {
    let note = format!("<system-reminder>\n{text}\n</system-reminder>");
    out.content = match out.content {
        kodact_types::ToolResultContent::Text(t) => kodact_types::ToolResultContent::Text(format!("{t}\n\n{note}")),
        kodact_types::ToolResultContent::Blocks(mut b) => {
            b.push(ContentBlock::text(note));
            kodact_types::ToolResultContent::Blocks(b)
        }
    };
    out
}

/// Apply an accepted `PermissionUpdate` to the session and hand it to the persister.
/// Tell the host the mode changed on the engine's side (a plan approved,
/// plan mode entered, an answer that set a mode), as Claude Code does:
/// `system/status` with `permissionMode`, so its mode picker follows.
fn announce_mode(shared: &Shared, mode: PermissionMode) {
    shared.sink.emit(EngineEvent::System {
        subtype: "status".into(),
        data: json!({ "status": null, "permissionMode": mode.as_str() }),
    });
}

pub(crate) fn apply_permission_update(shared: &Shared, upd: &Value) {
    let Ok(s) = serde_json::from_value::<Suggestion>(upd.clone()) else {
        tracing::warn!(update = %upd, "ignoring unknown permission update");
        return;
    };
    let destination = match &s {
        Suggestion::AddRules { rules, behavior, destination } => {
            let mut perm = shared.permissions.write().unwrap();
            for r in rules {
                if let Ok(rule) = Rule::parse(&r.to_rule_string()) {
                    perm.add_rule(*behavior, rule);
                }
            }
            destination.clone()
        }
        Suggestion::SetMode { mode, destination } => {
            if let Some(m) = PermissionMode::parse(mode) {
                shared.permissions.write().unwrap().mode = m;
                shared.transcript.append_system("permission_mode", json!({"mode": m.as_str()}));
                announce_mode(shared, m);
            }
            destination.clone()
        }
        Suggestion::AddDirectories { directories, destination } => {
            for d in directories {
                let p = std::path::PathBuf::from(d);
                shared.permissions.write().unwrap().add_directory(&p);
                let mut wd = shared.tool_ctx.working_dirs.write().unwrap();
                let p = kodact_permissions::normalize(&p, &shared.tool_ctx.project_dir);
                if !wd.contains(&p) {
                    wd.push(p);
                }
            }
            let dirs = shared.tool_ctx.working_dirs.read().unwrap().iter().skip(1).cloned().collect::<Vec<_>>();
            shared.transcript.append_meta(json!({"additionalDirectories": dirs}));
            destination.clone()
        }
    };
    if destination != "session" {
        if let Some(f) = shared.on_permission_update.lock().unwrap().as_ref() {
            f(upd);
        }
    }
}
