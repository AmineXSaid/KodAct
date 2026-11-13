//! Context compaction (contract C9).
//!
//! Two mechanisms keep a long session inside the context window:
//! * **micro-compaction** clears the content of old, large tool results that
//!   can be fetched again; the cleared ids are sticky so the prompt prefix
//!   changes once, not on every request;
//! * **auto-compaction** replaces the conversation with a structured summary
//!   when the window is nearly full (or on `/compact`).
//!
//! This crate holds the pure parts: thresholds, eligibility and the summary
//! prompt. The engine runs them.

use std::collections::{HashMap, HashSet};

use kodact_types::{ContentBlock, Message, Role};

/// Tools whose output can be produced again by re-running them.
pub const REFETCHABLE: &[&str] = &["Read", "Grep", "Glob", "LS", "Bash", "BashOutput", "WebFetch", "WebSearch"];
/// Tool results at most this long are never cleared.
pub const MICRO_MIN_CHARS: usize = 8_000;
/// Results from this many most recent user turns are kept.
pub const MICRO_KEEP_TURNS: usize = 3;
/// Reserved for the summary itself and the next reply.
pub const AUTOCOMPACT_BUFFER: u64 = 13_000;

/// Context tokens at which auto-compaction runs: the window less room for the next
/// reply and for the summary. Both reserves scale with the window, so a small model
/// (32k) still gets most of its window before compacting, instead of a threshold of
/// zero that would compact every turn; a large one (200k and up) keeps the full
/// reserves: 32k for the reply and [`AUTOCOMPACT_BUFFER`] for the summary.
pub fn autocompact_threshold(window: u64, max_output: u32) -> u64 {
    let reply = u64::from(max_output.min(32_000)).min(window / 4);
    let buffer = AUTOCOMPACT_BUFFER.min(window / 8);
    window.saturating_sub(reply).saturating_sub(buffer)
}

/// Context tokens above which micro-compaction runs (half the auto threshold).
pub fn micro_threshold(window: u64, max_output: u32) -> u64 {
    autocompact_threshold(window, max_output) / 2
}

/// Is this user message a person's turn (not just tool results)?
fn is_user_turn(m: &Message) -> bool {
    m.role == Role::User
        && m.content.iter().any(|b| matches!(b, ContentBlock::Text { .. }))
        && !m.content.iter().any(|b| matches!(b, ContentBlock::ToolResult { .. }))
}

/// Tool-result ids that micro-compaction may clear now.
pub fn micro_candidates(messages: &[Message], already: &HashSet<String>) -> Vec<String> {
    let turns: Vec<usize> = messages.iter().enumerate().filter(|(_, m)| is_user_turn(m)).map(|(i, _)| i).collect();
    if turns.len() <= MICRO_KEEP_TURNS {
        return vec![];
    }
    let cutoff = turns[turns.len() - MICRO_KEEP_TURNS];
    let mut tool_of: HashMap<&str, &str> = HashMap::new();
    for m in &messages[..cutoff] {
        for b in &m.content {
            if let ContentBlock::ToolUse { id, name, .. } = b {
                tool_of.insert(id, name);
            }
        }
    }
    let mut out = vec![];
    for m in &messages[..cutoff] {
        for b in &m.content {
            if let ContentBlock::ToolResult { tool_use_id, content, .. } = b {
                let name = tool_of.get(tool_use_id.as_str()).copied().unwrap_or("");
                let refetchable = REFETCHABLE.contains(&name) || name.starts_with("mcp__");
                if refetchable && !already.contains(tool_use_id) && content.to_text().len() > MICRO_MIN_CHARS {
                    out.push(tool_use_id.clone());
                }
            }
        }
    }
    out
}

/// Rough token estimate for text that has not been through the API yet.
pub fn estimate_tokens(messages: &[Message]) -> u64 {
    estimate_sent_tokens(messages, true)
}

/// The same estimate, for a request that sends earlier reasoning back (`with_reasoning`) or
/// leaves it out (chat completions).
pub fn estimate_sent_tokens(messages: &[Message], with_reasoning: bool) -> u64 {
    let chars: usize = messages
        .iter()
        .flat_map(|m| m.content.iter())
        .map(|b| match b {
            ContentBlock::Text { text, .. } => text.len(),
            ContentBlock::ToolResult { content, .. } => content.to_text().len(),
            ContentBlock::ToolUse { input, .. } => input.to_string().len(),
            ContentBlock::Thinking { .. } | ContentBlock::RedactedThinking { .. } if !with_reasoning => 0,
            ContentBlock::Thinking { thinking, .. } => thinking.len(),
            ContentBlock::Image { .. } | ContentBlock::Document { .. } => 6_000,
            _ => 200,
        })
        .sum();
    (chars / 4) as u64
}

/// The instruction appended to the conversation to ask for a summary.
pub fn summary_instruction(custom: Option<&str>) -> String {
    let mut s = String::from(
        "Stop working on the task for a moment. The conversation is about to be replaced by a summary of it, \
         and you will continue from that summary alone, so write it now. Do not call any tools.\n\n\
         First think through the conversation in order inside <analysis> tags: what the user asked for at each \
         point, what you did, the decisions made, the files touched, the errors met and how they were fixed, and \
         any feedback the user gave. Then write the summary inside <summary> tags with these sections:\n\n\
         1. Goal: what the user wants overall, and any constraints or preferences they stated.\n\
         2. Key facts: technologies, conventions and decisions that matter for continuing.\n\
         3. Files: every file read, created or changed, why it matters, and the essential code (signatures, the \
            exact lines changed) needed to continue without re-reading everything.\n\
         4. Problems: errors hit, their causes and fixes, and anything still unresolved. Name each approach \
            that was tried and given up, and why, so it is not tried again.\n\
         5. User messages: they are attached after this summary word for word, so do not copy them; note only \
            what the summary needs about them (a change of mind, a clarification, which one is current).\n\
         6. Open tasks: work the user asked for that is not finished.\n\
         7. Current state: exactly what was being done right before this summary, with file names and code.\n\
         8. Next step: the immediate next action, only if it follows directly from the user's latest request; \
            quote the relevant request so the intent is not lost.\n\n\
         Be precise and complete; prefer concrete names, paths and code over general descriptions.",
    );
    if let Some(c) = custom.filter(|c| !c.trim().is_empty()) {
        s.push_str(&format!("\n\nAdditional instructions for this summary:\n{}", c.trim()));
    }
    s
}

/// Pull the `<summary>` body out of the model's reply (falls back to the whole text).
pub fn extract_summary(reply: &str) -> String {
    match (reply.find("<summary>"), reply.rfind("</summary>")) {
        (Some(a), Some(b)) if b > a => reply[a + "<summary>".len()..b].trim().to_string(),
        _ => {
            let without_analysis = match (reply.find("<analysis>"), reply.find("</analysis>")) {
                (Some(a), Some(b)) if b > a => format!("{}{}", &reply[..a], &reply[b + "</analysis>".len()..]),
                _ => reply.to_string(),
            };
            without_analysis.trim().to_string()
        }
    }
}

/// The person's requests, oldest first, for the message that starts the conversation after
/// compaction: word for word, so the task never depends on how good a summary was. Within
/// `budget` characters (the caller sizes it from the model's window): the first request, which
/// usually states the task, then the latest ones; a request too long for the budget keeps its
/// beginning. `None` when there are none.
pub fn requests_block(requests: &[String], budget: usize) -> Option<String> {
    let texts: Vec<&str> = requests.iter().map(|r| r.trim()).filter(|r| !r.is_empty()).collect();
    if texts.is_empty() {
        return None;
    }
    let clip = |t: &str, max: usize| -> String {
        let n = t.chars().count();
        if n <= max {
            t.to_string()
        } else {
            let head: String = t.chars().take(max).collect();
            format!("{head}… ({} more characters)", n - max)
        }
    };
    let first = clip(texts[0], budget);
    let mut room = budget.saturating_sub(first.chars().count());
    let mut latest: Vec<(usize, &str)> = vec![];
    for (i, t) in texts.iter().enumerate().skip(1).rev() {
        let n = t.chars().count();
        if n > room {
            break;
        }
        room -= n;
        latest.push((i, t));
    }
    latest.reverse();
    let mut out = String::from("The person's requests in this session, oldest first, word for word:\n");
    out.push_str(&format!("<request n=\"1\">\n{first}\n</request>\n"));
    let left_out = latest.first().map_or(texts.len() - 1, |(i, _)| i - 1);
    if left_out > 0 {
        out.push_str(&format!("({left_out} earlier request{} left out)\n", if left_out == 1 { "" } else { "s" }));
    }
    for (i, t) in latest {
        out.push_str(&format!("<request n=\"{}\">\n{t}\n</request>\n", i + 1));
    }
    Some(out)
}

/// The user message that starts the conversation after compaction: the context to re-attach,
/// the person's requests word for word ([`requests_block`]), then the summary.
pub fn summary_message(summary: &str, context: Option<&str>, requests: Option<&str>, continue_work: bool) -> Message {
    let mut text = String::new();
    if let Some(c) = context.filter(|c| !c.trim().is_empty()) {
        text.push_str(&format!("<system-reminder>\n{c}\n</system-reminder>\n\n"));
    }
    text.push_str("This session continues an earlier conversation that ran out of context.\n\n");
    if let Some(r) = requests {
        text.push_str(r);
        text.push('\n');
    }
    text.push_str("Here is a summary of the conversation:\n\n");
    text.push_str(summary);
    if continue_work {
        text.push_str(
            "\n\nContinue the work from where it stopped, without asking the user further questions. Pick up the \
             last task you were working on.",
        );
    }
    Message::user_text(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kodact_types::ContentBlock;
    use serde_json::json;

    fn tool_round(id: &str, name: &str, size: usize) -> Vec<Message> {
        vec![
            Message::assistant(vec![ContentBlock::ToolUse {
                id: id.into(),
                name: name.into(),
                input: json!({}),
                cache_control: None,
                provider: None,
            }]),
            Message::user(vec![ContentBlock::tool_result(id, "x".repeat(size), false)]),
        ]
    }

    #[test]
    fn reasoning_is_estimated_only_where_it_is_sent() {
        let m = Message::assistant(vec![
            ContentBlock::Thinking { thinking: "x".repeat(4_000), signature: String::new() },
            ContentBlock::RedactedThinking { data: "z".into() },
            ContentBlock::text("y".repeat(400)),
        ]);
        assert_eq!(estimate_tokens(std::slice::from_ref(&m)), (4_000 + 200 + 400) / 4);
        assert_eq!(estimate_sent_tokens(&[m], false), 100);
    }

    #[test]
    fn c9_micro_clears_only_eligible_results() {
        let mut msgs = vec![Message::user_text("turn 1")];
        msgs.extend(tool_round("big_read", "Read", 20_000));
        msgs.extend(tool_round("small_read", "Read", 100));
        msgs.extend(tool_round("big_edit", "Edit", 20_000));
        msgs.extend(tool_round("big_mcp", "mcp__docs__fetch", 20_000));
        msgs.push(Message::user_text("turn 2"));
        msgs.push(Message::user_text("turn 3"));
        msgs.extend(tool_round("recent_read", "Read", 20_000));
        msgs.push(Message::user_text("turn 4"));
        let c = micro_candidates(&msgs, &HashSet::new());
        assert_eq!(c, vec!["big_read".to_string(), "big_mcp".to_string()]);
        // Already-cleared ids are not reported again.
        let again = micro_candidates(&msgs, &HashSet::from(["big_read".to_string()]));
        assert_eq!(again, vec!["big_mcp".to_string()]);
        // Too few turns: nothing is old enough.
        assert!(micro_candidates(&msgs[..msgs.len() - 3], &HashSet::new()).is_empty());
    }

    #[test]
    fn thresholds() {
        assert_eq!(autocompact_threshold(200_000, 32_000), 155_000);
        assert_eq!(autocompact_threshold(1_000_000, 64_000), 955_000);
        assert_eq!(micro_threshold(200_000, 32_000), 77_500);
        // A small window keeps most of itself before compacting, never zero.
        assert_eq!(autocompact_threshold(32_768, 32_000), 20_480);
        assert_eq!(autocompact_threshold(10_000, 32_000), 6_250);
        assert_eq!(autocompact_threshold(131_072, 32_000), 86_072);
    }

    #[test]
    fn summary_parsing() {
        assert_eq!(extract_summary("<analysis>thinking</analysis>\n<summary>\nThe gist\n</summary>"), "The gist");
        assert_eq!(extract_summary("<analysis>a</analysis> plain"), "plain");
        assert!(summary_instruction(Some("focus on tests")).ends_with("focus on tests"));
        let m = summary_message("S", Some("memory"), None, true);
        let t = m.text();
        assert!(t.starts_with("<system-reminder>\nmemory") && t.contains("S\n\nContinue the work"));
        // The requests come word for word, before the summary.
        let block = requests_block(&["Write report.toml".into()], 1_000);
        let t = summary_message("S", None, block.as_deref(), false).text();
        let (req, sum) = (t.find("Write report.toml").unwrap(), t.find("summary of the conversation").unwrap());
        assert!(req < sum, "{t}");
    }

    #[test]
    fn requests_keep_the_first_and_the_latest_within_the_budget() {
        let reqs: Vec<String> =
            vec!["the task: ".to_string() + &"x".repeat(50), "a".repeat(30), "b".repeat(30), "c".repeat(30)];
        // Room for everything: all four, in order.
        let all = requests_block(&reqs, 10_000).unwrap();
        assert!(all.contains("n=\"1\"") && all.contains("n=\"4\"") && !all.contains("left out"), "{all}");
        // Room for the task and the two latest: the second is counted, not kept.
        let some = requests_block(&reqs, 60 + 65).unwrap();
        assert!(
            some.contains("the task") && some.contains(&"c".repeat(30)) && some.contains(&"b".repeat(30)),
            "{some}"
        );
        assert!(!some.contains(&"a".repeat(30)) && some.contains("(1 earlier request left out)"), "{some}");
        // A task longer than the budget keeps its beginning and says how much is cut.
        let long = requests_block(&["y".repeat(500)], 100).unwrap();
        assert!(long.contains(&"y".repeat(100)) && long.contains("(400 more characters)"), "{long}");
        assert_eq!(requests_block(&["  ".into()], 100), None);
    }
}
