//! `/goal` (contract C18): a condition KodAct keeps working toward. After each
//! turn a small model checks the conversation against it. When the goal isn't
//! met, the driver starts another turn. The loop ends when the goal is met,
//! judged impossible or cleared, or a guard pauses it.

use std::time::Instant;

use serde_json::{json, Value};

pub use kodact_engine::judge::{evaluator_transcript, harness_note, parse_verdict, Verdict, EVALUATOR_PROMPT};

/// Longest condition accepted.
pub const MAX_CONDITION: usize = 4000;
/// Consecutive turns without a tool call before the loop pauses.
pub const MAX_IDLE_TURNS: u32 = 3;
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
    use kodact_types::{ContentBlock, Message};

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
        // No verdict to read is never a pass, and never a "not met" either: it judged nothing.
        assert_eq!(parse_verdict("met!").0, Verdict::Unclear, "no JSON is never a pass");
        assert_eq!(parse_verdict(r#"{"verdict":"maybe"}"#).0, Verdict::Unclear);
        assert_eq!(parse_verdict("").0, Verdict::Unclear, "a reply cut off while it reasoned");
        assert_eq!(parse_verdict(r#"{"verdict": "not_met", "reason": "cut o"#).0, Verdict::Unclear);
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
        let ran = harness_note(Some(&Verified::Ran { command: "make test | tail".into() }), &[]);
        assert!(ran.contains("`make test | tail`") && ran.contains("not known"), "{ran}");
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
