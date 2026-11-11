//! What the engine publishes for readers outside the turn (contract C17,
//! "Immediate commands"): a read-only copy of its state, replaced whole after
//! each model call, after each tool batch, at the end of a turn and whenever
//! the driver asks ([`crate::Engine::publish`]).
//!
//! Readers clone the inner `Arc` ([`crate::EngineHandle::snapshot`]) and drop
//! the lock at once, so no guard is held for long, and never across `.await`.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

use kodact_api::models::model_info;
use kodact_types::{ContentBlock, Message, MessagesRequest, SystemBlock, ToolSpec, Usage};
use serde_json::{json, Map, Value};

use crate::engine::{Runtime, FAST_MODE_BETA};
use crate::request::{apply_cache_breakpoints, normalize, thinking_params};

/// The turn in progress, as far as it has got.
#[derive(Debug, Clone)]
pub struct TurnProgress {
    pub started: Instant,
    /// Model calls so far in this turn.
    pub api_calls: u32,
    pub api_ms: u64,
    pub tool_calls: u32,
}

/// A read-only copy of the engine's state.
#[derive(Debug, Clone, Default)]
pub struct EngineSnapshot {
    pub messages: Arc<Vec<Message>>,
    /// Tool results cleared by micro-compaction (requests show a placeholder for them).
    pub microcompacted: Arc<HashSet<String>>,
    pub system: Arc<Vec<SystemBlock>>,
    pub tools: Arc<Vec<ToolSpec>>,
    pub total_usage: Usage,
    pub total_cost_usd: f64,
    pub model_usage: Map<String, Value>,
    /// Models used without a known price: their cost isn't in `total_cost_usd`.
    pub unpriced: Vec<String>,
    pub context_tokens: u64,
    /// `Some` while a turn runs.
    pub turn: Option<TurnProgress>,
    pub provider_name: String,
    pub max_output_tokens: u32,
    pub json_schema: Option<Value>,
    pub metadata_user_id: Option<String>,
    pub autocompact_window: Option<u64>,
    pub auto_compact: bool,
}

impl EngineSnapshot {
    /// The window compaction is measured against, for `model`.
    pub fn context_window(&self, model: &str) -> u64 {
        self.autocompact_window.unwrap_or_else(|| kodact_api::models::model_info_or_default(model).context_window)
    }

    /// The context size at which automatic compaction runs, for `model`.
    pub fn autocompact_at(&self, model: &str) -> u64 {
        kodact_compact::autocompact_threshold(self.context_window(model), self.max_output_tokens)
    }
}

/// Everything a model request is built from.
pub(crate) struct RequestParts<'a> {
    pub messages: &'a [Message],
    pub microcompacted: &'a HashSet<String>,
    pub system: &'a [SystemBlock],
    pub tools: Vec<ToolSpec>,
    pub max_output_tokens: u32,
    pub json_schema: Option<&'a Value>,
    pub metadata_user_id: Option<&'a str>,
    pub provider_name: &'a str,
}

pub(crate) fn sends_fast(rt: &Runtime, model: &str, provider_name: &str) -> bool {
    rt.fast && model_info(model).is_some_and(|m| m.supports_fast_mode) && provider_name != "openai"
}

/// The request for `model`: the conversation's requests and side questions share it, so a
/// side question reuses the cached prefix.
pub(crate) fn assemble(model: &str, rt: &Runtime, p: RequestParts<'_>) -> MessagesRequest {
    let info = kodact_api::models::model_info_or_default(model);
    let (thinking, mut output_config, max_tokens) =
        thinking_params(&info, rt.max_thinking_tokens, rt.effort.as_deref(), p.max_output_tokens);
    // An OpenAI-compatible endpoint takes a reasoning level for any model (`reasoning_effort`,
    // see kodact-api's openai.rs), so the effort asked for is kept for a model KodAct has no
    // table entry for. An endpoint that refuses it gets the request again without it.
    if output_config.is_none() && p.provider_name == "openai" {
        if let Some(e) = rt.effort.as_deref().filter(|e| crate::request::EFFORT_LEVELS.contains(e)) {
            output_config = Some(json!({"effort": e}));
        }
    }
    if let Some(schema) = p.json_schema {
        let oc = output_config.get_or_insert_with(|| json!({}));
        oc["format"] = json!({"type": "json_schema", "schema": schema});
    }
    // Fast mode only where the model offers it; elsewhere the flag is ignored.
    let fast = sends_fast(rt, model, p.provider_name);
    let mut messages = normalize(p.messages, p.microcompacted);
    apply_cache_breakpoints(&mut messages);
    let mut tools = p.tools;
    if let Some(last) = tools.last_mut() {
        last.cache_control = Some(kodact_types::CacheControl::ephemeral());
    }
    let mut system = p.system.to_vec();
    if let Some(last) = system.last_mut() {
        last.cache_control = Some(kodact_types::CacheControl::ephemeral());
    }
    MessagesRequest {
        model: model.to_string(),
        max_tokens,
        messages,
        system,
        tools,
        tool_choice: None,
        thinking,
        temperature: None,
        metadata: p.metadata_user_id.map(|u| json!({"user_id": u})),
        output_config,
        speed: fast.then(|| "fast".to_string()),
        stream: true,
        betas: if fast { vec![FAST_MODE_BETA.to_string()] } else { vec![] },
    }
}

/// Sent with a `/btw` question.
const SIDE_QUESTION_NOTE: &str = "<system-reminder>\nThis is a side question from the user. Answer it briefly from \
what you already know of this conversation. You can't use tools for it, and neither the question nor your answer \
becomes part of the main conversation.\n</system-reminder>";

/// What is running right now, for a side question asked mid-turn: the tool
/// calls the model made that haven't returned (sub-agents included), with how
/// long the turn has run, and `background` (shells, subtasks) when given.
pub fn live_state(snap: &EngineSnapshot, background: Option<&str>) -> Option<String> {
    let mut s = String::new();
    if let Some(last) = snap.messages.last().filter(|m| m.role == kodact_types::Role::Assistant) {
        let calls: Vec<String> = last
            .tool_uses()
            .map(|(_, name, input)| {
                let arg = ["description", "subagent_type", "command", "file_path", "pattern", "prompt"]
                    .iter()
                    .find_map(|k| input.get(*k).and_then(Value::as_str))
                    .map(|a| {
                        let one: String = a.lines().next().unwrap_or("").chars().take(120).collect();
                        format!("({one})")
                    })
                    .unwrap_or_default();
                format!("- {name}{arg}")
            })
            .collect();
        if !calls.is_empty() {
            let since = snap
                .turn
                .as_ref()
                .map(|t| format!(", in a turn that started {}s ago", t.started.elapsed().as_secs()))
                .unwrap_or_default();
            s.push_str(&format!(
                "These tool calls are running now and haven't returned yet{since} (sub-agents run as Task calls):\n{}\n",
                calls.join("\n")
            ));
            let said = last.text();
            if !said.trim().is_empty() {
                let said: String = said.trim().chars().take(600).collect();
                s.push_str(&format!("What you said when you started them: {said}\n"));
            }
        }
    }
    if let Some(b) = background.filter(|b| !b.trim().is_empty()) {
        s.push_str(b.trim_end());
        s.push('\n');
    }
    (!s.is_empty()).then(|| {
        format!(
            "<system-reminder>\nLive state, as of this question (newer than the conversation above):\n{s}\
             </system-reminder>"
        )
    })
}

/// A `/btw` request: the conversation as `snap` has it (same system prompt and tools, so the
/// cached prefix is reused), earlier side questions, then `question`, with `tool_choice: none`.
/// Mid-turn, a model reply whose tool calls haven't been answered yet is left out of the
/// conversation, and described in the live state instead, so answers about progress are right.
pub fn side_question_request(
    snap: &EngineSnapshot,
    rt: &Runtime,
    question: &str,
    earlier: &[(String, String)],
    background: Option<&str>,
) -> MessagesRequest {
    let live = live_state(snap, background);
    let mut messages: &[Message] = &snap.messages;
    if let Some(last) = messages.last() {
        if last.role == kodact_types::Role::Assistant && last.tool_uses().next().is_some() {
            messages = &messages[..messages.len() - 1];
        }
    }
    let mut req = assemble(
        &rt.model,
        rt,
        RequestParts {
            messages,
            microcompacted: &snap.microcompacted,
            system: &snap.system,
            tools: snap.tools.to_vec(),
            max_output_tokens: snap.max_output_tokens,
            json_schema: snap.json_schema.as_ref(),
            metadata_user_id: snap.metadata_user_id.as_deref(),
            provider_name: &snap.provider_name,
        },
    );
    let mut extra = vec![];
    for (q, a) in earlier {
        extra.push(Message::user_text(q.clone()));
        extra.push(Message::assistant(vec![ContentBlock::text(a.clone())]));
    }
    let live = live.map(|l| format!("{l}\n\n")).unwrap_or_default();
    extra.push(Message::user_text(format!("{SIDE_QUESTION_NOTE}\n\n{live}{question}")));
    for m in extra {
        match req.messages.last_mut() {
            Some(last) if last.role == m.role => last.content.extend(m.content),
            _ => req.messages.push(m),
        }
    }
    req.tool_choice = Some(json!({"type": "none"}));
    req
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap_with_running_task() -> EngineSnapshot {
        let call = Message::assistant(vec![
            ContentBlock::text("Launching two auditors."),
            ContentBlock::ToolUse {
                id: "t1".into(),
                name: "Task".into(),
                input: json!({"description": "audit the parser", "prompt": "Read src/"}),
                cache_control: None,
                provider: None,
            },
        ]);
        EngineSnapshot {
            messages: Arc::new(vec![Message::user_text("audit"), call]),
            turn: Some(TurnProgress { started: Instant::now(), api_calls: 1, api_ms: 10, tool_calls: 1 }),
            ..Default::default()
        }
    }

    #[test]
    fn side_questions_see_the_tool_calls_still_running() {
        let snap = snap_with_running_task();
        let rt = Runtime { model: "m".into(), max_thinking_tokens: None, effort: None, fast: false };
        let background = Some("Background tasks:\n  bash_1 [running] npm test");
        let req = side_question_request(&snap, &rt, "how far are you?", &[], background);
        let last = req.messages.last().unwrap().text();
        assert!(
            last.contains("These tool calls are running now") && last.contains("- Task(audit the parser)"),
            "{last}"
        );
        assert!(last.contains("What you said when you started them: Launching two auditors."), "{last}");
        assert!(last.contains("bash_1 [running] npm test") && last.ends_with("how far are you?"), "{last}");
        assert!(!req.messages.iter().any(|m| m.tool_uses().next().is_some()), "the unanswered call itself stays out");
    }

    #[test]
    fn an_openai_compatible_endpoint_gets_the_effort_for_any_model() {
        let request = |provider: &str, effort: Option<&str>| {
            let rt = Runtime {
                model: "deep-thinking".into(),
                max_thinking_tokens: None,
                effort: effort.map(Into::into),
                fast: false,
            };
            let parts = RequestParts {
                messages: &[Message::user_text("hi")],
                microcompacted: &HashSet::new(),
                system: &[],
                tools: vec![],
                max_output_tokens: 32_000,
                json_schema: None,
                metadata_user_id: None,
                provider_name: provider,
            };
            assemble("deep-thinking", &rt, parts).output_config
        };
        // A model KodAct doesn't know, behind an OpenAI-compatible endpoint: the effort asked for.
        assert_eq!(request("openai", Some("medium")), Some(json!({"effort": "medium"})));
        // Nothing asked: nothing sent, as before. Behind the Messages API: as before too.
        assert_eq!(request("openai", None), None);
        assert_eq!(request("messages", Some("medium")), None);
        assert_eq!(request("openai", Some("bogus")), None);
    }

    #[test]
    fn no_live_state_when_idle() {
        let snap = EngineSnapshot { messages: Arc::new(vec![Message::user_text("hi")]), ..Default::default() };
        assert!(live_state(&snap, None).is_none());
    }
}
