//! Language-server diagnostics after edits (GOALS pillar 3; `verification.lsp`).
//!
//! The parser check in [`crate::diagnose`] says when a file no longer parses. A language server
//! says more: a name that doesn't exist, a wrong argument, a type that doesn't fit, also across
//! files. With `verification.lsp` on, the file just edited is sent to its language's server (when
//! one is installed) and the errors the edit *introduced* are added to the edit's result.
//!
//! * Only errors, never warnings or hints.
//! * Only new ones: the file as it was before the edit is the baseline, so a project the server
//!   can't fully resolve (missing dependencies, generated code) doesn't repeat its standing
//!   errors with every edit.
//! * A server that is missing, slow to start or silent adds nothing; one that fails is not
//!   started again in this session.
//!
//! It is off by default: a language server is a long-running process that indexes the project.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

use crate::diagnose::Diagnostic;

/// How long a server may take to answer `initialize`.
const START_TIMEOUT: Duration = Duration::from_secs(20);
/// How long to wait for a file's diagnostics when `waitMs` is not set.
const DEFAULT_WAIT: Duration = Duration::from_secs(4);
/// After a server's first answer for a change, how long a fuller one is waited for (servers
/// publish syntax errors first and semantic ones right after).
const SETTLE: Duration = Duration::from_millis(400);
/// The first errors, not the whole list: the size of a message, whatever the model.
const MAX_ERRORS: usize = 10;
const MAX_MESSAGE_CHARS: usize = 300;

/// The servers tried for an extension when `verification.lsp` doesn't name one: the first whose
/// program is installed is used.
fn builtin(ext: &str) -> &'static [&'static [&'static str]] {
    match ext {
        "rs" => &[&["rust-analyzer"]],
        "py" | "pyi" => &[&["pyright-langserver", "--stdio"], &["basedpyright-langserver", "--stdio"], &["pylsp"]],
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" | "mts" | "cts" => &[&["typescript-language-server", "--stdio"]],
        "go" => &[&["gopls"]],
        "c" | "h" | "cc" | "cpp" | "cxx" | "hpp" => &[&["clangd"]],
        _ => &[],
    }
}

fn language_id(ext: &str) -> &'static str {
    match ext {
        "rs" => "rust",
        "py" | "pyi" => "python",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "go" => "go",
        "c" => "c",
        "h" | "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        _ => "plaintext",
    }
}

/// `verification.lsp`: `true` for the built-in servers, or
/// `{"servers": {"<ext>": ["program", "arg"…]}, "waitMs": N}` to name them (an empty list
/// turns an extension off).
#[derive(Debug, Clone, PartialEq)]
pub struct LspConfig {
    /// Extension (lower case, no dot) to command; `Some(vec![])` turns the extension off.
    pub servers: HashMap<String, Vec<String>>,
    pub wait: Duration,
}

impl LspConfig {
    pub fn from_setting(v: Option<&Value>) -> Option<Self> {
        match v? {
            Value::Bool(true) => Some(LspConfig { servers: HashMap::new(), wait: DEFAULT_WAIT }),
            Value::Object(o) => {
                if o.get("enabled").and_then(Value::as_bool) == Some(false) {
                    return None;
                }
                let servers = o
                    .get("servers")
                    .and_then(Value::as_object)
                    .map(|m| {
                        m.iter()
                            .map(|(ext, cmd)| {
                                let cmd = cmd
                                    .as_array()
                                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                                    .unwrap_or_default();
                                (ext.trim_start_matches('.').to_ascii_lowercase(), cmd)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let wait = o.get("waitMs").and_then(Value::as_u64).map(Duration::from_millis).unwrap_or(DEFAULT_WAIT);
                Some(LspConfig { servers, wait })
            }
            _ => None,
        }
    }

    /// The commands to try for a file, in order.
    fn commands(&self, ext: &str) -> Vec<Vec<String>> {
        match self.servers.get(ext) {
            Some(cmd) if cmd.is_empty() => vec![],
            Some(cmd) => vec![cmd.clone()],
            None => builtin(ext).iter().map(|c| c.iter().map(|s| s.to_string()).collect()).collect(),
        }
    }
}

/// `file://` URI of an absolute path.
fn uri_of(path: &Path) -> String {
    let mut out = String::from("file://");
    for b in path.to_string_lossy().replace('\\', "/").bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// The path a `file://` URI names (servers re-encode URIs their own way, so URIs are compared
/// as paths).
fn path_of(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = (bytes[i] == b'%').then(|| rest.get(i + 1..i + 3)).flatten();
        match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
            Some(b) => {
                out.push(b);
                i += 3;
            }
            None => {
                out.push(bytes[i]);
                i += 1;
            }
        }
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out).into_owned()))
}

#[derive(Default)]
struct Inbox {
    /// What the server last published for each file: when (a counter), for which version of
    /// the text, and the diagnostics.
    published: HashMap<PathBuf, (u64, Option<i64>, Vec<Value>)>,
    responses: HashMap<i64, Value>,
    counter: u64,
    closed: bool,
}

struct Server {
    name: String,
    /// Kept for `kill_on_drop`: the server ends with the session.
    _child: tokio::process::Child,
    out: mpsc::UnboundedSender<Value>,
    inbox: Arc<Mutex<Inbox>>,
    tick: watch::Receiver<u64>,
    /// Files opened on the server, with the version of the text it has.
    open: HashMap<PathBuf, i64>,
}

fn frame(v: &Value) -> Vec<u8> {
    let body = v.to_string();
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

async fn read_message<R: AsyncBufReadExt + Unpin>(r: &mut R) -> Option<Value> {
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().ok()?;
        }
    }
    let mut body = vec![0u8; len];
    r.read_exact(&mut body).await.ok()?;
    // A message that isn't JSON is skipped, not the end of the stream.
    Some(serde_json::from_slice(&body).unwrap_or(Value::Null))
}

impl Server {
    async fn start(command: &[String], root: &Path) -> Option<Server> {
        let (program, args) = command.split_first()?;
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        // Not installed: nothing is said.
        let mut child = cmd.spawn().ok()?;
        let mut stdin = child.stdin.take()?;
        let stdout = child.stdout.take()?;
        let inbox = Arc::new(Mutex::new(Inbox::default()));
        let (tick_tx, tick) = watch::channel(0u64);
        let (out, mut out_rx) = mpsc::unbounded_channel::<Value>();
        tokio::spawn(async move {
            while let Some(v) = out_rx.recv().await {
                if stdin.write_all(&frame(&v)).await.is_err() || stdin.flush().await.is_err() {
                    break;
                }
            }
        });
        let (inbox2, out2) = (inbox.clone(), out.clone());
        tokio::spawn(async move {
            let mut r = BufReader::new(stdout);
            while let Some(v) = read_message(&mut r).await {
                let method = v.get("method").and_then(Value::as_str);
                match (method, v.get("id")) {
                    // A request from the server: answered so it doesn't wait on the client.
                    (Some(m), Some(id)) => {
                        let result = match m {
                            "workspace/configuration" => {
                                let n = v.pointer("/params/items").and_then(Value::as_array).map_or(0, Vec::len);
                                Value::Array(vec![Value::Null; n])
                            }
                            _ => Value::Null,
                        };
                        let _ = out2.send(json!({"jsonrpc": "2.0", "id": id, "result": result}));
                    }
                    (Some("textDocument/publishDiagnostics"), None) => {
                        let p = &v["params"];
                        if let Some(path) = p["uri"].as_str().and_then(path_of) {
                            let mut ib = inbox2.lock().unwrap();
                            ib.counter += 1;
                            let n = ib.counter;
                            let list = p["diagnostics"].as_array().cloned().unwrap_or_default();
                            ib.published.insert(path, (n, p["version"].as_i64(), list));
                            let _ = tick_tx.send(n);
                        }
                    }
                    (None, Some(id)) => {
                        if let Some(id) = id.as_i64() {
                            let mut ib = inbox2.lock().unwrap();
                            ib.counter += 1;
                            let n = ib.counter;
                            ib.responses.insert(id, v);
                            let _ = tick_tx.send(n);
                        }
                    }
                    _ => {}
                }
            }
            let mut ib = inbox2.lock().unwrap();
            ib.closed = true;
            ib.counter += 1;
            let _ = tick_tx.send(ib.counter);
        });

        let mut server = Server { name: program.clone(), _child: child, out, inbox, tick, open: HashMap::new() };
        let root_uri = uri_of(root);
        let mut init = json!({
            "processId": std::process::id(),
            "clientInfo": {"name": "kodact"},
            "rootUri": root_uri,
            "rootPath": root,
            "workspaceFolders": [{"uri": root_uri, "name": "project"}],
            "capabilities": {
                "textDocument": {
                    "synchronization": {"dynamicRegistration": false},
                    "publishDiagnostics": {"versionSupport": true, "relatedInformation": false},
                },
                "workspace": {"workspaceFolders": true, "configuration": true},
            },
        });
        if server.name.contains("rust-analyzer") {
            // Its build-based checks run cargo in the project's target directory, where they
            // would hold the lock the agent's own builds need: only its own analysis is used.
            init["initializationOptions"] = json!({"checkOnSave": false});
        }
        server.send(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": init}));
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            {
                let ib = server.inbox.lock().unwrap();
                if ib.closed {
                    return None;
                }
                if let Some(r) = ib.responses.get(&1) {
                    if r.get("error").is_some() {
                        return None;
                    }
                    break;
                }
            }
            if tokio::time::timeout_at(deadline, server.tick.changed()).await.is_err() {
                return None;
            }
        }
        server.send(json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}));
        Some(server)
    }

    fn send(&self, v: Value) {
        let _ = self.out.send(v);
    }

    /// Give the server `text` for `path` and wait for what it publishes about it: `None` when
    /// it says nothing in time.
    async fn diagnostics_for(&mut self, path: &Path, text: &str, wait: Duration) -> Option<Vec<Value>> {
        let since = self.inbox.lock().unwrap().counter;
        let uri = uri_of(path);
        let version = match self.open.get(path).copied() {
            None => {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
                self.send(json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {
                    "textDocument": {"uri": uri, "languageId": language_id(&ext), "version": 1, "text": text},
                }}));
                1
            }
            Some(v) => {
                self.send(json!({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
                    "textDocument": {"uri": uri, "version": v + 1},
                    "contentChanges": [{"text": text}],
                }}));
                v + 1
            }
        };
        self.open.insert(path.to_path_buf(), version);
        let deadline = Instant::now() + wait;
        let mut got: Option<(u64, Vec<Value>)> = None;
        let mut settle = deadline;
        loop {
            {
                let ib = self.inbox.lock().unwrap();
                if let Some((n, v, list)) = ib.published.get(path) {
                    let fresh = *n > since && v.is_none_or(|v| v >= version);
                    if fresh && got.as_ref().is_none_or(|(seen, _)| seen != n) {
                        got = Some((*n, list.clone()));
                        settle = (Instant::now() + SETTLE).min(deadline);
                    }
                }
                if ib.closed {
                    break;
                }
            }
            let until = if got.is_some() { settle } else { deadline };
            if tokio::time::timeout_at(until, self.tick.changed()).await.is_err() {
                break;
            }
        }
        got.map(|(_, list)| list)
    }
}

/// The error messages in a published list (severity 1; a missing severity counts, as the
/// protocol leaves it to the client), each with its line.
fn errors(list: &[Value]) -> Vec<(u64, String)> {
    list.iter()
        .filter(|d| d.get("severity").and_then(Value::as_u64).unwrap_or(1) == 1)
        .filter_map(|d| {
            let line = d.pointer("/range/start/line").and_then(Value::as_u64).unwrap_or(0) + 1;
            let msg: String = d["message"].as_str()?.split_whitespace().collect::<Vec<_>>().join(" ");
            let msg: String = msg.chars().take(MAX_MESSAGE_CHARS).collect();
            (!msg.is_empty()).then_some((line, msg))
        })
        .collect()
}

/// The errors in `after` that `before` didn't have, by message: lines move with an edit, so
/// they don't count, and an error that occurred twice and now occurs three times is new once.
fn introduced(before: &[(u64, String)], after: &[(u64, String)]) -> Vec<(u64, String)> {
    let mut had: HashMap<&str, usize> = HashMap::new();
    for (_, m) in before {
        *had.entry(m.as_str()).or_default() += 1;
    }
    after
        .iter()
        .filter(|(_, m)| match had.get_mut(m.as_str()) {
            Some(n) if *n > 0 => {
                *n -= 1;
                false
            }
            _ => true,
        })
        .cloned()
        .collect()
}

/// The session's language servers, started when a file of their language is first edited.
pub struct Lsp {
    cfg: LspConfig,
    root: PathBuf,
    /// By command line; `None` once a server failed to start or closed.
    servers: HashMap<String, Option<Server>>,
}

impl Lsp {
    pub fn new(cfg: LspConfig, root: &Path) -> Self {
        Lsp { cfg, root: root.to_path_buf(), servers: HashMap::new() }
    }

    async fn server_for(&mut self, path: &Path) -> Option<&mut Server> {
        let ext = path.extension().and_then(|e| e.to_str())?.to_ascii_lowercase();
        for command in self.cfg.commands(&ext) {
            let key = command.join(" ");
            if !self.servers.contains_key(&key) {
                let started = Server::start(&command, &self.root).await;
                self.servers.insert(key.clone(), started);
            }
            if self.servers.get(&key).is_some_and(Option::is_some) {
                return self.servers.get_mut(&key).and_then(Option::as_mut);
            }
        }
        None
    }

    /// The errors the edit of `path` introduced, per its language server. `original` is the
    /// file's text before the edit, when the tool gave it (the baseline for a file the server
    /// hasn't seen yet).
    pub(crate) async fn check(&mut self, path: &Path, original: Option<&str>) -> Option<Diagnostic> {
        let text = tokio::fs::read_to_string(path).await.ok()?;
        let wait = self.cfg.wait;
        let server = self.server_for(path).await?;
        // What the server said about the file before this edit.
        let known = server.open.contains_key(path);
        let before = match (known, original) {
            (true, _) => server.inbox.lock().unwrap().published.get(path).map(|(_, _, l)| errors(l)),
            (false, Some(orig)) => server.diagnostics_for(path, orig, wait).await.map(|l| errors(&l)),
            (false, None) => None,
        }
        .unwrap_or_default();
        let after = server.diagnostics_for(path, &text, wait).await;
        let name = server.name.clone();
        if server.inbox.lock().unwrap().closed {
            let key =
                self.servers.iter().find(|(_, s)| s.as_ref().is_some_and(|s| s.name == name)).map(|(k, _)| k.clone());
            if let Some(k) = key {
                self.servers.insert(k, None);
            }
        }
        let new = introduced(&before, &errors(&after?));
        if new.is_empty() {
            return None;
        }
        let shown = path.strip_prefix(&self.root).unwrap_or(path).display().to_string();
        let mut lines: Vec<String> =
            new.iter().take(MAX_ERRORS).map(|(line, msg)| format!("{shown}:{line}: error: {msg}")).collect();
        if new.len() > MAX_ERRORS {
            lines.push(format!("… and {} more", new.len() - MAX_ERRORS));
        }
        Some(Diagnostic { parser: name, message: lines.join("\n"), from_server: true })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn the_setting_names_servers_or_takes_the_built_in_ones() {
        assert_eq!(LspConfig::from_setting(None), None);
        assert_eq!(LspConfig::from_setting(Some(&json!(false))), None);
        assert_eq!(LspConfig::from_setting(Some(&json!({"enabled": false}))), None);
        let on = LspConfig::from_setting(Some(&json!(true))).unwrap();
        assert_eq!(on.wait, DEFAULT_WAIT);
        assert_eq!(on.commands("rs"), vec![vec!["rust-analyzer".to_string()]]);
        assert!(on.commands("py").len() > 1 && on.commands("md").is_empty());
        let named =
            LspConfig::from_setting(Some(&json!({"servers": {".PY": ["pylsp"], "rs": []}, "waitMs": 250}))).unwrap();
        assert_eq!(named.commands("py"), vec![vec!["pylsp".to_string()]]);
        assert!(named.commands("rs").is_empty(), "an empty command turns the extension off");
        assert_eq!(named.commands("go"), vec![vec!["gopls".to_string()]]);
        assert_eq!(named.wait, Duration::from_millis(250));
    }

    #[test]
    fn uris_round_trip_whatever_the_server_encodes() {
        let p = Path::new("/work/my app/src/a+b.rs");
        assert_eq!(uri_of(p), "file:///work/my%20app/src/a%2Bb.rs");
        assert_eq!(path_of(&uri_of(p)).unwrap(), p);
        assert_eq!(path_of("file:///work/my%20app/src/a+b.rs").unwrap(), p, "a server's own encoding");
        assert_eq!(path_of("untitled:1"), None);
    }

    #[test]
    fn only_errors_the_edit_introduced_are_kept() {
        let list = json!([
            {"severity": 1, "message": "cannot find name 'x'", "range": {"start": {"line": 4}}},
            {"severity": 2, "message": "unused variable", "range": {"start": {"line": 1}}},
            {"severity": 1, "message": "import \"dep\"\n  could not be resolved", "range": {"start": {"line": 0}}},
        ]);
        let after = errors(list.as_array().unwrap());
        assert_eq!(after.len(), 2, "warnings are left out");
        assert_eq!(after[1].1, "import \"dep\" could not be resolved");
        let before = vec![(9, "import \"dep\" could not be resolved".to_string())];
        assert_eq!(introduced(&before, &after), vec![(5, "cannot find name 'x'".to_string())]);
        assert!(introduced(&after, &after).is_empty());
        let twice = vec![after[0].clone(), after[0].clone()];
        assert_eq!(introduced(&after, &twice).len(), 1, "one more of the same is new once");
    }

    /// A stand-in server (python, when this machine has it): it reports an error on every line
    /// that contains `BAD`, and a standing one on every line that contains `OLD`.
    pub(crate) const FAKE: &str = r#"
import json, sys
def send(m):
    b = json.dumps(m).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(b) + b); sys.stdout.buffer.flush()
def publish(doc, text):
    d = [{"severity": 1, "message": ("undefined name" if "BAD" in l else "standing error"),
          "range": {"start": {"line": i, "character": 0}, "end": {"line": i, "character": 1}}}
         for i, l in enumerate(text.split("\n")) if "BAD" in l or "OLD" in l]
    d.append({"severity": 2, "message": "a warning", "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}})
    send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
          "params": {"uri": doc["uri"], "version": doc.get("version"), "diagnostics": d}})
while True:
    n = 0
    while True:
        h = sys.stdin.buffer.readline()
        if not h: sys.exit(0)
        h = h.strip()
        if not h: break
        if h.lower().startswith(b"content-length:"): n = int(h.split(b":")[1])
    m = json.loads(sys.stdin.buffer.read(n))
    if m.get("method") == "initialize":
        send({"jsonrpc": "2.0", "id": 7, "method": "workspace/configuration", "params": {"items": [{}]}})
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"capabilities": {}}})
    elif m.get("method") == "textDocument/didOpen":
        publish(m["params"]["textDocument"], m["params"]["textDocument"]["text"])
    elif m.get("method") == "textDocument/didChange":
        publish(m["params"]["textDocument"], m["params"]["contentChanges"][0]["text"])
"#;

    pub(crate) fn python() -> Option<&'static str> {
        ["python3", "python"].into_iter().find(|p| std::process::Command::new(p).arg("--version").output().is_ok())
    }

    fn fake(dir: &Path, ext: &str) -> Option<Lsp> {
        let script = dir.join("fake_server.py");
        std::fs::write(&script, FAKE).unwrap();
        let cmd = json!([python()?, script]);
        let cfg = LspConfig::from_setting(Some(&json!({"servers": {ext: cmd}, "waitMs": 3000}))).unwrap();
        Some(Lsp::new(cfg, dir))
    }

    #[tokio::test]
    async fn an_edit_is_told_the_errors_it_introduced_and_no_others() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let Some(mut lsp) = fake(&root, "zz") else { return };
        let f = root.join("my mod.zz");
        // The file had a standing error before the edit; the edit adds one of its own.
        let original = "ok\nOLD\n";
        std::fs::write(&f, "ok\nOLD\nBAD call\n").unwrap();
        let d = lsp.check(&f, Some(original)).await.expect("the new error is reported");
        assert!(d.from_server && d.parser.contains("python"), "{d:?}");
        assert_eq!(d.message, "my mod.zz:3: error: undefined name");
        assert!(d.text("my mod.zz").contains("reports errors this change introduced"), "{}", d.text("x"));
        // Fixed: nothing is said, and the standing error still isn't.
        std::fs::write(&f, "ok\nOLD\ngood call\n").unwrap();
        assert_eq!(lsp.check(&f, None).await, None);
        // Broken again, now against what the server last said about the file.
        std::fs::write(&f, "BAD\nok\nOLD\n").unwrap();
        assert_eq!(lsp.check(&f, None).await.unwrap().message, "my mod.zz:1: error: undefined name");
        // A language with no server, and a file that is gone: nothing.
        std::fs::write(root.join("a.txt"), "BAD").unwrap();
        assert_eq!(lsp.check(&root.join("a.txt"), None).await, None);
        assert_eq!(lsp.check(&root.join("gone.zz"), None).await, None);
    }

    #[tokio::test]
    async fn a_server_that_is_missing_or_dies_says_nothing_and_is_not_started_again() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let f = root.join("a.zz");
        std::fs::write(&f, "BAD").unwrap();
        let cfg = LspConfig::from_setting(Some(&json!({"servers": {"zz": ["kodact-no-such-language-server"]}})));
        let mut lsp = Lsp::new(cfg.unwrap(), &root);
        assert_eq!(lsp.check(&f, None).await, None);
        // One that exits at once (`true` reads nothing and answers nothing).
        let cfg = LspConfig::from_setting(Some(&json!({"servers": {"zz": ["true"]}})));
        let mut lsp = Lsp::new(cfg.unwrap(), &root);
        let started = std::time::Instant::now();
        assert_eq!(lsp.check(&f, None).await, None);
        assert_eq!(lsp.check(&f, None).await, None);
        assert!(started.elapsed() < START_TIMEOUT, "a closed server is not waited for");
        assert!(lsp.servers.values().all(Option::is_none));
    }

    /// With a real language server, when this machine has one (pyright).
    #[tokio::test]
    async fn a_real_server_reports_an_undefined_name_in_a_file_that_parses() {
        if std::process::Command::new("pyright-langserver").arg("--version").output().is_err() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let cfg = LspConfig::from_setting(Some(&json!({"waitMs": 30000}))).unwrap();
        let mut lsp = Lsp::new(cfg, &root);
        let f = root.join("m.py");
        let original = "def total(xs):\n    return sum(xs)\n";
        std::fs::write(&f, "def total(xs):\n    return sum(xs) + offset\n").unwrap();
        let d = lsp.check(&f, Some(original)).await.expect("pyright reports the undefined name");
        assert!(d.message.starts_with("m.py:2: error: ") && d.message.contains("offset"), "{}", d.message);
        std::fs::write(&f, original).unwrap();
        assert_eq!(lsp.check(&f, None).await, None);
    }
}
