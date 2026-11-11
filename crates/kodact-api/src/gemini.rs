//! Google's Gemini API, natively: `models/<model>:streamGenerateContent`
//! (Google AI Studio, Vertex AI, or a gateway that speaks it). Requests are
//! translated from the Messages shape and the streamed responses back into
//! Messages API stream events, so the engine never sees the difference.
//!
//! What Gemini needs back, it gets back exactly:
//! - A `functionCall` part's `thoughtSignature` (Gemini 3 refuses a history
//!   whose function call lost it) is kept in the tool-use block's `provider`
//!   and replayed on the same part. With parallel calls only the first has
//!   one; the others go back without, as they came.
//! - A thought part's signature is the thinking block's `signature`; a text
//!   part's, the text block's `provider`.
//! - A call Gemini 3 never made (history from another model, or an old
//!   session) gets Google's placeholder signature instead, on the first call
//!   of its turn only.
//!
//! The wire is stateless, like the Messages API: the whole history goes with
//! every request, so compaction, rewind and forks work unchanged.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use kodact_types::{
    ApiMessage, ContentBlock, Delta, MediaSource, Message, MessageDeltaBody, MessagesRequest, Role, StopReason,
    StreamEvent, ThinkingConfig, ToolResultContent, Usage,
};
use serde_json::{json, Map, Value};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::gemini_rules::{self, ToolNames};
use crate::sse::SseDecoder;
use crate::{backoff_delay, ApiError, EventStream, Provider};

/// The signature Google documents for a function call it didn't make itself
/// (history from another model): it skips the signature check for that call.
pub const PLACEHOLDER_SIGNATURE: &str = "skip_thought_signature_validator";

/// Ids KodAct gives calls Gemini sent without one. They are never sent back:
/// Gemini matches a response to its call by name and position then.
const GENERATED_ID: &str = "kodact_call_";

/// How the key travels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AuthHeader {
    /// By host: `x-goog-api-key` for Google AI Studio, a bearer token for
    /// Vertex AI, both for anything else (gateways take one or the other).
    #[default]
    Auto,
    GoogApiKey,
    Bearer,
    Both,
}

impl AuthHeader {
    /// `gemini.authHeader` / `KODACT_GEMINI_AUTH_HEADER`: `x-goog-api-key`,
    /// `bearer`, `both` or `auto`.
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "auto" | "" => AuthHeader::Auto,
            "x-goog-api-key" | "api-key" | "key" => AuthHeader::GoogApiKey,
            "bearer" | "authorization" => AuthHeader::Bearer,
            "both" => AuthHeader::Both,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct GeminiConfig {
    /// Up to the version (`https://generativelanguage.googleapis.com/v1beta`),
    /// or Vertex AI's `.../locations/<loc>/publishers/google`.
    pub base_url: String,
    pub api_key: Option<String>,
    pub auth_header: AuthHeader,
    pub extra_headers: Vec<(String, String)>,
    pub max_retries: u32,
    pub timeout: Duration,
    /// The variable or setting that chose `base_url`.
    pub url_from: Option<&'static str>,
    /// Set when `api_key` came from a helper command (its setting name).
    pub key_helper: Option<&'static str>,
}

impl Default for GeminiConfig {
    fn default() -> Self {
        GeminiConfig {
            base_url: "https://generativelanguage.googleapis.com/v1beta".into(),
            api_key: None,
            auth_header: AuthHeader::Auto,
            extra_headers: vec![],
            max_retries: 2,
            timeout: Duration::from_secs(300),
            url_from: None,
            key_helper: None,
        }
    }
}

impl GeminiConfig {
    /// The endpoint and where its key came from, for error hints.
    pub fn origin(&self) -> crate::auth::Origin {
        use crate::auth::KeyFrom;
        let key_from = match (self.api_key.is_some(), self.key_helper) {
            (false, _) => KeyFrom::None,
            (true, Some(h)) => KeyFrom::Helper(h),
            (true, None) => KeyFrom::GeminiKey,
        };
        crate::auth::Origin { backend: crate::auth::Backend::Gemini, url_from: self.url_from, key_from }
    }

    /// The header the key goes in, for this URL.
    pub fn header(&self) -> AuthHeader {
        if self.auth_header != AuthHeader::Auto {
            return self.auth_header;
        }
        let host = reqwest::Url::parse(&self.base_url).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase));
        match host.as_deref() {
            Some(h) if h.ends_with("aiplatform.googleapis.com") => AuthHeader::Bearer,
            Some(h) if h.ends_with("googleapis.com") => AuthHeader::GoogApiKey,
            _ => AuthHeader::Both,
        }
    }

    /// The base URL, with Google AI Studio's version when only its host was given.
    fn base(&self) -> String {
        let b = self.base_url.trim_end_matches('/');
        match reqwest::Url::parse(b) {
            Ok(u)
                if u.host_str() == Some("generativelanguage.googleapis.com")
                    && u.path().trim_matches('/').is_empty() =>
            {
                format!("{b}/v1beta")
            }
            _ => b.to_string(),
        }
    }

    /// `{base}/models/<model>` for a model id (a leading `models/` is allowed).
    fn model_url(&self, model: &str) -> String {
        format!("{}/models/{}", self.base(), model.trim().trim_start_matches("models/"))
    }
}

/// What an endpoint refused before, per model, so later turns send what it takes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Adapt {
    /// Tool schemas as `parameters` (the OpenAPI subset), not `parametersJsonSchema`.
    pub legacy_schema: bool,
    pub no_thinking_level: bool,
    pub no_thinking_budget: bool,
    pub no_include_thoughts: bool,
    pub no_temperature: bool,
}

impl Adapt {
    /// Learn from a refusal: true when it names something this client sends,
    /// so the request is worth sending again without it.
    pub fn learn(&mut self, body: &Value, err: &ApiError) -> bool {
        let ApiError::Http { status: 400 | 422, message, .. } = err else { return false };
        let m = message.to_ascii_lowercase().replace(['_', ' '], "");
        let before = *self;
        let gc = body.get("generationConfig");
        let sent = |p: &str| gc.and_then(|g| g.pointer(p)).is_some();
        if m.contains("parametersjsonschema") && !self.legacy_schema {
            self.legacy_schema = true;
        }
        if m.contains("thinkinglevel") && sent("/thinkingConfig/thinkingLevel") {
            self.no_thinking_level = true;
        }
        if m.contains("thinkingbudget") && sent("/thinkingConfig/thinkingBudget") {
            self.no_thinking_budget = true;
        }
        if m.contains("includethoughts") && sent("/thinkingConfig/includeThoughts") {
            self.no_include_thoughts = true;
        }
        // "Thinking is not supported for this model": no thinking settings at all.
        if m.contains("thinking") && m.contains("notsupported") && sent("/thinkingConfig") {
            self.no_thinking_level = true;
            self.no_thinking_budget = true;
            self.no_include_thoughts = true;
        }
        if m.contains("temperature") && sent("/temperature") {
            self.no_temperature = true;
        }
        *self != before
    }
}

/// Tool names as this wire sends them: an alias for each name Gemini wouldn't
/// take, whatever the model is called.
pub fn tool_names(req: &MessagesRequest) -> ToolNames {
    ToolNames::new(req.tools.iter().map(|t| t.name.as_str()))
}

/// The signature a block's provider data carries for Gemini: its own
/// (`part`), or one an OpenAI-compatible endpoint or gateway attached to the
/// same call (`extra_content.google.thought_signature`, `thought_signature`).
pub fn signature_of(provider: &Value) -> Option<String> {
    const PATHS: [&str; 9] = [
        "/part/thoughtSignature",
        "/part/thought_signature",
        "/tool_call/extra_content/google/thought_signature",
        "/tool_call/thought_signature",
        "/tool_call/thoughtSignature",
        "/function/thought_signature",
        "/function/thoughtSignature",
        "/message/thinking_signature",
        "/message/thought_signature",
    ];
    PATHS.iter().find_map(|p| provider.pointer(p).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string))
}

/// Translate a Messages request into a `generateContent` body. `note`, when
/// given, ends the history as a user text (a retry's instruction to the model).
pub fn to_gemini_request(req: &MessagesRequest, names: &ToolNames, adapt: &Adapt, note: Option<&str>) -> Value {
    let model = req.model.as_str();
    let gemini3 = gemini_rules::is_gemini3_or_later(model);
    let call_names: HashMap<String, String> = req
        .messages
        .iter()
        .flat_map(|m| m.tool_uses().map(|(id, name, _)| (id.to_string(), name.to_string())))
        .collect();
    let mut contents: Vec<Value> = Vec::new();
    for m in &req.messages {
        let (role, parts) = match m.role {
            Role::Assistant => ("model", model_parts(m, names, gemini3)),
            Role::User => ("user", user_parts(m, names, &call_names)),
        };
        push_content(&mut contents, role, parts);
    }
    if let Some(n) = note {
        push_content(&mut contents, "user", vec![json!({"text": n})]);
    }
    // A history starts with the user.
    if contents.first().and_then(|c| c["role"].as_str()) == Some("model") {
        contents.insert(0, json!({"role": "user", "parts": [{"text": "(continued)"}]}));
    }
    let mut body = json!({"contents": contents});
    let system: Vec<Value> =
        req.system.iter().filter(|b| !b.text.is_empty()).map(|b| json!({"text": b.text})).collect();
    if !system.is_empty() {
        body["systemInstruction"] = json!({"parts": system});
    }
    let decls: Vec<Value> = req
        .tools
        .iter()
        .filter(|t| t.kind.is_none())
        .map(|t| {
            let mut d = json!({"name": names.sent(&t.name), "description": t.description});
            // No schema at all for a tool without arguments (an empty object is refused).
            if let Some(sanitized) = gemini_rules::sanitize_schema(&t.input_schema) {
                if adapt.legacy_schema {
                    d["parameters"] = sanitized;
                } else {
                    let mut s = t.input_schema.clone();
                    if let Some(o) = s.as_object_mut() {
                        o.remove("$schema");
                    }
                    d["parametersJsonSchema"] = s;
                }
            }
            d
        })
        .collect();
    if !decls.is_empty() {
        body["tools"] = json!([{"functionDeclarations": decls}]);
        body["toolConfig"] = json!({"functionCallingConfig": calling_config(req.tool_choice.as_ref(), names)});
    }
    body["generationConfig"] = generation_config(req, adapt);
    body
}

/// `tool_choice` as Gemini's calling mode.
fn calling_config(choice: Option<&Value>, names: &ToolNames) -> Value {
    match choice.and_then(|c| c.get("type")).and_then(Value::as_str) {
        Some("any") => json!({"mode": "ANY"}),
        Some("none") => json!({"mode": "NONE"}),
        Some("tool") => {
            let name = choice.and_then(|c| c.get("name")).and_then(Value::as_str).unwrap_or_default();
            json!({"mode": "ANY", "allowedFunctionNames": [names.sent(name)]})
        }
        _ => json!({"mode": "AUTO"}),
    }
}

/// Length, temperature (only when set: Gemini 3 is tuned for its default) and
/// thinking: a level for Gemini 3 and later, a token budget for 2.5.
fn generation_config(req: &MessagesRequest, adapt: &Adapt) -> Value {
    let model = req.model.as_str();
    let mut gc = json!({"maxOutputTokens": req.max_tokens});
    if let Some(t) = req.temperature.filter(|_| !adapt.no_temperature) {
        gc["temperature"] = json!(t);
    }
    if !gemini_rules::is_gemini(model) {
        return gc;
    }
    let effort = req.output_config.as_ref().and_then(|c| c.get("effort")).and_then(Value::as_str);
    let mut tc = Map::new();
    if gemini_rules::is_gemini3_or_later(model) {
        if let Some(level) = effort.and_then(|e| gemini_rules::thinking_level(e, model)) {
            if !adapt.no_thinking_level {
                tc.insert("thinkingLevel".into(), json!(level));
            }
        }
    } else if !adapt.no_thinking_budget {
        let off = matches!(req.thinking, Some(ThinkingConfig::Disabled));
        // 2.5 Pro can't think less than its minimum; Flash can stop thinking.
        if off && gemini_rules::is_flash(model) {
            tc.insert("thinkingBudget".into(), json!(0));
        } else if let Some(e) = effort {
            tc.insert("thinkingBudget".into(), json!(gemini_rules::thinking_budget(e)));
        }
    }
    let thinking_off = tc.get("thinkingBudget") == Some(&json!(0));
    if !adapt.no_include_thoughts && !thinking_off {
        tc.insert("includeThoughts".into(), json!(true));
    }
    if !tc.is_empty() {
        gc["thinkingConfig"] = Value::Object(tc);
    }
    gc
}

/// Append parts as a turn, merged into the last one when the role is the same.
fn push_content(contents: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    if let Some(last) = contents.last_mut().filter(|c| c["role"] == role) {
        if let Some(a) = last["parts"].as_array_mut() {
            a.extend(parts);
            return;
        }
    }
    contents.push(json!({"role": role, "parts": parts}));
}

fn model_parts(m: &Message, names: &ToolNames, gemini3: bool) -> Vec<Value> {
    let mut parts = Vec::new();
    let mut first_call = true;
    for b in &m.content {
        match b {
            ContentBlock::Thinking { thinking, signature } => {
                if thinking.is_empty() && signature.is_empty() {
                    continue;
                }
                let mut p = json!({"text": thinking, "thought": true});
                if !signature.is_empty() {
                    p["thoughtSignature"] = json!(signature);
                }
                parts.push(p);
            }
            ContentBlock::Text { text, provider, .. } => {
                let sig = provider.as_ref().and_then(signature_of);
                if text.is_empty() && sig.is_none() {
                    continue;
                }
                let mut p = json!({"text": text});
                if let Some(s) = sig {
                    p["thoughtSignature"] = json!(s);
                }
                parts.push(p);
            }
            ContentBlock::ToolUse { id, name, input, provider, .. } => {
                let args = if input.is_object() { input.clone() } else { json!({}) };
                let mut call = json!({"name": names.sent(name), "args": args});
                if !id.starts_with(GENERATED_ID) {
                    call["id"] = json!(id);
                }
                let mut p = json!({"functionCall": call});
                match provider.as_ref().and_then(signature_of) {
                    Some(s) => p["thoughtSignature"] = json!(s),
                    // A call Gemini 3 didn't make: the first of its turn needs a signature.
                    None if gemini3 && first_call => p["thoughtSignature"] = json!(PLACEHOLDER_SIGNATURE),
                    None => {}
                }
                first_call = false;
                parts.push(p);
            }
            _ => {}
        }
    }
    parts
}

fn user_parts(m: &Message, names: &ToolNames, call_names: &HashMap<String, String>) -> Vec<Value> {
    let mut parts = Vec::new();
    // What tool results returned besides text (a screenshot, a PDF) follows them.
    let mut media = Vec::new();
    for b in &m.content {
        match b {
            ContentBlock::ToolResult { tool_use_id, content, is_error, .. } => {
                let text = result_text(content, &mut media);
                let name = call_names.get(tool_use_id).map(|n| names.sent(n)).unwrap_or_else(|| "unknown".into());
                let response = if *is_error == Some(true) { json!({"error": text}) } else { json!({"output": text}) };
                let mut fr = json!({"name": name, "response": response});
                if !tool_use_id.starts_with(GENERATED_ID) {
                    fr["id"] = json!(tool_use_id);
                }
                parts.push(json!({"functionResponse": fr}));
            }
            ContentBlock::Text { text, .. } if !text.is_empty() => parts.push(json!({"text": text})),
            ContentBlock::Image { source, .. } => parts.push(media_part(source, None)),
            ContentBlock::Document { source, title, .. } => parts.push(media_part(source, title.as_deref())),
            _ => {}
        }
    }
    if !media.is_empty() {
        parts.push(json!({"text": "The tool results above returned these files:"}));
        parts.extend(media);
    }
    parts
}

/// An image or document as a part: the file itself (`inlineData`, a PDF
/// included), or its text.
fn media_part(source: &MediaSource, title: Option<&str>) -> Value {
    match source {
        MediaSource::Base64 { media_type, data } => json!({"inlineData": {"mimeType": media_type, "data": data}}),
        MediaSource::Url { url } => json!({"text": format!("[File at {url}: this endpoint can't fetch it.]")}),
        MediaSource::Text { .. } => json!({"text": crate::openai::document_text(source, title)}),
    }
}

/// A tool result's text; images and documents it returned go to `media`.
fn result_text(content: &ToolResultContent, media: &mut Vec<Value>) -> String {
    match content {
        ToolResultContent::Text(s) => s.clone(),
        ToolResultContent::Blocks(blocks) => blocks
            .iter()
            .map(|b| match b {
                ContentBlock::Text { text, .. } => text.clone(),
                ContentBlock::Image { source, .. } | ContentBlock::Document { source, .. } => {
                    let title = match b {
                        ContentBlock::Document { title, .. } => title.as_deref(),
                        _ => None,
                    };
                    if let MediaSource::Text { .. } = source {
                        return crate::openai::document_text(source, title);
                    }
                    media.push(media_part(source, title));
                    format!("[file {}: attached after the tool results]", media.len())
                }
                other => serde_json::to_string(other).unwrap_or_default(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

/// A Gemini error body (`{"error": {"code", "message", "status", "details"}}`,
/// sometimes inside an array) as an API error, with the delay `RetryInfo` asks for.
pub fn parse_error(status: u16, text: &str, retry_after: Option<Duration>, url: String) -> ApiError {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let e = v.get("error").or_else(|| v.pointer("/0/error"));
    let Some(e) = e else {
        let message = if text.trim().is_empty() { format!("HTTP {status}") } else { text.trim().to_string() };
        return ApiError::Http { status, kind: "error".into(), message, retry_after, url, origin: None };
    };
    let (kind, message, delay) = error_parts(e);
    ApiError::Http { status, kind, message, retry_after: delay.or(retry_after), url, origin: None }
}

/// The kind (in the Messages API's words, so retries and hints work the
/// same), the message, and `RetryInfo.retryDelay`.
fn error_parts(e: &Value) -> (String, String, Option<Duration>) {
    let status = e.get("status").and_then(Value::as_str).unwrap_or_default();
    let message = e.get("message").and_then(Value::as_str).unwrap_or("unknown error").to_string();
    let details = e.get("details").and_then(Value::as_array).cloned().unwrap_or_default();
    let reason = details.iter().find_map(|d| d.get("reason").and_then(Value::as_str)).unwrap_or_default();
    let delay = details
        .iter()
        .filter(|d| d.get("@type").and_then(Value::as_str).is_some_and(|t| t.ends_with("RetryInfo")))
        .find_map(|d| d.get("retryDelay").and_then(Value::as_str))
        .and_then(|s| s.trim().trim_end_matches('s').parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(Duration::from_secs_f64);
    let kind = match status {
        _ if reason == "API_KEY_INVALID" || message.contains("API key not valid") => "authentication_error",
        "UNAUTHENTICATED" => "authentication_error",
        "PERMISSION_DENIED" => "permission_error",
        "NOT_FOUND" => "not_found_error",
        "RESOURCE_EXHAUSTED" => "rate_limit_error",
        "UNAVAILABLE" => "overloaded_error",
        "INVALID_ARGUMENT" | "FAILED_PRECONDITION" | "OUT_OF_RANGE" => "invalid_request_error",
        "DEADLINE_EXCEEDED" | "INTERNAL" | "UNKNOWN" => "api_error",
        _ => "error",
    };
    let message = if status.is_empty() { message } else { format!("{message} ({status})") };
    (kind.to_string(), message, delay)
}

async fn http_error(resp: reqwest::Response) -> ApiError {
    let status = resp.status().as_u16();
    let url = crate::auth::display_url(resp.url().as_str());
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<f64>().ok())
        .map(Duration::from_secs_f64);
    let text = resp.text().await.unwrap_or_default();
    parse_error(status, &text, retry_after, url)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Open {
    Text,
    Thinking,
}

/// Translates streamed `GenerateContentResponse`s into Messages API stream events.
#[derive(Debug, Default)]
pub struct GeminiTranslator {
    started: bool,
    finished: bool,
    next_index: usize,
    open: Option<(Open, usize)>,
    stop: Option<StopReason>,
    usage: Usage,
    model: String,
    names: ToolNames,
    saw_tool: bool,
    /// Text or a tool call was streamed (a retry would repeat it).
    wrote: bool,
    /// The finish reason arrived (a stream without one was cut off).
    saw_finish: bool,
    /// `MALFORMED_FUNCTION_CALL` and the like: why, for the retry note.
    malformed: Option<String>,
}

impl GeminiTranslator {
    pub fn new(model: &str) -> Self {
        GeminiTranslator { model: model.into(), ..Default::default() }
    }

    pub fn with_tool_names(mut self, names: ToolNames) -> Self {
        self.names = names;
        self
    }

    fn ensure_started(&mut self, out: &mut Vec<StreamEvent>, id: Option<&str>) {
        if !self.started {
            self.started = true;
            out.push(StreamEvent::MessageStart {
                message: ApiMessage {
                    id: id.unwrap_or("gemini").to_string(),
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
        if let Some((_, index)) = self.open.take() {
            out.push(StreamEvent::ContentBlockStop { index });
        }
    }

    fn start(&mut self, out: &mut Vec<StreamEvent>, block: ContentBlock) -> usize {
        self.close(out);
        let index = self.next_index;
        self.next_index += 1;
        out.push(StreamEvent::ContentBlockStart { index, content_block: block });
        index
    }

    /// The open block of this kind, or a new one.
    fn block(&mut self, out: &mut Vec<StreamEvent>, kind: Open) -> usize {
        if let Some((k, i)) = self.open {
            if k == kind {
                return i;
            }
        }
        let block = match kind {
            Open::Text => ContentBlock::text(""),
            Open::Thinking => ContentBlock::Thinking { thinking: String::new(), signature: String::new() },
        };
        let i = self.start(out, block);
        self.open = Some((kind, i));
        i
    }

    /// Feed one `data:` payload.
    pub fn push(&mut self, data: &str) -> Result<Vec<StreamEvent>, ApiError> {
        let v: Value = serde_json::from_str(data).map_err(|e| ApiError::Parse(format!("{e}: {data}")))?;
        let mut out = Vec::new();
        match v {
            Value::Array(items) => {
                for item in &items {
                    self.response(&mut out, item)?;
                }
            }
            item => self.response(&mut out, &item)?,
        }
        Ok(out)
    }

    fn response(&mut self, out: &mut Vec<StreamEvent>, v: &Value) -> Result<(), ApiError> {
        if let Some(e) = v.get("error") {
            let (kind, message, _) = error_parts(e);
            return Err(ApiError::Stream { kind, message });
        }
        self.ensure_started(out, v.get("responseId").and_then(Value::as_str));
        if let Some(u) = v.get("usageMetadata") {
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            let cached = n("cachedContentTokenCount");
            self.usage.input_tokens = (n("promptTokenCount") + n("toolUsePromptTokenCount")).saturating_sub(cached);
            self.usage.cache_read_input_tokens = cached;
            self.usage.output_tokens = n("candidatesTokenCount") + n("thoughtsTokenCount");
        }
        if let Some(reason) = v.pointer("/promptFeedback/blockReason").and_then(Value::as_str) {
            self.saw_finish = true;
            self.stop = Some(StopReason::Refusal);
            let i = self.block(out, Open::Text);
            out.push(StreamEvent::ContentBlockDelta {
                index: i,
                delta: Delta::TextDelta { text: format!("Gemini blocked this prompt ({reason}).") },
            });
            return Ok(());
        }
        let Some(cand) = v.pointer("/candidates/0") else { return Ok(()) };
        for part in cand.pointer("/content/parts").and_then(Value::as_array).into_iter().flatten() {
            self.part(out, part);
        }
        if let Some(reason) = cand.get("finishReason").and_then(Value::as_str) {
            self.finish_reason(out, reason, cand.get("finishMessage").and_then(Value::as_str));
        }
        Ok(())
    }

    fn part(&mut self, out: &mut Vec<StreamEvent>, part: &Value) {
        let sig = part
            .get("thoughtSignature")
            .or_else(|| part.get("thought_signature"))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        if let Some(call) = part.get("functionCall") {
            let id = call.get("id").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
            let id = id.unwrap_or_else(|| {
                format!("{GENERATED_ID}{}", NEXT_CALL.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
            });
            let name = self.names.original(call.get("name").and_then(Value::as_str).unwrap_or_default());
            let provider = sig.map(|s| json!({"part": {"thoughtSignature": s}}));
            let index =
                self.start(out, ContentBlock::ToolUse { id, name, input: json!({}), cache_control: None, provider });
            let args = call.get("args").filter(|a| a.is_object()).cloned().unwrap_or_else(|| json!({}));
            out.push(StreamEvent::ContentBlockDelta {
                index,
                delta: Delta::InputJsonDelta { partial_json: args.to_string() },
            });
            out.push(StreamEvent::ContentBlockStop { index });
            self.saw_tool = true;
            self.wrote = true;
            return;
        }
        let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
        let thought = part.get("thought").and_then(Value::as_bool).unwrap_or(false);
        if thought {
            let i = self.block(out, Open::Thinking);
            if !text.is_empty() {
                out.push(StreamEvent::ContentBlockDelta {
                    index: i,
                    delta: Delta::ThinkingDelta { thinking: text.into() },
                });
            }
            if let Some(s) = sig {
                out.push(StreamEvent::ContentBlockDelta { index: i, delta: Delta::SignatureDelta { signature: s } });
            }
            return;
        }
        if text.is_empty() && sig.is_none() {
            return;
        }
        // A signature-only part belongs to the open thought, else to text.
        if text.is_empty() {
            if let Some((Open::Thinking, i)) = self.open {
                out.push(StreamEvent::ContentBlockDelta {
                    index: i,
                    delta: Delta::SignatureDelta { signature: sig.unwrap_or_default() },
                });
                return;
            }
        }
        let i = self.block(out, Open::Text);
        if !text.is_empty() {
            self.wrote = true;
            out.push(StreamEvent::ContentBlockDelta { index: i, delta: Delta::TextDelta { text: text.into() } });
        }
        if let Some(s) = sig {
            out.push(StreamEvent::ContentBlockDelta {
                index: i,
                delta: Delta::ProviderDelta { provider: json!({"part": {"thoughtSignature": s}}) },
            });
        }
    }

    fn finish_reason(&mut self, out: &mut Vec<StreamEvent>, reason: &str, message: Option<&str>) {
        self.saw_finish = true;
        self.stop = Some(match reason {
            "MAX_TOKENS" => StopReason::MaxTokens,
            "SAFETY" | "RECITATION" | "BLOCKLIST" | "PROHIBITED_CONTENT" | "SPII" | "IMAGE_SAFETY" | "LANGUAGE" => {
                let i = self.block(out, Open::Text);
                let why = message.map(|m| format!(": {m}")).unwrap_or_default();
                out.push(StreamEvent::ContentBlockDelta {
                    index: i,
                    delta: Delta::TextDelta { text: format!("\n\n[Gemini stopped this answer ({reason}){why}.]") },
                });
                StopReason::Refusal
            }
            "MALFORMED_FUNCTION_CALL" | "UNEXPECTED_TOOL_CALL" | "TOO_MANY_TOOL_CALLS" => {
                self.malformed = Some(match message {
                    Some(m) => format!("{reason}: {m}"),
                    None => reason.to_string(),
                });
                return;
            }
            _ => StopReason::EndTurn,
        });
    }

    /// The call Gemini couldn't form, once: why, and whether a retry can
    /// continue this message (nothing but thinking was streamed yet).
    pub fn take_malformed(&mut self) -> Option<(String, bool)> {
        let why = self.malformed.take()?;
        Some((why, !self.wrote))
    }

    /// Before a retry streams into the same message: the open block closes.
    pub fn resume(&mut self, out: &mut Vec<StreamEvent>) {
        self.close(out);
        self.saw_finish = false;
        self.stop = None;
    }

    /// The stream ended before Gemini's finish reason: it was cut off.
    pub fn cut_off(&self) -> bool {
        !self.saw_finish
    }

    /// Close everything; idempotent.
    pub fn finish_into(&mut self, out: &mut Vec<StreamEvent>) {
        if self.finished {
            return;
        }
        self.finished = true;
        self.ensure_started(out, None);
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

/// Ids for calls Gemini sent without one, unique in this process.
static NEXT_CALL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// The note a retry after a malformed call ends the history with.
fn retry_note(why: &str) -> String {
    format!(
        "Your last function call could not be used ({why}). Call the tool again, with arguments that are valid \
         JSON for its schema, or answer without a tool."
    )
}

#[derive(Clone)]
struct Conn {
    config: GeminiConfig,
    http: reqwest::Client,
    /// What this endpoint refused, per model (see [`Adapt`]).
    adapted: Arc<Mutex<HashMap<String, Adapt>>>,
}

impl Conn {
    fn authed(&self, mut rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(k) = &self.config.api_key {
            rb = match self.config.header() {
                AuthHeader::GoogApiKey => rb.header("x-goog-api-key", k),
                AuthHeader::Bearer => rb.bearer_auth(k),
                AuthHeader::Both | AuthHeader::Auto => rb.header("x-goog-api-key", k).bearer_auth(k),
            };
        }
        for (k, v) in &self.config.extra_headers {
            rb = rb.header(k, v);
        }
        rb
    }

    /// Send a streaming request, retrying what may pass on a second try and
    /// resending without what the endpoint refused.
    async fn open(
        &self,
        req: &MessagesRequest,
        names: &ToolNames,
        note: Option<&str>,
        cancel: &CancellationToken,
    ) -> Result<reqwest::Response, ApiError> {
        let url = format!("{}:streamGenerateContent?alt=sse", self.config.model_url(&req.model));
        let mut adapt = self.adapted.lock().ok().and_then(|m| m.get(&req.model).copied()).unwrap_or_default();
        let (mut attempt, mut adaptations) = (0, 0);
        loop {
            let body = to_gemini_request(req, names, &adapt, note);
            let rb = self.authed(self.http.post(&url).json(&body));
            let res = tokio::select! {
                _ = cancel.cancelled() => return Err(ApiError::Cancelled),
                r = rb.send() => r.map_err(crate::messages::network_error),
            };
            let err = match res {
                Ok(r) if r.status().is_success() => return Ok(r),
                Ok(r) => http_error(r).await,
                Err(e) => e,
            };
            if adaptations < 4 && adapt.learn(&body, &err) {
                tracing::warn!(error = %err, "the endpoint refused a setting; sending again without it");
                if let Ok(mut m) = self.adapted.lock() {
                    m.insert(req.model.clone(), adapt);
                }
                adaptations += 1;
                continue;
            }
            if err.retries_quickly() && attempt < self.config.max_retries {
                let retry_after = match &err {
                    ApiError::Http { retry_after, .. } => *retry_after,
                    _ => None,
                };
                tracing::warn!(attempt, error = %err, "retrying API request");
                tokio::select! {
                    _ = cancel.cancelled() => return Err(ApiError::Cancelled),
                    _ = tokio::time::sleep(backoff_delay(attempt, retry_after)) => {}
                }
                attempt += 1;
                continue;
            }
            return Err(err.with_origin(self.config.origin()));
        }
    }
}

pub struct GeminiProvider {
    conn: Conn,
}

impl GeminiProvider {
    pub fn new(config: GeminiConfig) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(config.timeout)
            .build()
            .map_err(|e| ApiError::network(e.to_string()))?;
        Ok(GeminiProvider { conn: Conn { config, http, adapted: Default::default() } })
    }
}

/// Feed one response body through the translator. `Err(None)` when the
/// receiver went away or the request was cancelled (already reported).
async fn pump(
    resp: reqwest::Response,
    tr: &mut GeminiTranslator,
    tx: &mpsc::Sender<Result<StreamEvent, ApiError>>,
    cancel: &CancellationToken,
) -> Result<(), Option<ApiError>> {
    let mut body = resp.bytes_stream();
    let mut decoder = SseDecoder::new();
    loop {
        let chunk = tokio::select! {
            _ = cancel.cancelled() => {
                let _ = tx.send(Err(ApiError::Cancelled)).await;
                return Err(None);
            }
            c = body.next() => c,
        };
        let (events, done) = match chunk {
            Some(Ok(bytes)) => (decoder.push(&bytes), false),
            Some(Err(e)) => return Err(Some(ApiError::network(e.to_string()))),
            None => (decoder.finish().into_iter().collect(), true),
        };
        for ev in events {
            if ev.data.trim().is_empty() {
                continue;
            }
            for item in tr.push(&ev.data).map_err(Some)? {
                if tx.send(Ok(item)).await.is_err() {
                    return Err(None);
                }
            }
        }
        if done {
            return Ok(());
        }
    }
}

#[async_trait::async_trait]
impl Provider for GeminiProvider {
    fn name(&self) -> &str {
        "gemini"
    }

    fn origin(&self) -> Option<crate::auth::Origin> {
        Some(self.conn.config.origin())
    }

    fn base_url(&self) -> Option<String> {
        Some(crate::auth::display_url(&self.conn.config.base()))
    }

    async fn list_models(&self) -> Option<Result<Vec<String>, ApiError>> {
        let c = &self.conn;
        let r = async {
            let mut ids = Vec::new();
            let mut token: Option<String> = None;
            for _ in 0..20 {
                let mut url = format!("{}/models?pageSize=1000", c.config.base());
                if let Some(t) = &token {
                    url.push_str(&format!("&pageToken={t}"));
                }
                let res = c
                    .authed(c.http.get(&url).timeout(Duration::from_secs(15)))
                    .send()
                    .await
                    .map_err(crate::messages::network_error)?;
                if !res.status().is_success() {
                    return Err(http_error(res).await);
                }
                let body: Value = res.json().await.map_err(|e| ApiError::Parse(e.to_string()))?;
                ids.extend(listed_models(&body));
                token = body.get("nextPageToken").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string);
                if token.is_none() {
                    break;
                }
            }
            ids.sort();
            ids.dedup();
            Ok(ids)
        }
        .await;
        Some(r.map_err(|e| e.with_origin(c.config.origin())))
    }

    async fn count_tokens(&self, request: &MessagesRequest) -> Option<u64> {
        let c = &self.conn;
        let names = tool_names(request);
        let adapt = c.adapted.lock().ok().and_then(|m| m.get(&request.model).copied()).unwrap_or_default();
        let mut inner = to_gemini_request(request, &names, &adapt, None);
        if let Some(o) = inner.as_object_mut() {
            o.remove("generationConfig");
            o.remove("toolConfig");
            o.insert("model".into(), json!(format!("models/{}", request.model.trim_start_matches("models/"))));
        }
        let url = format!("{}:countTokens", c.config.model_url(&request.model));
        let rb = c.authed(c.http.post(&url).json(&json!({"generateContentRequest": inner})));
        let res = rb.timeout(Duration::from_secs(10)).send().await.ok()?;
        if !res.status().is_success() {
            return None;
        }
        let body: Value = res.json().await.ok()?;
        body.get("totalTokens").and_then(Value::as_u64)
    }

    async fn stream(&self, request: MessagesRequest, cancel: CancellationToken) -> Result<EventStream, ApiError> {
        let names = tool_names(&request);
        let first = self.conn.open(&request, &names, None, &cancel).await?;
        let conn = self.conn.clone();
        let (tx, rx) = mpsc::channel::<Result<StreamEvent, ApiError>>(64);
        tokio::spawn(async move {
            let mut tr = GeminiTranslator::new(&request.model).with_tool_names(names.clone());
            let mut resp = first;
            let mut retried = false;
            loop {
                match pump(resp, &mut tr, &tx, &cancel).await {
                    Ok(()) => {}
                    Err(Some(e)) => {
                        let _ = tx.send(Err(e)).await;
                        return;
                    }
                    Err(None) => return,
                }
                // A call Gemini couldn't form: once, ask again with a note.
                if let Some((why, can_retry)) = tr.take_malformed() {
                    if retried || !can_retry {
                        let _ = tx
                            .send(Err(ApiError::Stream {
                                kind: "api_error".into(),
                                message: format!("Gemini could not form a function call ({why})"),
                            }))
                            .await;
                        return;
                    }
                    retried = true;
                    tracing::warn!(%why, "Gemini sent a malformed function call; asking again");
                    let mut out = Vec::new();
                    tr.resume(&mut out);
                    for ev in out {
                        let _ = tx.send(Ok(ev)).await;
                    }
                    let note = retry_note(&why);
                    match conn.open(&request, &names, Some(&note), &cancel).await {
                        Ok(r) => {
                            resp = r;
                            continue;
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e)).await;
                            return;
                        }
                    }
                }
                if tr.cut_off() {
                    let _ = tx.send(Err(ApiError::Parse("stream ended before Gemini's finish reason".into()))).await;
                    return;
                }
                let mut out = Vec::new();
                tr.finish_into(&mut out);
                for ev in out {
                    if tx.send(Ok(ev)).await.is_err() {
                        return;
                    }
                }
                return;
            }
        });
        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}

/// Model ids from `GET /models` that can generate content, with the limits
/// each reports recorded for `/context` and compaction.
pub fn listed_models(body: &Value) -> Vec<String> {
    let mut ids = Vec::new();
    for m in body.get("models").and_then(Value::as_array).into_iter().flatten() {
        let Some(name) = m.get("name").and_then(Value::as_str) else { continue };
        let methods = m.get("supportedGenerationMethods").and_then(Value::as_array);
        if methods.is_some_and(|ms| !ms.iter().any(|x| x == "generateContent")) {
            continue;
        }
        let id = name.rsplit('/').next().unwrap_or(name).to_string();
        let limits = crate::models::Limits {
            context_window: m.get("inputTokenLimit").and_then(Value::as_u64).filter(|n| *n > 0),
            max_output: m
                .get("outputTokenLimit")
                .and_then(Value::as_u64)
                .filter(|n| *n > 0)
                .map(|n| n.min(u64::from(u32::MAX)) as u32),
        };
        if limits != crate::models::Limits::default() {
            crate::models::listed_limits(&id, limits);
        }
        ids.push(id);
    }
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MessageAccumulator;
    use kodact_types::{SystemBlock, ToolSpec};

    fn tool(name: &str, schema: Value) -> ToolSpec {
        serde_json::from_value(json!({"name": name, "description": "d", "input_schema": schema})).unwrap()
    }

    fn request(model: &str, messages: Vec<Message>) -> MessagesRequest {
        MessagesRequest {
            model: model.into(),
            max_tokens: 1000,
            messages,
            system: vec![SystemBlock { kind: "text".into(), text: "Be brief.".into(), cache_control: None }],
            tools: vec![
                tool("Read", json!({"type": "object", "properties": {"file_path": {"type": "string"}}})),
                tool("Clock", json!({"type": "object", "properties": {}})),
            ],
            tool_choice: None,
            thinking: None,
            temperature: None,
            metadata: None,
            output_config: Some(json!({"effort": "medium"})),
            speed: None,
            stream: true,
            betas: vec![],
        }
    }

    fn call(id: &str, provider: Option<Value>) -> ContentBlock {
        ContentBlock::ToolUse {
            id: id.into(),
            name: "Read".into(),
            input: json!({"file_path": "a.rs"}),
            cache_control: None,
            provider,
        }
    }

    fn result(id: &str) -> ContentBlock {
        ContentBlock::ToolResult {
            tool_use_id: id.into(),
            content: ToolResultContent::Text("fn main() {}".into()),
            is_error: None,
            cache_control: None,
        }
    }

    /// Run SSE payloads through a translator and accumulate the message.
    fn accumulate(model: &str, chunks: &[Value]) -> ApiMessage {
        let mut tr = GeminiTranslator::new(model);
        let mut acc = MessageAccumulator::new();
        for c in chunks {
            for ev in tr.push(&c.to_string()).unwrap() {
                acc.push(&ev).unwrap();
            }
        }
        let mut out = Vec::new();
        tr.finish_into(&mut out);
        for ev in out {
            acc.push(&ev).unwrap();
        }
        acc.finish().unwrap()
    }

    #[test]
    fn a_signed_call_streams_in_and_goes_back_on_the_same_part() {
        let m = accumulate(
            "gemini-3-flash-preview",
            &[
                json!({"candidates": [{"content": {"role": "model", "parts": [
                    {"text": "Reading the file first.", "thought": true}
                ]}}], "responseId": "r1"}),
                json!({"candidates": [{"content": {"role": "model", "parts": [
                    {"functionCall": {"name": "Read", "args": {"file_path": "a.rs"}}, "thoughtSignature": "SIG_A"},
                    {"functionCall": {"name": "Read", "args": {"file_path": "b.rs"}}}
                ]}, "finishReason": "STOP"}],
                "usageMetadata": {"promptTokenCount": 120, "cachedContentTokenCount": 20, "candidatesTokenCount": 30, "thoughtsTokenCount": 50}}),
            ],
        );
        assert_eq!(m.stop_reason, Some(StopReason::ToolUse));
        assert_eq!((m.usage.input_tokens, m.usage.cache_read_input_tokens, m.usage.output_tokens), (100, 20, 80));
        let calls: Vec<&ContentBlock> =
            m.content.iter().filter(|b| matches!(b, ContentBlock::ToolUse { .. })).collect();
        assert_eq!(calls.len(), 2, "parallel calls stay two calls: {:#?}", m.content);
        let ContentBlock::ToolUse { id, provider, input, .. } = calls[0] else { unreachable!() };
        assert!(id.starts_with(GENERATED_ID));
        assert_eq!(input, &json!({"file_path": "a.rs"}));
        assert_eq!(provider.as_ref().and_then(signature_of).as_deref(), Some("SIG_A"));

        // Next request: the history goes back with the signature on the first call only.
        let mut history = vec![Message::user(vec![ContentBlock::text("read both")])];
        history.push(Message { role: Role::Assistant, content: m.content.clone() });
        let ids: Vec<String> = history[1].tool_uses().map(|(id, _, _)| id.to_string()).collect();
        history.push(Message::user(ids.iter().map(|i| result(i)).collect()));
        let req = request("gemini-3-flash-preview", history);
        let body = to_gemini_request(&req, &tool_names(&req), &Adapt::default(), None);
        let model_parts = body["contents"][1]["parts"].as_array().unwrap();
        assert_eq!(model_parts[0]["thought"], true);
        assert_eq!(model_parts[1]["thoughtSignature"], "SIG_A");
        assert!(model_parts[1]["functionCall"].get("id").is_none(), "generated ids are never sent");
        assert!(model_parts[2].get("thoughtSignature").is_none(), "the second parallel call goes back as it came");
        let responses = body["contents"][2]["parts"].as_array().unwrap();
        assert_eq!(responses[0]["functionResponse"]["name"], "Read");
        assert_eq!(responses[0]["functionResponse"]["response"]["output"], "fn main() {}");
    }

    #[test]
    fn ids_gemini_gave_go_back_unchanged() {
        let m = accumulate(
            "gemini-3-pro-preview",
            &[json!({"candidates": [{"content": {"parts": [
                {"functionCall": {"id": "abc123", "name": "Read", "args": {}}, "thoughtSignature": "S"}
            ]}, "finishReason": "STOP"}]})],
        );
        let history = vec![
            Message::user(vec![ContentBlock::text("go")]),
            Message { role: Role::Assistant, content: m.content.clone() },
            Message::user(vec![result("abc123")]),
        ];
        let req = request("gemini-3-pro-preview", history);
        let body = to_gemini_request(&req, &tool_names(&req), &Adapt::default(), None);
        assert_eq!(body["contents"][1]["parts"][0]["functionCall"]["id"], "abc123");
        assert_eq!(body["contents"][2]["parts"][0]["functionResponse"]["id"], "abc123");
    }

    #[test]
    fn a_call_gemini_3_never_signed_gets_the_placeholder_once_per_turn() {
        let history = vec![
            Message::user(vec![ContentBlock::text("go")]),
            Message { role: Role::Assistant, content: vec![call("toolu_1", None), call("toolu_2", None)] },
            Message::user(vec![result("toolu_1"), result("toolu_2")]),
        ];
        let req = request("gemini-3.1-pro-preview", history.clone());
        let body = to_gemini_request(&req, &tool_names(&req), &Adapt::default(), None);
        let parts = &body["contents"][1]["parts"];
        assert_eq!(parts[0]["thoughtSignature"], PLACEHOLDER_SIGNATURE);
        assert!(parts[1].get("thoughtSignature").is_none());
        // Gemini 2.5 checks no signatures: none is invented.
        let req = request("gemini-2.5-pro", history);
        let body = to_gemini_request(&req, &tool_names(&req), &Adapt::default(), None);
        assert!(body["contents"][1]["parts"][0].get("thoughtSignature").is_none());
    }

    #[test]
    fn a_signature_from_the_openai_wire_is_replayed_natively() {
        let p = json!({"tool_call": {"extra_content": {"google": {"thought_signature": "FROM_OPENAI"}}}});
        let history = vec![
            Message::user(vec![ContentBlock::text("go")]),
            Message { role: Role::Assistant, content: vec![call("call_1", Some(p))] },
            Message::user(vec![result("call_1")]),
        ];
        let req = request("gemini-3-flash-preview", history);
        let body = to_gemini_request(&req, &tool_names(&req), &Adapt::default(), None);
        assert_eq!(body["contents"][1]["parts"][0]["thoughtSignature"], "FROM_OPENAI");
    }

    #[test]
    fn thoughts_text_signatures_and_settings() {
        let m = accumulate(
            "gemini-3-flash-preview",
            &[
                json!({"candidates": [{"content": {"parts": [{"text": "plan", "thought": true}]}}]}),
                json!({"candidates": [{"content": {"parts": [{"text": "Done."}]}}]}),
                json!({"candidates": [{"content": {"parts": [{"text": "", "thoughtSignature": "TEXT_SIG"}]}, "finishReason": "STOP"}]}),
            ],
        );
        assert_eq!(m.stop_reason, Some(StopReason::EndTurn));
        assert!(matches!(&m.content[0], ContentBlock::Thinking { thinking, .. } if thinking == "plan"));
        let ContentBlock::Text { text, provider, .. } = &m.content[1] else { panic!("{:#?}", m.content) };
        assert_eq!(text, "Done.");
        assert_eq!(provider.as_ref().and_then(signature_of).as_deref(), Some("TEXT_SIG"));

        let req = request("gemini-3-flash-preview", vec![Message::user(vec![ContentBlock::text("hi")])]);
        let body = to_gemini_request(&req, &tool_names(&req), &Adapt::default(), None);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "Be brief.");
        assert_eq!(
            body["generationConfig"],
            json!({"maxOutputTokens": 1000, "thinkingConfig": {"thinkingLevel": "medium", "includeThoughts": true}})
        );
        let decls = &body["tools"][0]["functionDeclarations"];
        assert_eq!(decls[0]["parametersJsonSchema"]["properties"]["file_path"]["type"], "string");
        assert!(decls[1].get("parametersJsonSchema").is_none(), "no schema for a tool without arguments");
        assert_eq!(body["toolConfig"]["functionCallingConfig"]["mode"], "AUTO");

        let req = request("gemini-2.5-flash", vec![Message::user(vec![ContentBlock::text("hi")])]);
        let body = to_gemini_request(&req, &tool_names(&req), &Adapt::default(), None);
        assert_eq!(body["generationConfig"]["thinkingConfig"]["thinkingBudget"], 8192);
        assert!(body["generationConfig"]["thinkingConfig"].get("thinkingLevel").is_none());
    }

    #[test]
    fn refusals_are_learned_and_the_schema_falls_back() {
        let req = request("gemini-3-flash-preview", vec![Message::user(vec![ContentBlock::text("hi")])]);
        let names = tool_names(&req);
        let mut adapt = Adapt::default();
        let body = to_gemini_request(&req, &names, &adapt, None);
        let e = parse_error(
            400,
            r#"{"error": {"code": 400, "message": "Invalid JSON payload received. Unknown name \"parametersJsonSchema\" at 'tools[0].function_declarations[0]': Cannot find field.", "status": "INVALID_ARGUMENT"}}"#,
            None,
            String::new(),
        );
        assert!(adapt.learn(&body, &e));
        let body = to_gemini_request(&req, &names, &adapt, None);
        assert_eq!(body["tools"][0]["functionDeclarations"][0]["parameters"]["type"], "object");
        assert!(!adapt.learn(&body, &e), "nothing left to change");
        let e = parse_error(
            400,
            r#"{"error": {"code": 400, "message": "Unable to submit request because thinking_level is not supported by this model.", "status": "INVALID_ARGUMENT"}}"#,
            None,
            String::new(),
        );
        assert!(adapt.learn(&body, &e));
        let body = to_gemini_request(&req, &names, &adapt, None);
        assert!(body.pointer("/generationConfig/thinkingConfig/thinkingLevel").is_none());
    }

    #[test]
    fn errors_carry_googles_delay_and_kinds() {
        let e = parse_error(
            429,
            r#"[{"error": {"code": 429, "message": "Quota exceeded", "status": "RESOURCE_EXHAUSTED", "details": [{"@type": "type.googleapis.com/google.rpc.RetryInfo", "retryDelay": "20s"}]}}]"#,
            None,
            String::new(),
        );
        let ApiError::Http { kind, retry_after, .. } = &e else { panic!() };
        assert_eq!((kind.as_str(), *retry_after), ("rate_limit_error", Some(Duration::from_secs(20))));
        assert!(e.is_retryable());
        let e = parse_error(
            400,
            r#"{"error": {"code": 400, "message": "API key not valid. Please pass a valid API key.", "status": "INVALID_ARGUMENT", "details": [{"reason": "API_KEY_INVALID"}]}}"#,
            None,
            String::new(),
        )
        .with_origin(GeminiConfig::default().origin());
        assert!(e.is_auth_failure() && e.is_unrecoverable());
        assert!(e.hint().unwrap().contains("KODACT_GEMINI_API_KEY"), "{:?}", e.hint());
        let e = parse_error(
            503,
            r#"{"error": {"code": 503, "message": "The model is overloaded.", "status": "UNAVAILABLE"}}"#,
            None,
            String::new(),
        );
        assert!(e.is_overloaded() && e.is_retryable());
    }

    #[test]
    fn malformed_calls_safety_and_cut_streams() {
        let mut tr = GeminiTranslator::new("gemini-3-flash-preview");
        tr.push(&json!({"candidates": [{"content": {"parts": [{"text": "t", "thought": true}]}, "finishReason": "MALFORMED_FUNCTION_CALL", "finishMessage": "bad json"}]}).to_string()).unwrap();
        assert_eq!(tr.take_malformed(), Some(("MALFORMED_FUNCTION_CALL: bad json".into(), true)));
        assert!(tr.take_malformed().is_none());
        assert!(retry_note("x").contains("Call the tool again"));

        let m = accumulate(
            "gemini-3-flash-preview",
            &[json!({"candidates": [{"content": {"parts": [{"text": "Part"}]}, "finishReason": "SAFETY"}]})],
        );
        assert_eq!(m.stop_reason, Some(StopReason::Refusal));
        assert!(m.content.iter().filter_map(|b| b.as_text()).collect::<String>().contains("(SAFETY)"));

        let mut tr = GeminiTranslator::new("gemini-3-flash-preview");
        tr.push(&json!({"candidates": [{"content": {"parts": [{"text": "half"}]}}]}).to_string()).unwrap();
        assert!(tr.cut_off());
    }

    #[test]
    fn long_tool_names_are_aliased_and_mapped_back() {
        let long = "mcp__github-enterprise-server__list_pull_request_review_comments_for_repository";
        let mut req = request("gemini-3-flash-preview", vec![Message::user(vec![ContentBlock::text("hi")])]);
        req.tools.push(tool(long, json!({"type": "object", "properties": {"n": {"type": "integer"}}})));
        let names = tool_names(&req);
        let body = to_gemini_request(&req, &names, &Adapt::default(), None);
        let sent = body["tools"][0]["functionDeclarations"][2]["name"].as_str().unwrap().to_string();
        assert!(sent.len() <= 64 && gemini_rules::valid_tool_name(&sent));
        let mut tr = GeminiTranslator::new("gemini-3-flash-preview").with_tool_names(names);
        let evs = tr
            .push(
                &json!({"candidates": [{"content": {"parts": [{"functionCall": {"name": sent, "args": {"n": 1}}}]}}]})
                    .to_string(),
            )
            .unwrap();
        assert!(evs.iter().any(|e| matches!(e, StreamEvent::ContentBlockStart { content_block: ContentBlock::ToolUse { name, .. }, .. } if name == long)));
    }

    #[test]
    fn the_key_header_follows_the_host_and_models_are_listed() {
        let c = |u: &str| GeminiConfig { base_url: u.into(), ..Default::default() };
        assert_eq!(c("https://generativelanguage.googleapis.com/v1beta").header(), AuthHeader::GoogApiKey);
        assert_eq!(
            c("https://us-central1-aiplatform.googleapis.com/v1/projects/p/locations/us-central1/publishers/google")
                .header(),
            AuthHeader::Bearer
        );
        assert_eq!(c("http://127.0.0.1:20128/v1beta").header(), AuthHeader::Both);
        assert_eq!(
            c("https://generativelanguage.googleapis.com").base(),
            "https://generativelanguage.googleapis.com/v1beta"
        );
        assert_eq!(
            c("https://generativelanguage.googleapis.com/v1beta").model_url("models/gemini-3-flash-preview"),
            "https://generativelanguage.googleapis.com/v1beta/models/gemini-3-flash-preview"
        );
        let ids = listed_models(&json!({"models": [
            {"name": "models/gemini-test-listed", "inputTokenLimit": 1048576, "outputTokenLimit": 65536, "supportedGenerationMethods": ["generateContent", "countTokens"]},
            {"name": "models/text-embedding-004", "supportedGenerationMethods": ["embedContent"]}
        ]}));
        assert_eq!(ids, vec!["gemini-test-listed"]);
        assert_eq!(crate::models::model_info_or_default("gemini-test-listed").context_window, 1_048_576);
    }

    type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

    /// A strict Gemini endpoint: it refuses a history whose function call lost
    /// its signature, as Google does. Each request gets the next scripted
    /// stream; it records (path, headers, body).
    async fn strict_gemini(script: Vec<Vec<Value>>) -> (String, Seen) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen: Seen = Default::default();
        let log = seen.clone();
        tokio::spawn(async move {
            let mut script = script.into_iter();
            loop {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let (head, body) = loop {
                    let mut chunk = [0u8; 8192];
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
                            break (
                                text[..end].to_string(),
                                serde_json::from_slice::<Value>(&buf[end + 4..end + 4 + len]).unwrap(),
                            );
                        }
                    }
                };
                let path = head.lines().next().unwrap_or_default().split(' ').nth(1).unwrap_or_default().to_string();
                log.lock().unwrap().push((path, head.to_ascii_lowercase(), body.clone()));
                // Google's rule: every model functionCall that came signed goes back signed.
                let unsigned = body["contents"].as_array().unwrap().iter().filter(|c| c["role"] == "model").any(|c| {
                    c["parts"].as_array().unwrap().iter().any(|p| {
                        p.get("functionCall").is_some()
                            && p.get("thoughtSignature").is_none()
                            && p["functionCall"]["args"]["file_path"] == "a.rs"
                    })
                });
                let (status, ct, out) = if unsigned {
                    ("400 Bad Request", "application/json", json!({"error": {"code": 400, "message": "Function call is missing a thought_signature in functionCall parts.", "status": "INVALID_ARGUMENT"}}).to_string())
                } else {
                    let chunks = script.next().unwrap_or_default();
                    ("200 OK", "text/event-stream", chunks.iter().map(|c| format!("data: {c}\r\n\r\n")).collect())
                };
                let head = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: {ct}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    out.len()
                );
                sock.write_all(head.as_bytes()).await.unwrap();
                sock.write_all(out.as_bytes()).await.unwrap();
            }
        });
        (format!("http://{addr}/v1beta"), seen)
    }

    async fn run(p: &GeminiProvider, req: MessagesRequest) -> Result<ApiMessage, ApiError> {
        crate::complete(p, req, &CancellationToken::new()).await
    }

    #[tokio::test]
    async fn a_tool_loop_against_a_strict_endpoint_and_a_malformed_call_retried() {
        let (url, seen) = strict_gemini(vec![
            vec![json!({"candidates": [{"content": {"role": "model", "parts": [
                {"functionCall": {"name": "Read", "args": {"file_path": "a.rs"}}, "thoughtSignature": "SIG_LOOP"}
            ]}, "finishReason": "STOP"}]})],
            vec![json!({"candidates": [{"content": {"parts": [{"text": "x", "thought": true}]}, "finishReason": "MALFORMED_FUNCTION_CALL"}]})],
            vec![json!({"candidates": [{"content": {"parts": [{"text": "It is empty."}]}, "finishReason": "STOP"}]})],
        ])
        .await;
        let p =
            GeminiProvider::new(GeminiConfig { base_url: url, api_key: Some("k-123".into()), ..Default::default() })
                .unwrap();
        let mut history = vec![Message::user(vec![ContentBlock::text("read a.rs")])];
        let first = run(&p, request("gemini-3-flash-preview", history.clone())).await.unwrap();
        assert_eq!(first.stop_reason, Some(StopReason::ToolUse));
        let id = first
            .content
            .iter()
            .find_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .unwrap();
        history.push(Message { role: Role::Assistant, content: first.content.clone() });
        history.push(Message::user(vec![result(&id)]));
        // Through a transcript and back, as a resumed session would.
        let history: Vec<Message> = serde_json::from_value(serde_json::to_value(&history).unwrap()).unwrap();
        let second = run(&p, request("gemini-3-flash-preview", history)).await.unwrap();
        assert_eq!(second.stop_reason, Some(StopReason::EndTurn));
        assert!(second.content.iter().filter_map(|b| b.as_text()).collect::<String>().contains("It is empty."));

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 3, "the malformed call was asked again once");
        assert!(
            seen[0].0.ends_with("/v1beta/models/gemini-3-flash-preview:streamGenerateContent?alt=sse"),
            "{}",
            seen[0].0
        );
        assert!(seen[0].1.contains("x-goog-api-key: k-123"));
        assert_eq!(seen[1].2["contents"][1]["parts"][0]["thoughtSignature"], "SIG_LOOP");
        let last = seen[2].2["contents"].as_array().unwrap().last().unwrap().clone();
        assert!(last["parts"].as_array().unwrap().last().unwrap()["text"]
            .as_str()
            .unwrap()
            .contains("Call the tool again"));
    }

    #[tokio::test]
    async fn a_lost_signature_is_refused_as_google_does() {
        let (url, _) = strict_gemini(vec![]).await;
        let p = GeminiProvider::new(GeminiConfig { base_url: url, max_retries: 0, ..Default::default() }).unwrap();
        // 2.5 gets no placeholder, so the strict endpoint sees the call unsigned.
        let history = vec![
            Message::user(vec![ContentBlock::text("go")]),
            Message { role: Role::Assistant, content: vec![call("c1", None)] },
            Message::user(vec![result("c1")]),
        ];
        let e = run(&p, request("gemini-2.5-flash", history)).await.unwrap_err();
        assert!(e.to_string().contains("missing a thought_signature"), "{e}");
        assert!(!e.is_retryable());
    }

    /// Live: `KODACT_GEMINI_API_KEY=... cargo test -p kodact-api real_gemini -- --ignored`
    /// (`KODACT_GEMINI_MODEL` picks the model). One tool call and its answer,
    /// so the signature round trip is checked by Google itself.
    #[tokio::test]
    #[ignore]
    async fn real_gemini_tool_loop() {
        let key = std::env::var("KODACT_GEMINI_API_KEY").expect("KODACT_GEMINI_API_KEY");
        let model = std::env::var("KODACT_GEMINI_MODEL").unwrap_or_else(|_| "gemini-3-flash-preview".into());
        let base_url = std::env::var("KODACT_GEMINI_BASE_URL").unwrap_or_else(|_| GeminiConfig::default().base_url);
        let p = GeminiProvider::new(GeminiConfig { base_url, api_key: Some(key), ..Default::default() }).unwrap();
        let mut history = vec![Message::user(vec![ContentBlock::text(
            "Read the file a.rs with the Read tool, then say what it holds.",
        )])];
        let first = run(&p, request(&model, history.clone())).await.unwrap();
        let ids: Vec<String> = first
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        assert!(!ids.is_empty(), "no tool call: {:#?}", first.content);
        history.push(Message { role: Role::Assistant, content: first.content });
        history.push(Message::user(ids.iter().map(|i| result(i)).collect()));
        let second = run(&p, request(&model, history)).await.unwrap();
        assert!(second.content.iter().any(|b| b.as_text().is_some_and(|t| !t.is_empty())), "{:#?}", second.content);
    }
}
