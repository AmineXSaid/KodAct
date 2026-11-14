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

pub struct Finish;

#[async_trait::async_trait]
impl Tool for Finish {
    fn name(&self) -> &str {
        NAME
    }

    fn description(&self) -> String {
        "End the run, once the work is done. `summary` is your final answer: what you changed and how you \
         checked it. Set `verified` to true only if a check you ran after your last change would have failed \
         had the work been wrong, and name that check in the summary: tests or a program run whose result \
         depends on what you chose. A check that \
         passes whatever you chose (a sample every option handles alike, a test of a part you didn't change) \
         verifies nothing: then set false, and name in the summary what you guessed and what was not checked. \
         Call it on its own, not with other tools."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": "The final answer: what changed, how it was checked, and any guess no check could tell apart"},
                "verified": {"type": "boolean", "description": "True only if a check run after the last change would have failed had the work been wrong"}
            },
            "required": ["summary", "verified"],
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

/// The claim a lone Finish call makes: `{"summary", "verified"}`.
pub(crate) fn claim(input: &Value) -> Value {
    json!({
        "summary": input.get("summary").and_then(Value::as_str).unwrap_or_default(),
        "verified": input.get("verified").and_then(Value::as_bool).unwrap_or(false),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `verified` is defined by a check that could have failed; a guess no check could tell
    /// apart is named in the summary instead.
    #[test]
    fn verified_asks_for_a_check_that_could_have_failed() {
        let d = Finish.description();
        assert!(d.contains("would have failed had the work been wrong, and name that check"), "{d}");
        assert!(d.contains("passes whatever you chose") && d.contains("name in the summary what you guessed"), "{d}");
        let schema = Finish.input_schema();
        let verified = schema["properties"]["verified"]["description"].as_str().unwrap();
        assert!(verified.contains("would have failed had the work been wrong"), "{verified}");
    }
}
