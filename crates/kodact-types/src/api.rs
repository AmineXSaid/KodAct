//! Messages API types.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

/// Image or document payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MediaSource {
    Base64 { media_type: String, data: String },
    Url { url: String },
    Text { media_type: String, data: String },
}

/// Content of a `tool_result` block: either a plain string or nested blocks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ToolResultContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl ToolResultContent {
    /// Flatten to text (images become a placeholder).
    pub fn to_text(&self) -> String {
        match self {
            ToolResultContent::Text(s) => s.clone(),
            ToolResultContent::Blocks(blocks) => blocks
                .iter()
                .map(|b| match b {
                    ContentBlock::Text { text, .. } => text.clone(),
                    ContentBlock::Image { .. } => "[image]".to_string(),
                    other => serde_json::to_string(other).unwrap_or_default(),
                })
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }
}

impl Default for ToolResultContent {
    fn default() -> Self {
        ToolResultContent::Text(String::new())
    }
}

/// Prompt-caching marker attached to a block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheControl {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ttl: Option<String>,
}

impl CacheControl {
    pub fn ephemeral() -> Self {
        CacheControl { kind: "ephemeral".into(), ttl: None }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        citations: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
        /// What a provider attached to this text and needs back exactly as it
        /// sent it (Gemini's `thoughtSignature` on a final text part). Kept in
        /// transcripts; never sent on the Messages wire.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<Value>,
    },
    Image {
        source: MediaSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    Document {
        source: MediaSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        title: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
        /// What a provider attached to this call and needs back exactly as it
        /// sent it: Gemini's `thoughtSignature`, an OpenAI-compatible
        /// endpoint's `extra_content`, a gateway's `thought_signature`. Kept in
        /// transcripts; never sent on the Messages wire.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<Value>,
    },
    ToolResult {
        tool_use_id: String,
        #[serde(default)]
        content: ToolResultContent,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    /// A tool executed by the API itself (e.g. `web_search`).
    ServerToolUse {
        id: String,
        name: String,
        input: Value,
    },
    WebSearchToolResult {
        tool_use_id: String,
        content: Value,
    },
    WebFetchToolResult {
        tool_use_id: String,
        content: Value,
    },
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        ContentBlock::Text { text: text.into(), citations: None, cache_control: None, provider: None }
    }

    pub fn tool_result(id: impl Into<String>, content: impl Into<String>, is_error: bool) -> Self {
        ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: ToolResultContent::Text(content.into()),
            is_error: if is_error { Some(true) } else { None },
            cache_control: None,
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text { text, .. } => Some(text),
            _ => None,
        }
    }

    /// Set (or clear) the cache marker on blocks that support one.
    pub fn set_cache_control(&mut self, cc: Option<CacheControl>) {
        match self {
            ContentBlock::Text { cache_control, .. }
            | ContentBlock::Image { cache_control, .. }
            | ContentBlock::Document { cache_control, .. }
            | ContentBlock::ToolUse { cache_control, .. }
            | ContentBlock::ToolResult { cache_control, .. } => *cache_control = cc,
            _ => {}
        }
    }
}

/// Message content: the API accepts a bare string for user turns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

impl MessageContent {
    pub fn into_blocks(self) -> Vec<ContentBlock> {
        match self {
            MessageContent::Text(t) => vec![ContentBlock::text(t)],
            MessageContent::Blocks(b) => b,
        }
    }

    pub fn blocks(&self) -> Vec<ContentBlock> {
        self.clone().into_blocks()
    }
}

/// A message as sent to the API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn user_text(text: impl Into<String>) -> Self {
        Message { role: Role::User, content: vec![ContentBlock::text(text)] }
    }

    pub fn user(content: Vec<ContentBlock>) -> Self {
        Message { role: Role::User, content }
    }

    pub fn assistant(content: Vec<ContentBlock>) -> Self {
        Message { role: Role::Assistant, content }
    }

    /// Concatenated text of all text blocks.
    pub fn text(&self) -> String {
        self.content.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("")
    }

    pub fn tool_uses(&self) -> impl Iterator<Item = (&str, &str, &Value)> {
        self.content.iter().filter_map(|b| match b {
            ContentBlock::ToolUse { id, name, input, .. } => Some((id.as_str(), name.as_str(), input)),
            _ => None,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_tool_use: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<String>,
    /// Of `output_tokens`, the reasoning an endpoint reports without sending it
    /// (OpenAI-compatible `completion_tokens_details.reasoning_tokens`). KodAct's own
    /// book-keeping: never on the wire.
    #[serde(skip)]
    pub reasoning_output_tokens: u64,
}

impl Usage {
    pub fn add(&mut self, other: &Usage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.reasoning_output_tokens += other.reasoning_output_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
    }

    /// Tokens that occupied the context window for the request.
    pub fn context_tokens(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens + self.output_tokens
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    StopSequence,
    ToolUse,
    PauseTurn,
    Refusal,
    ModelContextWindowExceeded,
    #[serde(other)]
    Other,
}

/// A complete assistant response (`type: "message"`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiMessage {
    pub id: String,
    #[serde(rename = "type", default = "message_type")]
    pub kind: String,
    pub role: Role,
    pub model: String,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<StopReason>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
    #[serde(default)]
    pub usage: Usage,
}

fn message_type() -> String {
    "message".into()
}

impl ApiMessage {
    pub fn to_message(&self) -> Message {
        Message { role: self.role, content: self.content.clone() }
    }
}

/// Tool definition sent to the API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub input_schema: Value,
    /// Server tools (`web_search_20250305`, ...) carry a `type` and no schema.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

/// System prompt block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SystemBlock {
    #[serde(rename = "type")]
    pub kind: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_control: Option<CacheControl>,
}

impl SystemBlock {
    pub fn text(text: impl Into<String>) -> Self {
        SystemBlock { kind: "text".into(), text: text.into(), cache_control: None }
    }
}

/// Extended-thinking configuration.
///
/// Current models take `adaptive` (depth is steered by `output_config.effort`);
/// older ones take an explicit `budget_tokens`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ThinkingConfig {
    Enabled {
        budget_tokens: u32,
    },
    Disabled,
    Adaptive {
        /// `summarized`, `omitted` or `updates`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<String>,
    },
    /// Thinking off except between tool calls (Sonnet 5.5's replacement for `disabled`).
    BetweenTools,
}

/// A request to `POST /v1/messages`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    pub messages: Vec<Message>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub system: Vec<SystemBlock>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_choice: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<ThinkingConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_config: Option<Value>,
    /// `"fast"`: fast mode, on models that support it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    #[serde(default)]
    pub stream: bool,
    /// Beta flags this request needs beyond the provider's own. They go in a
    /// header, not the body.
    #[serde(skip)]
    pub betas: Vec<String>,
    /// The chat-completions writer sends each assistant message's reasoning back
    /// (`reasoning_content`) where its Thinking blocks are still in the request: the engine keeps
    /// them only for the replies it carries (`carryReasoning`). Not part of any wire format.
    #[serde(skip)]
    pub carry_reasoning: bool,
}

/// Content-block delta inside `content_block_delta`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Delta {
    TextDelta {
        text: String,
    },
    InputJsonDelta {
        partial_json: String,
    },
    ThinkingDelta {
        thinking: String,
    },
    SignatureDelta {
        signature: String,
    },
    /// Provider data for the block (merged into its `provider`): what an
    /// OpenAI-compatible or Gemini stream attaches after the block starts.
    /// Never on the Messages wire.
    ProviderDelta {
        provider: Value,
    },
    CitationsDelta {
        citation: Value,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageDeltaBody {
    pub stop_reason: Option<StopReason>,
    #[serde(default)]
    pub stop_sequence: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    #[serde(rename = "type")]
    pub kind: String,
    pub message: String,
}

/// A server-sent event from a streaming Messages request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StreamEvent {
    MessageStart {
        message: ApiMessage,
    },
    ContentBlockStart {
        index: usize,
        content_block: ContentBlock,
    },
    ContentBlockDelta {
        index: usize,
        delta: Delta,
    },
    ContentBlockStop {
        index: usize,
    },
    MessageDelta {
        delta: MessageDeltaBody,
        #[serde(default)]
        usage: Usage,
    },
    MessageStop,
    Ping,
    Error {
        error: ApiErrorBody,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_result_accepts_string_and_blocks() {
        let a: ContentBlock =
            serde_json::from_value(json!({"type":"tool_result","tool_use_id":"t","content":"ok"})).unwrap();
        let b: ContentBlock = serde_json::from_value(
            json!({"type":"tool_result","tool_use_id":"t","content":[{"type":"text","text":"ok"}]}),
        )
        .unwrap();
        match (a, b) {
            (ContentBlock::ToolResult { content: c1, .. }, ContentBlock::ToolResult { content: c2, .. }) => {
                assert_eq!(c1.to_text(), "ok");
                assert_eq!(c2.to_text(), "ok");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn stream_events_round_trip() {
        let raw = json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hi"}});
        let ev: StreamEvent = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(serde_json::to_value(&ev).unwrap(), raw);
        let stop: StreamEvent = serde_json::from_value(
            json!({"type":"message_delta","delta":{"stop_reason":"tool_use","stop_sequence":null},"usage":{"output_tokens":3}}),
        )
        .unwrap();
        assert!(matches!(stop, StreamEvent::MessageDelta { .. }));
    }

    #[test]
    fn unknown_stop_reason_is_other() {
        let s: StopReason = serde_json::from_value(json!("something_new")).unwrap();
        assert_eq!(s, StopReason::Other);
    }
}
