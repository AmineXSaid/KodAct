//! The independent check of a coding agent's work (contracts C18 and C12): a separate request
//! that reads the conversation as evidence, treats the agent's own words as claims, and answers
//! met, not met or impossible. `/goal` checks its condition with it after each turn; an
//! unattended run checks its request with it when the model calls Finish.

use kodact_types::{ContentBlock, Message, Role};
use serde_json::Value;

/// How much of the conversation the evaluator sees, in characters (the newest part).
const TRANSCRIPT_CHARS: usize = 60_000;
/// How much of each tool result it sees (the end, where results usually are).
const RESULT_CHARS: usize = 2_000;
/// How much room the list of failed tool calls may take.
const FAILURES_CHARS: usize = 6_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    Met,
    NotMet,
    Impossible,
}

/// Read the evaluator's answer: a JSON object, tolerating text around it. A
/// `met` with no evidence is not a pass: the check has to point at tool output.
pub fn parse_verdict(text: &str) -> (Verdict, String) {
    let json = text.find('{').zip(text.rfind('}')).filter(|(a, b)| a < b).map(|(a, b)| &text[a..=b]);
    if let Some(v) = json.and_then(|j| serde_json::from_str::<Value>(j).ok()) {
        let reason = v.get("reason").and_then(Value::as_str).unwrap_or("").trim().to_string();
        let evidence = v
            .get("evidence")
            .and_then(Value::as_array)
            .is_some_and(|e| e.iter().any(|x| x.as_str().is_some_and(|s| !s.trim().is_empty())));
        let verdict = match v.get("verdict").and_then(Value::as_str).map(|s| s.to_ascii_lowercase()) {
            Some(s) if s == "met" && !evidence => {
                return (Verdict::NotMet, "the check named no tool output that shows it".into());
            }
            Some(s) if s == "met" => Verdict::Met,
            Some(s) if s == "impossible" => Verdict::Impossible,
            Some(s) if s == "not_met" || s == "not met" => Verdict::NotMet,
            _ => return (Verdict::NotMet, "the check gave no clear verdict".into()),
        };
        return (verdict, reason);
    }
    (Verdict::NotMet, "the check's answer could not be read".into())
}

/// The reason of an explicit "not met" verdict, and `None` for anything else: met, impossible,
/// text that isn't a complete JSON object (cut off by the output cap, prose around braces), a
/// missing or unknown verdict. For callers that must fail open: only the checker's own finding
/// sends work back, never a parse fallback of [`parse_verdict`].
pub fn explicit_not_met(text: &str) -> Option<String> {
    let json = text.find('{').zip(text.rfind('}')).filter(|(a, b)| a < b).map(|(a, b)| &text[a..=b])?;
    let v = serde_json::from_str::<Value>(json).ok()?;
    let verdict = v.get("verdict").and_then(Value::as_str)?.to_ascii_lowercase();
    if verdict != "not_met" && verdict != "not met" {
        return None;
    }
    let reason = v.get("reason").and_then(Value::as_str).unwrap_or("").trim().to_string();
    Some(if reason.is_empty() { "the check gave no reason".into() } else { reason })
}

pub const EVALUATOR_PROMPT: &str = "You are the independent checker of a coding agent's goal. Decide whether the \
goal holds now, using only evidence: the tool calls in the transcript and what they returned (command output, exit \
codes, test runs, file contents, diffs).\n\n\
Rules:\n\
- AGENT lines are claims, not evidence. A summary, report or checklist the agent wrote proves nothing by itself: \
check every claim the goal depends on against TOOL RESULT lines.\n\
- A TOOL ERROR, a non-zero exit code, or a sub-agent result that starts with FAILED means that piece of work did \
not happen, unless a later successful tool result shows it was done again.\n\
- Details the agent states that no tool result shows (counts, test totals, names, file contents) are unverified. \
If the goal depends on them, it is not met.\n\
- If the goal names a check (tests pass, a build succeeds, a file exists), the transcript must show that check \
succeeding after the last change.\n\
- A HARNESS line is the agent harness's own record of the last turn (which changed files no check has run on, \
whether the last check passed or failed). It is evidence, and it outranks an AGENT claim that contradicts it.\n\
- When in doubt, the answer is not_met.\n\n\
Reply with only a JSON object: {\"verdict\": \"met\" | \"not_met\" | \"impossible\", \"evidence\": [\"the \
tool results that prove it, quoted or closely paraphrased\"], \"reason\": \"one or two sentences\"}.\n\
- met: the evidence shows the goal holds now. A met verdict with no evidence counts as not_met.\n\
- impossible: it can't be reached from here (it contradicts itself, needs access or information the agent can't \
get, or the work showed it can't be done).\n\
- not_met: anything else; the reason says what is still missing or unproven.";

fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &s[start..])
}

fn is_reminder(text: &str) -> bool {
    text.trim_start().starts_with("<system-reminder>")
}

/// The conversation for the evaluator: prompts, replies, tool calls with their
/// input, and the end of each tool result, labelled with its tool. The newest
/// part when it is long, after a list of every failed tool call, so a failure
/// early in a long session can't be cut off.
pub fn evaluator_transcript(messages: &[Message]) -> String {
    let mut names: std::collections::HashMap<&str, &str> = std::collections::HashMap::new();
    for m in messages {
        for b in &m.content {
            if let ContentBlock::ToolUse { id, name, .. } = b {
                names.insert(id.as_str(), name.as_str());
            }
        }
    }
    let last_agent = messages.iter().rposition(|m| {
        m.role == Role::Assistant
            && m.content.iter().any(|b| matches!(b, ContentBlock::Text { text, .. } if !text.trim().is_empty()))
    });
    let mut out = String::new();
    let mut failures = vec![];
    for (i, m) in messages.iter().enumerate() {
        for b in &m.content {
            match (m.role, b) {
                (Role::User, ContentBlock::Text { text, .. }) if !is_reminder(text) => {
                    out.push_str(&format!("USER: {}\n\n", text.trim()));
                }
                (Role::User, ContentBlock::ToolResult { tool_use_id, content, is_error, .. }) => {
                    let name = names.get(tool_use_id.as_str()).copied().unwrap_or("tool");
                    let text = content.to_text();
                    let failed = *is_error == Some(true) || text.trim_start().starts_with("FAILED");
                    if failed {
                        failures.push(format!("- {name}: {}", first_chars(text.trim(), 300)));
                    }
                    let tag = if failed { "TOOL ERROR" } else { "TOOL RESULT" };
                    out.push_str(&format!("{tag} ({name}): {}\n\n", tail(text.trim(), RESULT_CHARS)));
                }
                (Role::Assistant, ContentBlock::Text { text, .. }) if !text.trim().is_empty() => {
                    let tag = if Some(i) == last_agent { "AGENT (final message; claims to check)" } else { "AGENT" };
                    out.push_str(&format!("{tag}: {}\n\n", text.trim()));
                }
                (Role::Assistant, ContentBlock::ToolUse { name, input, .. }) => {
                    out.push_str(&format!("TOOL CALL {name}: {}\n\n", tail(&input.to_string(), RESULT_CHARS)));
                }
                _ => {}
            }
        }
    }
    let body = tail(out.trim_end(), TRANSCRIPT_CHARS);
    if failures.is_empty() {
        return body;
    }
    let list = tail(&failures.join("\n"), FAILURES_CHARS);
    format!("FAILED TOOL CALLS in this session, oldest first:\n{list}\n\nTRANSCRIPT:\n{body}")
}

fn first_chars(s: &str, max: usize) -> String {
    let one: String = s.chars().take(max).collect();
    if one.len() < s.len() {
        format!("{}…", one.replace('\n', " "))
    } else {
        one.replace('\n', " ")
    }
}

/// What KodAct itself recorded about the last turn's changes, for the evaluator: the same
/// state the verification reminder and the result's `verification` field are decided on. Said
/// only where it bears on a goal: changes nobody checked, how the last check ended, and the
/// files the turn's request asks for that don't exist (`missing`, the result's
/// `verification.missing`). A turn that changed nothing and left nothing missing says nothing
/// (an earlier turn's changes are in the transcript).
pub fn harness_note(state: Option<&crate::Verified>, missing: &[String]) -> String {
    use crate::Verified;
    let mut said = match state {
        Some(Verified::Unchecked { files, shell: false }) if !files.is_empty() => {
            format!("in the last turn these files changed and no check has run since: {}.", files.join(", "))
        }
        Some(Verified::Unchecked { .. }) => {
            "in the last turn files changed through shell commands and no check has run since.".to_string()
        }
        Some(Verified::Failed { command }) => {
            format!("the last check (`{command}`) failed, and nothing changed after it.")
        }
        Some(Verified::Passed { command }) => {
            format!("the last check (`{command}`) passed, and nothing changed after it.")
        }
        Some(Verified::Ran { command }) => {
            format!(
                "the last check (`{command}`) ran and nothing changed after it, but the command hides the check's \
                 exit code (a pipe, or commands after it), so whether it passed is not known."
            )
        }
        _ => String::new(),
    };
    if !missing.is_empty() {
        let files = missing.join(", ");
        said = if said.is_empty() {
            format!("files the last request names don't exist: {files}.")
        } else {
            format!("{said} Files the last request names don't exist: {files}.")
        };
    }
    if said.is_empty() {
        return String::new();
    }
    format!("\n\nHARNESS: {said}")
}
