//! Tools the model can call.
//!
//! A [`Tool`] declares its schema and how it is checked (read-only or not,
//! safe to run concurrently or not, what a permission rule matches against)
//! and implements `call`. The engine owns ordering, permissions and hooks;
//! tools only do the work.

pub mod builtin;
mod context;
mod files;
pub mod html;
pub mod injection;
mod registry;
pub mod sandbox;
pub mod shells;
mod util;

pub use context::{Checkpointer, SandboxCell, ToolContext};
pub use files::FileState;
pub use registry::ToolRegistry;
pub use util::{expand_path, fit_output, missing_path_hint, truncate_middle};

use kodact_permissions::Subject;
use kodact_types::{ContentBlock, ToolResultContent};
use serde_json::Value;

/// What a tool returns.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub content: ToolResultContent,
    pub is_error: bool,
    /// Structured result for SDK hosts (`tool_use_result` on the user message).
    pub structured: Option<Value>,
}

impl ToolOutput {
    pub fn text(s: impl Into<String>) -> Self {
        ToolOutput { content: ToolResultContent::Text(s.into()), is_error: false, structured: None }
    }

    pub fn error(s: impl Into<String>) -> Self {
        ToolOutput { content: ToolResultContent::Text(s.into()), is_error: true, structured: None }
    }

    pub fn blocks(blocks: Vec<ContentBlock>) -> Self {
        ToolOutput { content: ToolResultContent::Blocks(blocks), is_error: false, structured: None }
    }

    pub fn with_structured(mut self, v: Value) -> Self {
        self.structured = Some(v);
        self
    }

    pub fn text_content(&self) -> String {
        self.content.to_text()
    }
}

/// Text shown to the model when a call is aborted by an interrupt (contract C3).
pub const INTERRUPTED: &str = "Interrupted by user";

/// How a Task agent's result begins when the user stopped that agent alone (a host's
/// `stop_task`) and the turn went on. Hosts match this prefix to show the call as stopped.
pub const AGENT_STOPPED: &str = "Stopped by user";

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;

    /// Built-in description (the engine may replace it from `KODACT_PROMPTS_DIR`).
    fn description(&self) -> String;

    fn input_schema(&self) -> Value;

    /// The call cannot modify anything.
    fn is_read_only(&self, _input: &Value) -> bool {
        false
    }

    /// The call may run in parallel with other concurrency-safe calls.
    fn is_concurrency_safe(&self, input: &Value) -> bool {
        self.is_read_only(input)
    }

    /// The call is a question for the person (AskUserQuestion, plan approval): it always
    /// asks, whatever the mode or rules allow (contract C8).
    fn needs_user(&self) -> bool {
        false
    }

    /// Will this call run inside the OS sandbox? Sandboxed calls need no permission prompt.
    fn sandboxed(&self, _input: &Value, _ctx: &ToolContext) -> bool {
        false
    }

    /// What permission rules match against for this call.
    fn permission_subject(&self, _input: &Value, _ctx: &ToolContext) -> Subject {
        Subject::None
    }

    /// Reject malformed input before permissions are asked.
    fn validate(&self, input: &Value, _ctx: &ToolContext) -> Result<(), String> {
        validate_required(&self.input_schema(), input)
    }

    /// Present to the model? (e.g. tools needing an unavailable backend are hidden)
    fn is_enabled(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput;
}

/// Minimal schema check: object input, required keys present, primitive types right.
pub fn validate_required(schema: &Value, input: &Value) -> Result<(), String> {
    let Some(obj) = input.as_object() else { return Err("input must be a JSON object".into()) };
    if let Some(req) = schema.get("required").and_then(Value::as_array) {
        for k in req.iter().filter_map(Value::as_str) {
            if !obj.contains_key(k) || obj[k].is_null() {
                return Err(missing_parameter(k, schema, obj));
            }
        }
    }
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        for (k, v) in obj {
            let Some(ty) = props.get(k).and_then(|p| p.get("type")).and_then(Value::as_str) else { continue };
            let ok = match ty {
                "string" => v.is_string(),
                "number" => v.is_number(),
                "integer" => v.is_i64() || v.is_u64(),
                "boolean" => v.is_boolean(),
                "array" => v.is_array(),
                "object" => v.is_object(),
                _ => true,
            };
            if !ok && !v.is_null() {
                return Err(format!(
                    "The parameter `{k}` type is expected as `{ty}` but provided as `{}`",
                    type_name(v)
                ));
            }
        }
    }
    Ok(())
}

/// The error for a missing required parameter, naming the keys the call has that the tool doesn't
/// take. A model that wrote its file body under `file_path_content` would otherwise read only
/// "`content` is missing", not see why, and lose the work instead of sending the call again.
fn missing_parameter(k: &str, schema: &Value, obj: &serde_json::Map<String, Value>) -> String {
    let base = format!("The required parameter `{k}` is missing");
    let Some(props) = schema.get("properties").and_then(Value::as_object) else { return base };
    let unknown: Vec<&String> = obj.keys().filter(|key| !props.contains_key(key.as_str())).collect();
    if unknown.is_empty() {
        return base;
    }
    let names = unknown.iter().map(|u| format!("`{u}`")).collect::<Vec<_>>().join(", ");
    let has = format!("{base}. The call has {names}, which this tool does not take");
    // A near miss: the one unknown key contains the missing name, or the other way round.
    if let [u] = unknown.as_slice() {
        if u.contains(k) || k.contains(u.as_str()) {
            return format!("{has}: did you mean `{k}`?");
        }
    }
    let params = props.keys().map(|p| format!("`{p}`")).collect::<Vec<_>>().join(", ");
    format!("{has}. Its parameters are {params}.")
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_schema() -> Value {
        json!({
            "type": "object",
            "properties": {"file_path": {"type": "string"}, "content": {"type": "string"}},
            "required": ["file_path", "content"],
        })
    }

    #[test]
    fn a_missing_parameter_names_keys_the_tool_does_not_take() {
        // Seen in a run: the file body sent as `file_path_content`.
        let e = validate_required(&write_schema(), &json!({"file_path": "/a.pl", "file_path_content": "code"}))
            .unwrap_err();
        assert_eq!(
            e,
            "The required parameter `content` is missing. The call has `file_path_content`, which this tool \
             does not take: did you mean `content`?"
        );

        // No stray key: the message is the usual one.
        let e = validate_required(&write_schema(), &json!({"file_path": "/a.pl"})).unwrap_err();
        assert_eq!(e, "The required parameter `content` is missing");

        // An unrelated key: no guess, the parameters are listed instead.
        let e = validate_required(&write_schema(), &json!({"file_path": "/a.pl", "text": "code"})).unwrap_err();
        assert!(
            e.contains("The call has `text`, which this tool does not take")
                && e.ends_with("Its parameters are `file_path`, `content`."),
            "{e}"
        );

        // A complete call still passes, stray keys and all.
        assert!(validate_required(&write_schema(), &json!({"file_path": "/a", "content": "x", "extra": 1})).is_ok());
    }
}
