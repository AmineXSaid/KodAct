//! The finish tool of unattended runs (`--autonomous`): the run ends with a summary and the
//! model's claim of whether its own tool output shows the requirements met, recorded in the
//! result (`finish`). It is the model's word: the result carries no verdict of KodAct's own on
//! the last check (`verification.status` was removed; a pass there proved unreliable).
//!
//! `verified` is asked for only when a check would have failed had the work been wrong. In 8
//! Terminal-Bench runs of pytorch-model-cli (2026-11-14), 7 claimed it after checks that passed
//! whatever input scaling the model had guessed; the 3 whose hidden test ran all failed it. A
//! check that can't tell the options apart verifies none of them (task `finish-guess`).
//!
//! The claim lists the request's requirements, each with its evidence and whether it is met
//! (`requirements`): work is done when each requirement has evidence read from the current
//! state, as in Codex's goal audit (docs/research/harness-study/01-codex.md). In the
//! deep-thinking runs of 2026-11-16 and 17, 15 of 94 claims of `verified` failed their tests,
//! most after checks of something other than what the request states: a tuple where it asks
//! for a list, a call shaped unlike the one it describes, a requirement it names that no check
//! looked at, a value just past its limit measured on other data. `verified` can't claim more
//! than its parts: a requirement not shown met makes it false. The first Finish of a turn that
//! lists one, with time left, is answered with them ([`unmet_note`], task `finish-unmet`): an
//! unattended run has nobody to ask for another try.
//!
//! The idea comes from Cline's `submit_and_exit` (Apache-2.0), described in
//! docs/research/harness-study/05-cline.md; this is KodAct's own form of it. A call alone
//! ends the reply as text would, through the same end-of-turn checks: KodAct's reminders can
//! still send the model back, and then the claim is dropped.

use kodact_permissions::Subject;
use kodact_tools::{Tool, ToolContext, ToolOutput};
use serde_json::{json, Value};

pub const NAME: &str = "Finish";

/// The answer to a Finish call that ends the reply.
pub(crate) const RECEIVED: &str = "Received.";

/// A Finish call made with other tools: their results are not read yet.
const NOT_ALONE: &str = "Finish was not accepted: it was called together with other tools, whose results you have \
                         not read yet. Read them, then call Finish on its own.";

/// Requirements the note names; the rest are counted.
const NOTE_ITEMS: usize = 8;
/// Characters of a requirement, or of its evidence, the note quotes.
const NOTE_CHARS: usize = 300;

pub struct Finish;

#[async_trait::async_trait]
impl Tool for Finish {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> String {
        "End the run, once the work is done. `summary` is your final answer: what you changed and how you \
         checked it. In `requirements`, list what the request asks for, in its words: each file, interface, \
         format, value or limit it states. For each, `evidence` is a check you ran after your last change that \
         would have failed had the requirement not been met (the command and what it showed), run in the form \
         the request describes (its calls, arguments, formats and types), not only as your own code uses it. A \
         check that passes whatever you chose (a sample every option handles alike, a test of a part you didn't \
         change) shows nothing, and neither does a value near its limit measured on other data or another \
         machine than the one it will be judged on: mark such a requirement not met, and name in the summary \
         what you guessed. Meet what you still can before you call Finish. Set `verified` to true only if every \
         requirement is met. Call it on its own, not with other tools."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": "The final answer: what changed, how it was checked, and any guess no check could tell apart"},
                "requirements": {
                    "type": "array",
                    "description": "What the request asks for, one item per requirement",
                    "items": {
                        "type": "object",
                        "properties": {
                            "requirement": {"type": "string", "description": "The requirement, in the request's words"},
                            "evidence": {"type": "string", "description": "The check run after the last change that shows it met: the command and what it showed. Or why there is none"},
                            "met": {"type": "boolean", "description": "True only if the evidence shows it met"}
                        },
                        "required": ["requirement", "evidence", "met"],
                        "additionalProperties": false
                    }
                },
                "verified": {"type": "boolean", "description": "True only if every requirement is met, shown by a check run after the last change that would have failed had the work been wrong"}
            },
            "required": ["summary", "requirements", "verified"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn permission_subject(&self, _input: &Value, _ctx: &ToolContext) -> Subject {
        Subject::Name(NAME.into())
    }

    /// Reached only when called with other tools: alone, the engine takes the call as the
    /// reply's end and does not run it.
    async fn call(&self, _input: Value, _ctx: &ToolContext) -> ToolOutput {
        ToolOutput::error(NOT_ALONE)
    }
}

/// The claim a lone Finish call makes: `{"summary", "requirements", "verified"}`. A requirement
/// without `met` isn't shown met, and `verified` is false while any isn't.
pub(crate) fn claim(input: &Value) -> Value {
    let requirements: Vec<Value> = input
        .get("requirements")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|r| {
            // A bare string is a requirement named with no evidence.
            let (text, evidence, met) = match r {
                Value::String(s) => (s.as_str(), "", false),
                _ => (
                    r.get("requirement").and_then(Value::as_str)?,
                    r.get("evidence").and_then(Value::as_str).unwrap_or_default(),
                    r.get("met").and_then(Value::as_bool).unwrap_or(false),
                ),
            };
            let text = text.trim();
            (!text.is_empty()).then(|| json!({"requirement": text, "evidence": evidence.trim(), "met": met}))
        })
        .collect();
    let all_met = requirements.iter().all(|r| r["met"] == true);
    json!({
        "summary": input.get("summary").and_then(Value::as_str).unwrap_or_default(),
        "requirements": requirements,
        "verified": input.get("verified").and_then(Value::as_bool).unwrap_or(false) && all_met,
    })
}

/// The requirements a claim lists and doesn't show met: `(requirement, evidence)`.
pub(crate) fn unmet(claim: &Value) -> Vec<(String, String)> {
    claim["requirements"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["met"] != true)
        .map(|r| {
            let field = |k: &str| r[k].as_str().unwrap_or_default().to_string();
            (field("requirement"), field("evidence"))
        })
        .collect()
}

/// The answer to the first Finish of a turn that lists requirements it doesn't show met. `clock`
/// is the run's time, used and left, when it has a limit: a fact the model has no other way to
/// know.
pub(crate) fn unmet_note(unmet: &[(String, String)], clock: Option<&str>) -> String {
    let mut lines = vec!["<system-reminder>".to_string(), "Your Finish lists requirements not shown met:".to_string()];
    for (requirement, evidence) in unmet.iter().take(NOTE_ITEMS) {
        let evidence = if evidence.trim().is_empty() { "none given".to_string() } else { clip(evidence) };
        lines.push(format!("- {} (evidence: {evidence})", clip(requirement)));
    }
    if unmet.len() > NOTE_ITEMS {
        lines.push(format!("- and {} more", unmet.len() - NOTE_ITEMS));
    }
    if let Some(clock) = clock {
        lines.push(clock.to_string());
    }
    lines.push(
        "Meet them, check each in the form the request describes, then call Finish again. If one can't be met \
         here, call Finish again with it marked not met and the reason in the summary."
            .into(),
    );
    lines.push("</system-reminder>".into());
    lines.join("\n")
}

/// `text` on one line, cut to [`NOTE_CHARS`] characters.
fn clip(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match one_line.char_indices().nth(NOTE_CHARS) {
        Some((at, _)) => format!("{}…", &one_line[..at]),
        None => one_line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Evidence is defined by a check that could have failed, in the request's own form; a guess
    /// no check could tell apart is named in the summary instead.
    #[test]
    fn evidence_is_a_check_that_could_have_failed_in_the_requests_form() {
        let d = Finish.description();
        assert!(d.contains("would have failed had the requirement not been met"), "{d}");
        assert!(d.contains("in the form the request describes"), "{d}");
        assert!(d.contains("passes whatever you chose") && d.contains("name in the summary what you guessed"), "{d}");
        assert!(d.contains("measured on other data or another machine"), "{d}");
        let schema = Finish.input_schema();
        let verified = schema["properties"]["verified"]["description"].as_str().unwrap();
        assert!(verified.contains("would have failed had the work been wrong"), "{verified}");
        let item = &schema["properties"]["requirements"]["items"];
        assert_eq!(item["required"], json!(["requirement", "evidence", "met"]));
        assert_eq!(schema["required"], json!(["summary", "requirements", "verified"]));
    }

    /// `verified` holds only while every requirement listed is met; one not met, without `met`,
    /// or named with no evidence makes it false. A claim without the list is read as before.
    #[test]
    fn verified_claims_no_more_than_its_requirements() {
        let all_met = json!({"summary": "s", "verified": true, "requirements": [
            {"requirement": "out.json has `total`", "evidence": "`python3 check.py` printed total=15", "met": true},
        ]});
        let c = claim(&all_met);
        assert_eq!(c["verified"], true);
        assert!(unmet(&c).is_empty());

        for not_shown in [
            json!({"requirement": "a list of x", "evidence": "printed a tuple", "met": false}),
            json!({"requirement": "a list of x", "evidence": "printed a tuple"}),
            json!("a list of x"),
        ] {
            let c = claim(&json!({"summary": "s", "verified": true, "requirements": [
                {"requirement": "out.json exists", "evidence": "ls", "met": true}, not_shown,
            ]}));
            assert_eq!(c["verified"], false, "{c}");
            assert_eq!(unmet(&c).len(), 1, "{c}");
            assert_eq!(unmet(&c)[0].0, "a list of x");
        }

        // An empty or missing list: the model's word, as before the list existed.
        for old in
            [json!({"summary": "s", "verified": true}), json!({"summary": "s", "verified": true, "requirements": []})]
        {
            let c = claim(&old);
            assert_eq!(c["verified"], true);
            assert_eq!(c["requirements"], json!([]));
        }
        // Items with no text are dropped, and the rest kept trimmed.
        let c = claim(
            &json!({"verified": false, "requirements": [{"requirement": "  ", "met": false}, {"requirement": " a ", "evidence": " b ", "met": true}]}),
        );
        assert_eq!(c["requirements"], json!([{"requirement": "a", "evidence": "b", "met": true}]));
    }

    /// The note quotes what the claim lists as not met, clipped, with the clock when the run has
    /// a limit, and asks for a check in the request's form.
    #[test]
    fn the_note_quotes_what_is_not_met_and_the_time_left() {
        let unmet = vec![
            ("coords_x: a list of x coordinates".to_string(), "printed (1, 2)".to_string()),
            ("at least 0.62 accuracy".to_string(), String::new()),
        ];
        let note = unmet_note(&unmet, Some("Time: 12m00s of 60m00s used, 48m00s left."));
        assert!(note.contains("- coords_x: a list of x coordinates (evidence: printed (1, 2))"), "{note}");
        assert!(note.contains("- at least 0.62 accuracy (evidence: none given)"), "{note}");
        assert!(note.contains("48m00s left"), "{note}");
        assert!(note.contains("in the form the request describes"), "{note}");
        assert!(!unmet_note(&unmet, None).contains("Time:"));

        let long = "word ".repeat(200);
        let many: Vec<_> = (0..11).map(|i| (format!("{i} {long}"), long.clone())).collect();
        let note = unmet_note(&many, None);
        assert!(note.contains("- and 3 more"), "{note}");
        assert!(note.lines().all(|l| l.chars().count() < 2 * NOTE_CHARS + 40), "{note}");
    }
}
