//! OpenAI-compatible `/v1/chat/completions` endpoints (custom gateways,
//! local servers). Requests are translated from the Messages shape and the
//! streamed chunks are translated back into Messages API stream events, so the
//! engine never sees the difference.

use std::collections::HashMap;
use std::time::Duration;

use kodact_types::{
    ApiMessage, ContentBlock, Delta, MediaSource, Message, MessageDeltaBody, MessagesRequest, Role, StopReason,
    StreamEvent, ToolResultContent, Usage,
};
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

use crate::gemini_rules::{self, ToolNames};
use crate::messages::{http_error, spawn_sse_pump};
use crate::{backoff_delay, ApiError, EventStream, Provider};

#[derive(Debug, Clone)]
pub struct OpenAiConfig {
    /// Base URL up to and including `/v1` (e.g. `http://localhost:11434/v1`).
    pub base_url: String,
    pub api_key: Option<String>,
    pub extra_headers: Vec<(String, String)>,
    pub max_retries: u32,
    pub timeout: Duration,
    /// The variable or setting that chose `base_url`.
    pub url_from: Option<&'static str>,
    /// Set when `api_key` came from a helper command (its setting name).
    pub key_helper: Option<&'static str>,
}

impl OpenAiConfig {
    /// The endpoint and where its key came from, for error hints.
    pub fn origin(&self) -> crate::auth::Origin {
        use crate::auth::KeyFrom;
        let key_from = match (self.api_key.is_some(), self.key_helper) {
            (false, _) => KeyFrom::None,
            (true, Some(h)) => KeyFrom::Helper(h),
            (true, None) => KeyFrom::OpenAiKey,
        };
        crate::auth::Origin { backend: crate::auth::Backend::OpenAi, url_from: self.url_from, key_from }
    }
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        OpenAiConfig {
            base_url: "http://localhost:8000/v1".into(),
            api_key: None,
            extra_headers: vec![],
            max_retries: 2,
            timeout: Duration::from_secs(300),
            url_from: None,
            key_helper: None,
        }
    }
}

pub struct OpenAiProvider {
    config: OpenAiConfig,
    http: reqwest::Client,
    /// What this endpoint refused, per model, so later turns send what it
    /// takes first time: the keys it refused (`adapt_to_refusal`).
    refused: std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
}

/// Apply what an endpoint refused before to a fresh body.
fn apply_refused(body: &mut Value, refused: &[String]) {
    let Some(o) = body.as_object_mut() else { return };
    // Both length names refused (a gateway that turns one into the other):
    // send no length limit at all.
    if LENGTH_KEYS.iter().all(|k| refused.iter().any(|r| r == k)) {
        for k in LENGTH_KEYS {
            o.remove(k);
        }
        return;
    }
    for key in refused {
        match key.as_str() {
            "max_tokens" | "max_completion_tokens" => {
                let other = if key == "max_tokens" { "max_completion_tokens" } else { "max_tokens" };
                if let Some(v) = o.remove(key.as_str()) {
                    o.insert(other.into(), v);
                }
            }
            k => {
                o.remove(k);
            }
        }
    }
}

const LENGTH_KEYS: [&str; 2] = ["max_tokens", "max_completion_tokens"];

/// The keys `before` had that `after` lacks: what a refusal took out.
fn dropped_keys(before: &Value, after: &Value) -> Vec<String> {
    before
        .as_object()
        .map(|o| o.keys().filter(|k| after.get(k.as_str()).is_none()).cloned().collect())
        .unwrap_or_default()
}

impl OpenAiProvider {
    /// LiteLLM's per-model limits, at `<base>/model/info` or, for a base ending in `/v1`, at
    /// the gateway's root. Errors and other servers' pages are ignored: this only adds windows.
    async fn read_model_info(&self) {
        let base = self.config.base_url.trim_end_matches('/');
        let mut urls = vec![format!("{base}/model/info")];
        if let Some(root) = base.strip_suffix("/v1") {
            urls.push(format!("{root}/model/info"));
        }
        for url in urls {
            let mut rb = self.http.get(&url).timeout(Duration::from_secs(5));
            if let Some(k) = &self.config.api_key {
                rb = rb.bearer_auth(k);
            }
            for (k, v) in &self.config.extra_headers {
                rb = rb.header(k, v);
            }
            let Ok(res) = rb.send().await else { continue };
            if !res.status().is_success() {
                continue;
            }
            if let Ok(body) = res.json::<Value>().await {
                if record_model_info(&body) {
                    return;
                }
            }
        }
    }

    pub fn new(config: OpenAiConfig) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(config.timeout)
            .build()
            .map_err(|e| ApiError::network(e.to_string()))?;
        Ok(OpenAiProvider { config, http, refused: Default::default() })
    }
}

/// The tool names a request sends: Gemini models get an alias for each name
/// they wouldn't take (`gemini_rules`); everything else, the names as they are.
pub fn tool_names(req: &MessagesRequest) -> ToolNames {
    if gemini_rules::is_gemini(&req.model) {
        ToolNames::new(req.tools.iter().map(|t| t.name.as_str()))
    } else {
        ToolNames::default()
    }
}

/// What writing history back needs to know about the request.
struct Writing<'a> {
    /// A Gemini model (behind Google's endpoint or a gateway such as OmniRoute).
    gemini: bool,
    /// Gemini 3 or later: the first call of a turn needs a thought signature.
    gemini3: bool,
    names: &'a ToolNames,
    /// Each tool call's name, by id, for the `tool` messages that answer it.
    call_names: HashMap<String, String>,
}

/// Translate a Messages request into a chat-completions body.
pub fn to_chat_request(req: &MessagesRequest) -> Value {
    let gemini = gemini_rules::is_gemini(&req.model);
    let names = tool_names(req);
    let call_names = req
        .messages
        .iter()
        .flat_map(|m| m.tool_uses().map(|(id, name, _)| (id.to_string(), name.to_string())))
        .collect();
    let w = Writing { gemini, gemini3: gemini_rules::is_gemini3_or_later(&req.model), names: &names, call_names };
    let mut messages = Vec::new();
    let system: Vec<&str> = req.system.iter().map(|b| b.text.as_str()).collect();
    if !system.is_empty() {
        messages.push(json!({"role": "system", "content": system.join("\n\n")}));
    }
    for m in &req.messages {
        push_message(&mut messages, m, &w);
    }
    let tools: Vec<Value> = req
        .tools
        .iter()
        .filter(|t| t.kind.is_none())
        .map(|t| {
            let mut f = json!({"name": names.sent(&t.name), "description": t.description});
            // Gemini takes a narrower schema, and none for a tool without arguments.
            let params =
                if gemini { gemini_rules::sanitize_schema(&t.input_schema) } else { Some(t.input_schema.clone()) };
            if let Some(p) = params {
                f["parameters"] = p;
            }
            json!({"type": "function", "function": f})
        })
        .collect();
    let reasoning = is_reasoning_model(&req.model);
    let mut body = json!({
        "model": req.model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    // OpenAI's reasoning models take `max_completion_tokens` and only their
    // default temperature; everything else takes `max_tokens`.
    body[if reasoning { "max_completion_tokens" } else { "max_tokens" }] = json!(req.max_tokens);
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools);
    }
    if let Some(t) = req.temperature.filter(|_| !reasoning) {
        body["temperature"] = json!(t);
    }
    // Gemini's thinking depth, from the effort (Google's endpoint and OmniRoute
    // both read `reasoning_effort`).
    if gemini {
        let effort = req.output_config.as_ref().and_then(|c| c.get("effort")).and_then(Value::as_str);
        if let Some(level) = effort.and_then(|e| gemini_rules::thinking_level(e, &req.model)) {
            body["reasoning_effort"] = json!(level);
        }
    }
    body
}

/// OpenAI's reasoning models: the o-series, GPT-5 and Codex. A gateway
/// (OmniRoute, OpenRouter, LiteLLM) puts a provider before the id
/// (`cx/gpt-5-codex`, `openai/o3`), so only the last segment is read.
pub fn is_reasoning_model(model: &str) -> bool {
    let id = model.rsplit('/').next().unwrap_or(model).to_ascii_lowercase();
    id.contains("codex")
        || id.starts_with("gpt-5")
        || ["o1", "o3", "o4"].iter().any(|p| id == *p || id.starts_with(&format!("{p}-")))
}

/// The length parameter a (lowercased) refusal names: the one it names first,
/// as in "'max_tokens' is not supported ... use 'max_completion_tokens'" or
/// "Unrecognized request argument: max_completion_tokens". Named alone, a
/// parameter counts as refused only in so many words: "max_tokens is too
/// large" is about its value, not its name.
fn refused_length_key(m: &str) -> Option<&'static str> {
    let refusal =
        ["unsupported", "not supported", "unrecognized", "unknown", "not permitted", "extra inputs", "not allowed"]
            .iter()
            .any(|w| m.contains(w));
    let (old, new) = (m.find("max_tokens"), m.find("max_completion_tokens"));
    match (old, new) {
        (Some(a), Some(b)) if a < b => Some("max_tokens"),
        (Some(_), None) if refusal => Some("max_tokens"),
        (_, Some(_)) if refusal || old.is_some() => Some("max_completion_tokens"),
        _ => None,
    }
}

/// Change the body as a refusal asks, when it names a parameter this client
/// sets: `max_tokens` for `max_completion_tokens` (OpenAI's reasoning models,
/// under any name a gateway gives them), no `temperature`, no
/// `stream_options`. True when something changed, so the request is worth
/// sending again.
pub fn adapt_to_refusal(body: &mut Value, err: &ApiError) -> bool {
    let ApiError::Http { status: 400 | 422, message, .. } = err else { return false };
    let m = message.to_ascii_lowercase();
    let Some(o) = body.as_object_mut() else { return false };
    let mut changed = false;
    if let Some(from) = refused_length_key(&m) {
        let to = if from == "max_tokens" { "max_completion_tokens" } else { "max_tokens" };
        if let Some(v) = o.remove(from) {
            o.insert(to.into(), v);
            changed = true;
        }
    }
    if m.contains("temperature") && o.remove("temperature").is_some() {
        changed = true;
    }
    if m.contains("stream_options") && o.remove("stream_options").is_some() {
        changed = true;
    }
    if m.contains("reasoning_effort") && o.remove("reasoning_effort").is_some() {
        changed = true;
    }
    changed
}

/// Put back what a provider attached (`provider` on a block, as the stream
/// reader kept it): `tool_call` keys onto the call, `function` keys onto its
/// function, `message` keys onto the message, unchanged.
fn restore_provider(provider: &Value, call: &mut Value, msg: &mut Value) {
    let put = |from: Option<&Value>, into: &mut Value| {
        if let (Some(Value::Object(f)), Some(o)) = (from, into.as_object_mut()) {
            for (k, v) in f {
                o.insert(k.clone(), v.clone());
            }
        }
    };
    put(provider.get("tool_call"), call);
    put(provider.get("function"), &mut call["function"]);
    put(provider.get("message"), msg);
}

fn push_message(out: &mut Vec<Value>, m: &Message, w: &Writing) {
    match m.role {
        Role::Assistant => {
            let text: String = m.content.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("");
            let mut msg =
                json!({"role": "assistant", "content": if text.is_empty() { Value::Null } else { json!(text) }});
            // Gemini's thoughts go back as it sent them (OmniRoute turns them into thought parts).
            let thinking: String = m
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::Thinking { thinking, .. } => Some(thinking.as_str()),
                    _ => None,
                })
                .collect();
            if w.gemini && !thinking.is_empty() {
                msg["reasoning_content"] = json!(thinking);
            }
            let mut calls = vec![];
            for b in &m.content {
                if let ContentBlock::ToolUse { id, name, input, provider, .. } = b {
                    let mut call = json!({"id": id, "type": "function", "function": {
                        "name": w.names.sent(name), "arguments": input.to_string()
                    }});
                    if let Some(p) = provider {
                        restore_provider(p, &mut call, &mut msg);
                    }
                    // A call Gemini 3 never signed (another model's, an old
                    // session's): Google's placeholder, on the turn's first call.
                    let signed = provider.as_ref().and_then(crate::gemini::signature_of).is_some();
                    if w.gemini3 && calls.is_empty() && !signed {
                        call["extra_content"] =
                            json!({"google": {"thought_signature": crate::gemini::PLACEHOLDER_SIGNATURE}});
                    }
                    calls.push(call);
                }
            }
            if !calls.is_empty() {
                msg["tool_calls"] = Value::Array(calls);
            }
            out.push(msg);
        }
        Role::User => {
            let mut parts = Vec::new();
            // A tool message carries text only: pictures a tool returned
            // (Read on a screenshot) follow in the user message after it.
            let mut tool_images = Vec::new();
            for b in &m.content {
                match b {
                    ContentBlock::ToolResult { tool_use_id, content, is_error, .. } => {
                        let mut text = tool_result_text(content, &mut tool_images);
                        if is_error == &Some(true) {
                            text = format!("Error: {text}");
                        }
                        let mut tool = json!({"role": "tool", "tool_call_id": tool_use_id, "content": text});
                        // Some endpoints (Gemini's among them) match a result to its call by name too.
                        if let Some(name) = w.call_names.get(tool_use_id) {
                            tool["name"] = json!(w.names.sent(name));
                        }
                        out.push(tool);
                    }
                    ContentBlock::Text { text, .. } => parts.push(json!({"type": "text", "text": text})),
                    ContentBlock::Image { source, .. } => parts.extend(image_part(source)),
                    // No document block on this wire: the document's text instead.
                    ContentBlock::Document { source, title, .. } => {
                        parts.push(json!({"type": "text", "text": document_text(source, title.as_deref())}))
                    }
                    _ => {}
                }
            }
            if !tool_images.is_empty() {
                parts.push(json!({"type": "text", "text": "The tool results above returned these images:"}));
                parts.extend(tool_images);
            }
            if !parts.is_empty() {
                out.push(json!({"role": "user", "content": parts}));
            }
        }
    }
}

fn image_part(source: &MediaSource) -> Option<Value> {
    match source {
        MediaSource::Base64 { media_type, data } => {
            Some(json!({"type": "image_url", "image_url": {"url": format!("data:{media_type};base64,{data}")}}))
        }
        MediaSource::Url { url } => Some(json!({"type": "image_url", "image_url": {"url": url}})),
        MediaSource::Text { .. } => None,
    }
}

/// A tool result as the text a `tool` message carries: documents become
/// their text; images are set aside for the user message that follows.
fn tool_result_text(content: &ToolResultContent, images: &mut Vec<Value>) -> String {
    match content {
        ToolResultContent::Text(s) => s.clone(),
        ToolResultContent::Blocks(blocks) => blocks
            .iter()
            .map(|b| match b {
                ContentBlock::Text { text, .. } => text.clone(),
                ContentBlock::Image { source, .. } => match image_part(source) {
                    Some(part) => {
                        images.push(part);
                        format!("[image {}: shown in the next message]", images.len())
                    }
                    None => "[image]".to_string(),
                },
                ContentBlock::Document { source, title, .. } => document_text(source, title.as_deref()),
                other => serde_json::to_string(other).unwrap_or_default(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// A document block as text: a PDF's extracted text, a text document as
/// it is. Never its base64.
pub(crate) fn document_text(source: &MediaSource, title: Option<&str>) -> String {
    use base64::Engine as _;
    let name = title.unwrap_or("document").replace('"', "'");
    let body = match source {
        MediaSource::Text { data, .. } => Some(data.clone()),
        MediaSource::Base64 { media_type, data } => {
            let bytes = base64::engine::general_purpose::STANDARD.decode(data.trim()).unwrap_or_default();
            if media_type == "application/pdf" {
                crate::pdf_text::extract(&bytes)
            } else if media_type.starts_with("text/") {
                Some(String::from_utf8_lossy(&bytes).into_owned())
            } else {
                None
            }
        }
        MediaSource::Url { url } => return format!("[Document \"{name}\" at {url}: this endpoint can't fetch it.]"),
    };
    match body {
        Some(text) => format!("<document title=\"{name}\">\n{text}\n</document>"),
        None => format!(
            "[Document \"{name}\": its text couldn't be read here (it may be scanned, or its fonts encoded). This endpoint can't take the file itself; try another tool, e.g. `pdftotext` or Python, on the file.]"
        ),
    }
}

/// Translates chat-completion chunks into Messages API stream events.
#[derive(Debug, Default)]
pub struct ChunkTranslator {
    started: bool,
    finished: bool,
    next_index: usize,
    /// (kind, block index, openai tool index)
    open: Option<(OpenKind, usize, Option<u64>)>,
    stop: Option<StopReason>,
    usage: Usage,
    model: String,
    /// A tool call was streamed: the turn ends in tool use whatever
    /// `finish_reason` says (some providers send "stop" with tool calls).
    saw_tool: bool,
    /// The open tool call's id: a delta with another id is another call,
    /// even at the same `index` (some endpoints send every call at 0).
    open_call_id: Option<String>,
    /// What each tool call carried besides its id, name and arguments, by
    /// block: Gemini's `extra_content`, a gateway's `thought_signature`.
    provider: HashMap<usize, Value>,
    /// The turn's first tool call's block, for a message-level signature.
    first_call: Option<usize>,
    /// A message-level signature seen before any tool call.
    pending_message: Option<Value>,
    /// Tool names as sent (aliases for Gemini), to map back.
    names: ToolNames,
    /// A Gemini model: `<thought>…</thought>` in content is thinking.
    gemini: bool,
    /// Inside a `<thought>` in content, and a tag cut across chunks, held back.
    in_thought: bool,
    held: String,
}

/// The keys a tool call has that are not its own (id, type, function, index):
/// what a provider attached. The same for its function (beyond name and arguments).
fn extra_keys(v: &Value, own: &[&str]) -> Option<Value> {
    let o = v.as_object()?;
    let extra: serde_json::Map<String, Value> =
        o.iter().filter(|(k, _)| !own.contains(&k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
    (!extra.is_empty()).then_some(Value::Object(extra))
}

/// Message-level signatures some gateways send with a tool-call turn.
const MESSAGE_SIGNATURES: [&str; 3] = ["thinking_signature", "thought_signature", "thoughtSignature"];

/// Tool-call ids for providers that send none, unique in this process.
static NEXT_CALL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenKind {
    Text,
    Thinking,
    Tool,
}

impl ChunkTranslator {
    pub fn new(model: &str) -> Self {
        ChunkTranslator { model: model.into(), gemini: gemini_rules::is_gemini(model), ..Default::default() }
    }

    /// Map tool names the request sent as aliases back to the tools' names.
    pub fn with_tool_names(mut self, names: ToolNames) -> Self {
        self.names = names;
        self
    }

    /// Record provider data for a tool block and send it on.
    fn add_provider(&mut self, out: &mut Vec<StreamEvent>, index: usize, key: &str, data: Value) {
        let entry = self.provider.entry(index).or_insert_with(|| json!({}));
        let slot = entry.as_object_mut().expect("object").entry(key).or_insert_with(|| json!({}));
        if let (Some(a), Value::Object(b)) = (slot.as_object_mut(), data) {
            a.extend(b);
        }
        out.push(StreamEvent::ContentBlockDelta { index, delta: Delta::ProviderDelta { provider: entry.clone() } });
    }

    /// Text that may hold `<thought>…</thought>` (Gemini through an
    /// OpenAI-compatible endpoint): the thoughts as thinking, the rest as text.
    /// A tag cut across chunks is held back until it is whole.
    fn content(&mut self, out: &mut Vec<StreamEvent>, text: &str) {
        let mut rest = std::mem::take(&mut self.held) + text;
        while !rest.is_empty() {
            let tag = if self.in_thought { "</thought>" } else { "<thought>" };
            let (now, after) = match rest.find(tag) {
                Some(at) => (rest[..at].to_string(), Some(rest[at + tag.len()..].to_string())),
                None => {
                    // The end may be the start of a tag: hold it.
                    let keep = (1..tag.len()).rev().find(|n| rest.ends_with(&tag[..*n])).unwrap_or(0);
                    self.held = rest[rest.len() - keep..].to_string();
                    (rest[..rest.len() - keep].to_string(), None)
                }
            };
            if !now.is_empty() {
                if self.in_thought {
                    self.thinking(out, &now);
                } else {
                    self.text(out, &now);
                }
            }
            match after {
                Some(a) => {
                    self.in_thought = !self.in_thought;
                    rest = a;
                }
                None => break,
            }
        }
    }

    fn thinking(&mut self, out: &mut Vec<StreamEvent>, r: &str) {
        let index = match self.open {
            Some((OpenKind::Thinking, i, _)) => i,
            _ => self.open(
                out,
                OpenKind::Thinking,
                ContentBlock::Thinking { thinking: String::new(), signature: String::new() },
                None,
            ),
        };
        out.push(StreamEvent::ContentBlockDelta { index, delta: Delta::ThinkingDelta { thinking: r.into() } });
    }

    fn text(&mut self, out: &mut Vec<StreamEvent>, text: &str) {
        let index = match self.open {
            Some((OpenKind::Text, i, _)) => i,
            _ => self.open(out, OpenKind::Text, ContentBlock::text(""), None),
        };
        out.push(StreamEvent::ContentBlockDelta { index, delta: Delta::TextDelta { text: text.into() } });
    }

    fn ensure_started(&mut self, out: &mut Vec<StreamEvent>, id: Option<&str>) {
        if !self.started {
            self.started = true;
            out.push(StreamEvent::MessageStart {
                message: ApiMessage {
                    id: id.unwrap_or("chatcmpl").to_string(),
                    kind: "message".into(),
                    role: Role::Assistant,
                    model: self.model.clone(),
                    content: vec![],
                    stop_reason: None,
                    stop_sequence: None,
                    usage: Usage::default(),
                },
            });
        }
    }

    fn close(&mut self, out: &mut Vec<StreamEvent>) {
        if let Some((_, index, _)) = self.open.take() {
            out.push(StreamEvent::ContentBlockStop { index });
        }
    }

    fn open(&mut self, out: &mut Vec<StreamEvent>, kind: OpenKind, block: ContentBlock, tool: Option<u64>) -> usize {
        self.close(out);
        let index = self.next_index;
        self.next_index += 1;
        out.push(StreamEvent::ContentBlockStart { index, content_block: block });
        self.open = Some((kind, index, tool));
        index
    }

    /// Feed one `data:` payload.
    pub fn push(&mut self, data: &str) -> Result<Vec<StreamEvent>, ApiError> {
        let mut out = Vec::new();
        if data.trim() == "[DONE]" {
            self.finish_into(&mut out);
            return Ok(out);
        }
        let v: Value = serde_json::from_str(data).map_err(|e| ApiError::Parse(format!("{e}: {data}")))?;
        if let Some(err) = v.get("error") {
            return Err(ApiError::Stream {
                kind: "api_error".into(),
                message: err.get("message").and_then(Value::as_str).unwrap_or("unknown error").into(),
            });
        }
        self.ensure_started(&mut out, v.get("id").and_then(Value::as_str));
        if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
            self.usage.input_tokens = u.get("prompt_tokens").and_then(Value::as_u64).unwrap_or(0);
            self.usage.output_tokens = u.get("completion_tokens").and_then(Value::as_u64).unwrap_or(0);
            self.usage.reasoning_output_tokens =
                u.pointer("/completion_tokens_details/reasoning_tokens").and_then(Value::as_u64).unwrap_or(0);
            if let Some(c) = u.pointer("/prompt_tokens_details/cached_tokens").and_then(Value::as_u64) {
                self.usage.cache_read_input_tokens = c;
                self.usage.input_tokens = self.usage.input_tokens.saturating_sub(c);
            }
        }
        let Some(choice) = v.pointer("/choices/0") else { return Ok(out) };
        let delta = choice.get("delta").cloned().unwrap_or(Value::Null);
        let reasoning = delta.get("reasoning_content").or_else(|| delta.get("reasoning")).and_then(Value::as_str);
        if let Some(r) = reasoning.filter(|r| !r.is_empty()) {
            self.thinking(&mut out, r);
        }
        if let Some(text) = delta.get("content").and_then(Value::as_str).filter(|t| !t.is_empty()) {
            if self.gemini {
                self.content(&mut out, text);
            } else {
                self.text(&mut out, text);
            }
        }
        // A signature some gateways send on the message, not the call: it
        // belongs to the turn's first tool call.
        let message_sig: serde_json::Map<String, Value> = MESSAGE_SIGNATURES
            .iter()
            .filter_map(|k| {
                delta.get(*k).filter(|v| v.as_str().is_some_and(|s| !s.is_empty())).map(|v| (k.to_string(), v.clone()))
            })
            .collect();
        if !message_sig.is_empty() {
            match self.first_call {
                Some(i) => self.add_provider(&mut out, i, "message", Value::Object(message_sig)),
                None => self.pending_message = Some(Value::Object(message_sig)),
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                let tidx = call.get("index").and_then(Value::as_u64).unwrap_or(0);
                let call_id = call.get("id").and_then(Value::as_str).filter(|i| !i.is_empty());
                let same_call = call_id.is_none() || call_id == self.open_call_id.as_deref();
                let index = match self.open {
                    Some((OpenKind::Tool, i, Some(t))) if t == tidx && same_call => i,
                    _ => {
                        // Without an id, results couldn't be matched to calls: give one.
                        let id = call_id.map(str::to_string).unwrap_or_else(|| {
                            format!("call_forge_{}", NEXT_CALL.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
                        });
                        let sent = call.pointer("/function/name").and_then(Value::as_str).unwrap_or("");
                        let name = self.names.original(sent);
                        self.saw_tool = true;
                        let i = self.open(
                            &mut out,
                            OpenKind::Tool,
                            ContentBlock::ToolUse {
                                id: id.clone(),
                                name,
                                input: json!({}),
                                cache_control: None,
                                provider: None,
                            },
                            Some(tidx),
                        );
                        self.open_call_id = Some(id);
                        if self.first_call.is_none() {
                            self.first_call = Some(i);
                            if let Some(m) = self.pending_message.take() {
                                self.add_provider(&mut out, i, "message", m);
                            }
                        }
                        i
                    }
                };
                // What the call carries besides its own fields goes back with it, unchanged.
                if let Some(extra) = extra_keys(call, &["id", "type", "function", "index"]) {
                    self.add_provider(&mut out, index, "tool_call", extra);
                }
                if let Some(extra) = call.get("function").and_then(|f| extra_keys(f, &["name", "arguments"])) {
                    self.add_provider(&mut out, index, "function", extra);
                }
                // Arguments come as JSON text, in pieces; some providers send
                // the object itself, whole.
                let args = match call.pointer("/function/arguments") {
                    Some(Value::String(a)) => a.clone(),
                    Some(o @ Value::Object(_)) => o.to_string(),
                    _ => String::new(),
                };
                if !args.is_empty() {
                    out.push(StreamEvent::ContentBlockDelta {
                        index,
                        delta: Delta::InputJsonDelta { partial_json: args },
                    });
                }
            }
        }
        if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
            self.stop = Some(match reason {
                "tool_calls" | "function_call" => StopReason::ToolUse,
                "length" => StopReason::MaxTokens,
                "content_filter" => StopReason::Refusal,
                _ => StopReason::EndTurn,
            });
        }
        Ok(out)
    }

    /// A whole completion (`"object": "chat.completion"`), from a server that
    /// answered a streaming request without streaming: its message as one
    /// chunk, then the end.
    pub fn push_completion(&mut self, body: &Value) -> Result<Vec<StreamEvent>, ApiError> {
        let mut message = body.pointer("/choices/0/message").cloned().unwrap_or(Value::Null);
        if let Some(calls) = message.get_mut("tool_calls").and_then(Value::as_array_mut) {
            for (i, c) in calls.iter_mut().enumerate() {
                if c.get("index").is_none() {
                    c["index"] = json!(i);
                }
            }
        }
        let mut chunk = json!({
            "id": body.get("id"),
            "usage": body.get("usage"),
            "choices": [{"delta": message, "finish_reason": body.pointer("/choices/0/finish_reason")}],
        });
        if let Some(e) = body.get("error") {
            chunk["error"] = e.clone();
        }
        let mut out = self.push(&chunk.to_string())?;
        self.finish_into(&mut out);
        Ok(out)
    }

    /// Close everything; idempotent.
    pub fn finish_into(&mut self, out: &mut Vec<StreamEvent>) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.ensure_started(out, None);
        // Text held back as a possible tag start was text after all.
        let held = std::mem::take(&mut self.held);
        if !held.is_empty() {
            if self.in_thought {
                self.thinking(out, &held);
            } else {
                self.text(out, &held);
            }
        }
        self.close(out);
        let stop = match self.stop {
            None | Some(StopReason::EndTurn) if self.saw_tool => StopReason::ToolUse,
            s => s.unwrap_or(StopReason::EndTurn),
        };
        out.push(StreamEvent::MessageDelta {
            delta: MessageDeltaBody { stop_reason: Some(stop), stop_sequence: None },
            usage: self.usage.clone(),
        });
        out.push(StreamEvent::MessageStop);
    }
}

/// Model ids from an OpenAI-style `GET /models` answer (`{"data": [{"id": ...}]}`), sorted.
pub fn model_ids(body: &Value) -> Vec<String> {
    let mut ids: Vec<String> =
        body["data"].as_array().into_iter().flatten().filter_map(|m| m["id"].as_str().map(str::to_string)).collect();
    ids.sort();
    ids.dedup();
    ids
}

/// The limits a model-list entry reports, under the names gateways use
/// (OpenRouter `context_length`, vLLM `max_model_len`, LiteLLM `max_input_tokens`, ...).
pub fn listed_limits(entry: &Value) -> crate::models::Limits {
    // A whole number, also when a gateway sends it as text ("32768") or a decimal (32768.0).
    let count = |v: &Value| -> Option<u64> {
        v.as_u64()
            .or_else(|| v.as_f64().filter(|f| f.fract() == 0.0 && *f > 0.0 && *f < 1e12).map(|f| f as u64))
            .or_else(|| v.as_str().and_then(|s| s.trim().replace('_', "").parse().ok()))
    };
    let first = |paths: &[&str]| paths.iter().find_map(|p| entry.pointer(p).and_then(count)).filter(|n| *n > 0);
    crate::models::Limits {
        context_window: first(&[
            "/context_length",
            "/context_window",
            "/max_model_len",
            "/max_context_length",
            "/max_input_tokens",
            "/top_provider/context_length",
            "/model_info/max_input_tokens",
        ]),
        max_output: first(&[
            "/max_output_tokens",
            "/max_completion_tokens",
            "/top_provider/max_completion_tokens",
            "/model_info/max_output_tokens",
        ])
        .map(|n| n.min(u64::from(u32::MAX)) as u32),
    }
}

/// Record the limits LiteLLM's `/model/info` gives each model (`model_name`, with
/// `model_info.max_input_tokens` and `max_output_tokens`), for models without a window yet.
/// True when the page held at least one model.
pub fn record_model_info(body: &Value) -> bool {
    let mut any = false;
    for m in body["data"].as_array().into_iter().flatten() {
        let Some(id) = m["model_name"].as_str() else { continue };
        any = true;
        let l = listed_limits(m);
        if l.context_window.is_some() && crate::models::window_unknown(id) {
            crate::models::listed_limits(id, l);
        }
    }
    any
}

/// Record the limits each listed model reports, so compaction uses the real window.
pub fn record_listed_limits(body: &Value) {
    for m in body["data"].as_array().into_iter().flatten() {
        if let Some(id) = m["id"].as_str() {
            let l = listed_limits(m);
            if l != crate::models::Limits::default() {
                crate::models::listed_limits(id, l);
            }
        }
    }
}

#[async_trait::async_trait]
impl Provider for OpenAiProvider {
    fn name(&self) -> &str {
        "openai"
    }

    /// `push_message` sends reasoning back only to Gemini models (as `reasoning_content`).
    fn resends_reasoning(&self, model: &str) -> bool {
        gemini_rules::is_gemini(model)
    }

    fn origin(&self) -> Option<crate::auth::Origin> {
        Some(self.config.origin())
    }

    fn base_url(&self) -> Option<String> {
        Some(crate::auth::display_url(&self.config.base_url))
    }

    async fn list_models(&self) -> Option<Result<Vec<String>, ApiError>> {
        let url = format!("{}/models", self.config.base_url.trim_end_matches('/'));
        let mut rb = self.http.get(&url).timeout(Duration::from_secs(15));
        if let Some(k) = &self.config.api_key {
            rb = rb.bearer_auth(k);
        }
        for (k, v) in &self.config.extra_headers {
            rb = rb.header(k, v);
        }
        let r = async {
            let res = rb.send().await.map_err(crate::messages::network_error)?;
            if !res.status().is_success() {
                return Err(http_error(res).await);
            }
            let body: Value = res.json().await.map_err(|e| ApiError::Parse(e.to_string()))?;
            record_listed_limits(&body);
            let ids = model_ids(&body);
            // A LiteLLM gateway lists no window in /models but keeps one per model in
            // /model/info (`model_info.max_input_tokens`): ask there when a model lacks one.
            if ids.iter().any(|id| crate::models::window_unknown(id)) {
                self.read_model_info().await;
            }
            Ok(ids)
        }
        .await;
        Some(r.map_err(|e| e.with_origin(self.config.origin())))
    }

    async fn stream(&self, request: MessagesRequest, cancel: CancellationToken) -> Result<EventStream, ApiError> {
        let mut body = to_chat_request(&request);
        if let Some(r) = self.refused.lock().ok().and_then(|m| m.get(&request.model).cloned()) {
            apply_refused(&mut body, &r);
        }
        let url = format!("{}/chat/completions", self.config.base_url.trim_end_matches('/'));
        let mut attempt = 0;
        // Another try with the body each refusal asks for (see `adapt_to_refusal`),
        // a few times at most: each one takes a key out or swaps a length name.
        let mut adaptations = 0;
        let mut refused_now: Vec<String> = Vec::new();
        let resp = loop {
            let mut rb = self.http.post(&url).json(&body);
            if let Some(k) = &self.config.api_key {
                rb = rb.bearer_auth(k);
            }
            for (k, v) in &self.config.extra_headers {
                rb = rb.header(k, v);
            }
            let res = tokio::select! {
                _ = cancel.cancelled() => return Err(ApiError::Cancelled),
                r = rb.send() => r.map_err(crate::messages::network_error),
            };
            let res = match res {
                Ok(r) if r.status().is_success() => Ok(r),
                Ok(r) => Err(http_error(r).await),
                Err(e) => Err(e),
            };
            match res {
                Ok(r) => break r,
                Err(e)
                    if adaptations < 3 && {
                        let before = body.clone();
                        // A length name refused again after it was swapped away: a
                        // gateway turns one name into the other, so refuse both.
                        let again = match &e {
                            ApiError::Http { status: 400 | 422, message, .. } => {
                                refused_length_key(&message.to_ascii_lowercase())
                                    .filter(|k| refused_now.iter().any(|r| r == k) && body.get(k).is_none())
                            }
                            _ => None,
                        };
                        if again.is_some() {
                            for k in LENGTH_KEYS {
                                if !refused_now.iter().any(|r| r == k) {
                                    refused_now.push(k.to_string());
                                }
                            }
                        }
                        (adapt_to_refusal(&mut body, &e) || again.is_some()) && {
                            for k in dropped_keys(&before, &body) {
                                if !refused_now.contains(&k) {
                                    refused_now.push(k);
                                }
                            }
                            apply_refused(&mut body, &refused_now);
                            // Remembered for this model's later turns.
                            if let Ok(mut m) = self.refused.lock() {
                                let keys = m.entry(request.model.clone()).or_default();
                                for k in &refused_now {
                                    if !keys.contains(k) {
                                        keys.push(k.clone());
                                    }
                                }
                            }
                            true
                        }
                    } =>
                {
                    tracing::warn!(error = %e, "the endpoint refused a parameter; sending again without it");
                    adaptations += 1;
                }
                Err(e) if e.retries_quickly() && attempt < self.config.max_retries => {
                    let retry_after = match &e {
                        ApiError::Http { retry_after, .. } => *retry_after,
                        _ => None,
                    };
                    tracing::warn!(attempt, error = %e, "retrying API request");
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(ApiError::Cancelled),
                        _ = tokio::time::sleep(backoff_delay(attempt, retry_after)) => {}
                    }
                    attempt += 1;
                }
                Err(e) => return Err(e.with_origin(self.config.origin())),
            }
        };
        let mut tr = ChunkTranslator::new(&request.model).with_tool_names(tool_names(&request));
        // Asked to stream, some servers answer with the whole completion.
        let whole = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|ct| ct.contains("application/json"));
        if whole {
            let body: Value = resp.json().await.map_err(|e| ApiError::Parse(format!("a whole completion: {e}")))?;
            let events = tr.push_completion(&body)?;
            return Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))));
        }
        Ok(spawn_sse_pump(resp, cancel, move |ev| match tr.push(&ev.data) {
            Ok(events) => events.into_iter().map(Ok).collect(),
            Err(e) => vec![Err(e)],
        }))
    }
}

#[cfg(test)]
mod tests {
    /// LiteLLM's /model/info: a window per model name, read only for models without one.
    #[test]
    fn litellm_model_info_gives_each_model_its_window() {
        let body = json!({"data": [
            {"model_name": "info-small", "litellm_params": {"model": "hosted_vllm/Qwen"}, "model_info": {"max_input_tokens": 100000, "max_output_tokens": 32000}},
            {"model_name": "info-large", "model_info": {"max_input_tokens": 1000000}},
            {"model_name": "info-none", "model_info": {"max_input_tokens": null}}
        ]});
        assert!(record_model_info(&body));
        let w = |id: &str| crate::models::model_info_or_default(id).context_window;
        assert_eq!((w("info-small"), crate::models::model_info_or_default("info-small").max_output), (100_000, 32_000));
        assert_eq!(w("info-large"), 1_000_000);
        assert!(crate::models::window_unknown("info-none"));
        assert!(!record_model_info(&json!({"data": [{"id": "not-model-info"}]})), "another server's page");
    }

    use super::*;
    use crate::MessageAccumulator;

    fn b64(b: &[u8]) -> String {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(b)
    }

    const PDF: &[u8] = b"%PDF-1.4\n4 0 obj << /Length 44 >>\nstream\nBT 72 720 Td (Total due: 1,234.50 EUR) Tj ET\nendstream\nendobj\n%%EOF";

    /// How history is written for a model that isn't Gemini.
    fn plain() -> Writing<'static> {
        static NAMES: std::sync::OnceLock<ToolNames> = std::sync::OnceLock::new();
        Writing {
            gemini: false,
            gemini3: false,
            names: NAMES.get_or_init(ToolNames::default),
            call_names: HashMap::new(),
        }
    }

    fn pdf_block() -> ContentBlock {
        ContentBlock::Document {
            source: MediaSource::Base64 { media_type: "application/pdf".into(), data: b64(PDF) },
            title: Some("invoice.pdf".into()),
            cache_control: None,
        }
    }

    #[test]
    fn a_pdf_reaches_an_openai_endpoint_as_its_text() {
        let mut out = vec![];
        push_message(&mut out, &Message::user(vec![ContentBlock::text("What is the total?"), pdf_block()]), &plain());
        let parts = out[0]["content"].as_array().unwrap();
        assert_eq!(parts.len(), 2, "the document is not dropped");
        assert_eq!(parts[1]["text"], "<document title=\"invoice.pdf\">\nTotal due: 1,234.50 EUR\n</document>");
    }

    #[test]
    fn a_tool_result_never_carries_base64_and_its_images_follow() {
        let png = ContentBlock::Image {
            source: MediaSource::Base64 { media_type: "image/png".into(), data: "iVBORw0KGgo=".into() },
            cache_control: None,
        };
        let m = Message::user(vec![
            ContentBlock::ToolResult {
                tool_use_id: "t1".into(),
                content: ToolResultContent::Blocks(vec![pdf_block()]),
                is_error: None,
                cache_control: None,
            },
            ContentBlock::ToolResult {
                tool_use_id: "t2".into(),
                content: ToolResultContent::Blocks(vec![png]),
                is_error: None,
                cache_control: None,
            },
        ]);
        let mut out = vec![];
        push_message(&mut out, &m, &plain());
        assert_eq!(out[0]["role"], "tool");
        let t1 = out[0]["content"].as_str().unwrap();
        assert!(t1.contains("Total due: 1,234.50 EUR") && !t1.contains(&b64(PDF)), "{t1}");
        assert_eq!(out[1]["content"], "[image 1: shown in the next message]");
        // Tool messages first, then one user message with the picture.
        assert_eq!(out[2]["role"], "user");
        assert_eq!(out[2]["content"][1]["image_url"]["url"], "data:image/png;base64,iVBORw0KGgo=");
    }

    #[test]
    fn an_unreadable_pdf_is_named_not_dropped() {
        let doc = ContentBlock::Document {
            source: MediaSource::Base64 { media_type: "application/pdf".into(), data: b64(b"%PDF-1.4 no text") },
            title: Some("scan.pdf".into()),
            cache_control: None,
        };
        let mut out = vec![];
        push_message(&mut out, &Message::user(vec![doc]), &plain());
        let t = out[0]["content"][0]["text"].as_str().unwrap();
        assert!(t.starts_with("[Document \"scan.pdf\": its text couldn't be read here"), "{t}");
    }

    #[test]
    fn reads_context_windows_from_a_models_listing() {
        let body = json!({"data": [
            {"id": "router-model", "context_length": 163_840, "top_provider": {"max_completion_tokens": 8192}},
            {"id": "vllm-model", "max_model_len": 32_768},
            {"id": "plain"}
        ]});
        let l = listed_limits(&body["data"][0]);
        assert_eq!((l.context_window, l.max_output), (Some(163_840), Some(8192)));
        assert_eq!(listed_limits(&body["data"][1]).context_window, Some(32_768));
        assert_eq!(listed_limits(&body["data"][2]), crate::models::Limits::default());
        // Text and decimals are read as numbers; nothing else is.
        for (v, want) in
            [(json!("32768"), Some(32_768)), (json!(32_768.0), Some(32_768)), (json!(" 131_072 "), Some(131_072))]
        {
            assert_eq!(listed_limits(&json!({"id": "x", "max_model_len": v})).context_window, want);
        }
        for v in [json!(0), json!(-1), json!(32_768.5), json!("lots"), json!(null), json!([1])] {
            assert_eq!(listed_limits(&json!({"id": "x", "max_model_len": v})).context_window, None, "{v}");
        }
        record_listed_limits(&body);
        assert_eq!(crate::models::model_info_or_default("vllm-model").context_window, 32_768);
    }

    #[test]
    fn reads_model_ids_from_a_models_listing() {
        let body =
            json!({"object": "list", "data": [{"id": "qwen3-coder"}, {"id": "llama-4"}, {"id": "llama-4"}, {"x": 1}]});
        assert_eq!(model_ids(&body), ["llama-4", "qwen3-coder"]);
        assert!(model_ids(&json!({})).is_empty());
    }
    use kodact_types::{SystemBlock, ToolSpec};

    fn http_400(message: &str) -> ApiError {
        ApiError::Http {
            status: 400,
            kind: "invalid_request_error".into(),
            message: message.into(),
            retry_after: None,
            url: String::new(),
            origin: None,
        }
    }

    #[test]
    fn knows_openai_reasoning_models_behind_any_gateway() {
        for id in [
            "gpt-5",
            "gpt-5-codex",
            "gpt-5.1-codex-mini",
            "o3",
            "o4-mini",
            "o1",
            "cx/gpt-5-codex",
            "openai/o3-pro",
            "codex-mini-latest",
            "GPT-5",
        ] {
            assert!(is_reasoning_model(id), "{id}");
        }
        for id in [
            "gpt-4o",
            "gpt-4.1",
            "qwen3-coder:30b",
            "auto/best-coding",
            "oc/big-pickle",
            "o",
            "omni-moderation",
            "llama-3.3-70b",
        ] {
            assert!(!is_reasoning_model(id), "{id}");
        }
    }

    #[test]
    fn reasoning_models_get_max_completion_tokens_and_no_temperature() {
        let req = |model: &str| MessagesRequest {
            model: model.into(),
            max_tokens: 1000,
            messages: vec![Message::user_text("hi")],
            system: vec![],
            tools: vec![],
            tool_choice: None,
            thinking: None,
            temperature: Some(0.2),
            metadata: None,
            output_config: None,
            speed: None,
            stream: true,
            betas: vec![],
        };
        let b = to_chat_request(&req("cx/gpt-5-codex"));
        assert_eq!(b["max_completion_tokens"], 1000);
        assert!(b.get("max_tokens").is_none() && b.get("temperature").is_none(), "{b}");
        let b = to_chat_request(&req("gpt-4o"));
        assert_eq!(b["max_tokens"], 1000);
        assert!((b["temperature"].as_f64().unwrap() - 0.2).abs() < 1e-6, "{b}");
        assert!(b.get("max_completion_tokens").is_none());
    }

    #[test]
    fn a_refused_parameter_is_changed_once_as_asked() {
        // OpenAI's words, for a reasoning model under a name nobody recognises.
        let mut b = json!({"model": "dva/anything", "max_tokens": 64, "temperature": 0.5, "stream_options": {"include_usage": true}});
        let e = http_400("Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.");
        assert!(adapt_to_refusal(&mut b, &e));
        assert_eq!(b["max_completion_tokens"], 64);
        assert!(b.get("max_tokens").is_none());
        // Already as asked: nothing more to change, so no endless retrying.
        assert!(!adapt_to_refusal(&mut b, &e));
        let e = http_400("Unsupported value: 'temperature' does not support 0.5 with this model. Only the default (1) value is supported.");
        assert!(adapt_to_refusal(&mut b, &e) && b.get("temperature").is_none());
        // A server that only knows max_tokens, refusing the newer name.
        let mut b = json!({"max_completion_tokens": 64});
        assert!(adapt_to_refusal(&mut b, &http_400("Unrecognized request argument: max_completion_tokens")));
        assert_eq!(b["max_tokens"], 64);
        let mut b = json!({"max_tokens": 64, "stream_options": {}});
        assert!(adapt_to_refusal(&mut b, &http_400("stream_options: Extra inputs are not permitted")));
        assert!(b.get("stream_options").is_none() && b["max_tokens"] == 64);
        // A value refused, not the name: the request is left as it is.
        let mut b = json!({"max_tokens": 64000});
        assert!(!adapt_to_refusal(
            &mut b,
            &http_400("max_tokens is too large: 64000. This model supports at most 16384 completion tokens.")
        ));
        assert_eq!(b["max_tokens"], 64000);
        // Anything else is not a parameter problem.
        let mut b = json!({"max_tokens": 64});
        assert!(!adapt_to_refusal(&mut b, &http_400("The model `x` does not exist")));
        let server_error = ApiError::Http {
            status: 500,
            kind: "server_error".into(),
            message: "max_completion_tokens".into(),
            retry_after: None,
            url: String::new(),
            origin: None,
        };
        assert!(!adapt_to_refusal(&mut b, &server_error));
    }

    #[test]
    fn what_was_refused_is_applied_to_the_next_turn() {
        let before = json!({"max_tokens": 64, "temperature": 0.5, "stream": true});
        let mut after = before.clone();
        assert!(adapt_to_refusal(&mut after, &http_400("Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.")));
        let refused = dropped_keys(&before, &after);
        assert_eq!(refused, vec!["max_tokens".to_string()]);
        let mut next = json!({"max_tokens": 128, "temperature": 0.5, "stream": true});
        apply_refused(&mut next, &refused);
        assert_eq!(next, json!({"max_completion_tokens": 128, "temperature": 0.5, "stream": true}));
        let mut next = json!({"max_tokens": 128, "temperature": 0.5});
        apply_refused(&mut next, &["temperature".to_string()]);
        assert_eq!(next, json!({"max_tokens": 128}));
    }

    /// Feed chunks, return the message they make.
    fn assemble(chunks: &[&str]) -> kodact_types::ApiMessage {
        let mut tr = ChunkTranslator::new("m");
        let mut acc = MessageAccumulator::default();
        for c in chunks {
            for ev in tr.push(c).unwrap() {
                acc.push(&ev).unwrap();
            }
        }
        let mut tail = vec![];
        tr.finish_into(&mut tail);
        for ev in tail {
            acc.push(&ev).unwrap();
        }
        acc.finish().unwrap()
    }

    #[test]
    fn tool_calls_end_in_tool_use_even_when_the_provider_says_stop() {
        let m = assemble(&[
            r#"{"id":"c","choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"Read","arguments":"{\"file_path\":\"x\"}"}}]}}]}"#,
            r#"{"id":"c","choices":[{"delta":{},"finish_reason":"stop"}]}"#,
        ]);
        assert_eq!(m.stop_reason, Some(StopReason::ToolUse));
        // No finish_reason at all, as some streams end: still tool use.
        let m = assemble(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"LS","arguments":"{}"}}]}}]}"#,
        ]);
        assert_eq!(m.stop_reason, Some(StopReason::ToolUse));
        // A cut-off turn stays cut off.
        let m = assemble(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"a","function":{"name":"LS","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#,
        ]);
        assert_eq!(m.stop_reason, Some(StopReason::MaxTokens));
        // Text alone ends the turn.
        let m = assemble(&[r#"{"choices":[{"delta":{"content":"hi"},"finish_reason":"stop"}]}"#]);
        assert_eq!(m.stop_reason, Some(StopReason::EndTurn));
    }

    #[test]
    fn tool_calls_without_ids_get_distinct_ones_and_object_arguments_are_read() {
        let m = assemble(&[
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"Read","arguments":{"file_path":"a"}}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"","function":{"name":"Read","arguments":{"file_path":"b"}}}]}}]}"#,
            r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
        ]);
        let calls: Vec<(String, Value)> = m
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, input, .. } => Some((id.clone(), input.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(calls.len(), 2);
        assert_ne!(calls[0].0, calls[1].0, "{calls:?}");
        assert!(calls.iter().all(|(id, _)| !id.is_empty()));
        assert_eq!(calls[0].1, json!({"file_path": "a"}));
        assert_eq!(calls[1].1, json!({"file_path": "b"}));
    }

    #[test]
    fn a_whole_completion_reads_as_a_stream_would() {
        let body = json!({
            "id": "cmpl", "object": "chat.completion",
            "choices": [{"index": 0, "finish_reason": "tool_calls", "message": {
                "role": "assistant", "content": "Let me look.", "reasoning_content": "Check the file.",
                "tool_calls": [
                    {"id": "t1", "type": "function", "function": {"name": "Read", "arguments": "{\"file_path\":\"a\"}"}},
                    {"id": "t2", "type": "function", "function": {"name": "Read", "arguments": "{\"file_path\":\"b\"}"}}
                ]}}],
            "usage": {"prompt_tokens": 9, "completion_tokens": 5}
        });
        let mut tr = ChunkTranslator::new("m");
        let mut acc = MessageAccumulator::default();
        for ev in tr.push_completion(&body).unwrap() {
            acc.push(&ev).unwrap();
        }
        let m = acc.finish().unwrap();
        assert_eq!(m.stop_reason, Some(StopReason::ToolUse));
        assert_eq!(m.usage.input_tokens, 9);
        let tools = m.content.iter().filter(|b| matches!(b, ContentBlock::ToolUse { .. })).count();
        assert_eq!(tools, 2, "{:?}", m.content);
        assert!(m.content.iter().any(|b| matches!(b, ContentBlock::Text { text, .. } if text == "Let me look.")));
        assert!(m.content.iter().any(|b| matches!(b, ContentBlock::Thinking { .. })));
        // An error answered whole is an error.
        let mut tr = ChunkTranslator::new("m");
        assert!(tr.push_completion(&json!({"error": {"message": "rate limited"}})).is_err());
    }

    #[test]
    fn translates_tool_call_chunks() {
        let mut tr = ChunkTranslator::new("local");
        let mut events = vec![];
        for chunk in [
            r#"{"id":"c1","choices":[{"delta":{"content":"Let me look."}}]}"#,
            r#"{"id":"c1","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_a","function":{"name":"Read","arguments":"{\"file_"}}]}}]}"#,
            r#"{"id":"c1","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"path\":\"/x\"}"}}]}}]}"#,
            r#"{"id":"c1","choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            r#"{"id":"c1","choices":[],"usage":{"prompt_tokens":11,"completion_tokens":4}}"#,
            "[DONE]",
        ] {
            events.extend(tr.push(chunk).unwrap());
        }
        let mut acc = MessageAccumulator::new();
        for e in &events {
            acc.push(e).unwrap();
        }
        let m = acc.finish().unwrap();
        assert_eq!(m.to_message().text(), "Let me look.");
        assert_eq!(m.stop_reason, Some(StopReason::ToolUse));
        let (id, name, input) =
            m.to_message().tool_uses().map(|(a, b, c)| (a.to_string(), b.to_string(), c.clone())).next().unwrap();
        assert_eq!((id.as_str(), name.as_str()), ("call_a", "Read"));
        assert_eq!(input["file_path"], "/x");
        assert_eq!(m.usage.input_tokens, 11);
    }

    #[test]
    fn hidden_reasoning_is_counted_apart() {
        // A gateway that reasons without sending the reasoning still counts it in completion_tokens.
        let mut tr = ChunkTranslator::new("local");
        let mut events = vec![];
        for chunk in [
            r#"{"id":"c1","choices":[{"delta":{"content":"Hi."}}]}"#,
            r#"{"id":"c1","choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            r#"{"id":"c1","choices":[],"usage":{"prompt_tokens":9000,"completion_tokens":1200,
                "completion_tokens_details":{"reasoning_tokens":1190}}}"#,
            "[DONE]",
        ] {
            events.extend(tr.push(chunk).unwrap());
        }
        let mut acc = MessageAccumulator::new();
        for e in &events {
            acc.push(e).unwrap();
        }
        let m = acc.finish().unwrap();
        assert_eq!(m.usage.output_tokens, 1200, "billed output keeps the reasoning");
        assert_eq!(m.usage.reasoning_output_tokens, 1190);
    }

    #[test]
    fn request_translation_splits_tool_results() {
        let req = MessagesRequest {
            model: "m".into(),
            max_tokens: 100,
            messages: vec![
                Message::user_text("hi"),
                Message::assistant(vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "Bash".into(),
                    input: json!({"command":"ls"}),
                    cache_control: None,
                    provider: None,
                }]),
                Message::user(vec![ContentBlock::tool_result("t1", "a\nb", false)]),
            ],
            system: vec![SystemBlock::text("sys")],
            tools: vec![ToolSpec {
                name: "Bash".into(),
                description: "run".into(),
                input_schema: json!({"type":"object"}),
                kind: None,
                extra: Default::default(),
                cache_control: None,
            }],
            tool_choice: None,
            thinking: None,
            temperature: None,
            metadata: None,
            output_config: None,
            speed: None,
            stream: true,
            betas: vec![],
        };
        let body = to_chat_request(&req);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[2]["tool_calls"][0]["function"]["name"], "Bash");
        assert_eq!(msgs[3]["role"], "tool");
        assert_eq!(body["tools"][0]["function"]["name"], "Bash");
    }

    /// A gateway in front of a reasoning model: it turns `max_completion_tokens`
    /// into `max_tokens`, which the model refuses. Answers with a whole JSON
    /// completion. Returns its address and the bodies it was sent.
    async fn rewriting_gateway() -> (String, std::sync::Arc<std::sync::Mutex<Vec<Value>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let body = loop {
                    let mut chunk = [0u8; 4096];
                    let n = sock.read(&mut chunk).await.unwrap();
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len: usize = text[..end]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse().unwrap())
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len {
                            break serde_json::from_slice::<Value>(&buf[end + 4..end + 4 + len]).unwrap();
                        }
                    }
                };
                log.lock().unwrap().push(body.clone());
                let mut up = body;
                if let Some(v) = up.as_object_mut().unwrap().remove("max_completion_tokens") {
                    up["max_tokens"] = v;
                }
                let (status, reply) = if up.get("max_tokens").is_some() {
                    (
                        "400 Bad Request",
                        json!({"error": {"message": "Unsupported parameter: 'max_tokens' is not supported with this model. Use 'max_completion_tokens' instead.", "type": "invalid_request_error"}}),
                    )
                } else {
                    (
                        "200 OK",
                        json!({"id": "x", "choices": [{"message": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]}),
                    )
                };
                let out = reply.to_string();
                let head = format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", out.len());
                sock.write_all(head.as_bytes()).await.unwrap();
                sock.write_all(out.as_bytes()).await.unwrap();
            }
        });
        (format!("http://{addr}/v1"), seen)
    }

    #[tokio::test]
    async fn a_gateway_that_refuses_both_length_names_gets_neither() {
        use futures::StreamExt;
        let (base_url, seen) = rewriting_gateway().await;
        let provider = OpenAiProvider::new(OpenAiConfig {
            base_url,
            api_key: None,
            extra_headers: vec![],
            max_retries: 0,
            timeout: Duration::from_secs(10),
            url_from: None,
            key_helper: None,
        })
        .unwrap();
        let req = || MessagesRequest {
            model: "vllm/house-reasoner".into(),
            max_tokens: 1000,
            messages: vec![Message::user_text("hi")],
            system: vec![],
            tools: vec![],
            tool_choice: None,
            thinking: None,
            temperature: None,
            metadata: None,
            output_config: None,
            speed: None,
            stream: true,
            betas: vec![],
        };
        for _ in 0..2 {
            let mut events = provider.stream(req(), CancellationToken::new()).await.unwrap();
            let mut acc = MessageAccumulator::default();
            while let Some(ev) = events.next().await {
                acc.push(&ev.unwrap()).unwrap();
            }
            assert_eq!(acc.finish().unwrap().content.iter().filter_map(|b| b.as_text()).collect::<String>(), "ok");
        }
        let lengths: Vec<_> = seen
            .lock()
            .unwrap()
            .iter()
            .map(|b| LENGTH_KEYS.iter().filter(|k| b.get(**k).is_some()).copied().collect::<Vec<_>>())
            .collect();
        // Turn one: max_tokens refused, the swap refused too, then no limit.
        // Turn two goes straight to no limit.
        assert_eq!(lengths, vec![vec!["max_tokens"], vec!["max_completion_tokens"], vec![], vec![]]);
    }

    #[test]
    fn both_length_names_refused_means_no_limit_is_sent() {
        let mut b = json!({"max_tokens": 64, "stream": true});
        apply_refused(&mut b, &["max_tokens".into(), "max_completion_tokens".into()]);
        assert_eq!(b, json!({"stream": true}));
    }

    /// The engine sizes the next request by `resends_reasoning`, so it must say what the
    /// request writer does with an earlier reply's reasoning.
    #[test]
    fn resends_reasoning_says_what_the_request_carries() {
        let provider = OpenAiProvider::new(OpenAiConfig {
            base_url: "http://127.0.0.1:9/v1".into(),
            api_key: None,
            extra_headers: vec![],
            max_retries: 0,
            timeout: Duration::from_secs(1),
            url_from: None,
            key_helper: None,
        })
        .unwrap();
        for model in ["quick-thinking", "deepseek-reasoner", "gemini-2.5-pro"] {
            let req = MessagesRequest {
                model: model.into(),
                max_tokens: 1000,
                messages: vec![
                    Message::user_text("hi"),
                    Message::assistant(vec![
                        ContentBlock::Thinking { thinking: "the reasoning so far".into(), signature: String::new() },
                        ContentBlock::text("ok"),
                    ]),
                    Message::user_text("go on"),
                ],
                system: vec![],
                tools: vec![],
                tool_choice: None,
                thinking: None,
                temperature: None,
                metadata: None,
                output_config: None,
                speed: None,
                stream: true,
                betas: vec![],
            };
            let carried = to_chat_request(&req).to_string().contains("the reasoning so far");
            assert_eq!(carried, provider.resends_reasoning(model), "{model}");
        }
        assert!(!provider.resends_reasoning("quick-thinking") && provider.resends_reasoning("gemini-2.5-pro"));
    }
}

/// Gemini (through Google's OpenAI-compatible endpoint) and gateways such as
/// OmniRoute: what they attach to a turn comes back exactly as they sent it.
#[cfg(test)]
mod gemini_tests {
    use super::*;
    use crate::MessageAccumulator;
    use kodact_types::{ApiMessage, ToolSpec};

    fn turn(model: &str, names: ToolNames, chunks: &[&str]) -> ApiMessage {
        let mut tr = ChunkTranslator::new(model).with_tool_names(names);
        let mut acc = MessageAccumulator::new();
        for c in chunks {
            for e in tr.push(c).unwrap() {
                acc.push(&e).unwrap();
            }
        }
        let mut end = vec![];
        tr.finish_into(&mut end);
        for e in &end {
            acc.push(e).unwrap();
        }
        acc.finish().unwrap()
    }

    #[test]
    fn a_call_gemini_3_never_signed_gets_the_placeholder() {
        let call = |id: &str| ContentBlock::ToolUse {
            id: id.into(),
            name: "Read".into(),
            input: json!({}),
            cache_control: None,
            provider: None,
        };
        let history = vec![
            Message::user(vec![ContentBlock::text("go")]),
            Message { role: Role::Assistant, content: vec![call("a"), call("b")] },
        ];
        let body = to_chat_request(&request("gemini-3-flash-preview", history.clone(), vec![], None));
        let calls = &body["messages"][1]["tool_calls"];
        assert_eq!(calls[0]["extra_content"]["google"]["thought_signature"], crate::gemini::PLACEHOLDER_SIGNATURE);
        assert!(calls[1].get("extra_content").is_none());
        let body = to_chat_request(&request("gpt-5", history, vec![], None));
        assert!(body["messages"][1]["tool_calls"][0].get("extra_content").is_none());
    }

    fn request(model: &str, messages: Vec<Message>, tools: Vec<ToolSpec>, effort: Option<&str>) -> MessagesRequest {
        MessagesRequest {
            model: model.into(),
            max_tokens: 1000,
            messages,
            system: vec![],
            tools,
            tool_choice: None,
            thinking: None,
            temperature: None,
            metadata: None,
            output_config: effort.map(|e| json!({"effort": e})),
            speed: None,
            stream: true,
            betas: vec![],
        }
    }

    fn tool(name: &str, schema: Value) -> ToolSpec {
        serde_json::from_value(json!({"name": name, "description": "d", "input_schema": schema})).unwrap()
    }

    /// The history after a tool-call turn: the user's prompt, the turn, its results.
    fn after(turn: &ApiMessage) -> Vec<Message> {
        let results: Vec<ContentBlock> = turn
            .to_message()
            .tool_uses()
            .map(|(id, _, _)| ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: ToolResultContent::Text("ok".into()),
                is_error: None,
                cache_control: None,
            })
            .collect();
        vec![Message::user_text("go"), turn.to_message(), Message::user(results)]
    }

    #[test]
    fn google_s_signatures_go_back_on_the_calls_they_came_with() {
        // Parallel calls: only the first carries a signature; the second comes at the same index with its own id.
        let t = turn(
            "gemini-3.1-pro-preview",
            ToolNames::default(),
            &[
                r#"{"id":"c","choices":[{"delta":{"role":"assistant","tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"Read","arguments":"{\"file_path\":\"/a\"}"},"extra_content":{"google":{"thought_signature":"U0lHX0E="}}}]}}]}"#,
                r#"{"id":"c","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_b","type":"function","function":{"name":"Read","arguments":"{\"file_path\":\"/b\"}"}}]}}]}"#,
                r#"{"id":"c","choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
                "[DONE]",
            ],
        );
        let calls: Vec<(String, Value)> =
            t.to_message().tool_uses().map(|(id, _, input)| (id.to_string(), input.clone())).collect();
        assert_eq!(
            calls,
            [("call_a".into(), json!({"file_path": "/a"})), ("call_b".into(), json!({"file_path": "/b"}))]
        );
        let body = to_chat_request(&request("gemini-3.1-pro-preview", after(&t), vec![tool("Read", json!({}))], None));
        let msgs = body["messages"].as_array().unwrap();
        let sent = &msgs[1]["tool_calls"];
        // Ids exactly as issued; the signature back on its own call, unchanged; none invented for the other.
        assert_eq!((sent[0]["id"].as_str(), sent[1]["id"].as_str()), (Some("call_a"), Some("call_b")));
        assert_eq!(sent[0]["extra_content"], json!({"google": {"thought_signature": "U0lHX0E="}}));
        assert!(sent[1].get("extra_content").is_none());
        // Results name their call.
        assert_eq!((msgs[2]["name"].as_str(), msgs[3]["name"].as_str()), (Some("Read"), Some("Read")));
    }

    #[test]
    fn a_gateway_s_message_signature_and_reasoning_go_back_too() {
        // OmniRoute-style: reasoning as `reasoning_content`, a signature on the message, not the call.
        let t = turn(
            "gc/gemini-3-flash",
            ToolNames::default(),
            &[
                r#"{"id":"c","choices":[{"delta":{"reasoning_content":"Look at the file first."}}]}"#,
                r#"{"id":"c","choices":[{"delta":{"thinking_signature":"TVNH"}}]}"#,
                r#"{"id":"c","choices":[{"delta":{"tool_calls":[{"index":0,"id":"toolu_gem_1","function":{"name":"Read","arguments":"{}","thought_signature":"RlNJRw=="}}]}}]}"#,
                r#"{"id":"c","choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#,
            ],
        );
        let body = to_chat_request(&request("gc/gemini-3-flash", after(&t), vec![], None));
        let m = &body["messages"][1];
        assert_eq!(m["reasoning_content"], "Look at the file first.");
        assert_eq!(m["thinking_signature"], "TVNH");
        assert_eq!(m["tool_calls"][0]["id"], "toolu_gem_1");
        assert_eq!(m["tool_calls"][0]["function"]["thought_signature"], "RlNJRw==");
        // Another provider's model: no reasoning sent back.
        let gpt = to_chat_request(&request("gpt-4o", after(&t), vec![], None));
        assert!(gpt["messages"][1].get("reasoning_content").is_none());
    }

    #[test]
    fn gemini_gets_names_schemas_and_effort_it_takes() {
        let long = "mcp__github-enterprise-server__list_pull_request_review_comments_for_repository";
        let tools = vec![
            tool(
                long,
                json!({"type": "object", "properties": {"n": {"type": ["integer", "null"]}}, "additionalProperties": false}),
            ),
            tool("Ping", json!({"type": "object", "properties": {}})),
        ];
        let req = request("gemini-3-flash", vec![Message::user_text("hi")], tools, Some("max"));
        let body = to_chat_request(&req);
        let sent = body["tools"][0]["function"]["name"].as_str().unwrap().to_string();
        assert!(sent.len() <= 64 && sent != long, "{sent}");
        assert_eq!(
            body["tools"][0]["function"]["parameters"],
            json!({"type": "object", "properties": {"n": {"type": "integer", "nullable": true}}})
        );
        assert!(body["tools"][1]["function"].get("parameters").is_none(), "no arguments: no schema");
        assert_eq!(body["reasoning_effort"], "high");
        assert!(body.get("temperature").is_none());
        // The model calls the alias; KodAct sees the tool's own name.
        let chunk = format!(
            r#"{{"id":"c","choices":[{{"delta":{{"tool_calls":[{{"index":0,"id":"c1","function":{{"name":"{sent}","arguments":"{{}}"}}}}]}},"finish_reason":"tool_calls"}}]}}"#
        );
        let t = turn("gemini-3-flash", tool_names(&req), &[&chunk]);
        assert_eq!(t.to_message().tool_uses().next().unwrap().1, long);
        // Not Gemini: schemas and names as they are, no effort sent this way.
        let other = to_chat_request(&request("gpt-4o", vec![Message::user_text("hi")], req.tools.clone(), Some("max")));
        assert_eq!(other["tools"][0]["function"]["name"], long);
        assert!(other.get("reasoning_effort").is_none());
    }

    #[test]
    fn thoughts_in_content_tags_become_thinking() {
        let t = turn(
            "gemini-2.5-pro",
            ToolNames::default(),
            &[
                r#"{"id":"c","choices":[{"delta":{"content":"<thou"}}]}"#,
                r#"{"id":"c","choices":[{"delta":{"content":"ght>Plan it.</th"}}]}"#,
                r#"{"id":"c","choices":[{"delta":{"content":"ought>The answer is 4 <"}}]}"#,
                r#"{"id":"c","choices":[{"delta":{"content":"3"}}]}"#,
                r#"{"id":"c","choices":[{"delta":{},"finish_reason":"stop"}]}"#,
            ],
        );
        assert_eq!(
            t.content,
            vec![
                ContentBlock::Thinking { thinking: "Plan it.".into(), signature: String::new() },
                ContentBlock::text("The answer is 4 <3"),
            ]
        );
        // Others' content is left as it is.
        let o = turn(
            "gpt-4o",
            ToolNames::default(),
            &[r#"{"id":"c","choices":[{"delta":{"content":"<thought>x</thought>"},"finish_reason":"stop"}]}"#],
        );
        assert_eq!(o.to_message().text(), "<thought>x</thought>");
    }
}
