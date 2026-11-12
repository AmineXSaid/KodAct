//! `/goal` (contract C18): a condition KodAct keeps working toward. After each
//! turn a small model checks the conversation against it. When the goal isn't
//! met, the driver starts another turn. The loop ends when the goal is met,
//! judged impossible or cleared, or a guard pauses it.

use std::time::Instant;

use kodact_types::{ContentBlock, Message, Role};
use serde_json::{json, Value};

/// Longest condition accepted.
pub const MAX_CONDITION: usize = 4000;
/// Consecutive turns without a tool call before the loop pauses.
pub const MAX_IDLE_TURNS: u32 = 3;
/// How much of the conversation the evaluator sees, in characters (the newest part).
const TRANSCRIPT_CHARS: usize = 60_000;
/// How much of each tool result it sees (the end, where results usually are).
const RESULT_CHARS: usize = 2_000;
/// How much room the list of failed tool calls may take.
const FAILURES_CHARS: usize = 6_000;

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Active,
    Achieved,
    Failed(String),
    Cleared,
}

impl Status {
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Active => "active",
            Status::Achieved => "achieved",
            Status::Failed(_) => "failed",
            Status::Cleared => "cleared",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Goal {
    pub condition: String,
    pub status: Status,
    /// Turns checked so far.
    pub checks: u32,
    /// Consecutive goal turns without a tool call.
    pub idle: u32,
    /// The loop stopped (a guard, an error, an interrupt) until the next prompt.
    pub paused: Option<String>,
    pub last_reason: Option<String>,
    pub started: Instant,
    /// Session cost when the goal was set.
    pub cost_at_start: f64,
}

impl Goal {
    pub fn new(condition: &str, cost: f64) -> Self {
        Goal {
            condition: condition.to_string(),
            status: Status::Active,
            checks: 0,
            idle: 0,
            paused: None,
            last_reason: None,
            started: Instant::now(),
            cost_at_start: cost,
        }
    }

    pub fn is_active(&self) -> bool {
        self.status == Status::Active
    }

    /// The loop stopped without meeting the goal: judged impossible, ended by
    /// an error, or paused by a guard.
    pub fn ended_unmet(&self) -> bool {
        match self.status {
            Status::Failed(_) | Status::Cleared => true,
            Status::Active => self.paused.is_some(),
            Status::Achieved => false,
        }
    }

    /// The transcript record.
    pub fn record(&self) -> Value {
        let reason = match &self.status {
            Status::Failed(r) => Some(r.clone()),
            _ => self.last_reason.clone(),
        };
        json!({"condition": self.condition, "status": self.status.as_str(), "reason": reason})
    }

    /// A goal from a resumed transcript, if it was still active.
    pub fn restore(record: &Value, cost: f64) -> Option<Goal> {
        (record.get("status").and_then(Value::as_str) == Some("active"))
            .then(|| record.get("condition").and_then(Value::as_str))
            .flatten()
            .map(|c| Goal::new(c, cost))
    }
}

/// Words that clear the goal: `/goal clear`.
pub fn is_clear_word(s: &str) -> bool {
    matches!(s.to_ascii_lowercase().as_str(), "clear" | "stop" | "off" | "reset" | "none" | "cancel")
}

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
pub fn harness_note(state: Option<&kodact_engine::Verified>, missing: &[String]) -> String {
    use kodact_engine::Verified;
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

/// The prompt that continues the work after a check.
pub fn continue_prompt(condition: &str, reason: &str) -> String {
    let reason = if reason.is_empty() { String::new() } else { format!(" {reason}") };
    format!(
        "<system-reminder>\nGoal check: not met yet.{reason}\nKeep working toward the goal: {condition}\n\
         </system-reminder>"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_verdicts_tolerantly() {
        assert_eq!(
            parse_verdict(r#"{"verdict":"met","evidence":["cargo test: 12 passed"],"reason":"tests pass"}"#),
            (Verdict::Met, "tests pass".into())
        );
        assert_eq!(
            parse_verdict(r#"{"verdict":"met","reason":"the agent says so"}"#).0,
            Verdict::NotMet,
            "met without evidence is not a pass"
        );
        assert_eq!(parse_verdict(r#"{"verdict":"met","evidence":[" "],"reason":"x"}"#).0, Verdict::NotMet);
        assert_eq!(
            parse_verdict("Sure.\n```json\n{\"verdict\": \"NOT_MET\", \"reason\": \"2 fail\"}\n```"),
            (Verdict::NotMet, "2 fail".into())
        );
        assert_eq!(parse_verdict(r#"{"verdict":"impossible","reason":"no network"}"#).0, Verdict::Impossible);
        assert_eq!(parse_verdict("met!").0, Verdict::NotMet, "no JSON is never a pass");
        assert_eq!(parse_verdict(r#"{"verdict":"maybe"}"#).0, Verdict::NotMet);
    }

    #[test]
    fn the_harness_note_says_what_bears_on_a_goal() {
        use kodact_engine::Verified;
        let unchecked = Verified::Unchecked { files: vec!["a.py".into(), "b.py".into()], shell: false };
        assert_eq!(
            harness_note(Some(&unchecked), &[]),
            "\n\nHARNESS: in the last turn these files changed and no check has run since: a.py, b.py."
        );
        assert!(harness_note(Some(&Verified::Unchecked { files: vec![], shell: true }), &[]).contains("shell commands"));
        assert!(
            harness_note(Some(&Verified::Failed { command: "make test".into() }), &[]).contains("(`make test`) failed")
        );
        assert!(harness_note(Some(&Verified::Passed { command: "make test".into() }), &[]).contains("passed"));
        // Nothing to say of a turn that changed nothing, or with the loop off.
        assert_eq!(harness_note(Some(&Verified::NoChanges), &[]), "");
        assert_eq!(harness_note(Some(&Verified::ReadBack), &[]), "");
        assert_eq!(harness_note(None, &[]), "");
        // The files the request asks for that were never made, with or without a check to report.
        let missing = ["data.comp".to_string()];
        assert_eq!(
            harness_note(Some(&Verified::NoChanges), &missing),
            "\n\nHARNESS: files the last request names don't exist: data.comp."
        );
        assert!(harness_note(Some(&Verified::Passed { command: "gcc -o x x.c".into() }), &missing)
            .ends_with("nothing changed after it. Files the last request names don't exist: data.comp."));
        assert!(EVALUATOR_PROMPT.contains("A HARNESS line"));
    }

    #[test]
    fn records_restore_only_active_goals() {
        let g = Goal::new("tests pass", 0.0);
        let rec = g.record();
        assert_eq!(rec["status"], "active");
        assert_eq!(Goal::restore(&rec, 1.0).unwrap().condition, "tests pass");
        let mut done = g.clone();
        done.status = Status::Achieved;
        assert!(Goal::restore(&done.record(), 0.0).is_none());
        assert!(is_clear_word("Stop") && !is_clear_word("stopwatch"));
    }

    #[test]
    fn transcript_keeps_the_end_of_results() {
        let long = format!("{}PASSED", "x".repeat(5000));
        let msgs = vec![
            Message::user_text("<system-reminder>\nmemory\n</system-reminder>"),
            Message::user_text("make tests pass"),
            Message::assistant(vec![ContentBlock::ToolUse {
                id: "t".into(),
                name: "Bash".into(),
                input: json!({"command": "cargo test"}),
                cache_control: None,
                provider: None,
            }]),
            Message::user(vec![ContentBlock::tool_result("t", long, false)]),
        ];
        let t = evaluator_transcript(&msgs);
        assert!(t.starts_with("USER: make tests pass"), "{t}");
        assert!(t.contains("TOOL CALL Bash: {\"command\":\"cargo test\"}") && t.contains("PASSED"));
        assert!(t.contains("TOOL RESULT (Bash):"), "{t}");
        assert!(!t.contains("memory"));
    }

    #[test]
    fn failed_sub_agents_head_the_transcript_and_the_summary_is_a_claim() {
        let call = |id: &str| {
            Message::assistant(vec![ContentBlock::ToolUse {
                id: id.into(),
                name: "Task".into(),
                input: json!({"prompt": "audit"}),
                cache_control: None,
                provider: None,
            }])
        };
        let mut msgs = vec![Message::user_text("audit the repo with sub-agents"), call("a")];
        msgs.push(Message::user(vec![ContentBlock::tool_result(
            "a",
            "FAILED: the general-purpose agent did not finish its task. API error 429",
            true,
        )]));
        // A long stretch of later work pushes the failure out of the tail.
        for i in 0..40 {
            msgs.push(Message::assistant(vec![ContentBlock::text(format!("reading file {i} {}", "y".repeat(2000)))]));
        }
        msgs.push(Message::assistant(vec![ContentBlock::text("Done: both sub-agents finished; 6 constants found.")]));
        let t = evaluator_transcript(&msgs);
        assert!(
            t.starts_with(
                "FAILED TOOL CALLS in this session, oldest first:\n- Task: FAILED: the general-purpose agent"
            ),
            "{}",
            &t[..300]
        );
        assert!(t.contains("AGENT (final message; claims to check): Done: both sub-agents finished"));
    }
}
