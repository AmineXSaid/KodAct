//! Model providers.
//!
//! Every backend implements [`Provider`]: given a [`MessagesRequest`] it
//! yields the Messages API stream-event sequence (`message_start`, block
//! start/delta/stop, `message_delta`, `message_stop`). Other backends
//! translate into that sequence, so the agent loop sees one protocol.

pub mod accumulate;
pub mod auth;
pub mod gemini;
pub mod gemini_rules;
pub mod limit;
pub mod messages;
pub mod mock;
pub mod models;
pub mod openai;
pub mod pdf_text;
pub mod repair;
pub mod sse;

use std::pin::Pin;
use std::time::Duration;

use futures::Stream;
use kodact_types::{MessagesRequest, StreamEvent};
use tokio_util::sync::CancellationToken;

pub use accumulate::{MessageAccumulator, INVALID_INPUT, MAX_RAW_INPUT, RAW_INPUT, TRUNCATED_INPUT};
pub use gemini::{GeminiConfig, GeminiProvider};
pub use messages::{describe_network_error, MessagesConfig, MessagesProvider};
pub use mock::{MockProvider, MockTurn};
pub use models::{resolve_model, ModelInfo, ThinkingStyle};
pub use openai::{OpenAiConfig, OpenAiProvider};
pub use repair::repair_json;

pub type EventStream = Pin<Box<dyn Stream<Item = Result<StreamEvent, ApiError>> + Send>>;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("HTTP {status} ({kind}){}: {message}", from_url(url))]
    Http {
        status: u16,
        kind: String,
        message: String,
        retry_after: Option<Duration>,
        /// The URL that answered (no credentials or query), or empty.
        url: String,
        /// Which endpoint and key the request used, for the hint.
        origin: Option<Box<auth::Origin>>,
    },
    #[error("network error: {message}")]
    Network { message: String, origin: Option<Box<auth::Origin>> },
    #[error("stream error ({kind}): {message}")]
    Stream { kind: String, message: String },
    #[error("could not parse API response: {0}")]
    Parse(String),
    #[error("request cancelled")]
    Cancelled,
    #[error("no credentials: set KODACT_API_KEY or KODACT_AUTH_TOKEN")]
    MissingCredentials,
}

fn from_url(url: &str) -> String {
    if url.is_empty() {
        String::new()
    } else {
        format!(" from {url}")
    }
}

impl ApiError {
    /// A transport failure.
    pub fn network(message: impl Into<String>) -> Self {
        ApiError::Network { message: message.into(), origin: None }
    }

    /// Attach the endpoint a request went to, so the hint names the right variable.
    pub fn with_origin(mut self, o: auth::Origin) -> Self {
        match &mut self {
            ApiError::Http { origin, .. } | ApiError::Network { origin, .. } => *origin = Some(Box::new(o)),
            _ => {}
        }
        self
    }

    /// The endpoint refused the credentials (or none were set): the user must fix a key.
    pub fn is_auth_failure(&self) -> bool {
        if self.is_outage() {
            return false;
        }
        match self {
            ApiError::MissingCredentials => true,
            ApiError::Http { status, kind, .. } => {
                matches!(status, 401 | 403) || matches!(kind.as_str(), "authentication_error" | "permission_error")
            }
            _ => false,
        }
    }

    /// The endpoint is down for a while and says so (scheduled maintenance, an outage
    /// page): a 403 or 503 whose message says it is temporarily unavailable. Waiting can fix it.
    pub fn is_outage(&self) -> bool {
        match self {
            ApiError::Http { status: 403 | 503, message, .. } => {
                let m = message.to_ascii_lowercase();
                m.contains("maintenance") || m.contains("temporarily unavailable")
            }
            _ => false,
        }
    }

    /// The endpoint says the model doesn't exist. Fatal on a first call (a wrong name);
    /// in the middle of a run where the model has answered, the endpoint lost it for a moment.
    pub fn is_model_missing(&self) -> bool {
        match self {
            ApiError::Http { status: 400 | 404, kind, message, .. } => {
                kind == "model_not_found" || message.to_ascii_lowercase().contains("model not found")
            }
            _ => false,
        }
    }

    /// Whether retrying the same request may succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            ApiError::Http { status, .. } => matches!(status, 408 | 409 | 429 | 500..=599),
            ApiError::Network { .. } => true,
            ApiError::Stream { kind, .. } => kind == "overloaded_error" || kind == "api_error",
            _ => false,
        }
    }

    /// An error retrying won't fix: credentials, billing, a missing model.
    pub fn is_unrecoverable(&self) -> bool {
        if self.is_outage() {
            return false;
        }
        match self {
            ApiError::MissingCredentials => true,
            ApiError::Http { status, kind, message, .. } => {
                matches!(status, 401..=404)
                    || matches!(kind.as_str(), "authentication_error" | "permission_error" | "billing_error")
                    || message.to_ascii_lowercase().contains("credit balance")
            }
            _ => false,
        }
    }

    /// Overloaded (529) - the trigger for `--fallback-model`.
    pub fn is_overloaded(&self) -> bool {
        match self {
            ApiError::Http { status, kind, .. } => *status == 529 || kind == "overloaded_error",
            ApiError::Stream { kind, .. } => kind == "overloaded_error",
            _ => false,
        }
    }

    /// The next useful action for a person reading this error.
    pub fn hint(&self) -> Option<String> {
        let origin = match self {
            ApiError::Http { origin, .. } | ApiError::Network { origin, .. } => origin.as_deref().copied(),
            _ => None,
        };
        Some(match self {
            // Gemini refuses a bad key with a 400 (API_KEY_INVALID), named by its kind.
            e @ ApiError::Http { .. } if e.is_auth_failure() => match origin {
                Some(o) => o.auth_hint(),
                None => "Check the endpoint's key; `kodact doctor` shows what is configured.".into(),
            },
            ApiError::Http { status: 404, .. } => match origin {
                Some(o) => o.not_found_hint(),
                None => "Check the model name (--model or KODACT_MODEL) and the endpoint URL.".into(),
            },
            ApiError::Http { status: 413, .. } => {
                "The request is too large: run /compact or start a new session.".into()
            }
            ApiError::Http { status: 429, .. } => {
                "Rate limited: wait a moment and retry, or lower maxConcurrentRequests.".into()
            }
            e if e.is_overloaded() => "The API is overloaded: retry later, or pass --fallback-model.".into(),
            e if e.is_prompt_too_long() => "The conversation is too long: run /compact or start a new session.".into(),
            ApiError::Http { status: 500..=599, .. } => "The API failed on its side: retry in a moment.".into(),
            ApiError::Network { .. } => match origin {
                Some(o) => o.network_hint(),
                None => "Check the endpoint URL, your network connection and proxy settings.".into(),
            },
            ApiError::MissingCredentials => "Run `kodact doctor` to see what is configured.".into(),
            _ => return None,
        })
    }

    /// One line for people: what failed, the cause, and what to do next.
    pub fn describe(&self) -> String {
        match self.hint() {
            Some(h) => format!("{self}. {h}"),
            None => self.to_string(),
        }
    }

    /// The prompt does not fit the context window, in the words different
    /// servers use (Messages API, OpenAI, vLLM, llama.cpp, LiteLLM, gateways).
    pub fn is_prompt_too_long(&self) -> bool {
        const PHRASES: &[&str] = &[
            "prompt is too long",
            "context window",
            "too many tokens",
            "maximum context length",
            "context length",
            "context_length_exceeded",
            "exceeds the context",
            "exceed context",
            "input is too long",
            "reduce the length",
            "too large for the model",
            "available context size", // llama.cpp
        ];
        match self {
            ApiError::Http { status: 400 | 413 | 422, kind, message, .. } => {
                let m = message.to_ascii_lowercase();
                kind == "context_length_exceeded" || PHRASES.iter().any(|p| m.contains(p))
            }
            ApiError::Stream { message, .. } => {
                let m = message.to_ascii_lowercase();
                PHRASES.iter().any(|p| m.contains(p))
            }
            _ => false,
        }
    }

    /// What a refusal for length says about the model: its context window and the size of the
    /// prompt it refused, when the server states them. vLLM and OpenAI: "maximum context length
    /// is 100000 tokens ... your prompt contains at least 68001 input tokens" (or "your messages
    /// resulted in N tokens"); llama.cpp: "request (N tokens) exceeds the available context size
    /// (M tokens)"; the Messages API: "prompt is too long: N tokens > M maximum". Gateways such as
    /// LiteLLM pass these words on. Any size is read; nothing is assumed about the server.
    pub fn reported_limits(&self) -> ReportedLimits {
        if !self.is_prompt_too_long() {
            return ReportedLimits::default();
        }
        let m = match self {
            ApiError::Http { message, .. } | ApiError::Stream { message, .. } => message.to_ascii_lowercase(),
            _ => return ReportedLimits::default(),
        };
        let pick = |pats: &[(&str, &str)]| pats.iter().find_map(|(before, after)| number_between(&m, before, after));
        let window = pick(&[
            ("maximum context length is ", "token"),
            ("maximum context length of ", "token"),
            ("available context size (", "token"),
            ("context window of ", "token"),
            (" tokens > ", "maximum"),
        ])
        .filter(|w| (1_000..=1_000_000_000).contains(w));
        let prompt = pick(&[
            ("prompt contains at least ", "input token"),
            ("your messages resulted in ", "token"),
            ("request (", "token"),
            ("prompt is too long: ", "token"),
        ]);
        ReportedLimits { window, prompt }
    }
}

/// See [`ApiError::reported_limits`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ReportedLimits {
    pub window: Option<u64>,
    pub prompt: Option<u64>,
}

/// The whole number right after `before` in `m`, when `after` follows it (spaces, commas and
/// underscores inside the number are skipped).
fn number_between(m: &str, before: &str, after: &str) -> Option<u64> {
    let mut rest = m;
    while let Some(i) = rest.find(before) {
        let tail = &rest[i + before.len()..];
        let len = tail.find(|c: char| !(c.is_ascii_digit() || c == ',' || c == '_')).unwrap_or(tail.len());
        let digits: String = tail[..len].chars().filter(char::is_ascii_digit).collect();
        if !digits.is_empty() && tail[len..].trim_start().starts_with(after) {
            return digits.parse().ok();
        }
        rest = tail;
    }
    None
}

/// A source of model turns.
/// Run one request to completion and return the whole message (for side
/// requests such as summaries; the agent loop streams instead).
pub async fn complete(
    provider: &dyn Provider,
    request: MessagesRequest,
    cancel: &CancellationToken,
) -> Result<kodact_types::ApiMessage, ApiError> {
    use futures::StreamExt;
    let mut stream = provider.stream(request, cancel.clone()).await?;
    let mut acc = MessageAccumulator::new();
    loop {
        let next = tokio::select! {
            n = stream.next() => n,
            _ = cancel.cancelled() => return Err(ApiError::Cancelled),
        };
        match next {
            Some(Ok(ev)) => acc.push(&ev)?,
            Some(Err(e)) => return Err(e),
            None => break,
        }
    }
    acc.finish()
}

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    /// Short identifier (`messages`, `openai`, `mock`).
    fn name(&self) -> &str;

    /// Start a streaming request. Implementations retry transient failures
    /// that happen before the first event; once events flow, errors surface
    /// in the stream.
    async fn stream(&self, request: MessagesRequest, cancel: CancellationToken) -> Result<EventStream, ApiError>;

    /// Count input tokens for a request, if the backend supports it.
    async fn count_tokens(&self, _request: &MessagesRequest) -> Option<u64> {
        None
    }

    /// Does a request send the model's earlier reasoning back to it? Then a reply's reasoning
    /// takes room in the next request. The Messages API and Gemini send it back; chat
    /// completions don't (except to Gemini), so there the reasoning is gone after its reply.
    fn resends_reasoning(&self, _model: &str) -> bool {
        true
    }

    /// The models this endpoint offers, for `/model`. `None`: it doesn't
    /// list them (any model id can still be set by hand).
    /// `kodact doctor --probe`: one authenticated request that changes nothing
    /// (the model list); `Some(Ok(n))` with the number of models listed.
    async fn probe(&self) -> Option<Result<usize, ApiError>> {
        self.list_models().await.map(|r| r.map(|m| m.len()))
    }

    /// The endpoint and where its key came from (`None` for test providers).
    fn origin(&self) -> Option<auth::Origin> {
        None
    }

    /// The base URL requests go to, when it isn't the provider's default.
    fn base_url(&self) -> Option<String> {
        None
    }

    async fn list_models(&self) -> Option<Result<Vec<String>, ApiError>> {
        None
    }
}

/// Exponential backoff with an upper bound; honours `retry-after` when given.
pub fn backoff_delay(attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(d) = retry_after {
        return d.min(Duration::from_secs(60));
    }
    let base = 500u64.saturating_mul(1u64 << attempt.min(6));
    Duration::from_millis(base.min(32_000))
}

#[cfg(test)]
mod outage_tests {
    use super::{ApiError, ReportedLimits};

    fn http(status: u16, message: &str) -> ApiError {
        ApiError::Http {
            status,
            kind: "error".into(),
            message: message.into(),
            retry_after: None,
            url: String::new(),
            origin: None,
        }
    }

    /// The window and prompt size a refusal states, in each server's words, at any size.
    #[test]
    fn a_refusal_for_length_gives_the_window_and_the_prompt() {
        // The run of 2026-11-10 (path-tracing): LiteLLM passing on vLLM's refusal, escaped.
        let litellm = r#"{"detail":"litellm.ContextWindowExceededError: litellm.BadRequestError: ContextWindowExceededError: Hosted_vllmException - {\"error\":{\"message\":\"This model's maximum context length is 100000 tokens. However, you requested 32000 output tokens and your prompt contains at least 68001 input tokens, for a total of at least 100001 tokens. Please reduce the length of the input prompt or the number of requested output tokens. (parameter=input_tokens, value=68001)\"}}"}"#;
        assert_eq!(
            http(400, litellm).reported_limits(),
            ReportedLimits { window: Some(100_000), prompt: Some(68_001) }
        );
        let openai = "This model's maximum context length is 128000 tokens. However, your messages resulted in 130512 tokens. Please reduce the length of the messages.";
        assert_eq!(
            http(400, openai).reported_limits(),
            ReportedLimits { window: Some(128_000), prompt: Some(130_512) }
        );
        let llama = "the request exceeds the available context size, try increasing it: request (8193 tokens) exceeds the available context size (8192 tokens)";
        assert_eq!(http(400, llama).reported_limits(), ReportedLimits { window: Some(8_192), prompt: Some(8_193) });
        let messages = "prompt is too long: 1000001 tokens > 1000000 maximum";
        assert_eq!(
            http(400, messages).reported_limits(),
            ReportedLimits { window: Some(1_000_000), prompt: Some(1_000_001) }
        );
        let huge = "This model's maximum context length is 10,000,000 tokens. However, your messages resulted in 10,000,500 tokens.";
        assert_eq!(http(400, huge).reported_limits().window, Some(10_000_000));
        // A refusal that names no size, and errors of another kind, say nothing.
        assert_eq!(http(400, "input is too long for requested model").reported_limits(), ReportedLimits::default());
        assert_eq!(http(500, openai).reported_limits(), ReportedLimits::default());
        assert_eq!(
            http(400, "maximum context length is 12 tokens").reported_limits().window,
            None,
            "not a real window"
        );
    }

    #[test]
    fn maintenance_is_an_outage_not_a_refused_key() {
        let m = http(403, "This service is temporarily unavailable while we perform scheduled maintenance");
        assert!(m.is_outage() && !m.is_auth_failure() && !m.is_unrecoverable());
        let key = http(403, "Forbidden");
        assert!(!key.is_outage() && key.is_auth_failure() && key.is_unrecoverable());
        assert!(http(503, "Service temporarily unavailable").is_outage());
        let gone = http(400, r#"{"detail":"Model not found"}"#);
        assert!(
            gone.is_model_missing() && !gone.is_retryable(),
            "not retried unless the engine knows the model answered"
        );
        assert!(!http(400, "bad request").is_model_missing());
    }
}
