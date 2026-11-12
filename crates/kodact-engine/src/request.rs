//! Turning engine state into an API request.

use std::collections::HashSet;

use kodact_api::models::{ModelInfo, OffMode, ThinkingStyle};
use kodact_types::{CacheControl, ContentBlock, Message, Role, ThinkingConfig, ToolResultContent};
use serde_json::{json, Value};

/// What a cleared (micro-compacted) tool result is replaced with (contract C9).
pub const CLEARED_RESULT: &str = "[Old tool result cleared to save context. Re-run the tool if you need it again.]";

/// Make a history acceptable to the API:
/// * consecutive same-role messages are merged;
/// * tool results come first in a user message;
/// * every `tool_use` gets a `tool_result` (a crash or interrupt can leave one dangling);
/// * cleared tool results are replaced (contract C9);
/// * empty text blocks are dropped.
pub fn normalize(messages: &[Message], cleared: &HashSet<String>) -> Vec<Message> {
    let mut out: Vec<Message> = vec![];
    for m in messages {
        let content: Vec<ContentBlock> = m
            .content
            .iter()
            .filter(|b| !matches!(b, ContentBlock::Text { text, .. } if text.trim().is_empty()))
            .map(|b| match b {
                ContentBlock::ToolResult { tool_use_id, is_error, .. } if cleared.contains(tool_use_id) => {
                    ContentBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content: ToolResultContent::Text(CLEARED_RESULT.into()),
                        is_error: *is_error,
                        cache_control: None,
                    }
                }
                other => {
                    let mut o = other.clone();
                    o.set_cache_control(None);
                    o
                }
            })
            .collect();
        if content.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(prev) if prev.role == m.role => prev.content.extend(content),
            _ => out.push(Message { role: m.role, content }),
        }
    }
    // Pair tool uses with results.
    let mut i = 0;
    while i < out.len() {
        if out[i].role == Role::Assistant {
            let ids: Vec<String> = out[i].tool_uses().map(|(id, _, _)| id.to_string()).collect();
            if !ids.is_empty() {
                if out.get(i + 1).map(|n| n.role != Role::User).unwrap_or(true) {
                    out.insert(i + 1, Message::user(vec![]));
                }
                let next = &mut out[i + 1];
                for id in ids {
                    let has = next
                        .content
                        .iter()
                        .any(|b| matches!(b, ContentBlock::ToolResult { tool_use_id, .. } if *tool_use_id == id));
                    if !has {
                        next.content.push(ContentBlock::tool_result(
                            id,
                            "Tool call was interrupted before it returned.",
                            true,
                        ));
                    }
                }
            }
        }
        if out[i].role == Role::User {
            let (mut results, rest): (Vec<_>, Vec<_>) =
                out[i].content.drain(..).partition(|b| matches!(b, ContentBlock::ToolResult { .. }));
            results.extend(rest);
            out[i].content = results;
        }
        i += 1;
    }
    // Drop orphan tool results (their tool_use was lost to compaction).
    let mut known: HashSet<String> = HashSet::new();
    for m in out.iter_mut() {
        if m.role == Role::Assistant {
            known.extend(m.tool_uses().map(|(id, _, _)| id.to_string()));
        } else {
            m.content.retain(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => known.contains(tool_use_id),
                _ => true,
            });
        }
    }
    out.retain(|m| !m.content.is_empty());
    if out.first().map(|m| m.role == Role::Assistant).unwrap_or(false) {
        out.insert(0, Message::user_text("(continued)"));
    }
    out
}

/// Cache the conversation prefix: a breakpoint on the last block of the last
/// message (the system prompt and tools carry their own).
pub fn apply_cache_breakpoints(messages: &mut [Message]) {
    if let Some(last) = messages.last_mut() {
        if let Some(b) = last
            .content
            .iter_mut()
            .rev()
            .find(|b| !matches!(b, ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. }))
        {
            b.set_cache_control(Some(CacheControl::ephemeral()));
        }
    }
}

/// The effort levels KodAct takes (`--effort`, `effortLevel`).
pub(crate) const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// Runtime thinking control: `None` = the model's default, `Some(0)` = off,
/// `Some(n)` = on (a budget for models that take one).
pub fn thinking_params(
    info: &ModelInfo,
    max_thinking_tokens: Option<u32>,
    effort: Option<&str>,
    max_tokens: u32,
) -> (Option<ThinkingConfig>, Option<Value>, u32) {
    let effort = effort.filter(|e| info.effort_levels.contains(e));
    let high_effort = matches!(effort, Some("xhigh") | Some("max"));
    let mut max_tokens = max_tokens.min(info.max_output);
    let thinking = match (info.thinking, max_thinking_tokens) {
        (ThinkingStyle::AlwaysOn, _) => None,
        (ThinkingStyle::Adaptive { .. }, Some(0)) if high_effort => Some(ThinkingConfig::Adaptive { display: None }),
        (ThinkingStyle::Adaptive { off: OffMode::Disabled }, Some(0)) => Some(ThinkingConfig::Disabled),
        (ThinkingStyle::Adaptive { off: OffMode::BetweenTools }, Some(0)) => Some(ThinkingConfig::BetweenTools),
        (ThinkingStyle::Adaptive { .. }, _) => Some(ThinkingConfig::Adaptive { display: Some("summarized".into()) }),
        (ThinkingStyle::Budget, Some(n)) if n >= 1024 => {
            let budget = n.min(info.max_output.saturating_sub(4096));
            max_tokens = max_tokens.max(budget + 4096).min(info.max_output);
            Some(ThinkingConfig::Enabled { budget_tokens: budget })
        }
        (ThinkingStyle::Budget, _) => None,
    };
    let output_config = effort.map(|e| json!({"effort": e}));
    (thinking, output_config, max_tokens)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodact_api::models::model_info;

    fn tool_use(id: &str) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.into(),
            name: "Read".into(),
            input: json!({}),
            cache_control: None,
            provider: None,
        }
    }

    #[test]
    fn merges_and_pairs() {
        let msgs = vec![
            Message::user_text("a"),
            Message::user_text("b"),
            Message::assistant(vec![ContentBlock::text("x"), tool_use("t1"), tool_use("t2")]),
            Message::user(vec![ContentBlock::text("note"), ContentBlock::tool_result("t1", "ok", false)]),
        ];
        let n = normalize(&msgs, &HashSet::new());
        assert_eq!(n.len(), 3);
        assert_eq!(n[0].content.len(), 2);
        assert!(matches!(&n[2].content[0], ContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "t1"));
        assert!(n[2].content.iter().any(
            |b| matches!(b, ContentBlock::ToolResult { tool_use_id, is_error: Some(true), .. } if tool_use_id == "t2")
        ));
    }

    #[test]
    fn clears_marked_results() {
        let msgs = vec![
            Message::user_text("a"),
            Message::assistant(vec![tool_use("t1")]),
            Message::user(vec![ContentBlock::tool_result("t1", "big output", false)]),
        ];
        let n = normalize(&msgs, &HashSet::from(["t1".to_string()]));
        assert_eq!(n[2].content[0], ContentBlock::tool_result("t1", CLEARED_RESULT, false));
    }

    #[test]
    fn thinking_per_model() {
        let opus55 = model_info("claude-opus-5-5").unwrap();
        assert_eq!(thinking_params(opus55, Some(0), Some("high"), 32000).0, None);
        let sonnet = model_info("claude-sonnet-5-5").unwrap();
        assert_eq!(thinking_params(sonnet, Some(0), Some("low"), 32000).0, Some(ThinkingConfig::BetweenTools));
        assert!(matches!(
            thinking_params(sonnet, Some(0), Some("max"), 32000).0,
            Some(ThinkingConfig::Adaptive { .. })
        ));
        let opus5 = model_info("claude-opus-5").unwrap();
        assert_eq!(thinking_params(opus5, Some(0), None, 32000).0, Some(ThinkingConfig::Disabled));
        let haiku = model_info("claude-haiku-4-5").unwrap();
        let (t, oc, max) = thinking_params(haiku, Some(8000), Some("high"), 4000);
        assert_eq!(t, Some(ThinkingConfig::Enabled { budget_tokens: 8000 }));
        assert!(oc.is_none(), "haiku has no effort");
        assert_eq!(max, 12096);
        assert_eq!(thinking_params(opus55, None, Some("bogus"), 1).1, None);
    }
}
