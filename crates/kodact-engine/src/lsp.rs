//! Language-server diagnostics after edits (GOALS pillar 3).
//!
//! The parse check ([`crate::diagnose`]) sees a file that no longer parses. A language server
//! sees what a compiler would: a wrong type, a missing import, a call that no longer matches
//! its function, in the edited file and in the files the server watches. Its errors are told
//! with the edit's result, instead of turns later when a check runs.
//!
//! It is opt-in (`verification.languageServer`): a language server is a second program
//! reading the whole project, and some take minutes and gigabytes to index one. When on,
//! nothing is said unless a server for the file's language is installed, starts, and answers
//! within the wait; a server that is missing, slow or broken costs that wait once and is then
//! left alone. The edit itself never fails because of it.
//!
//! The client speaks the base protocol over stdio: `initialize`, `didOpen` / `didChange` with
//! the file's full text, then the diagnostics, pulled (`textDocument/diagnostic`) when the
//! server offers that, otherwise as it publishes them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{oneshot, Notify};

/// How long a server has to start and answer `initialize`.
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// After the first diagnostics for a change arrive, how long to wait for a later, fuller set
/// (servers often publish syntax errors first and type errors after).
const SETTLE: Duration = Duration::from_millis(400);

/// One language server: the file extensions it serves and how to start it.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerSpec {
    /// Lowercase, without the dot.
    pub extensions: Vec<String>,
    /// The program and its arguments.
    pub command: Vec<String>,
}

/// `verification.languageServer` and its tuning.
#[derive(Debug, Clone, PartialEq)]
pub struct LspConfig {
    /// Tried in order; the first whose program starts serves the extension.
    pub servers: Vec<ServerSpec>,
    /// How long an edit waits for the server's diagnostics.
    pub wait: Duration,
}

impl Default for LspConfig {
    fn default() -> Self {
        LspConfig { servers: builtin_servers(), wait: Duration::from_secs(3) }
    }
}

/// The servers looked for when none are configured. Only the ones installed are ever started.
pub fn builtin_servers() -> Vec<ServerSpec> {
    let spec = |exts: &[&str], cmd: &[&str]| ServerSpec {
        extensions: exts.iter().map(|e| e.to_string()).collect(),
        command: cmd.iter().map(|c| c.to_string()).collect(),
    };
    vec![
        spec(&["rs"], &["rust-analyzer"]),
        spec(&["ts", "tsx", "js", "jsx", "mts", "cts", "mjs", "cjs"], &["typescript-language-server", "--stdio"]),
        spec(&["py", "pyi"], &["pyright-langserver", "--stdio"]),
        spec(&["py", "pyi"], &["pylsp"]),
        spec(&["go"], &["gopls"]),
        spec(&["c", "h", "cc", "cpp", "cxx", "hpp"], &["clangd"]),
    ]
}

/// The protocol's language identifier for a file extension.
fn language_id(ext: &str) -> &str {
    match ext {
        "rs" => "rust",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "py" | "pyi" => "python",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" => "cpp",
        other => other,
    }
}

/// `file://` URI of an absolute path, percent-encoded as servers expect.
pub(crate) fn file_uri(path: &Path) -> String {
    let raw = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !raw.starts_with('/') {
        out.push('/'); // Windows: file:///C:/…
    }
    for b in raw.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => out.push(b as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The path a `file://` URI names (the inverse of [`file_uri`], for showing it).
fn uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let mut bytes = Vec::with_capacity(rest.len());
    let raw = rest.as_bytes();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 3 <= raw.len() && rest.is_char_boundary(i + 1) && rest.is_char_boundary(i + 3) {
            if let Ok(b) = u8::from_str_radix(&rest[i + 1..i + 3], 16) {
                bytes.push(b);
                i += 3;
                continue;
            }
        }
        bytes.push(raw[i]);
        i += 1;
    }
    let s = String::from_utf8(bytes).ok()?;
    // file:///C:/… names C:/…
    let s = match s.as_bytes() {
        [b'/', drive, b':', ..] if drive.is_ascii_alphabetic() => s[1..].to_string(),
        _ => s,
    };
    Some(PathBuf::from(s))
}

/// One error a server reported.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Problem {
    pub file: PathBuf,
    /// One-based.
    pub line: u64,
    pub column: u64,
    pub message: String,
}

/// The errors (severity 1) of one `diagnostics` array. Warnings and hints are left out: they
/// are advice about code that works, and a reminder for each would drown the errors.
fn errors_of(file: &Path, diagnostics: &Value) -> Vec<Problem> {
    diagnostics
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d.get("severity").and_then(Value::as_u64).unwrap_or(1) == 1)
        .map(|d| Problem {
            file: file.to_path_buf(),
            line: d["range"]["start"]["line"].as_u64().unwrap_or(0) + 1,
            column: d["range"]["start"]["character"].as_u64().unwrap_or(0) + 1,
            message: d["message"].as_str().unwrap_or_default().lines().next().unwrap_or_default().to_string(),
        })
        .collect()
}

/// What the edit's result is told.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Report {
    pub server: String,
    pub problems: Vec<Problem>,
    /// The errors from before the edit were known and are left out: these are new.
    pub new_only: bool,
}

impl Report {
    /// The text added to the result, within `max_chars` (a share of the model's window).
    pub fn text(&self, root: &Path, max_chars: usize) -> String {
        let mut body = String::new();
        let mut shown = 0;
        for p in &self.problems {
            let file = p.file.strip_prefix(root).unwrap_or(&p.file).display();
            let line = format!("{file}:{}:{}: {}\n", p.line, p.column, p.message);
            if body.len() + line.len() > max_chars && shown > 0 {
                break;
            }
            body.push_str(&line);
            shown += 1;
        }
        let more = match self.problems.len() - shown {
            0 => String::new(),
            n => format!("(and {n} more)\n"),
        };
        let (what, advice) = if self.new_only {
            (
                " that it did not report before it",
                "Fix them before going on, unless a later edit you already planned resolves them.",
            )
        } else {
            (
                "",
                "Fix what this change caused before going on. An error that was there before it, or that a later \
                 edit you already planned resolves, needs nothing now.",
            )
        };
        format!(
            "\n\n<system-reminder>\nAfter this change, the language server ({}) reports {} error{}{what}:\n\
             {body}{more}{advice}\n</system-reminder>",
            self.server,
            self.problems.len(),
            if self.problems.len() == 1 { "" } else { "s" },
        )
    }
}

/// Published diagnostics, by URI, each with the count of publishes when it arrived.
#[derive(Default)]
struct Published {
    by_uri: HashMap<String, (u64, Value)>,
    count: u64,
}

struct Client {
    name: String,
    stdin: Arc<tokio::sync::Mutex<tokio::process::ChildStdin>>,
    next_id: AtomicU64,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>>,
    published: Arc<Mutex<Published>>,
    arrived: Arc<Notify>,
    /// The server answers `textDocument/diagnostic`.
    pulls: bool,
    /// Versions of the documents opened so far.
    open: Mutex<HashMap<String, i64>>,
    _child: tokio::process::Child,
}

async fn write_message(stdin: &mut tokio::process::ChildStdin, msg: &Value) -> std::io::Result<()> {
    let body = msg.to_string();
    stdin.write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes()).await?;
    stdin.write_all(body.as_bytes()).await?;
    stdin.flush().await
}

/// One framed message, or `None` at the end of the stream or on a broken frame.
async fn read_message<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R) -> Option<Value> {
    let mut len: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line).await.ok()? == 0 {
            return None;
        }
        let line = line.trim_end();
        if line.is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':') {
            if k.eq_ignore_ascii_case("content-length") {
                len = v.trim().parse().ok();
            }
        }
    }
    let mut body = vec![0u8; len?];
    r.read_exact(&mut body).await.ok()?;
    serde_json::from_slice(&body).ok()
}

impl Client {
    async fn start(spec: &ServerSpec, root: &Path) -> Option<Client> {
        let (program, args) = spec.command.split_first()?;
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args).current_dir(root).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        cmd.kill_on_drop(true);
        let mut child = cmd.spawn().ok()?;
        let stdin = Arc::new(tokio::sync::Mutex::new(child.stdin.take()?));
        let stdout = child.stdout.take()?;
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Value>>>> = Default::default();
        let published: Arc<Mutex<Published>> = Default::default();
        let arrived = Arc::new(Notify::new());
        {
            let (pending, published, arrived) = (pending.clone(), published.clone(), arrived.clone());
            let stdin = stdin.clone();
            tokio::spawn(async move {
                let mut reader = BufReader::new(stdout);
                while let Some(msg) = read_message(&mut reader).await {
                    let method = msg.get("method").and_then(Value::as_str);
                    match (method, msg.get("id")) {
                        (Some("textDocument/publishDiagnostics"), _) => {
                            if let Some(uri) = msg["params"]["uri"].as_str() {
                                let mut p = published.lock().unwrap();
                                p.count += 1;
                                let n = p.count;
                                p.by_uri.insert(uri.to_string(), (n, msg["params"]["diagnostics"].clone()));
                            }
                            arrived.notify_waiters();
                        }
                        // Requests the server makes of the client (configuration, capability
                        // registration, progress tokens) are answered with nothing, so it
                        // doesn't wait on them.
                        (Some(method), Some(id)) => {
                            let result = match method {
                                "workspace/configuration" => {
                                    let n = msg["params"]["items"].as_array().map(Vec::len).unwrap_or(0);
                                    json!(vec![Value::Null; n])
                                }
                                _ => Value::Null,
                            };
                            let reply = json!({"jsonrpc": "2.0", "id": id, "result": result});
                            if write_message(&mut *stdin.lock().await, &reply).await.is_err() {
                                break;
                            }
                        }
                        (None, Some(id)) => {
                            if let Some(tx) = id.as_u64().and_then(|id| pending.lock().unwrap().remove(&id)) {
                                let _ = tx.send(msg);
                            }
                        }
                        _ => {}
                    }
                }
                // The server is gone: whoever waits is let go.
                pending.lock().unwrap().clear();
                arrived.notify_waiters();
            });
        }
        let init = json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
            "processId": std::process::id(),
            "rootUri": file_uri(root),
            "workspaceFolders": [{"uri": file_uri(root), "name": "workspace"}],
            "capabilities": {
                "textDocument": {
                    "synchronization": {"didSave": false},
                    "publishDiagnostics": {"relatedInformation": false},
                    "diagnostic": {"dynamicRegistration": false},
                },
                "workspace": {"configuration": true, "workspaceFolders": true},
            },
        }});
        let (tx, rx) = oneshot::channel();
        pending.lock().unwrap().insert(0, tx);
        write_message(&mut *stdin.lock().await, &init).await.ok()?;
        let answer = tokio::time::timeout(START_TIMEOUT, rx).await.ok()?.ok()?;
        let pulls = !answer["result"]["capabilities"]["diagnosticProvider"].is_null();
        let ready = json!({"jsonrpc": "2.0", "method": "initialized", "params": {}});
        write_message(&mut *stdin.lock().await, &ready).await.ok()?;
        Some(Client {
            name: program.clone(),
            stdin,
            next_id: AtomicU64::new(1),
            pending,
            published,
            arrived,
            pulls,
            open: Mutex::new(HashMap::new()),
            _child: child,
        })
    }

    async fn send(&self, msg: &Value) -> Option<()> {
        write_message(&mut *self.stdin.lock().await, msg).await.ok()
    }

    /// The errors after `path` changed to `text`, within `wait`. `None` when the server did
    /// not answer in time.
    async fn diagnose(&self, path: &Path, text: &str, wait: Duration) -> Option<Vec<Problem>> {
        let uri = file_uri(path);
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        let since = self.published.lock().unwrap().count;
        let version = {
            let mut open = self.open.lock().unwrap();
            let v = open.entry(uri.clone()).or_insert(0);
            *v += 1;
            *v
        };
        let note = if version == 1 {
            json!({"jsonrpc": "2.0", "method": "textDocument/didOpen", "params": {"textDocument": {
                "uri": uri, "languageId": language_id(&ext), "version": version, "text": text}}})
        } else {
            json!({"jsonrpc": "2.0", "method": "textDocument/didChange", "params": {
                "textDocument": {"uri": uri, "version": version}, "contentChanges": [{"text": text}]}})
        };
        self.send(&note).await?;
        let deadline = tokio::time::Instant::now() + wait;
        if self.pulls {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            let (tx, rx) = oneshot::channel();
            self.pending.lock().unwrap().insert(id, tx);
            let ask = json!({"jsonrpc": "2.0", "id": id, "method": "textDocument/diagnostic",
                "params": {"textDocument": {"uri": uri}}});
            self.send(&ask).await?;
            let answer = tokio::time::timeout_at(deadline, rx).await.ok()?.ok()?;
            self.pending.lock().unwrap().remove(&id);
            // An error answer (the server isn't ready) is no verdict.
            let items = answer.get("result")?.get("items")?.clone();
            return Some(errors_of(path, &items));
        }
        // Published: wait for this file's, then a little longer for a fuller set.
        let mut settle: Option<tokio::time::Instant> = None;
        loop {
            let has = self.published.lock().unwrap().by_uri.get(&uri).is_some_and(|(n, _)| *n > since);
            if has && settle.is_none() {
                settle = Some((tokio::time::Instant::now() + SETTLE).min(deadline));
            }
            let until = settle.unwrap_or(deadline);
            let notified = self.arrived.notified();
            if tokio::time::timeout_at(until, notified).await.is_err() {
                break;
            }
        }
        let p = self.published.lock().unwrap();
        p.by_uri.get(&uri).filter(|(n, _)| *n > since)?;
        // This file's errors, then those of other files the server reported on since the
        // change: a caller this edit broke.
        let mut out = vec![];
        let mut others: Vec<(&String, &(u64, Value))> = p.by_uri.iter().filter(|(_, (n, _))| *n > since).collect();
        others.sort_by_key(|(u, _)| (*u != &uri, (*u).clone()));
        for (u, (_, diags)) in others {
            let file = if *u == uri { path.to_path_buf() } else { uri_path(u)? };
            out.extend(errors_of(&file, diags));
        }
        Some(out)
    }
}

/// The session's language servers: started on the first edit of a file they serve.
pub(crate) struct Manager {
    cfg: LspConfig,
    root: PathBuf,
    /// Per extension: the running server, or `None` once none could be started or one stopped
    /// answering (it is not tried again this session).
    by_ext: tokio::sync::Mutex<HashMap<String, Option<Arc<Client>>>>,
    /// The error messages last reported for each file: the baseline the next edit is compared
    /// with, so an error that was already there is not told again with every edit.
    known: Mutex<HashMap<PathBuf, Vec<String>>>,
    /// A hash of the text each file had when it was last checked: a file that differs from
    /// it was changed by something else (a shell command), and is checked again.
    checked: Mutex<HashMap<PathBuf, u64>>,
}

fn text_hash(text: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

impl Manager {
    pub fn new(cfg: LspConfig, root: PathBuf) -> Self {
        Manager { cfg, root, by_ext: Default::default(), known: Default::default(), checked: Default::default() }
    }

    pub fn config(&self) -> &LspConfig {
        &self.cfg
    }

    async fn client_for(&self, ext: &str) -> Option<Arc<Client>> {
        let mut by_ext = self.by_ext.lock().await;
        if let Some(known) = by_ext.get(ext) {
            return known.clone();
        }
        let mut started = None;
        for spec in self.cfg.servers.iter().filter(|s| s.extensions.iter().any(|e| e == ext)) {
            if let Some(c) = Client::start(spec, &self.root).await {
                started = Some(Arc::new(c));
                break;
            }
        }
        // One server may serve several extensions: share it.
        if let Some(c) = &started {
            let spec = self.cfg.servers.iter().find(|s| s.command.first() == Some(&c.name));
            for e in spec.into_iter().flat_map(|s| s.extensions.iter()) {
                by_ext.entry(e.clone()).or_insert_with(|| started.clone());
            }
        }
        by_ext.insert(ext.to_string(), started.clone());
        started
    }

    /// Record `problems` as what is known of their files (and of `path`, which may have none),
    /// and return the ones that were not known: per file, by message, since an edit moves
    /// lines; one more of a message that was there twice is new once.
    fn take_new(&self, path: &Path, problems: Vec<Problem>) -> Vec<Problem> {
        let mut known = self.known.lock().unwrap();
        let mut now: HashMap<PathBuf, Vec<String>> = HashMap::new();
        now.insert(path.to_path_buf(), vec![]);
        let mut left: HashMap<PathBuf, Vec<String>> = HashMap::new();
        let mut new = vec![];
        for p in problems {
            now.entry(p.file.clone()).or_default().push(p.message.clone());
            let before = left.entry(p.file.clone()).or_insert_with(|| known.get(&p.file).cloned().unwrap_or_default());
            match before.iter().position(|m| *m == p.message) {
                Some(i) => {
                    before.swap_remove(i);
                }
                None => new.push(p),
            }
        }
        known.extend(now);
        new
    }

    /// After a shell command: the files this session already had checked whose text is no
    /// longer what was checked (`sed -i`, a code generator, `git checkout`), each checked
    /// again against what the server last said of it. Only those files: the command's other
    /// output is its own business, and the project's checks cover it.
    pub async fn check_changed(&self) -> Vec<Report> {
        let before: Vec<(PathBuf, u64)> = self.checked.lock().unwrap().iter().map(|(p, h)| (p.clone(), *h)).collect();
        let mut out = vec![];
        for (path, hash) in before {
            let Ok(text) = tokio::fs::read_to_string(&path).await else { continue };
            if text_hash(&text) != hash {
                out.extend(self.check(&path, None).await);
            }
        }
        out
    }

    /// The errors a language server reports for `path` as it is on disk now, without those it
    /// reported before the edit. `original` is the file's text before the edit, when the tool
    /// gave it: the baseline for a file the server has not been shown yet (an empty one is a
    /// new file, with nothing before). `None` when no server serves the file, the edit added
    /// no error, or the server did not answer in time.
    pub async fn check(&self, path: &Path, original: Option<&str>) -> Option<Report> {
        let ext = path.extension().and_then(|e| e.to_str())?.to_ascii_lowercase();
        if !self.cfg.servers.iter().any(|s| s.extensions.contains(&ext)) {
            return None;
        }
        let text = tokio::fs::read_to_string(path).await.ok()?;
        let client = self.client_for(&ext).await?;
        self.checked.lock().unwrap().insert(path.to_path_buf(), text_hash(&text));
        let seen = self.known.lock().unwrap().contains_key(path);
        let mut new_only = seen;
        let mut answer = Some(vec![]);
        if !seen {
            match original {
                Some("") => new_only = true,
                Some(before) => {
                    answer = client.diagnose(path, before, self.cfg.wait).await;
                    if let Some(problems) = answer.clone() {
                        self.take_new(path, problems);
                        new_only = true;
                    }
                }
                None => {}
            }
        }
        if answer.is_some() {
            answer = client.diagnose(path, &text, self.cfg.wait).await;
        }
        match answer.map(|problems| self.take_new(path, problems)) {
            Some(problems) if problems.is_empty() => None,
            Some(problems) => Some(Report { server: client.name.clone(), problems, new_only }),
            None => {
                // No answer in time: a server still indexing, or stuck. Each edit would wait
                // again for nothing, so it is left alone for the rest of the session.
                let mut by_ext = self.by_ext.lock().await;
                for slot in by_ext.values_mut() {
                    if slot.as_ref().is_some_and(|c| Arc::ptr_eq(c, &client)) {
                        *slot = None;
                    }
                }
                None
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn uris_round_trip() {
        let p = Path::new("/home/me/my project/a b.py");
        let uri = file_uri(p);
        assert_eq!(uri, "file:///home/me/my%20project/a%20b.py");
        assert_eq!(uri_path(&uri).unwrap(), p);
        assert_eq!(file_uri(Path::new("C:\\src\\a.rs")), "file:///C:/src/a.rs");
        assert_eq!(uri_path("file:///C:/src/a.rs").unwrap(), Path::new("C:/src/a.rs"));
        assert_eq!(uri_path("untitled:1"), None);
    }

    #[test]
    fn only_errors_are_kept() {
        let d = json!([
            {"severity": 1, "message": "mismatched types\nexpected u32", "range": {"start": {"line": 4, "character": 8}}},
            {"severity": 2, "message": "unused variable", "range": {"start": {"line": 1, "character": 0}}},
            {"message": "no severity means error", "range": {"start": {"line": 0, "character": 0}}},
        ]);
        let e = errors_of(Path::new("/p/a.rs"), &d);
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].line, e[0].column, e[0].message.as_str()), (5, 9, "mismatched types"));
    }

    #[test]
    fn the_report_fits_its_room_and_says_what_it_left_out() {
        let problems: Vec<Problem> = (1..=50)
            .map(|i| Problem { file: "/p/src/a.rs".into(), line: i, column: 1, message: format!("error number {i}") })
            .collect();
        let r = Report { server: "rust-analyzer".into(), problems, new_only: false };
        let small = r.text(Path::new("/p"), 200);
        assert!(small.contains("reports 50 errors") && small.contains("src/a.rs:1:1: error number 1"), "{small}");
        assert!(small.contains("more)") && !small.contains("error number 50"), "{small}");
        let large = r.text(Path::new("/p"), 100_000);
        assert!(large.contains("error number 50") && !large.contains("more)"));
        let one = Report { server: "gopls".into(), problems: r.problems[..1].to_vec(), new_only: false };
        assert!(one.text(Path::new("/p"), 10).contains("reports 1 error:"), "one is always shown");
    }

    /// A stand-in server: answers `initialize`, and after each open or change publishes one
    /// error for a file whose text holds `BROKEN`, none otherwise. `pull` makes it answer
    /// `textDocument/diagnostic` instead; `slow` makes it never answer after `initialize`.
    const FAKE_SERVER: &str = r#"
import json, sys
mode = sys.argv[1] if len(sys.argv) > 1 else "push"
texts = {}
def send(msg):
    body = json.dumps(msg).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()
def diags(uri):
    if "BROKEN" not in texts.get(uri, ""):
        return []
    return [{"severity": 1, "message": "name BROKEN is not defined", "range": {"start": {"line": 2, "character": 4}}},
            {"severity": 2, "message": "a warning", "range": {"start": {"line": 0, "character": 0}}}]
while True:
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            sys.exit(0)
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    msg = json.loads(sys.stdin.buffer.read(length))
    method = msg.get("method")
    if method == "initialize":
        caps = {"diagnosticProvider": {}} if mode == "pull" else {}
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"capabilities": caps}})
        send({"jsonrpc": "2.0", "id": 900, "method": "workspace/configuration", "params": {"items": [{}]}})
    elif mode == "slow":
        continue
    elif method in ("textDocument/didOpen", "textDocument/didChange"):
        doc = msg["params"]["textDocument"]
        texts[doc["uri"]] = doc.get("text") or msg["params"]["contentChanges"][0]["text"]
        if mode == "push":
            send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics",
                  "params": {"uri": doc["uri"], "diagnostics": diags(doc["uri"])}})
    elif method == "textDocument/diagnostic":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"kind": "full", "items": diags(msg["params"]["textDocument"]["uri"])}})
"#;

    pub(crate) fn python() -> Option<&'static str> {
        ["python3", "python"].into_iter().find(|p| std::process::Command::new(p).arg("--version").output().is_ok())
    }

    pub(crate) fn fake(dir: &Path, mode: &str, wait_ms: u64) -> Option<LspConfig> {
        let py = python()?;
        let script = dir.join("fake_lsp.py");
        std::fs::write(&script, FAKE_SERVER).unwrap();
        Some(LspConfig {
            servers: vec![ServerSpec {
                extensions: vec!["zz".into()],
                command: vec![py.into(), script.display().to_string(), mode.into()],
            }],
            wait: Duration::from_millis(wait_ms),
        })
    }

    #[tokio::test]
    async fn errors_are_reported_and_clean_files_are_not() {
        for mode in ["push", "pull"] {
            let dir = tempfile::tempdir().unwrap();
            let Some(cfg) = fake(dir.path(), mode, 5_000) else { return };
            let m = Manager::new(cfg, dir.path().to_path_buf());
            let f = dir.path().join("a.zz");
            std::fs::write(&f, "ok\n").unwrap();
            assert_eq!(m.check(&f, None).await, None, "{mode}: a clean file adds nothing");
            std::fs::write(&f, "ok\n\n    BROKEN\n").unwrap();
            let r = m.check(&f, None).await.unwrap_or_else(|| panic!("{mode}: no report"));
            assert_eq!(r.problems.len(), 1, "{mode}: the warning is left out");
            assert_eq!((r.problems[0].line, r.problems[0].column), (3, 5));
            assert!(r.text(dir.path(), 2_000).contains("a.zz:3:5: name BROKEN is not defined"));
            // Fixed again: the server is told the new text and reports nothing.
            std::fs::write(&f, "ok\n").unwrap();
            assert_eq!(m.check(&f, None).await, None, "{mode}: fixed");
            // A file no server serves is not looked at.
            let other = dir.path().join("a.txt");
            std::fs::write(&other, "BROKEN").unwrap();
            assert_eq!(m.check(&other, None).await, None);
        }
    }

    #[tokio::test]
    async fn a_server_that_is_missing_or_silent_costs_one_wait_and_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.zz");
        std::fs::write(&f, "BROKEN").unwrap();
        // Not installed.
        let missing = LspConfig {
            servers: vec![ServerSpec {
                extensions: vec!["zz".into()],
                command: vec!["no-such-language-server".into()],
            }],
            wait: Duration::from_millis(200),
        };
        let m = Manager::new(missing, dir.path().to_path_buf());
        assert_eq!(m.check(&f, None).await, None);
        assert_eq!(m.check(&f, None).await, None);
        // Starts, then never answers: the first edit waits, the next ones don't.
        let Some(cfg) = fake(dir.path(), "slow", 300) else { return };
        let m = Manager::new(cfg, dir.path().to_path_buf());
        let t = std::time::Instant::now();
        assert_eq!(m.check(&f, None).await, None);
        assert!(t.elapsed() >= Duration::from_millis(250));
        let t = std::time::Instant::now();
        assert_eq!(m.check(&f, None).await, None);
        assert!(t.elapsed() < Duration::from_millis(200), "not asked again: {:?}", t.elapsed());
    }

    /// An error the file already had is not told again with every edit: the text before the
    /// edit is the baseline, and after that what the server last said.
    #[tokio::test]
    async fn only_the_errors_an_edit_added_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for mode in ["push", "pull"] {
            let Some(cfg) = fake(&root, mode, 3000) else { return };
            let m = Manager::new(cfg, root.clone());
            // Broken before the edit and after it: nothing new.
            let old = root.join(format!("old-{mode}.zz"));
            std::fs::write(&old, "BROKEN\nand one more line\n").unwrap();
            assert_eq!(m.check(&old, Some("BROKEN\n")).await, None, "{mode}");
            assert_eq!(m.check(&old, None).await, None, "{mode}: still the same error");
            // Fine before, broken by the edit: told, as new.
            let f = root.join(format!("new-{mode}.zz"));
            std::fs::write(&f, "x\ny\nBROKEN\n").unwrap();
            let r = m.check(&f, Some("x\ny\n")).await.unwrap_or_else(|| panic!("{mode}: no report"));
            assert!(r.new_only && r.problems.len() == 1, "{r:?}");
            assert!(r.text(&root, 2_000).contains("that it did not report before it"));
            // A file created broken has nothing before it, and costs no baseline pass.
            let made = root.join(format!("made-{mode}.zz"));
            std::fs::write(&made, "BROKEN").unwrap();
            assert!(m.check(&made, Some("")).await.unwrap().new_only, "{mode}");
            // No text from before (unknown): everything is told, and said to be possibly old.
            let unknown = root.join(format!("unknown-{mode}.zz"));
            std::fs::write(&unknown, "BROKEN").unwrap();
            let r = m.check(&unknown, None).await.unwrap();
            assert!(!r.new_only && r.text(&root, 2_000).contains("was there before"), "{mode}");
        }
    }

    /// A shell command that rewrites a file the session already had checked gets the same
    /// check as an edit; files it left alone, or never seen here, are not looked at.
    #[tokio::test]
    async fn files_a_shell_command_changed_are_checked_again() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let Some(cfg) = fake(&root, "push", 3000) else { return };
        let m = Manager::new(cfg, root.clone());
        let (a, b, unseen) = (root.join("a.zz"), root.join("b.zz"), root.join("unseen.zz"));
        std::fs::write(&a, "fine\n").unwrap();
        std::fs::write(&b, "fine\n").unwrap();
        assert_eq!(m.check(&a, Some("")).await, None);
        assert_eq!(m.check(&b, Some("")).await, None);
        assert!(m.check_changed().await.is_empty(), "nothing changed");
        // As `sed -i` would.
        std::fs::write(&a, "now BROKEN\n").unwrap();
        std::fs::write(&unseen, "BROKEN\n").unwrap();
        let reports = m.check_changed().await;
        assert_eq!(reports.len(), 1, "{reports:?}");
        assert!(reports[0].new_only && reports[0].problems[0].file == a, "{reports:?}");
        assert!(m.check_changed().await.is_empty(), "told once");
    }
}
