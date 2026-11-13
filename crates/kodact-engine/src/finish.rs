//! The finish tool of unattended runs (`--autonomous`): the run ends with a summary and the
//! model's claim of whether its own tool output shows the requirements met, recorded next to
//! what KodAct observed (`verification.status`). A run that says "verified" with no check run
//! since its last change is then a false finish a script can see.
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
         checked it. Set `verified` to true only if tool output in this run shows every requirement met (the \
         tests or the program run after your last change, the files where they were asked for); otherwise \
         false, and say in the summary what was not checked. Call it on its own, not with other tools."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string", "description": "The final answer: what changed and how it was checked"},
                "verified": {"type": "boolean", "description": "True only if this run's tool output shows the requirements met"}
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
