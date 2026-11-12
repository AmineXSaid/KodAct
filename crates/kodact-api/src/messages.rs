//! The Messages API over HTTPS with SSE streaming.

use std::time::Duration;

use futures::StreamExt;
use kodact_types::{MessagesRequest, StreamEvent};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_TYPE};
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use crate::sse::SseDecoder;
use crate::{backoff_delay, ApiError, EventStream, Provider};

// Wire constants required by the Messages API: its default host, its version
// header and value, and its beta header. They are protocol, not branding.
pub const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub const API_VERSION: &str = "2023-06-01";
const VERSION_HEADER: &str = "anthropic-version";
const BETA_HEADER: &str = "anthropic-beta";

const KNOWN_EVENTS: &[&str] = &[
    "message_start",
    "content_block_start",
    "content_block_delta",
    "content_block_stop",
    "message_delta",
    "message_stop",
    "ping",
    "error",
];

#[derive(Debug, Clone)]
pub struct MessagesConfig {
    pub base_url: String,
    /// Sent as `x-api-key`.
    pub api_key: Option<String>,
    /// Sent as `Authorization: Bearer` (gateways, proxies).
    pub auth_token: Option<String>,
    pub betas: Vec<String>,
    pub extra_headers: Vec<(String, String)>,
    pub max_retries: u32,
    /// Longest silence allowed between reads of a response.
    pub timeout: Duration,
    pub user_agent: String,
    /// The variable or setting that chose `base_url` (`None`: the default host).
    pub url_from: Option<&'static str>,
    /// Set when `api_key` came from a helper command (its setting name).
    pub key_helper: Option<&'static str>,
}

impl Default for MessagesConfig {
    fn default() -> Self {
        MessagesConfig {
            base_url: DEFAULT_BASE_URL.into(),
            api_key: None,
            auth_token: None,
            betas: vec![],
            extra_headers: vec![],
            max_retries: 3,
            // No total deadline (long generations stream for many minutes); a stall of this long between reads fails the request.
            timeout: Duration::from_secs(300),
            user_agent: format!("kodact/{}", env!("CARGO_PKG_VERSION")),
            url_from: None,
            key_helper: None,
        }
    }
}

impl MessagesConfig {
    /// Read `KODACT_BASE_URL`, `KODACT_API_KEY`, `KODACT_AUTH_TOKEN`,
    /// `KODACT_CUSTOM_HEADERS` ("Name: value" per line) and `KODACT_MAX_RETRIES`.
    pub fn from_env() -> Self {
        let mut c = MessagesConfig::default();
        let get = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        if let Some(u) = get("KODACT_BASE_URL") {
            c.base_url = u;
            c.url_from = Some("KODACT_BASE_URL");
        }
        c.api_key = get("KODACT_API_KEY");
        c.auth_token = get("KODACT_AUTH_TOKEN");
        if let Some(h) = get("KODACT_CUSTOM_HEADERS") {
            c.extra_headers = parse_header_lines(&h);
        }
        if let Some(n) = get("KODACT_MAX_RETRIES").and_then(|v| v.parse().ok()) {
            c.max_retries = n;
        }
        c
    }

    pub fn has_credentials(&self) -> bool {
        self.api_key.is_some() || self.auth_token.is_some()
    }

    /// The endpoint and where its key came from, for error hints.
    pub fn origin(&self) -> crate::auth::Origin {
        use crate::auth::KeyFrom;
        let key_from = match (self.key_helper, self.api_key.is_some(), self.auth_token.is_some()) {
            (Some(h), true, _) => KeyFrom::Helper(h),
            (_, true, true) => KeyFrom::ApiKeyAndToken,
            (_, true, false) => KeyFrom::ApiKey,
            (_, false, true) => KeyFrom::AuthToken,
            (_, false, false) => KeyFrom::None,
        };
        crate::auth::Origin { backend: crate::auth::Backend::Messages, url_from: self.url_from, key_from }
    }

    /// Where the credential came from, for `system/init.apiKeySource`.
    pub fn key_source(&self) -> &'static str {
        self.origin().api_key_source()
    }
}

pub fn parse_header_lines(raw: &str) -> Vec<(String, String)> {
    raw.lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, _)| !k.is_empty())
        .collect()
}

pub struct MessagesProvider {
    config: MessagesConfig,
    http: reqwest::Client,
}

impl MessagesProvider {
    pub fn new(config: MessagesConfig) -> Result<Self, ApiError> {
        let http = reqwest::Client::builder()
            .user_agent(config.user_agent.clone())
            .connect_timeout(Duration::from_secs(30))
            .read_timeout(config.timeout)
            .build()
            .map_err(|e| ApiError::network(e.to_string()))?;
        Ok(MessagesProvider { config, http })
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{}", self.config.base_url.trim_end_matches('/'), path)
    }

    fn headers(&self, extra_betas: &[String]) -> Result<HeaderMap, ApiError> {
        let mut h = HeaderMap::new();
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        h.insert(VERSION_HEADER, HeaderValue::from_static(API_VERSION));
        if let Some(k) = &self.config.api_key {
            h.insert("x-api-key", header_value(k)?);
        }
        if let Some(t) = &self.config.auth_token {
            h.insert(reqwest::header::AUTHORIZATION, header_value(&format!("Bearer {t}"))?);
        }
        let mut betas = self.config.betas.clone();
        let mut add = |b: &str| {
            let b = b.trim();
            if !b.is_empty() && !betas.iter().any(|x| x == b) {
                betas.push(b.to_string());
            }
        };
        extra_betas.iter().for_each(|b| add(b));
        // A custom beta header joins the list instead of replacing it (fast mode needs its beta).
        for (k, v) in &self.config.extra_headers {
            if k.eq_ignore_ascii_case(BETA_HEADER) {
                v.split(',').for_each(&mut add);
            }
        }
        if !betas.is_empty() {
            h.insert(BETA_HEADER, header_value(&betas.join(","))?);
        }
        for (k, v) in &self.config.extra_headers {
            if k.eq_ignore_ascii_case(BETA_HEADER) {
                continue;
            }
            let name = HeaderName::from_bytes(k.as_bytes()).map_err(|e| ApiError::Parse(e.to_string()))?;
            h.insert(name, header_value(v)?);
        }
        Ok(h)
    }

    async fn send_once(&self, body: &Value, betas: &[String]) -> Result<reqwest::Response, ApiError> {
        let resp = self
            .http
            .post(self.endpoint("/v1/messages"))
            .headers(self.headers(betas)?)
            .json(body)
            .send()
            .await
            .map_err(network_error)?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        Err(http_error(resp).await)
    }
}

/// A transport failure with its root cause ("connection refused", "timed out", ...).
pub(crate) fn network_error(e: reqwest::Error) -> ApiError {
    ApiError::network(describe_network_error(&e))
}

/// "could not connect to <url>: <root cause>", for any reqwest failure.
pub fn describe_network_error(e: &reqwest::Error) -> String {
    let mut msg = if e.is_timeout() {
        "request timed out".to_string()
    } else if e.is_connect() {
        "could not connect".to_string()
    } else {
        "request failed".to_string()
    };
    if let Some(url) = e.url() {
        msg.push_str(&format!(" to {}", crate::auth::display_url(url.as_str())));
    }
    let mut source = std::error::Error::source(e);
    let mut last = None;
    while let Some(s) = source {
        last = Some(s.to_string());
        source = s.source();
    }
    if let Some(cause) = last {
        msg.push_str(&format!(": {cause}"));
    }
    msg
}

fn header_value(v: &str) -> Result<HeaderValue, ApiError> {
    HeaderValue::from_str(v).map_err(|e| ApiError::Parse(format!("invalid header value: {e}")))
}

/// Turn a non-2xx response into an [`ApiError::Http`].
/// Take out what other providers attached to blocks (`provider`): this wire's
/// API rejects fields it doesn't know.
pub(crate) fn strip_provider_data(body: &mut Value) {
    for m in body.get_mut("messages").and_then(Value::as_array_mut).into_iter().flatten() {
        for b in m.get_mut("content").and_then(Value::as_array_mut).into_iter().flatten() {
            if let Some(o) = b.as_object_mut() {
                o.remove("provider");
            }
        }
    }
}

/// The wait an error body states in words or in a `retryDelay` field: `1h36m48.5s`, `6m0s`,
/// `12s`, `350ms`. `None` when it states none, so the usual backoff applies.
pub(crate) fn stated_wait(text: &str) -> Option<Duration> {
    let lower = text.to_ascii_lowercase();
    let at = ["retry in ", "try again in ", "retrydelay\": \"", "retrydelay\":\""]
        .iter()
        .find_map(|k| lower.find(k).map(|i| i + k.len()))?;
    let (mut total, mut number, mut any) = (0f64, String::new(), false);
    let mut chars = lower[at..].chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '0'..='9' | '.' => number.push(c),
            'h' | 'm' | 's' if !number.is_empty() => {
                let n: f64 = number.trim_end_matches('.').parse().ok()?;
                number.clear();
                any = true;
                total += match c {
                    'h' => n * 3600.0,
                    'm' if chars.peek() == Some(&'s') => {
                        chars.next();
                        n / 1000.0
                    }
                    'm' => n * 60.0,
                    _ => n,
                };
            }
            _ => break,
        }
    }
    (any && total.is_finite() && total >= 0.0).then(|| Duration::from_secs_f64(total))
}

pub(crate) async fn http_error(resp: reqwest::Response) -> ApiError {
    let status = resp.status().as_u16();
    let url = crate::auth::display_url(resp.url().as_str());
    let retry_after = resp
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<f64>().ok())
        .map(Duration::from_secs_f64);
    let text = resp.text().await.unwrap_or_default();
    // No header, but the body says how long ("Please retry in 1h36m48.5s", `"retryDelay": "20s"`):
    // a gateway in front of another provider often passes only the body on.
    let retry_after = retry_after.or_else(|| stated_wait(&text));
    let (kind, message) = match serde_json::from_str::<Value>(&text) {
        Ok(v) => (
            v.pointer("/error/type").and_then(Value::as_str).unwrap_or("error").to_string(),
            v.pointer("/error/message").and_then(Value::as_str).map(str::to_string).unwrap_or(text.clone()),
        ),
        Err(_) => ("error".to_string(), text),
    };
    ApiError::Http { status, kind, message, retry_after, url, origin: None }
}

/// Parse one SSE payload; `Ok(None)` for event types this client does not know.
pub fn parse_event(data: &str) -> Result<Option<StreamEvent>, ApiError> {
    let v: Value = serde_json::from_str(data).map_err(|e| ApiError::Parse(format!("{e}: {data}")))?;
    let kind = v.get("type").and_then(Value::as_str).unwrap_or_default().to_string();
    if !KNOWN_EVENTS.contains(&kind.as_str()) {
        tracing::debug!(kind = %kind, "skipping unknown stream event");
        return Ok(None);
    }
    serde_json::from_value(v).map(Some).map_err(|e| ApiError::Parse(format!("{kind}: {e}")))
}

/// Pump an SSE byte stream into a channel of [`StreamEvent`]s.
pub(crate) fn spawn_sse_pump<F>(resp: reqwest::Response, cancel: CancellationToken, mut parse: F) -> EventStream
where
    F: FnMut(&crate::sse::SseEvent) -> Vec<Result<StreamEvent, ApiError>> + Send + 'static,
{
    let (tx, rx) = mpsc::channel::<Result<StreamEvent, ApiError>>(64);
    tokio::spawn(async move {
        let mut body = resp.bytes_stream();
        let mut decoder = SseDecoder::new();
        loop {
            let chunk = tokio::select! {
                _ = cancel.cancelled() => {
                    let _ = tx.send(Err(ApiError::Cancelled)).await;
                    return;
                }
                c = body.next() => c,
            };
            let (events, finished) = match chunk {
                Some(Ok(bytes)) => (decoder.push(&bytes), false),
                Some(Err(e)) => {
                    let _ = tx.send(Err(ApiError::network(e.to_string()))).await;
                    return;
                }
                None => (decoder.finish().into_iter().collect(), true),
            };
            for ev in &events {
                for item in parse(ev) {
                    if tx.send(item).await.is_err() {
                        return;
                    }
                }
            }
            if finished {
                return;
            }
        }
    });
    Box::pin(ReceiverStream::new(rx))
}

#[async_trait::async_trait]
impl Provider for MessagesProvider {
    fn name(&self) -> &str {
        "messages"
    }

    fn origin(&self) -> Option<crate::auth::Origin> {
        Some(self.config.origin())
    }

    async fn probe(&self) -> Option<Result<usize, ApiError>> {
        let r = async {
            let res = self
                .http
                .get(self.endpoint("/v1/models"))
                .headers(self.headers(&[])?)
                .timeout(Duration::from_secs(10))
                .send()
                .await
                .map_err(network_error)?;
            if !res.status().is_success() {
                return Err(http_error(res).await);
            }
            let body: Value = res.json().await.map_err(|e| ApiError::Parse(e.to_string()))?;
            Ok(body.get("data").and_then(Value::as_array).map(Vec::len).unwrap_or(0))
        }
        .await;
        Some(r.map_err(|e| e.with_origin(self.config.origin())))
    }

    fn base_url(&self) -> Option<String> {
        self.config.url_from.map(|_| crate::auth::display_url(&self.config.base_url))
    }

    async fn stream(&self, mut request: MessagesRequest, cancel: CancellationToken) -> Result<EventStream, ApiError> {
        if !self.config.has_credentials() {
            return Err(ApiError::MissingCredentials);
        }
        let origin = self.config.origin();
        request.stream = true;
        let mut body = serde_json::to_value(&request).map_err(|e| ApiError::Parse(e.to_string()))?;
        strip_provider_data(&mut body);
        let mut attempt = 0;
        let resp = loop {
            let res = tokio::select! {
                _ = cancel.cancelled() => return Err(ApiError::Cancelled),
                r = self.send_once(&body, &request.betas) => r,
            };
            match res {
                Ok(r) => break r,
                Err(e) if e.retries_quickly() && attempt < self.config.max_retries => {
                    let wait = backoff_delay(
                        attempt,
                        match &e {
                            ApiError::Http { retry_after, .. } => *retry_after,
                            _ => None,
                        },
                    );
                    tracing::warn!(attempt, ?wait, error = %e, "retrying API request");
                    attempt += 1;
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(ApiError::Cancelled),
                        _ = tokio::time::sleep(wait) => {}
                    }
                }
                Err(e) => return Err(e.with_origin(origin)),
            }
        };
        Ok(spawn_sse_pump(resp, cancel, |ev| match parse_event(&ev.data) {
            Ok(Some(e)) => vec![Ok(e)],
            Ok(None) => vec![],
            Err(e) => vec![Err(e)],
        }))
    }

    async fn count_tokens(&self, request: &MessagesRequest) -> Option<u64> {
        let mut body = serde_json::to_value(request).ok()?;
        strip_provider_data(&mut body);
        let obj = body.as_object_mut()?;
        for k in ["max_tokens", "stream", "metadata", "temperature"] {
            obj.remove(k);
        }
        let resp = self
            .http
            .post(self.endpoint("/v1/messages/count_tokens"))
            .headers(self.headers(&request.betas).ok()?)
            .json(&body)
            .send()
            .await
            .ok()?;
        let v: Value = resp.json().await.ok()?;
        v.get("input_tokens").and_then(Value::as_u64)
    }
}

#[cfg(test)]
mod provider_data_tests {
    use kodact_types::{ContentBlock, Message, MessagesRequest};
    use serde_json::json;

    #[test]
    fn provider_data_stays_in_transcripts_but_never_reaches_this_wire() {
        let call = ContentBlock::ToolUse {
            id: "toolu_1".into(),
            name: "Read".into(),
            input: json!({"file_path": "/a"}),
            cache_control: None,
            provider: Some(json!({"thoughtSignature": "c2ln"})),
        };
        // A transcript keeps it, and reads it back.
        let saved = serde_json::to_string(&call).unwrap();
        assert!(saved.contains("thoughtSignature"));
        assert_eq!(serde_json::from_str::<ContentBlock>(&saved).unwrap(), call);
        // An old transcript without it still reads.
        let old: ContentBlock =
            serde_json::from_value(json!({"type": "tool_use", "id": "t", "name": "Read", "input": {}})).unwrap();
        assert!(matches!(old, ContentBlock::ToolUse { provider: None, .. }));
        // The request body this wire sends has none.
        let req = MessagesRequest {
            model: "m".into(),
            max_tokens: 10,
            messages: vec![Message::assistant(vec![call])],
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
        let mut body = serde_json::to_value(&req).unwrap();
        super::strip_provider_data(&mut body);
        assert!(!body.to_string().contains("provider") && body.to_string().contains("toolu_1"));
    }

    /// A gateway (OmniRoute) in front of Gemini on this wire carries Gemini's
    /// signature as the thinking block's: it and the gateway's tool ids go back as they came.
    #[test]
    fn thinking_signatures_and_tool_ids_go_back_unchanged() {
        let turn = Message::assistant(vec![
            ContentBlock::Thinking { thinking: "Read it first.".into(), signature: "R0VNSU5JX1NJRw==".into() },
            ContentBlock::ToolUse {
                id: "toolu_vrtx_01Gx".into(),
                name: "Read".into(),
                input: json!({"file_path": "/a"}),
                cache_control: None,
                provider: None,
            },
        ]);
        let req = MessagesRequest {
            model: "gemini-3-flash".into(),
            max_tokens: 10,
            messages: vec![Message::user_text("go"), turn],
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
        let mut body = serde_json::to_value(&req).unwrap();
        super::strip_provider_data(&mut body);
        assert_eq!(
            body["messages"][1]["content"],
            json!([
                {"type": "thinking", "thinking": "Read it first.", "signature": "R0VNSU5JX1NJRw=="},
                {"type": "tool_use", "id": "toolu_vrtx_01Gx", "name": "Read", "input": {"file_path": "/a"}}
            ])
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_events_are_skipped() {
        assert!(parse_event(r#"{"type":"something_new","x":1}"#).unwrap().is_none());
        assert!(matches!(parse_event(r#"{"type":"ping"}"#).unwrap(), Some(StreamEvent::Ping)));
    }

    #[test]
    fn custom_headers_parse() {
        let h = parse_header_lines("X-A: 1\nbad line\nX-B:two");
        assert_eq!(h, vec![("X-A".into(), "1".into()), ("X-B".into(), "two".into())]);
    }

    #[test]
    fn a_custom_beta_header_joins_the_beta_list() {
        let p = MessagesProvider::new(MessagesConfig {
            betas: vec!["from-flag".into()],
            extra_headers: vec![
                ("Anthropic-Beta".into(), "long-context, from-flag".into()),
                ("X-A".into(), "1".into()),
            ],
            ..Default::default()
        })
        .unwrap();
        let h = p.headers(&["fast-mode-2026-02-01".into()]).unwrap();
        assert_eq!(h.get(BETA_HEADER).unwrap(), "from-flag,fast-mode-2026-02-01,long-context");
        assert_eq!(h.get_all(BETA_HEADER).iter().count(), 1);
        assert_eq!(h.get("x-a").unwrap(), "1");
    }
}

#[cfg(test)]
mod stated_wait_tests {
    use super::stated_wait;
    use std::time::Duration;

    #[test]
    fn a_wait_stated_in_the_body_is_read() {
        let day = "Quota exceeded for metric: requests per day limit exceeded on Free Tier.\nPlease retry in 1h36m48.519213421s.";
        assert_eq!(stated_wait(day).unwrap().as_secs(), 5808);
        assert_eq!(stated_wait("Rate limit reached. Please try again in 6m0s."), Some(Duration::from_secs(360)));
        assert_eq!(stated_wait("please retry in 12s"), Some(Duration::from_secs(12)));
        assert_eq!(stated_wait("Try again in 350ms"), Some(Duration::from_millis(350)));
        assert_eq!(stated_wait(r#"{"details": [{"retryDelay": "20s"}]}"#), Some(Duration::from_secs(20)));
        assert_eq!(stated_wait("Rate limit reached, retry in a moment"), None);
        assert_eq!(stated_wait("overloaded"), None);
    }
}
