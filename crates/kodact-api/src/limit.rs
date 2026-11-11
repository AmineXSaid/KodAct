//! One limit on concurrent model requests for the whole session: the main
//! agent, sub-agents, compaction, goal checks and side questions all share it.
//!
//! A request waits for a free slot instead of failing. When the endpoint
//! answers 429 (too many requests), the limit halves (never below 1), the
//! request waits and is sent again, honouring `Retry-After`; after a run of
//! successes the limit grows back by one, up to the configured maximum.

use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use futures::Stream;
use kodact_types::{MessagesRequest, StreamEvent};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use crate::{ApiError, EventStream, Provider};

/// Requests in a row that must succeed before the limit grows by one.
const GROW_AFTER: u32 = 8;
/// Extra attempts for a rate-limited request, on top of the provider's own retries.
const RATE_LIMIT_RETRIES: u32 = 6;
/// The longest wait between attempts.
const MAX_WAIT: Duration = Duration::from_secs(60);

/// Something to tell the person: the limit changed, or a request waits for a slot.
pub type Notifier = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Debug)]
struct State {
    in_flight: usize,
    limit: usize,
    max: usize,
    successes: u32,
    waiting: usize,
}

/// The shared limit.
pub struct Limiter {
    state: Mutex<State>,
    freed: Notify,
}

impl Limiter {
    pub fn new(max: usize) -> Arc<Self> {
        let max = max.max(1);
        Arc::new(Limiter {
            state: Mutex::new(State { in_flight: 0, limit: max, max, successes: 0, waiting: 0 }),
            freed: Notify::new(),
        })
    }

    /// The current limit (it may be below the maximum after 429s).
    pub fn limit(&self) -> usize {
        self.state.lock().unwrap().limit
    }

    /// Requests waiting for a slot right now.
    pub fn waiting(&self) -> usize {
        self.state.lock().unwrap().waiting
    }

    async fn acquire(self: &Arc<Self>, cancel: &CancellationToken) -> Result<Slot, ApiError> {
        let mut counted = false;
        loop {
            let notified = self.freed.notified();
            {
                let mut s = self.state.lock().unwrap();
                if s.in_flight < s.limit {
                    s.in_flight += 1;
                    if counted {
                        s.waiting -= 1;
                    }
                    return Ok(Slot(self.clone()));
                }
                if !counted {
                    s.waiting += 1;
                    counted = true;
                }
            }
            tokio::select! {
                _ = cancel.cancelled() => {
                    self.state.lock().unwrap().waiting -= 1;
                    return Err(ApiError::Cancelled);
                }
                _ = notified => {}
            }
        }
    }

    /// The endpoint said "too many requests": halve the limit. Returns the new
    /// limit when it changed.
    fn rate_limited(&self) -> Option<usize> {
        let mut s = self.state.lock().unwrap();
        s.successes = 0;
        // Halve what is actually in use, so a burst of 429s doesn't drop it to 1 at once.
        let next = (s.limit.min(s.in_flight.max(1)) / 2).max(1);
        (next < s.limit).then(|| {
            s.limit = next;
            next
        })
    }

    fn succeeded(&self) {
        let mut s = self.state.lock().unwrap();
        s.successes += 1;
        if s.limit < s.max && s.successes >= GROW_AFTER {
            s.limit += 1;
            s.successes = 0;
            drop(s);
            self.freed.notify_waiters();
        }
    }
}

/// A taken slot; freed when the response stream ends or is dropped.
struct Slot(Arc<Limiter>);

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().in_flight -= 1;
        self.0.freed.notify_waiters();
    }
}

/// A response stream that holds its slot until it ends.
struct Held {
    inner: EventStream,
    _slot: Slot,
}

impl Stream for Held {
    type Item = Result<StreamEvent, ApiError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// A provider whose requests share a [`Limiter`].
pub struct LimitedProvider {
    inner: Arc<dyn Provider>,
    limiter: Arc<Limiter>,
    notify: Option<Notifier>,
}

impl LimitedProvider {
    pub fn new(inner: Arc<dyn Provider>, limiter: Arc<Limiter>, notify: Option<Notifier>) -> Self {
        LimitedProvider { inner, limiter, notify }
    }

    fn tell(&self, text: String) {
        tracing::warn!("{text}");
        if let Some(n) = &self.notify {
            n(text);
        }
    }
}

/// How long to wait before attempt `n` (0-based) of a rate-limited request.
fn wait_for(attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(r) = retry_after {
        return r.min(MAX_WAIT);
    }
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let base = Duration::from_secs(2u64.saturating_pow(attempt + 1));
    // Spread the retries of requests that failed together.
    let n = N.fetch_add(1, Ordering::Relaxed);
    let jitter = Duration::from_millis(n.wrapping_mul(2_654_435_761) % 1000);
    (base + jitter).min(MAX_WAIT)
}

#[async_trait::async_trait]
impl Provider for LimitedProvider {
    fn name(&self) -> &str {
        self.inner.name()
    }

    fn resends_reasoning(&self, model: &str) -> bool {
        self.inner.resends_reasoning(model)
    }

    async fn stream(&self, request: MessagesRequest, cancel: CancellationToken) -> Result<EventStream, ApiError> {
        let mut attempt = 0;
        loop {
            let slot = self.limiter.acquire(&cancel).await?;
            match self.inner.stream(request.clone(), cancel.clone()).await {
                Ok(s) => {
                    self.limiter.succeeded();
                    return Ok(Box::pin(Held { inner: s, _slot: slot }));
                }
                Err(e @ ApiError::Http { status: 429, .. }) if attempt < RATE_LIMIT_RETRIES && e.retries_quickly() => {
                    let retry_after = match &e {
                        ApiError::Http { retry_after, .. } => *retry_after,
                        _ => None,
                    };
                    let wait = wait_for(attempt, retry_after);
                    let lowered = self.limiter.rate_limited();
                    drop(slot);
                    match lowered {
                        Some(n) => self.tell(format!(
                            "The endpoint is limiting requests (HTTP 429). KodAct now sends at most {n} at a time \
                             and retries in {}s.",
                            wait.as_secs().max(1)
                        )),
                        None => tracing::warn!(attempt, ?wait, "rate limited; retrying"),
                    }
                    attempt += 1;
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(ApiError::Cancelled),
                        _ = tokio::time::sleep(wait) => {}
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn count_tokens(&self, request: &MessagesRequest) -> Option<u64> {
        self.inner.count_tokens(request).await
    }

    async fn list_models(&self) -> Option<Result<Vec<String>, ApiError>> {
        self.inner.list_models().await
    }

    async fn probe(&self) -> Option<Result<usize, ApiError>> {
        self.inner.probe().await
    }

    fn origin(&self) -> Option<crate::auth::Origin> {
        self.inner.origin()
    }

    fn base_url(&self) -> Option<String> {
        self.inner.base_url()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MockProvider, MockTurn};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn requests_wait_for_a_slot() {
        let l = Limiter::new(2);
        let c = CancellationToken::new();
        let a = l.acquire(&c).await.unwrap();
        let _b = l.acquire(&c).await.unwrap();
        let l2 = l.clone();
        let waiter = tokio::spawn(async move {
            let c = CancellationToken::new();
            l2.acquire(&c).await.map(|_| ()).is_ok()
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(l.waiting(), 1, "the third waits");
        drop(a);
        assert!(waiter.await.unwrap(), "and gets the freed slot");
        assert_eq!(l.waiting(), 0);
    }

    #[test]
    fn the_limit_halves_on_429_and_grows_back() {
        let l = Limiter::new(8);
        l.state.lock().unwrap().in_flight = 6;
        assert_eq!(l.rate_limited(), Some(3));
        l.state.lock().unwrap().in_flight = 3;
        assert_eq!(l.rate_limited(), Some(1));
        assert_eq!(l.rate_limited(), None, "never below 1");
        for _ in 0..GROW_AFTER {
            l.succeeded();
        }
        assert_eq!(l.limit(), 2);
    }

    #[tokio::test(start_paused = true)]
    async fn a_rate_limited_request_is_retried_with_a_lower_limit() {
        let inner = Arc::new(MockProvider::new(vec![
            MockTurn::http_error(429, "rate_limit_error"),
            MockTurn::http_error(429, "rate_limit_error"),
            MockTurn::text("finally"),
        ]));
        let told = Arc::new(AtomicUsize::new(0));
        let t = told.clone();
        let notify: Notifier = Arc::new(move |_| {
            t.fetch_add(1, Ordering::SeqCst);
        });
        let limiter = Limiter::new(4);
        let p = LimitedProvider::new(inner.clone(), limiter.clone(), Some(notify));
        let req: MessagesRequest =
            serde_json::from_value(serde_json::json!({"model": "m", "max_tokens": 10, "messages": []})).unwrap();
        let msg = crate::complete(&p, req, &CancellationToken::new()).await.unwrap();
        assert_eq!(msg.to_message().text(), "finally");
        assert_eq!(inner.requests().len(), 3);
        assert_eq!(limiter.limit(), 1);
        assert!(told.load(Ordering::SeqCst) >= 1, "the person is told the limit changed");
    }
}
