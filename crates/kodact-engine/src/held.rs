//! A summary written ahead of compaction and held until compaction is due
//! (`compaction.background`, off until measured).
//!
//! Near the limit, the conversation is summarized in the background from the request the model
//! just answered: the provider reads that prefix from its cache, so the summary costs little,
//! and the person does not wait for it when the limit comes. It is held, not applied: applying
//! it early would rewrite the conversation and lose the cache for every request until
//! compaction is really due. When it is due, the held summary replaces what it covers, and the
//! messages since are kept word for word, without the reasoning bound to the replaced part.
//!
//! The design follows opencode's background compaction (MIT), described in
//! docs/research/harness-study/03-opencode.md; this is KodAct's own implementation.

use std::sync::Arc;

use futures::StreamExt;
use kodact_api::{MessageAccumulator, Provider};
use kodact_types::{MessagesRequest, Usage};
use tokio_util::sync::CancellationToken;

/// How far below the compaction threshold the summary is started: a fifth of the threshold,
/// or twice the largest step the conversation has grown by in one request so far, whichever is
/// more, and never more than half the threshold. A conversation that grows by large reads can
/// jump past a fixed lead in one step (44k to 65k on a 100k window), so the lead follows how
/// this run grows; on a large window it starts late, and a conversation that never nears its
/// limit never starts one.
pub(crate) fn lead(threshold: u64, largest_step: u64) -> u64 {
    (threshold / 5).max(largest_step.saturating_mul(2)).min(threshold / 2)
}

/// The most the messages after a held summary may hold when it is applied: two fifths of the
/// model's window. Beyond it the summary covers too little of the conversation to leave room
/// for the next cycle, and a fresh one is written instead (opencode keeps the same share).
pub(crate) fn tail_limit(window: u64) -> u64 {
    window * 2 / 5
}

/// A summary under way or written, of `state.messages[..at]`.
pub(crate) struct HeldSummary {
    pub at: usize,
    /// The uuid of the last message it covers: if the conversation was rewritten since (a
    /// rewind, a manual compaction, `/clear`), it no longer is the same conversation.
    pub anchor: Option<String>,
    pub model: String,
    pub task: Option<tokio::task::JoinHandle<Result<(String, Usage), String>>>,
    pub cancel: CancellationToken,
}

impl HeldSummary {
    /// Wait for the summary: `(text, usage)`, or why there is none.
    pub async fn take(mut self) -> Result<(String, Usage), String> {
        match self.task.take() {
            Some(t) => t.await.map_err(|e| e.to_string())?,
            None => Err("no summary".into()),
        }
    }
}

impl Drop for HeldSummary {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

/// Send the summary request and read its reply to the end.
pub(crate) async fn write(
    provider: Arc<dyn Provider>,
    request: MessagesRequest,
    cancel: CancellationToken,
) -> Result<(String, Usage), String> {
    let mut stream = provider.stream(request, cancel.clone()).await.map_err(|e| e.to_string())?;
    let mut acc = MessageAccumulator::new();
    while let Some(event) = stream.next().await {
        if cancel.is_cancelled() {
            return Err("interrupted".into());
        }
        acc.push(&event.map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    }
    let msg = acc.finish().map_err(|e| e.to_string())?;
    Ok((kodact_compact::extract_summary(&msg.to_message().text()), msg.usage.clone()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_lead_and_the_tail_limit_are_shares_of_the_threshold() {
        assert_eq!((super::lead(15_000, 0), super::tail_limit(24_000)), (3_000, 9_600));
        assert_eq!((super::lead(955_000, 0), super::tail_limit(1_000_000)), (191_000, 400_000));
        // A run that grows by 12.7k a request on a 62.5k threshold starts 25.4k ahead.
        assert_eq!(super::lead(62_500, 12_700), 25_400);
        // Never more than half the threshold.
        assert_eq!(super::lead(62_500, 40_000), 31_250);
    }
}
