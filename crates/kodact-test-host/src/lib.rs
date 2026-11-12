//! Test support for end-to-end runs of the `kodact` binary.
//!
//! * [`MockApi`] is a tiny HTTP server that answers `POST /v1/messages` with
//!   scripted SSE responses and records every request body.
//! * [`MockGemini`] answers Gemini's `:streamGenerateContent` as strictly as
//!   Google does: a function call that lost its thought signature, a tool name
//!   it wouldn't take or a setting a model refuses gets Google's 400.
//! * [`Host`] spawns `kodact` with stream-json in and out and talks to it the
//!   way an IDE extension does.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use kodact_api::MockTurn;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

/// A scripted Messages API endpoint.
pub struct MockApi {
    pub url: String,
    turns: Arc<Mutex<VecDeque<MockTurn>>>,
    requests: Arc<Mutex<Vec<Value>>>,
    /// Each request's header block, lowercased.
    heads: Arc<Mutex<Vec<String>>>,
}

impl MockApi {
    pub async fn start(turns: Vec<MockTurn>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}", listener.local_addr().unwrap());
        let turns = Arc::new(Mutex::new(VecDeque::from(turns)));
        let requests = Arc::new(Mutex::new(vec![]));
        let heads = Arc::new(Mutex::new(vec![]));
        let (t, r, hs) = (turns.clone(), requests.clone(), heads.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let (t, r, hs) = (t.clone(), r.clone(), hs.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 8192];
                    let (head_end, len) = loop {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
                            let len = head
                                .lines()
                                .find_map(|l| {
                                    l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))
                                })
                                .unwrap_or(0);
                            break (pos + 4, len);
                        }
                    };
                    while buf.len() < head_end + len {
                        let n = sock.read(&mut tmp).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    let body: Value = serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null);
                    hs.lock().unwrap().push(String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase());
                    r.lock().unwrap().push(body.clone());
                    let turn = t.lock().unwrap().pop_front().unwrap_or_else(|| MockTurn::text("(mock api exhausted)"));
                    let response = match turn {
                        MockTurn::Message { mut message, delay } => {
                            message.model = body["model"].as_str().unwrap_or("mock").to_string();
                            let _ = sock
                                .write_all(
                                    b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n",
                                )
                                .await;
                            for ev in kodact_api::mock::events_for(&message) {
                                let data = serde_json::to_string(&ev).unwrap();
                                let kind =
                                    serde_json::to_value(&ev).unwrap()["type"].as_str().unwrap_or("").to_string();
                                if !delay.is_zero() {
                                    tokio::time::sleep(delay).await;
                                }
                                if sock.write_all(format!("event: {kind}\ndata: {data}\n\n").as_bytes()).await.is_err()
                                {
                                    return;
                                }
                            }
                            return;
                        }
                        MockTurn::HttpError { status, kind, message } => {
                            let body =
                                serde_json::json!({"type": "error", "error": {"type": kind, "message": message}})
                                    .to_string();
                            format!("HTTP/1.1 {status} Error\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len())
                        }
                        MockTurn::Throttled { message, retry_after } => {
                            let body = serde_json::json!({"type": "error", "error": {"type": "rate_limit_error", "message": message}})
                                .to_string();
                            format!("HTTP/1.1 429 Error\r\ncontent-type: application/json\r\nretry-after: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", retry_after.as_secs(), body.len())
                        }
                        // An unreachable endpoint: the connection closes with no answer.
                        MockTurn::NetworkError { .. } => return,
                        MockTurn::StreamError { kind, message } => {
                            let err = serde_json::json!({"type": "error", "error": {"type": kind, "message": message}})
                                .to_string();
                            format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\nevent: error\ndata: {err}\n\n")
                        }
                    };
                    let _ = sock.write_all(response.as_bytes()).await;
                });
            }
        });
        MockApi { url, turns, requests, heads }
    }

    pub fn push(&self, turn: MockTurn) {
        self.turns.lock().unwrap().push_back(turn);
    }

    /// Each request's header block, lowercased, in order.
    pub fn headers(&self) -> Vec<String> {
        self.heads.lock().unwrap().clone()
    }

    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

/// A scripted Gemini endpoint that enforces Google's rules. Each request gets
/// the next turn's parts (as one streamed chunk); a rule broken gets the 400
/// Google sends, and the reason is kept in [`MockGemini::violations`].
pub struct MockGemini {
    /// The base URL (`http://127.0.0.1:<port>/v1beta`).
    pub url: String,
    requests: Arc<Mutex<Vec<Value>>>,
    violations: Arc<Mutex<Vec<String>>>,
}

/// What Google refuses in a request, given the signatures it issued (by
/// function name and arguments). `None` when the request is fine.
pub fn gemini_violation(path: &str, body: &Value, issued: &[(String, String)]) -> Option<String> {
    let model = path.rsplit("/models/").next().unwrap_or_default().split(':').next().unwrap_or_default();
    for t in body["tools"].as_array().into_iter().flatten() {
        for d in t["functionDeclarations"].as_array().into_iter().flatten() {
            let name = d["name"].as_str().unwrap_or_default();
            if name.len() > 64 || !name.chars().all(|c| c.is_ascii_alphanumeric() || "_.:-".contains(c)) {
                return Some(format!("Invalid function name: {name}. Must be at most 64 characters long."));
            }
            if let Some(p) = d.get("parameters") {
                for bad in ["$schema", "additionalProperties", "$ref", "const"] {
                    if p.to_string().contains(&format!("\"{bad}\"")) {
                        return Some(format!("Invalid JSON payload received. Unknown name \"{bad}\""));
                    }
                }
            }
        }
    }
    if model.starts_with("gemini-4") && body.pointer("/generationConfig/thinkingConfig/thinkingBudget").is_some() {
        return Some("thinking_budget is not supported by this model. Use thinking_level instead.".into());
    }
    let mut calls = 0;
    for c in body["contents"].as_array().into_iter().flatten() {
        for p in c["parts"].as_array().into_iter().flatten() {
            if let Some(fc) = p.get("functionCall").filter(|_| c["role"] == "model") {
                calls += 1;
                let key = format!("{}{}", fc["name"].as_str().unwrap_or_default(), fc["args"]);
                if let Some((_, sig)) = issued.iter().find(|(k, _)| *k == key) {
                    if p["thoughtSignature"].as_str() != Some(sig) {
                        return Some("Function call is missing a thought_signature in functionCall parts. This is required for tools to work correctly, and missing thought_signature may lead to degraded model performance.".into());
                    }
                }
            }
            if let Some(fr) = p.get("functionResponse") {
                if fr["name"].as_str().is_none_or(str::is_empty) {
                    return Some("functionResponse.name is required.".into());
                }
                calls -= 1;
            }
        }
    }
    (calls != 0).then(|| {
        "Please ensure that the number of function response parts is equal to the number of function call parts of the function call turn.".into()
    })
}

impl MockGemini {
    pub async fn start(turns: Vec<Vec<Value>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let url = format!("http://{}/v1beta", listener.local_addr().unwrap());
        let turns = Arc::new(Mutex::new(VecDeque::from(turns)));
        let requests = Arc::new(Mutex::new(vec![]));
        let violations = Arc::new(Mutex::new(vec![]));
        let issued: Arc<Mutex<Vec<(String, String)>>> = Default::default();
        let (t, r, v) = (turns.clone(), requests.clone(), violations.clone());
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { return };
                let (t, r, v, issued) = (t.clone(), r.clone(), v.clone(), issued.clone());
                tokio::spawn(async move {
                    let Some((head, body)) = read_request(&mut sock).await else { return };
                    let path = head.split(' ').nth(1).unwrap_or_default().to_string();
                    let reply = |status: &str, ct: &str, out: String| {
                        format!("HTTP/1.1 {status}\r\ncontent-type: {ct}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{out}", out.len())
                    };
                    let response = if path.contains(":countTokens") {
                        reply("200 OK", "application/json", r#"{"totalTokens": 1000}"#.into())
                    } else if !path.contains(":streamGenerateContent") {
                        reply("200 OK", "application/json", r#"{"models": []}"#.into())
                    } else {
                        r.lock().unwrap().push(body.clone());
                        let broken = gemini_violation(&path, &body, &issued.lock().unwrap());
                        match broken {
                            Some(why) => {
                                v.lock().unwrap().push(why.clone());
                                let e = serde_json::json!({"error": {"code": 400, "message": why, "status": "INVALID_ARGUMENT"}});
                                reply("400 Bad Request", "application/json", e.to_string())
                            }
                            None => {
                                // A title or summary request (no tools) gets a short text, not the script.
                                let parts = if body.get("tools").is_none() {
                                    vec![serde_json::json!({"text": "A title"})]
                                } else {
                                    t.lock()
                                        .unwrap()
                                        .pop_front()
                                        .unwrap_or_else(|| vec![serde_json::json!({"text": "(mock gemini exhausted)"})])
                                };
                                for p in &parts {
                                    if let (Some(fc), Some(sig)) =
                                        (p.get("functionCall"), p["thoughtSignature"].as_str())
                                    {
                                        let key = format!("{}{}", fc["name"].as_str().unwrap_or_default(), fc["args"]);
                                        issued.lock().unwrap().push((key, sig.to_string()));
                                    }
                                }
                                let chunk = serde_json::json!({
                                    "candidates": [{"content": {"role": "model", "parts": parts}, "finishReason": "STOP"}],
                                    "usageMetadata": {"promptTokenCount": 1000, "candidatesTokenCount": 20, "thoughtsTokenCount": 10}
                                });
                                reply("200 OK", "text/event-stream", format!("data: {chunk}\r\n\r\n"))
                            }
                        }
                    };
                    let _ = sock.write_all(response.as_bytes()).await;
                });
            }
        });
        MockGemini { url, requests, violations }
    }

    pub fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    /// The rules requests broke, in Google's words.
    pub fn violations(&self) -> Vec<String> {
        self.violations.lock().unwrap().clone()
    }
}

/// One HTTP request: its request line and its JSON body.
async fn read_request(sock: &mut tokio::net::TcpStream) -> Option<(String, Value)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let (head_end, len) = loop {
        let n = sock.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                .unwrap_or(0);
            break (pos + 4, len);
        }
    };
    while buf.len() < head_end + len {
        let n = sock.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let line = String::from_utf8_lossy(&buf[..head_end]).lines().next().unwrap_or_default().to_string();
    Some((line, serde_json::from_slice(&buf[head_end..]).unwrap_or(Value::Null)))
}

/// Path of the built `kodact` binary (set by Cargo for integration tests).
pub fn kodact_bin() -> PathBuf {
    PathBuf::from(std::env::var("CARGO_BIN_EXE_kodact").unwrap_or_else(|_| "kodact".into()))
}

/// A `kodact` process driven over stream-json.
pub struct Host {
    pub child: Child,
    stdin: Option<ChildStdin>,
    stdout: tokio::io::Lines<BufReader<ChildStdout>>,
    /// Every line read so far.
    pub transcript: Vec<Value>,
}

/// Environment for a hermetic run: its own KODACT_HOME and the mock endpoint.
pub fn command(bin: &Path, cwd: &Path, home: &Path, api: &str, args: &[&str]) -> Command {
    let mut c = Command::new(bin);
    c.args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("KODACT_HOME", home.join(".kodact"))
        .env("KODACT_BASE_URL", api)
        .env("KODACT_API_KEY", "test-key")
        .env("KODACT_MAX_RETRIES", "0")
        .kill_on_drop(true);
    c
}

impl Host {
    pub fn spawn(mut cmd: Command) -> Self {
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
        let mut child = cmd.spawn().expect("spawn kodact");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap()).lines();
        Host { child, stdin, stdout, transcript: vec![] }
    }

    pub async fn send(&mut self, v: Value) {
        let line = format!("{}\n", serde_json::to_string(&v).unwrap());
        self.stdin.as_mut().expect("stdin open").write_all(line.as_bytes()).await.unwrap();
    }

    pub async fn send_user(&mut self, text: &str) {
        self.send(serde_json::json!({"type": "user", "message": {"role": "user", "content": text}, "parent_tool_use_id": null, "session_id": ""})).await;
    }

    pub fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Next stdout line, or `None` at EOF / timeout.
    pub async fn next(&mut self) -> Option<Value> {
        let line = tokio::time::timeout(Duration::from_secs(20), self.stdout.next_line()).await.ok()?.ok()??;
        let v: Value =
            serde_json::from_str(&line).unwrap_or_else(|e| panic!("non-JSON line from kodact ({e}): {line}"));
        self.transcript.push(v.clone());
        Some(v)
    }

    /// Read until a line satisfies `pred`; returns it.
    pub async fn until(&mut self, pred: impl Fn(&Value) -> bool) -> Value {
        loop {
            match self.next().await {
                Some(v) if pred(&v) => return v,
                Some(_) => {}
                None => panic!("stream ended before the expected line; got: {:#?}", self.transcript),
            }
        }
    }

    pub async fn until_type(&mut self, ty: &str) -> Value {
        self.until(|v| v["type"] == ty).await
    }

    pub async fn wait(mut self) -> (i32, Vec<Value>) {
        self.close_stdin();
        while self.next().await.is_some() {}
        let code = tokio::time::timeout(Duration::from_secs(20), self.child.wait())
            .await
            .ok()
            .and_then(|r| r.ok())
            .and_then(|s| s.code())
            .unwrap_or(-1);
        (code, self.transcript)
    }
}
