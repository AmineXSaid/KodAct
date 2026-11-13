//! Signed thinking is sent back on top of the history it was written on.
//!
//! On models that keep their thinking between turns, each thinking block's signature covers
//! everything before it: the system prompt, the tool list and every earlier message. A later
//! request that sends the block back with any of that changed is refused (a 400) or, on some
//! models, answered by reasoning about text that is no longer there. Cache markers and request
//! parameters (effort, thinking settings) don't count; appending does not count.
//!
//! These tests record every request a session sends and check that rule for each signed block.
//! The ones marked `ignore` fail today: they are the cases to fix (study recommendation N1). Run
//! them with `cargo test -p kodact-engine prefix -- --ignored`.

use kodact_types::{Message, MessagesRequest, StopReason, SystemBlock};

use super::*;

/// The reply to request `i`, opening with a thinking block signed `sig-{i}`, so a later
/// request carrying it can be checked against request `i`.
fn signed(i: usize, turn: MockTurn) -> MockTurn {
    match turn {
        MockTurn::Message { mut message, delay } => {
            message.content.insert(
                0,
                ContentBlock::Thinking {
                    thinking: format!("reasoning for request {i}"),
                    signature: format!("sig-{i}"),
                },
            );
            MockTurn::Message { message, delay }
        }
        other => other,
    }
}

/// `v` as it goes on the wire, without cache markers (moving one doesn't break a signature).
fn wire(v: impl serde::Serialize) -> Value {
    fn strip(v: &mut Value) {
        match v {
            Value::Object(m) => {
                m.remove("cache_control");
                m.values_mut().for_each(strip);
            }
            Value::Array(a) => a.iter_mut().for_each(strip),
            _ => {}
        }
    }
    let mut v = serde_json::to_value(v).unwrap();
    strip(&mut v);
    v
}

/// Every place in `reqs` where a block signed `sig-{i}` is sent on a prefix other than the
/// request `i` it answered. `reqs` holds a session's requests in order (several processes'
/// requests concatenated, for a resume).
fn prefix_violations(reqs: &[MessagesRequest]) -> Vec<String> {
    let mut out = vec![];
    for (s, req) in reqs.iter().enumerate() {
        for (j, m) in req.messages.iter().enumerate() {
            for b in &m.content {
                let ContentBlock::Thinking { signature, .. } = b else { continue };
                let Some(i) = signature.strip_prefix("sig-").and_then(|n| n.parse::<usize>().ok()) else { continue };
                let Some(origin) = reqs.get(i).filter(|_| i < s) else {
                    out.push(format!("request {s}: {signature} has no earlier request {i}"));
                    continue;
                };
                if wire(&req.system) != wire(&origin.system) {
                    out.push(format!("request {s}: {signature}: the system prompt differs from request {i}'s"));
                }
                if wire(&req.tools) != wire(&origin.tools) {
                    out.push(format!("request {s}: {signature}: the tool list differs from request {i}'s"));
                }
                let before = &req.messages[..j];
                if before.len() != origin.messages.len() {
                    out.push(format!(
                        "request {s}: {signature} is at message {j}; request {i} had {} messages before it",
                        origin.messages.len()
                    ));
                } else if let Some(k) = (0..j).find(|&k| wire(&before[k]) != wire(&origin.messages[k])) {
                    out.push(format!("request {s}: {signature}: message {k} differs from request {i}'s"));
                }
            }
        }
    }
    out
}

fn assert_append_only(reqs: &[MessagesRequest]) {
    let carried = reqs
        .iter()
        .flat_map(|r| &r.messages)
        .flat_map(|m| &m.content)
        .any(|b| matches!(b, ContentBlock::Thinking { signature, .. } if signature.starts_with("sig-")));
    assert!(carried, "no signed thinking was sent back: the test checks nothing");
    let v = prefix_violations(reqs);
    assert!(v.is_empty(), "signed thinking sent on a changed history:\n{}", v.join("\n"));
}

fn system_on(date: &str) -> Vec<SystemBlock> {
    vec![SystemBlock::text(format!("sys\nToday's date: {date}"))]
}

/// The checker itself: it sees an edited message, a changed system prompt, and a moved block.
#[test]
fn the_checker_finds_each_kind_of_change() {
    let thought = |i: usize| ContentBlock::Thinking { thinking: "t".into(), signature: format!("sig-{i}") };
    let req = |system: &str, messages: Vec<Message>| MessagesRequest {
        model: "m".into(),
        max_tokens: 1,
        messages,
        system: vec![SystemBlock::text(system)],
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
    let first = req("sys", vec![Message::user_text("a")]);
    let reply = Message::assistant(vec![thought(0), ContentBlock::text("b")]);
    let good = req("sys", vec![Message::user_text("a"), reply.clone(), Message::user_text("c")]);
    assert!(prefix_violations(&[first.clone(), good.clone()]).is_empty());
    // A cache marker moved: still the same prefix.
    let mut marked = good.clone();
    marked.messages[0].content[0].set_cache_control(Some(kodact_types::CacheControl::ephemeral()));
    assert!(prefix_violations(&[first.clone(), marked]).is_empty());

    let edited = req("sys", vec![Message::user_text("A"), reply.clone()]);
    assert_eq!(prefix_violations(&[first.clone(), edited]), ["request 1: sig-0: message 0 differs from request 0's"]);
    let new_system = req("sys 2", vec![Message::user_text("a"), reply.clone()]);
    assert_eq!(
        prefix_violations(&[first.clone(), new_system]),
        ["request 1: sig-0: the system prompt differs from request 0's"]
    );
    let moved = req("sys", vec![Message::user_text("x"), Message::assistant(vec![]), Message::user_text("a"), reply]);
    assert_eq!(
        prefix_violations(&[first, moved]),
        ["request 1: sig-0 is at message 3; request 0 had 1 messages before it"]
    );
}

/// The ordinary path: prompts, tool calls, their results, reminders. Everything is appended.
#[tokio::test]
async fn a_session_with_tools_sends_signed_thinking_back_on_its_own_prefix() {
    let h = Harness::new(vec![]);
    std::fs::create_dir_all(h.cwd()).unwrap();
    std::fs::write(h.cwd().join("a.txt"), "alpha\n").unwrap();
    h.provider.push(signed(0, MockTurn::tool("Read", json!({"file_path": h.cwd().join("a.txt")}))));
    h.provider.push(signed(1, MockTurn::text("read it")));
    h.provider.push(signed(2, MockTurn::tool("Grep", json!({"pattern": "alpha", "path": h.cwd()}))));
    h.provider.push(signed(3, MockTurn::text("found it")));
    h.provider.push(signed(4, MockTurn::text("bye")));
    let mut e = h.engine();
    e.set_system(system_on("2026-11-13"));
    e.submit(prompt("read a.txt")).await;
    e.submit(prompt("grep for alpha")).await;
    e.submit(prompt("thanks")).await;
    assert_eq!(h.provider.requests().len(), 5);
    assert_append_only(&h.provider.requests());
}

/// A summary replaces the whole history: the next request carries no thinking from before
/// it, so nothing is sent on a changed prefix. The summary request itself appends its
/// instruction.
#[tokio::test]
async fn a_full_compaction_leaves_no_signed_thinking_behind() {
    let h = Harness::new(vec![
        signed(0, MockTurn::text("first answer")),
        signed(1, MockTurn::text("second answer")),
        MockTurn::text("<summary>Two questions answered.</summary>"),
        signed(3, MockTurn::text("third answer")),
        signed(4, MockTurn::text("fourth answer")),
    ]);
    let mut e = h.engine();
    e.submit(prompt("one")).await;
    e.submit(prompt("two")).await;
    e.compact(None).await.unwrap();
    e.submit(prompt("three")).await;
    e.submit(prompt("four")).await;
    assert_append_only(&h.provider.requests());
}

/// `--resume` the same day: the history read back from the transcript is sent exactly as the
/// model wrote it.
#[tokio::test]
async fn a_resume_on_the_same_day_sends_the_history_back_unchanged() {
    let h = Harness::new(vec![signed(0, MockTurn::text("first answer")), signed(1, MockTurn::text("second answer"))]);
    let mut e = h.engine();
    e.set_system(system_on("2026-11-13"));
    e.submit(prompt("one")).await;
    e.submit(prompt("two")).await;
    let path = SessionStore::new(h.dir.path().join("store")).session_path(&h.cwd(), SID).unwrap();
    let loaded = kodact_session::LoadedSession::load(&path, None).unwrap();

    let h2 = Harness::new(vec![signed(2, MockTurn::text("third answer"))]);
    let mut resumed = h2.engine();
    resumed.set_system(system_on("2026-11-13"));
    resumed.restore(&loaded);
    resumed.submit(prompt("three")).await;
    let reqs: Vec<_> = h.provider.requests().into_iter().chain(h2.provider.requests()).collect();
    assert_append_only(&reqs);
}

/// Micro-compaction replaces old tool results in place. Every signed block after a cleared
/// result was written on the full result, so the next request breaks its signature. In real
/// use: a long session on such a model fails or degrades at the first micro-compaction.
#[tokio::test]
#[ignore = "N1: micro-compaction edits earlier tool results"]
async fn micro_compaction_keeps_signed_thinking_on_its_prefix() {
    let h = Harness::new(vec![]);
    std::fs::create_dir_all(h.cwd()).unwrap();
    let big = h.cwd().join("big.txt");
    std::fs::write(&big, "line of text\n".repeat(1500)).unwrap();
    let mut e = h.engine_with(small_window(), PermissionMode::BypassPermissions, Arc::new(DenyPrompter), json!({}));
    // As in `c9_micro_is_sticky_across_requests`: the read result is cleared a few turns later.
    h.provider.push(signed(0, MockTurn::tool("Read", json!({"file_path": big}))).with_usage(usage(2_000)));
    h.provider.push(signed(1, MockTurn::text("read it")).with_usage(usage(9_000)));
    for i in 2..=5 {
        h.provider.push(signed(i, MockTurn::text(&format!("answer {i}"))).with_usage(usage(9_000)));
    }
    for i in 1..=5 {
        e.submit(prompt(&format!("turn {i}"))).await;
    }
    let reqs = h.provider.requests();
    assert!(
        reqs.iter().any(|r| result_text(r, "toolu_00_read").as_deref() == Some(request::CLEARED_RESULT)),
        "the result was cleared: the case this test is about happened"
    );
    assert_append_only(&reqs);
}

/// `--resume` the next day: the system prompt carries today's date, so the resumed session
/// sends the old thinking under a new system prompt. The same holds for the git status
/// snapshot after a commit.
#[tokio::test]
#[ignore = "N1: the system prompt carries the date and git status"]
async fn a_resume_on_another_day_keeps_signed_thinking_on_its_prefix() {
    let h = Harness::new(vec![signed(0, MockTurn::text("first answer"))]);
    let mut e = h.engine();
    e.set_system(system_on("2026-11-13"));
    e.submit(prompt("one")).await;
    let path = SessionStore::new(h.dir.path().join("store")).session_path(&h.cwd(), SID).unwrap();
    let loaded = kodact_session::LoadedSession::load(&path, None).unwrap();

    let h2 = Harness::new(vec![signed(1, MockTurn::text("second answer"))]);
    let mut resumed = h2.engine();
    resumed.set_system(system_on("2026-11-14"));
    resumed.restore(&loaded);
    resumed.submit(prompt("two")).await;
    let reqs: Vec<_> = h.provider.requests().into_iter().chain(h2.provider.requests()).collect();
    assert_append_only(&reqs);
}

/// `/rewind` summarize of the start of a conversation: the turns kept after the summary were
/// written on the turns it replaced.
#[tokio::test]
#[ignore = "N1: summarizing a range keeps later turns on a replaced prefix"]
async fn summarizing_the_start_keeps_later_signed_thinking_on_its_prefix() {
    let h = Harness::new(vec![
        signed(0, MockTurn::text("first answer")),
        signed(1, MockTurn::text("second answer")),
        signed(2, MockTurn::text("third answer")),
        MockTurn::blocks(vec![ContentBlock::text("<summary>One question answered.</summary>")], StopReason::EndTurn),
        signed(4, MockTurn::text("fourth answer")),
    ]);
    let mut e = h.engine();
    e.submit(prompt("one")).await;
    e.submit(prompt("two")).await;
    e.submit(prompt("three")).await;
    // Messages: one, first answer, two, second answer, three, third answer.
    e.summarize_range(0, 2, None).await.unwrap();
    e.submit(prompt("four")).await;
    assert_append_only(&h.provider.requests());
}
