//! The stream-json protocol (`--output-format stream-json`,
//! `--input-format stream-json`).
//!
//! Every line on stdout/stdin is one JSON object with a `type`. Conversation
//! traffic is `system` / `assistant` / `user` / `stream_event` / `result`; the
//! control channel is `control_request` / `control_response` and runs in both
//! directions (the host asks the CLI to interrupt or change mode; the CLI asks
//! the host whether a tool may run).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::api::{ApiMessage, MessageContent, Role, StreamEvent, Usage};

/// Version of the reference CLI whose protocol this implementation tracks.
pub const PROTOCOL_COMPAT_VERSION: &str = "2.1.290";

/// One line of the stream-json protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SdkMessage {
    System(SystemMessage),
    Assistant(AssistantMessage),
    User(UserMessage),
    StreamEvent(StreamEventMessage),
    Result(Box<ResultMessage>),
    ControlRequest(ControlRequest),
    ControlResponse(ControlResponse),
    ControlCancelRequest { request_id: String },
    ToolProgress(ToolProgress),
    KeepAlive,
}

/// `{"type":"system","subtype":...}`. The subtype decides the other fields,
/// so they are kept as an open map (`init`, `status`, `compact_boundary`,
/// `api_retry`, `hook_started`, `hook_response`, ...).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemMessage {
    pub subtype: String,
    #[serde(flatten)]
    pub data: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerStatus {
    pub name: String,
    pub status: String,
}

/// Fields of the `system/init` message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InitInfo {
    pub cwd: String,
    pub session_id: String,
    pub tools: Vec<String>,
    pub mcp_servers: Vec<McpServerStatus>,
    pub model: String,
    #[serde(rename = "permissionMode")]
    pub permission_mode: String,
    pub slash_commands: Vec<String>,
    #[serde(rename = "apiKeySource")]
    pub api_key_source: String,
    pub kodact_version: String,
    pub output_style: String,
    pub agents: Vec<String>,
    pub skills: Vec<String>,
    pub plugins: Vec<Value>,
    pub uuid: String,
}

impl SystemMessage {
    pub fn init(info: &InitInfo) -> Self {
        let Value::Object(data) = serde_json::to_value(info).expect("init serializes") else { unreachable!() };
        SystemMessage { subtype: "init".into(), data }
    }

    pub fn new(subtype: &str, data: Value) -> Self {
        let data = match data {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        SystemMessage { subtype: subtype.into(), data }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub message: ApiMessage,
    pub parent_tool_use_id: Option<String>,
    pub session_id: String,
    pub uuid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// User turn as it travels on the wire (`content` may be a bare string).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserPayload {
    pub role: Role,
    pub content: MessageContent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UserMessage {
    pub message: UserPayload,
    #[serde(default)]
    pub parent_tool_use_id: Option<String>,
    #[serde(default)]
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uuid: Option<String>,
    /// Set on replays emitted by `--replay-user-messages`.
    #[serde(rename = "isReplay", default, skip_serializing_if = "Option::is_none")]
    pub is_replay: Option<bool>,
    /// Set on synthetic messages (tool results, reminders) the CLI inserts.
    #[serde(rename = "isSynthetic", default, skip_serializing_if = "Option::is_none")]
    pub is_synthetic: Option<bool>,
    /// Structured result of the tool whose `tool_result` this message carries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamEventMessage {
    pub event: StreamEvent,
    pub parent_tool_use_id: Option<String>,
    pub session_id: String,
    pub uuid: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolProgress {
    pub tool_use_id: String,
    pub tool_name: String,
    pub parent_tool_use_id: Option<String>,
    pub elapsed_time_seconds: f64,
    pub session_id: String,
    pub uuid: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResultSubtype {
    Success,
    ErrorMaxTurns,
    ErrorDuringExecution,
    ErrorMaxBudgetUsd,
    ErrorMaxStructuredOutputRetries,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelUsage {
    #[serde(rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: u64,
    #[serde(rename = "cacheReadInputTokens")]
    pub cache_read_input_tokens: u64,
    #[serde(rename = "cacheCreationInputTokens")]
    pub cache_creation_input_tokens: u64,
    #[serde(rename = "webSearchRequests")]
    pub web_search_requests: u64,
    #[serde(rename = "costUSD")]
    pub cost_usd: f64,
    #[serde(rename = "contextWindow")]
    pub context_window: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermissionDenial {
    pub tool_name: String,
    pub tool_use_id: String,
    pub tool_input: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResultMessage {
    pub subtype: ResultSubtype,
    pub is_error: bool,
    pub duration_ms: u64,
    pub duration_api_ms: u64,
    pub num_turns: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    pub stop_reason: Option<String>,
    pub session_id: String,
    pub total_cost_usd: f64,
    pub usage: Usage,
    #[serde(rename = "modelUsage")]
    pub model_usage: Map<String, Value>,
    pub permission_denials: Vec<PermissionDenial>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
    /// `{"missing": [...]}`: the files the request asks to be made that don't exist when the
    /// turn ends. Absent when there are none, and with the verification loop off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<Value>,
    /// How an unattended run ended with the Finish tool: `{"summary", "verified"}`. `verified`
    /// is the model's claim; `verification` is what KodAct observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finish: Option<Value>,
    pub uuid: String,
    /// `true` on the answer to an immediate command a stream-json host sent (C17): it was
    /// answered at once, beside any turn in progress, and is not that turn's result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub immediate: Option<bool>,
}

/// `{"type":"control_request","request_id":..,"request":{"subtype":..,...}}`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlRequest {
    pub request_id: String,
    pub request: ControlRequestBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlRequestBody {
    pub subtype: String,
    #[serde(flatten)]
    pub data: Map<String, Value>,
}

impl ControlRequestBody {
    pub fn new(subtype: &str, data: Value) -> Self {
        let data = match data {
            Value::Object(m) => m,
            _ => Map::new(),
        };
        ControlRequestBody { subtype: subtype.into(), data }
    }

    pub fn get_str(&self, key: &str) -> Option<&str> {
        self.data.get(key).and_then(Value::as_str)
    }
}

/// `{"type":"control_response","response":{"subtype":"success"|"error",...}}`
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ControlResponse {
    pub response: ControlResponseBody,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "subtype", rename_all = "snake_case")]
pub enum ControlResponseBody {
    Success {
        request_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response: Option<Value>,
    },
    Error {
        request_id: String,
        error: String,
    },
}

impl ControlResponseBody {
    pub fn request_id(&self) -> &str {
        match self {
            ControlResponseBody::Success { request_id, .. } | ControlResponseBody::Error { request_id, .. } => {
                request_id
            }
        }
    }
}

impl SdkMessage {
    pub fn success(request_id: &str, response: Option<Value>) -> Self {
        SdkMessage::ControlResponse(ControlResponse {
            response: ControlResponseBody::Success { request_id: request_id.into(), response },
        })
    }

    pub fn error(request_id: &str, error: impl Into<String>) -> Self {
        SdkMessage::ControlResponse(ControlResponse {
            response: ControlResponseBody::Error { request_id: request_id.into(), error: error.into() },
        })
    }

    pub fn to_line(&self) -> String {
        serde_json::to_string(self).expect("sdk message serializes")
    }
}

/// The host's answer to a `can_use_tool` control request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum PermissionResult {
    Allow {
        #[serde(rename = "updatedInput", default, skip_serializing_if = "Option::is_none")]
        updated_input: Option<Value>,
        #[serde(rename = "updatedPermissions", default, skip_serializing_if = "Option::is_none")]
        updated_permissions: Option<Vec<Value>>,
    },
    Deny {
        #[serde(default)]
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        interrupt: Option<bool>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_host_user_message_with_string_content() {
        let line =
            r#"{"type":"user","message":{"role":"user","content":"hello"},"parent_tool_use_id":null,"session_id":""}"#;
        let msg: SdkMessage = serde_json::from_str(line).unwrap();
        match msg {
            SdkMessage::User(u) => assert_eq!(u.message.content, MessageContent::Text("hello".into())),
            _ => panic!("expected user"),
        }
    }

    #[test]
    fn control_round_trip() {
        let line = r#"{"type":"control_request","request_id":"req_1","request":{"subtype":"set_permission_mode","mode":"plan"}}"#;
        let msg: SdkMessage = serde_json::from_str(line).unwrap();
        let SdkMessage::ControlRequest(req) = &msg else { panic!() };
        assert_eq!(req.request.subtype, "set_permission_mode");
        assert_eq!(req.request.get_str("mode"), Some("plan"));
        assert_eq!(serde_json::to_value(&msg).unwrap(), serde_json::from_str::<Value>(line).unwrap());

        let ok = SdkMessage::success("req_1", None);
        assert_eq!(
            serde_json::to_value(ok).unwrap(),
            json!({"type":"control_response","response":{"subtype":"success","request_id":"req_1"}})
        );
    }

    #[test]
    fn permission_result_shapes() {
        let allow: PermissionResult =
            serde_json::from_value(json!({"behavior":"allow","updatedInput":{"command":"ls"}})).unwrap();
        assert!(matches!(allow, PermissionResult::Allow { updated_input: Some(_), .. }));
        let deny: PermissionResult = serde_json::from_value(json!({"behavior":"deny","message":"no"})).unwrap();
        assert!(matches!(deny, PermissionResult::Deny { .. }));
    }

    #[test]
    fn system_init_flattens() {
        let info = InitInfo {
            cwd: "/w".into(),
            session_id: "s".into(),
            tools: vec!["Bash".into()],
            mcp_servers: vec![],
            model: "m".into(),
            permission_mode: "default".into(),
            slash_commands: vec![],
            api_key_source: "KODACT_API_KEY".into(),
            kodact_version: "0.1.0".into(),
            output_style: "default".into(),
            agents: vec![],
            skills: vec![],
            plugins: vec![],
            uuid: "u".into(),
        };
        let v = serde_json::to_value(SdkMessage::System(SystemMessage::init(&info))).unwrap();
        assert_eq!(v["type"], "system");
        assert_eq!(v["subtype"], "init");
        assert_eq!(v["permissionMode"], "default");
        assert_eq!(v["tools"][0], "Bash");
    }
}
