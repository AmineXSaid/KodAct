//! Signed thinking goes back only on the history it was written on.
//!
//! On models that keep their thinking between turns, a thinking block's signature covers
//! everything before it: the system prompt, the tool set, every earlier message and the
//! thinking blocks before it. A request that sends the block back after any of that changed is
//! refused, or answered from text that is gone. KodAct changes earlier history on purpose
//! (micro-compaction clears old tool results, `/rewind` summarize replaces a range, a resumed
//! session gets today's system prompt), so before each request it checks every signed block
//! against a fingerprint of what preceded it when the model wrote it, and leaves out the ones
//! whose history changed. The words and tool calls of those turns stay; only their reasoning
//! goes, as the API itself does when asked to drop such blocks.
//!
//! A fingerprint is a hash of the request as sent (system prompt, tools by name, messages,
//! without cache markers). It is recorded when the reply arrives and kept in the transcript, so
//! a resumed session checks the same way. A block with no fingerprint (a transcript written
//! before this check) counts as changed: its history can't be shown to be the same.

use std::collections::HashMap;

use kodact_types::{ContentBlock, Message, MessagesRequest, Role, SystemBlock, ToolSpec};
use serde_json::Value;

/// A running hash of a request's prefix (FNV-1a, 64 bits: stable across processes and
/// versions, so a resumed session compares with what an earlier one recorded).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PrefixHash(u64);

impl PrefixHash {
    /// The hash of a request's system prompt and tools, before its messages. The tools count as
    /// a set: their order doesn't change what the model saw.
    pub(crate) fn start(system: &[SystemBlock], tools: &[ToolSpec]) -> Self {
        let mut h = PrefixHash(0xcbf2_9ce4_8422_2325);
        h.feed_value(system);
        let mut tools: Vec<&ToolSpec> = tools.iter().collect();
        tools.sort_by(|a, b| a.name.cmp(&b.name));
        h.feed_value(&tools);
        h
    }

    fn feed(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= u64::from(*b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    fn feed_value(&mut self, v: impl serde::Serialize) {
        let mut v = serde_json::to_value(v).unwrap_or(Value::Null);
        strip_cache_control(&mut v);
        self.feed(&serde_json::to_vec(&v).unwrap_or_default());
        self.feed(&[0xff]);
    }

    fn feed_role(&mut self, role: Role) {
        self.feed(match role {
            Role::User => b"\x00user\x00",
            Role::Assistant => b"\x00assistant\x00",
        });
    }

    pub(crate) fn hex(&self) -> String {
        format!("{:016x}", self.0)
    }
}

fn strip_cache_control(v: &mut Value) {
    match v {
        Value::Object(m) => {
            m.remove("cache_control");
            m.values_mut().for_each(strip_cache_control);
        }
        Value::Array(a) => a.iter_mut().for_each(strip_cache_control),
        _ => {}
    }
}

/// The key a signed thinking block is recorded under (a hash of its signature: signatures are
/// long). `None` for blocks no signature binds (an endpoint that sends reasoning unsigned).
pub(crate) fn thinking_key(b: &ContentBlock) -> Option<String> {
    let sig = match b {
        ContentBlock::Thinking { signature, .. } => signature,
        ContentBlock::RedactedThinking { data } => data,
        _ => return None,
    };
    if sig.is_empty() {
        return None;
    }
    let mut h = PrefixHash(0xcbf2_9ce4_8422_2325);
    h.feed(sig.as_bytes());
    Some(h.hex())
}

/// What one pass over a request found.
struct Pass {
    end: PrefixHash,
    dropped: usize,
    emptied: bool,
}

fn guard_pass(req: &mut MessagesRequest, known: &HashMap<String, String>) -> Pass {
    let mut h = PrefixHash::start(&req.system, &req.tools);
    let mut dropped = 0;
    let mut emptied = false;
    for m in &mut req.messages {
        h.feed_role(m.role);
        let had = m.content.len();
        let mut kept = Vec::with_capacity(had);
        for b in m.content.drain(..) {
            if let Some(key) = thinking_key(&b) {
                if known.get(&key) != Some(&h.hex()) {
                    dropped += 1;
                    continue;
                }
            }
            h.feed_value(&b);
            kept.push(b);
        }
        emptied |= had > 0 && kept.is_empty();
        m.content = kept;
    }
    Pass { end: h, dropped, emptied }
}

/// Leave out every signed thinking block whose history changed since the model wrote it.
/// Returns the hash of the whole request as sent (the prefix the reply is written on) and how
/// many blocks were left out.
pub(crate) fn guard(req: &mut MessagesRequest, known: &HashMap<String, String>) -> (PrefixHash, usize) {
    let mut total = 0;
    loop {
        let pass = guard_pass(req, known);
        total += pass.dropped;
        if !pass.emptied {
            return (pass.end, total);
        }
        // A reply that was only reasoning is gone: the API takes no empty message, and the
        // messages on either side (both the person's) become one.
        let mut merged: Vec<Message> = Vec::with_capacity(req.messages.len());
        for m in req.messages.drain(..).filter(|m| !m.content.is_empty()) {
            match merged.last_mut() {
                Some(prev) if prev.role == m.role => prev.content.extend(m.content),
                _ => merged.push(m),
            }
        }
        req.messages = merged;
    }
}

/// The fingerprints of the signed thinking blocks in a reply to a request whose prefix hash is
/// `sent`, keyed by [`thinking_key`]. The reply is hashed as the next request will send it.
pub(crate) fn record(sent: PrefixHash, reply: &[ContentBlock]) -> Vec<(String, String)> {
    let mut h = sent;
    h.feed_role(Role::Assistant);
    let mut out = vec![];
    for b in reply {
        // As `request::normalize` sends it: no empty text blocks.
        if matches!(b, ContentBlock::Text { text, .. } if text.trim().is_empty()) {
            continue;
        }
        if let Some(key) = thinking_key(b) {
            out.push((key, h.hex()));
        }
        h.feed_value(b);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thought(sig: &str) -> ContentBlock {
        ContentBlock::Thinking { thinking: "t".into(), signature: sig.into() }
    }

    fn req(messages: Vec<Message>) -> MessagesRequest {
        MessagesRequest {
            model: "m".into(),
            max_tokens: 1,
            messages,
            system: vec![SystemBlock::text("sys")],
            tools: vec![],
            tool_choice: None,
            thinking: None,
            temperature: None,
            metadata: None,
            output_config: None,
            speed: None,
            stream: true,
            betas: vec![],
        }
    }

    /// A reply recorded against its request keeps its thinking while nothing before it changes,
    /// and loses it, with every later block, once something does.
    #[test]
    fn keeps_thinking_on_its_history_and_drops_it_after_an_edit() {
        let mut first = req(vec![Message::user_text("a")]);
        let (sent, _) = guard(&mut first, &HashMap::new());
        let reply = vec![thought("s1"), ContentBlock::text("b")];
        let mut known: HashMap<String, String> = record(sent, &reply).into_iter().collect();

        let mut second = req(vec![Message::user_text("a"), Message::assistant(reply.clone()), Message::user_text("c")]);
        let (sent, dropped) = guard(&mut second, &known);
        assert_eq!(dropped, 0);
        let reply2 = vec![thought("s2"), ContentBlock::text("d")];
        known.extend(record(sent, &reply2));

        let history = |first: &str| {
            req(vec![
                Message::user_text(first),
                Message::assistant(reply.clone()),
                Message::user_text("c"),
                Message::assistant(reply2.clone()),
                Message::user_text("e"),
            ])
        };
        let mut same = history("a");
        assert_eq!(guard(&mut same, &known).1, 0, "nothing changed");

        let mut edited = history("A");
        assert_eq!(guard(&mut edited, &known).1, 2, "both blocks were written after the edit");
        assert!(edited.messages.iter().flat_map(|m| &m.content).all(|b| !matches!(b, ContentBlock::Thinking { .. })));
        assert_eq!(edited.messages[1].text(), "b", "the words stay");

        let mut moved_system = history("a");
        moved_system.system = vec![SystemBlock::text("sys, the next day")];
        assert_eq!(guard(&mut moved_system, &known).1, 2);
    }

    /// Thinking written after the change is kept: it was written on the changed history.
    #[test]
    fn thinking_written_after_a_change_stays() {
        let mut known = HashMap::new();
        let mut r = req(vec![Message::user_text("a")]);
        let (sent, _) = guard(&mut r, &known);
        let old = vec![thought("old"), ContentBlock::text("b")];
        known.extend(record(sent, &old));
        // The history changes: the old block goes from here on.
        let mut r = req(vec![Message::user_text("A"), Message::assistant(old.clone()), Message::user_text("c")]);
        let (sent, dropped) = guard(&mut r, &known);
        assert_eq!(dropped, 1);
        let new = vec![thought("new"), ContentBlock::text("d")];
        known.extend(record(sent, &new));
        let mut r = req(vec![
            Message::user_text("A"),
            Message::assistant(old),
            Message::user_text("c"),
            Message::assistant(new),
            Message::user_text("e"),
        ]);
        assert_eq!(guard(&mut r, &known).1, 1, "only the old block");
        assert!(matches!(&r.messages[3].content[0], ContentBlock::Thinking { signature, .. } if signature == "new"));
    }

    /// Cache markers and the order of the tools don't change the prefix; unsigned reasoning
    /// isn't checked; a block nobody recorded is left out.
    #[test]
    fn what_counts_and_what_does_not() {
        let mut r = req(vec![Message::user_text("a")]);
        let (sent, _) = guard(&mut r, &HashMap::new());
        let reply = vec![thought("s"), ContentBlock::text("b")];
        let known: HashMap<String, String> = record(sent, &reply).into_iter().collect();
        let mut marked = req(vec![Message::user_text("a"), Message::assistant(reply.clone())]);
        marked.messages[0].content[0].set_cache_control(Some(kodact_types::CacheControl::ephemeral()));
        assert_eq!(guard(&mut marked, &known).1, 0);

        let unsigned = ContentBlock::Thinking { thinking: "t".into(), signature: String::new() };
        let mut r = req(vec![Message::user_text("a"), Message::assistant(vec![unsigned, ContentBlock::text("b")])]);
        assert_eq!(guard(&mut r, &HashMap::new()).1, 0);

        let mut r = req(vec![Message::user_text("a"), Message::assistant(reply)]);
        assert_eq!(guard(&mut r, &HashMap::new()).1, 1, "no record: its history can't be shown to be the same");
    }

    /// A reply that was only reasoning leaves no empty message behind.
    #[test]
    fn a_reply_that_was_only_reasoning_leaves_no_empty_message() {
        let mut r = req(vec![Message::user_text("a"), Message::assistant(vec![thought("s")]), Message::user_text("b")]);
        assert_eq!(guard(&mut r, &HashMap::new()).1, 1);
        assert_eq!(r.messages.len(), 1);
        assert_eq!(r.messages[0].role, Role::User);
        assert_eq!(r.messages[0].content.len(), 2);
    }
}
