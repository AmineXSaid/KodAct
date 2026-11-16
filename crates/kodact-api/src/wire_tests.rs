//! The providers against a real socket: how long they wait for an answer's start, how the end
//! of a streamed body is read. A tiny HTTP server plays the endpoint.

use std::time::{Duration, Instant};

use futures::StreamExt;
use kodact_types::{Message, MessagesRequest, StreamEvent};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::{ApiError, Provider};

const SSE: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n";

/// An endpoint that answers every request with `head` (none: never answers) and the `body`
/// lines, then closes, or stays silent when `hang`.
async fn endpoint(head: Option<&'static str>, body: Vec<String>, hang: bool) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let body = body.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 1 << 16];
                let _ = sock.read(&mut buf).await;
                if let Some(h) = head {
                    let _ = sock.write_all(h.as_bytes()).await;
                    for line in body {
                        let _ = sock.write_all(line.as_bytes()).await;
                        let _ = sock.flush().await;
                    }
                }
                if hang || head.is_none() {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            });
        }
    });
    format!("http://{addr}")
}

fn request(effort: Option<&str>) -> MessagesRequest {
    MessagesRequest {
        model: "wire-model".into(),
        max_tokens: 100,
        messages: vec![Message::user_text("hi")],
        system: vec![],
        tools: vec![],
        tool_choice: None,
        thinking: None,
        temperature: None,
        metadata: None,
        output_config: effort.map(|e| json!({"effort": e})),
        speed: None,
        stream: true,
        betas: vec![],
        carry_reasoning: false,
    }
}

fn openai(url: &str) -> crate::openai::OpenAiProvider {
    crate::openai::OpenAiProvider::new(crate::openai::OpenAiConfig {
        base_url: format!("{url}/v1"),
        max_retries: 0,
        timeout: Duration::from_secs(1),
        ..Default::default()
    })
    .unwrap()
}

fn messages(url: &str) -> crate::messages::MessagesProvider {
    crate::messages::MessagesProvider::new(crate::messages::MessagesConfig {
        base_url: url.into(),
        api_key: Some("k".into()),
        max_retries: 0,
        timeout: Duration::from_secs(1),
        ..Default::default()
    })
    .unwrap()
}

fn gemini(url: &str) -> crate::gemini::GeminiProvider {
    crate::gemini::GeminiProvider::new(crate::gemini::GeminiConfig {
        base_url: url.into(),
        api_key: Some("k".into()),
        max_retries: 0,
        timeout: Duration::from_secs(1),
        ..Default::default()
    })
    .unwrap()
}

/// Everything a stream gives, or the error that ended it before it started.
async fn run(
    p: &dyn Provider,
    req: MessagesRequest,
) -> (Result<Vec<Result<StreamEvent, ApiError>>, ApiError>, Duration) {
    let t = Instant::now();
    let r = match p.stream(req, CancellationToken::new()).await {
        Ok(s) => Ok(s.collect::<Vec<_>>().await),
        Err(e) => Err(e),
    };
    (r, t.elapsed())
}

/// The stream's last item is an error that says the stream ended early.
fn cut(r: &Result<Vec<Result<StreamEvent, ApiError>>, ApiError>) -> bool {
    matches!(r, Ok(items) if matches!(items.last(), Some(Err(ApiError::Parse(m))) if m.starts_with("stream ended")))
}

fn whole(r: &Result<Vec<Result<StreamEvent, ApiError>>, ApiError>) -> bool {
    matches!(r, Ok(items) if items.iter().all(Result::is_ok) && matches!(items.last(), Some(Ok(StreamEvent::MessageStop))))
}

/// An endpoint that never answers fails a request with no level asked within its own timeout,
/// not the client's ceiling (four times it), on every provider.
#[tokio::test]
async fn an_endpoint_that_never_answers_fails_within_the_request_s_limit() {
    let url = endpoint(None, vec![], true).await;
    let providers: Vec<(&str, Box<dyn Provider>)> = vec![
        ("openai", Box::new(openai(&url))),
        ("messages", Box::new(messages(&url))),
        ("gemini", Box::new(gemini(&url))),
    ];
    for (name, p) in providers {
        let (r, took) = run(p.as_ref(), request(None)).await;
        assert!(r.is_err(), "{name}: {r:?}");
        assert!(took < Duration::from_millis(2500), "{name}: waited {took:?}");
    }
}

/// A body that goes silent: a second at no level, twice that at `high`, on every provider.
#[tokio::test]
async fn a_silent_body_fails_after_the_level_s_idle_limit() {
    let first = |p: &str| {
        match p {
        "openai" => "data: {\"choices\":[{\"delta\":{\"content\":\"thinking\"}}]}\n\n".to_string(),
        "messages" => "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"m\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"m\",\"content\":[],\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n".to_string(),
        _ => "data: {\"candidates\":[{\"content\":{\"role\":\"model\",\"parts\":[{\"text\":\"thinking\"}]}}]}\n\n".to_string(),
    }
    };
    for name in ["openai", "messages", "gemini"] {
        let url = endpoint(Some(SSE), vec![first(name)], true).await;
        let p: Box<dyn Provider> = match name {
            "openai" => Box::new(openai(&url)),
            "messages" => Box::new(messages(&url)),
            _ => Box::new(gemini(&url)),
        };
        for (effort, least, most) in [(None, 900, 1900), (Some("high"), 1900, 3200)] {
            let (r, took) = run(p.as_ref(), request(effort)).await;
            let failed = matches!(&r, Ok(items) if matches!(items.last(), Some(Err(ApiError::Network { .. }))));
            assert!(failed, "{name} {effort:?}: {r:?}");
            assert!(
                took >= Duration::from_millis(least) && took < Duration::from_millis(most),
                "{name} {effort:?}: {took:?}"
            );
        }
    }
}

/// Chat completions: content, then the body closes: cut. With a finish reason and no `[DONE]`:
/// whole.
#[tokio::test]
async fn a_chat_stream_closed_before_its_end_is_cut() {
    let content = "data: {\"choices\":[{\"delta\":{\"content\":\"The answer is\"}}]}\n\n".to_string();
    let finish = "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n".to_string();
    let url = endpoint(Some(SSE), vec![content.clone()], false).await;
    assert!(cut(&run(&openai(&url), request(None)).await.0));
    let url = endpoint(Some(SSE), vec![content, finish], false).await;
    assert!(whole(&run(&openai(&url), request(None)).await.0));
}

/// The Messages API: a body that closes after its stop reason, with no `message_stop`, is a
/// whole reply (as on chat completions); one that closes before it is cut.
#[tokio::test]
async fn a_messages_stream_ends_at_its_stop_reason() {
    let ev = |e: &str, d: serde_json::Value| format!("event: {e}\ndata: {d}\n\n");
    let start = ev(
        "message_start",
        json!({"type": "message_start", "message": {"id": "m", "type": "message", "role": "assistant", "model": "m",
            "content": [], "usage": {"input_tokens": 1, "output_tokens": 0}}}),
    );
    let text = [
        ev(
            "content_block_start",
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        ),
        ev(
            "content_block_delta",
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "42"}}),
        ),
        ev("content_block_stop", json!({"type": "content_block_stop", "index": 0})),
    ];
    let delta = ev(
        "message_delta",
        json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1}}),
    );
    let mut body = vec![start.clone()];
    body.extend(text.iter().cloned());
    let url = endpoint(Some(SSE), body.clone(), false).await;
    assert!(cut(&run(&messages(&url), request(None)).await.0), "closed before its stop reason");
    body.push(delta);
    let url = endpoint(Some(SSE), body, false).await;
    let (r, _) = run(&messages(&url), request(None)).await;
    assert!(matches!(&r, Ok(items) if items.iter().all(Result::is_ok)), "a stop reason ends it: {r:?}");
}
