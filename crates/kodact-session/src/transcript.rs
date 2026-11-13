use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use kodact_types::{new_uuid, ApiMessage, Message};
use serde_json::{json, Value};

use crate::store::SessionStore;
use crate::{now, SessionError};

/// One loaded conversation message with its transcript id.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub uuid: String,
    pub message: Message,
    /// Synthetic (reminders, interrupt markers) rather than typed by a person.
    pub is_meta: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LoadedSession {
    pub session_id: String,
    pub cwd: PathBuf,
    pub messages: Vec<Entry>,
    pub additional_dirs: Vec<PathBuf>,
    pub microcompacted: HashSet<String>,
    pub title: Option<String>,
    pub last_uuid: Option<String>,
    pub permission_mode: Option<String>,
    pub model: Option<String>,
    pub worktree: Option<Value>,
    /// The task list recorded at the last compaction (`system/todos`).
    pub todos: Option<Vec<Value>>,
    /// The person's requests recorded at the last compaction (`system/requests`), oldest first:
    /// the next compaction carries them on word for word.
    pub requests: Option<Vec<String>>,
    /// The last `/goal` record. Kept outside the message chain, like the
    /// title, so compaction doesn't drop it.
    pub goal: Option<Value>,
    /// The last list of scheduled tasks (`CronCreate`, `/loop`), kept outside the chain.
    pub schedule: Option<Value>,
    /// The last turn began (`system/turn_start`) and never ended (`system/turn_end`): the
    /// process was killed or crashed mid-turn. Transcripts written before these records
    /// existed never say so.
    pub cut_off: bool,
    /// What each signed thinking block was written on (`thinking_prefix` records), keyed by a
    /// hash of its signature: a block whose history changed since is left out of requests.
    /// Kept outside the message chain, so a rewind or compaction doesn't lose the records of
    /// the blocks it kept.
    pub thinking_prefixes: HashMap<String, String>,
}

/// Append-only writer for one session's JSONL file.
pub struct Transcript {
    path: Option<PathBuf>,
    pub session_id: String,
    pub cwd: PathBuf,
    pub version: String,
    pub git_branch: Option<String>,
    last_uuid: Mutex<Option<String>>,
    file: Mutex<Option<File>>,
    title: Mutex<Option<String>>,
}

impl Transcript {
    /// A new session. With `persist == false` nothing is written (`--no-session-persistence`).
    pub fn create(
        store: &SessionStore,
        cwd: &Path,
        session_id: &str,
        git_branch: Option<String>,
        persist: bool,
    ) -> Result<Self, SessionError> {
        let path = if persist {
            let p = store.session_path(cwd, session_id)?;
            if let Some(dir) = p.parent() {
                std::fs::create_dir_all(dir)?;
            }
            store.register_project(cwd, None)?;
            Some(p)
        } else {
            crate::validate_session_id(session_id)?;
            None
        };
        Ok(Transcript {
            path,
            session_id: session_id.to_string(),
            cwd: cwd.to_path_buf(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            git_branch,
            last_uuid: Mutex::new(None),
            file: Mutex::new(None),
            title: Mutex::new(None),
        })
    }

    /// Continue writing after a loaded session (same file), or into a fork.
    /// A fork keeps the leaf [`Transcript::write_fork_of`] set: the loaded
    /// session's leaf may be an entry the fork never copied.
    pub fn continue_from(&self, loaded: &LoadedSession) {
        if self.session_id == loaded.session_id {
            *self.last_uuid.lock().unwrap() = loaded.last_uuid.clone();
            *self.title.lock().unwrap() = loaded.title.clone();
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn last_uuid(&self) -> Option<String> {
        self.last_uuid.lock().unwrap().clone()
    }

    /// Point the chain at an earlier entry (rewind / resume-at): later entries branch from it.
    pub fn set_leaf(&self, uuid: Option<String>) {
        *self.last_uuid.lock().unwrap() = uuid;
    }

    fn write_line(&self, v: &Value) {
        let Some(path) = &self.path else { return };
        let mut guard = self.file.lock().unwrap();
        if guard.is_none() {
            // A crash can leave a torn last line. The loader skips it while it is
            // last; cut it off before appending, or the file would stop loading.
            drop_torn_tail(path);
            match OpenOptions::new().create(true).append(true).open(path) {
                Ok(f) => *guard = Some(f),
                Err(e) => {
                    tracing::warn!(error = %e, "cannot open transcript");
                    return;
                }
            }
        }
        if let Some(f) = guard.as_mut() {
            let mut line = serde_json::to_string(v).unwrap_or_default();
            line.push('\n');
            if let Err(e) = f.write_all(line.as_bytes()).and_then(|_| f.flush()) {
                tracing::warn!(error = %e, "cannot write transcript");
            }
        }
    }

    fn base(&self, kind: &str, uuid: &str, parent: Option<String>) -> Value {
        json!({
            "type": kind,
            "uuid": uuid,
            "parentUuid": parent,
            "sessionId": self.session_id,
            "timestamp": now(),
            "cwd": self.cwd,
            "version": self.version,
            "gitBranch": self.git_branch,
        })
    }

    /// Append a chained entry; returns its uuid.
    /// The leaf lock is held from reading the parent to moving the leaf, so two
    /// writers (the engine and a host's control request) can't share a parent.
    fn chained(&self, kind: &str, fields: Value) -> String {
        let uuid = new_uuid();
        let mut leaf = self.last_uuid.lock().unwrap();
        let mut v = self.base(kind, &uuid, leaf.clone());
        merge(&mut v, fields);
        self.write_line(&v);
        *leaf = Some(uuid.clone());
        uuid
    }

    pub fn append_user(&self, msg: &Message, is_meta: bool, extra: Value) -> String {
        let mut fields = json!({"message": msg});
        if is_meta {
            fields["isMeta"] = json!(true);
        }
        merge(&mut fields, extra);
        self.chained("user", fields)
    }

    pub fn append_assistant(&self, msg: &ApiMessage) -> String {
        self.chained("assistant", json!({"message": msg, "requestId": msg.id}))
    }

    pub fn append_system(&self, subtype: &str, data: Value) -> String {
        let mut fields = json!({"subtype": subtype});
        merge(&mut fields, data);
        self.chained("system", fields)
    }

    /// Write a message again, under its original uuid, after the current leaf
    /// (a partial summary keeps the messages around it). The loader takes the
    /// newest entry for a uuid, so the chain follows the copy.
    pub fn rewrite(&self, uuid: &str, msg: &Message, is_meta: bool) {
        let kind = if msg.role == kodact_types::Role::Assistant { "assistant" } else { "user" };
        let mut leaf = self.last_uuid.lock().unwrap();
        let mut v = self.base(kind, uuid, leaf.clone());
        merge(&mut v, json!({"message": msg}));
        if is_meta {
            v["isMeta"] = json!(true);
        }
        self.write_line(&v);
        *leaf = Some(uuid.to_string());
    }

    /// Start a new chain after compaction; earlier messages are not loaded on resume.
    pub fn append_compact_boundary(&self, data: Value) -> String {
        let uuid = new_uuid();
        let mut leaf = self.last_uuid.lock().unwrap();
        let mut v = self.base("system", &uuid, None);
        merge(&mut v, json!({"subtype": "compact_boundary", "logicalParentUuid": *leaf, "compactMetadata": data}));
        self.write_line(&v);
        *leaf = Some(uuid.clone());
        uuid
    }

    /// Unchained metadata (additional directories, worktree, ...).
    pub fn append_meta(&self, data: Value) {
        let uuid = new_uuid();
        let mut v = self.base("session_meta", &uuid, None);
        merge(&mut v, data);
        self.write_line(&v);
    }

    pub fn set_title(&self, title: &str) {
        *self.title.lock().unwrap() = Some(title.to_string());
        self.write_line(&json!({"type": "title", "title": title, "sessionId": self.session_id, "timestamp": now()}));
    }

    /// Record the session's goal (`/goal`): its condition and status.
    pub fn set_goal(&self, goal: Value) {
        self.write_line(&json!({"type": "goal", "goal": goal, "sessionId": self.session_id, "timestamp": now()}));
    }

    /// Record what the signed thinking blocks of a reply were written on: `blocks` maps a hash
    /// of each block's signature to the hash of the request before it.
    pub fn record_thinking_prefixes(&self, blocks: Value) {
        self.write_line(
            &json!({"type": "thinking_prefix", "blocks": blocks, "sessionId": self.session_id, "timestamp": now()}),
        );
    }

    /// Record the session's scheduled tasks.
    pub fn set_schedule(&self, tasks: Value) {
        self.write_line(&json!({"type": "schedule", "tasks": tasks, "sessionId": self.session_id, "timestamp": now()}));
    }

    /// The session's name, if it has one (`--name`, `/rename`, or a resumed title).
    pub fn title(&self) -> Option<String> {
        self.title.lock().unwrap().clone()
    }

    /// Copy a loaded chain into this (new) transcript: `--fork-session`.
    pub fn write_fork_of(&self, loaded: &LoadedSession) {
        let mut parent: Option<String> = None;
        for e in &loaded.messages {
            let kind = match e.message.role {
                kodact_types::Role::User => "user",
                kodact_types::Role::Assistant => "assistant",
            };
            let mut v = self.base(kind, &e.uuid, parent.clone());
            merge(&mut v, json!({"message": e.message, "forkedFrom": loaded.session_id}));
            if e.is_meta {
                v["isMeta"] = json!(true);
            }
            self.write_line(&v);
            parent = Some(e.uuid.clone());
        }
        if let Some(todos) = &loaded.todos {
            let mut v = self.base("system", &new_uuid(), parent.clone());
            merge(&mut v, json!({"subtype": "todos", "todos": todos}));
            let id = v["uuid"].as_str().unwrap_or_default().to_string();
            self.write_line(&v);
            parent = Some(id);
        }
        if !loaded.microcompacted.is_empty() {
            let mut v = self.base("system", &new_uuid(), parent.clone());
            merge(&mut v, json!({"subtype": "microcompact", "toolUseIds": loaded.microcompacted}));
            let id = v["uuid"].as_str().unwrap_or_default().to_string();
            self.write_line(&v);
            parent = Some(id);
        }
        if !loaded.thinking_prefixes.is_empty() {
            self.record_thinking_prefixes(json!(loaded.thinking_prefixes));
        }
        if !loaded.additional_dirs.is_empty() {
            self.append_meta(json!({"additionalDirectories": loaded.additional_dirs}));
        }
        *self.last_uuid.lock().unwrap() = parent;
    }
}

/// Cut an unfinished last line (a crash mid-write) off a transcript.
fn drop_torn_tail(path: &Path) {
    use std::io::{Read as _, Seek as _, SeekFrom};
    let Ok(mut f) = File::open(path) else { return };
    let mut last = [0u8; 1];
    let torn = f.seek(SeekFrom::End(-1)).is_ok() && f.read_exact(&mut last).is_ok() && last[0] != b'\n';
    if !torn {
        return;
    }
    let Ok(bytes) = std::fs::read(path) else { return };
    let keep = bytes.iter().rposition(|b| *b == b'\n').map(|i| i + 1).unwrap_or(0);
    if let Ok(f) = OpenOptions::new().write(true).open(path) {
        let _ = f.set_len(keep as u64);
    }
}

fn merge(into: &mut Value, from: Value) {
    if let (Value::Object(a), Value::Object(b)) = (into, from) {
        for (k, v) in b {
            a.insert(k, v);
        }
    }
}

/// Load a transcript, following the chain back from `leaf` (default: the last chained entry).
pub fn load(path: &Path, leaf: Option<&str>) -> Result<LoadedSession, SessionError> {
    let text = std::fs::read_to_string(path)?;
    let mut entries: Vec<Value> = vec![];
    let mut by_uuid: HashMap<String, usize> = HashMap::new();
    let mut out = LoadedSession::default();
    for (n, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            // A torn final line from a crash is tolerated; anything else is corruption.
            Err(_) if n + 1 == text.lines().count() => break,
            Err(e) => return Err(SessionError::Corrupt(format!("line {}: {e}", n + 1))),
        };
        match v.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                if let Some(d) = v.get("additionalDirectories").and_then(Value::as_array) {
                    out.additional_dirs = d.iter().filter_map(Value::as_str).map(PathBuf::from).collect();
                }
                if let Some(w) = v.get("worktree") {
                    out.worktree = Some(w.clone());
                }
            }
            Some("title") => out.title = v.get("title").and_then(Value::as_str).map(str::to_string),
            Some("goal") => out.goal = v.get("goal").cloned(),
            Some("schedule") => out.schedule = v.get("tasks").cloned(),
            Some("thinking_prefix") => {
                for (k, h) in v["blocks"].as_object().into_iter().flatten() {
                    if let Some(h) = h.as_str() {
                        out.thinking_prefixes.insert(k.clone(), h.to_string());
                    }
                }
            }
            Some(_) => {
                if let Some(u) = v.get("uuid").and_then(Value::as_str) {
                    by_uuid.insert(u.to_string(), entries.len());
                    entries.push(v);
                }
            }
            None => {}
        }
    }
    if out.session_id.is_empty() {
        out.session_id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    }
    let start = match leaf {
        Some(l) => Some(*by_uuid.get(l).ok_or_else(|| SessionError::NotFound(format!("message {l}")))?),
        None => entries.iter().rposition(|e| matches!(e["type"].as_str(), Some("user" | "assistant" | "system"))),
    };
    let mut chain = vec![];
    let mut cur = start;
    let mut seen = HashSet::new();
    while let Some(i) = cur {
        if !seen.insert(i) {
            return Err(SessionError::Corrupt("parentUuid cycle".into()));
        }
        chain.push(i);
        cur = entries[i].get("parentUuid").and_then(Value::as_str).and_then(|p| by_uuid.get(p).copied());
    }
    chain.reverse();
    out.last_uuid = start.and_then(|i| entries[i]["uuid"].as_str().map(str::to_string));
    for i in chain {
        let e = &entries[i];
        if out.cwd.as_os_str().is_empty() {
            if let Some(c) = e.get("cwd").and_then(Value::as_str) {
                out.cwd = PathBuf::from(c);
            }
        }
        match e["type"].as_str() {
            Some("user") | Some("assistant") => {
                let message: Message = match serde_json::from_value(e["message"].clone()) {
                    Ok(m) => m,
                    Err(err) => return Err(SessionError::Corrupt(format!("message {}: {err}", e["uuid"]))),
                };
                if e["type"] == "assistant" {
                    out.model = e.pointer("/message/model").and_then(Value::as_str).map(str::to_string);
                }
                out.messages.push(Entry {
                    uuid: e["uuid"].as_str().unwrap_or_default().to_string(),
                    message,
                    is_meta: e.get("isMeta").and_then(Value::as_bool).unwrap_or(false),
                });
            }
            Some("system") => match e["subtype"].as_str() {
                Some("turn_start") => out.cut_off = true,
                Some("turn_end") => out.cut_off = false,
                Some("microcompact") => {
                    for id in e["toolUseIds"].as_array().into_iter().flatten().filter_map(Value::as_str) {
                        out.microcompacted.insert(id.to_string());
                    }
                }
                Some("permission_mode") => out.permission_mode = e["mode"].as_str().map(str::to_string),
                Some("todos") => out.todos = e["todos"].as_array().cloned(),
                Some("requests") => {
                    out.requests = e["requests"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok(out)
}

impl LoadedSession {
    pub fn load(path: &Path, leaf: Option<&str>) -> Result<Self, SessionError> {
        load(path, leaf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::project_key;
    use kodact_types::{ContentBlock, Role, StopReason, Usage};

    fn assistant(text: &str) -> ApiMessage {
        ApiMessage {
            id: "msg".into(),
            kind: "message".into(),
            role: Role::Assistant,
            model: "m".into(),
            content: vec![ContentBlock::text(text)],
            stop_reason: Some(StopReason::EndTurn),
            stop_sequence: None,
            usage: Usage::default(),
        }
    }

    const ID: &str = "11111111-2222-3333-4444-555555555555";
    const ID2: &str = "aaaaaaaa-2222-3333-4444-555555555555";

    #[test]
    fn write_and_resume_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let store = SessionStore::new(d.path().join("projects"));
        let cwd = PathBuf::from("/work/app");
        let t = Transcript::create(&store, &cwd, ID, Some("main".into()), true).unwrap();
        t.append_user(&Message::user_text("hello"), false, json!({}));
        t.append_assistant(&assistant("hi"));
        t.append_system("microcompact", json!({"toolUseIds": ["toolu_1"]}));
        t.append_meta(json!({"additionalDirectories": ["/extra"]}));
        t.record_thinking_prefixes(json!({"aa": "01", "bb": "02"}));
        t.record_thinking_prefixes(json!({"cc": "03"}));
        let path = store.session_path(&cwd, ID).unwrap();
        let loaded = load(&path, None).unwrap();
        assert_eq!(loaded.thinking_prefixes.len(), 3, "every record, outside the chain");
        assert_eq!(loaded.thinking_prefixes["cc"], "03");
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[1].message.text(), "hi");
        assert!(loaded.microcompacted.contains("toolu_1"));
        assert_eq!(loaded.additional_dirs, vec![PathBuf::from("/extra")]);
        assert_eq!(loaded.cwd, cwd);
        let listed = store.list(&cwd);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].first_prompt, "hello");
        assert_eq!(store.find(ID).unwrap(), path);
    }

    #[test]
    fn c5_key_collision_keeps_projects_apart() {
        let d = tempfile::tempdir().unwrap();
        let store = SessionStore::new(d.path().join("projects"));
        let (a, b) = (PathBuf::from("/a-b"), PathBuf::from("/a/b"));
        assert_eq!(project_key(&a), project_key(&b));
        let ta = Transcript::create(&store, &a, ID, None, true).unwrap();
        ta.append_user(&Message::user_text("in a-b"), false, json!({}));
        let tb = Transcript::create(&store, &b, ID2, None, true).unwrap();
        tb.append_user(&Message::user_text("in a/b"), false, json!({}));
        let la = store.list(&a);
        let lb = store.list(&b);
        assert_eq!(la.len(), 1);
        assert_eq!(lb.len(), 1);
        assert_eq!(la[0].first_prompt, "in a-b");
        assert_eq!(store.latest(&b).unwrap().first_prompt, "in a/b");
    }

    #[test]
    fn long_keys_are_hashed() {
        let long = PathBuf::from(format!("/{}", "x".repeat(300)));
        let k = project_key(&long);
        assert!(k.len() <= 200, "{}", k.len());
        assert_ne!(k, project_key(&PathBuf::from(format!("/{}", "x".repeat(301)))));
    }

    #[test]
    fn compact_boundary_cuts_history_and_leaf_rewinds() {
        let d = tempfile::tempdir().unwrap();
        let store = SessionStore::new(d.path().to_path_buf());
        let cwd = PathBuf::from("/w");
        let t = Transcript::create(&store, &cwd, ID, None, true).unwrap();
        t.append_user(&Message::user_text("old"), false, json!({}));
        t.append_compact_boundary(json!({"trigger": "manual"}));
        let first = t.append_user(&Message::user_text("summary"), true, json!({}));
        t.append_assistant(&assistant("after"));
        let path = store.session_path(&cwd, ID).unwrap();
        let loaded = load(&path, None).unwrap();
        assert_eq!(loaded.messages.len(), 2);
        assert_eq!(loaded.messages[0].message.text(), "summary");
        // Branch from an earlier message.
        t.set_leaf(Some(first.clone()));
        t.append_assistant(&assistant("branch"));
        let loaded = load(&path, None).unwrap();
        assert_eq!(loaded.messages.iter().map(|e| e.message.text()).collect::<Vec<_>>(), vec!["summary", "branch"]);
        let at = load(&path, Some(&first)).unwrap();
        assert_eq!(at.messages.len(), 1);
    }

    #[test]
    fn fork_copies_chain_under_new_id() {
        let d = tempfile::tempdir().unwrap();
        let store = SessionStore::new(d.path().to_path_buf());
        let cwd = PathBuf::from("/w");
        let t = Transcript::create(&store, &cwd, ID, None, true).unwrap();
        t.append_user(&Message::user_text("q"), false, json!({}));
        t.append_assistant(&assistant("a"));
        let loaded = load(&store.session_path(&cwd, ID).unwrap(), None).unwrap();
        let f = Transcript::create(&store, &cwd, ID2, None, true).unwrap();
        f.write_fork_of(&loaded);
        f.append_user(&Message::user_text("more"), false, json!({}));
        let forked = load(&store.session_path(&cwd, ID2).unwrap(), None).unwrap();
        assert_eq!(forked.messages.len(), 3);
        assert_eq!(load(&store.session_path(&cwd, ID).unwrap(), None).unwrap().messages.len(), 2);
    }

    #[test]
    fn invalid_ids_and_torn_lines() {
        let d = tempfile::tempdir().unwrap();
        let store = SessionStore::new(d.path().to_path_buf());
        assert!(store.session_path(Path::new("/w"), "../../etc/passwd").is_err());
        let cwd = PathBuf::from("/w");
        let t = Transcript::create(&store, &cwd, ID, None, true).unwrap();
        t.append_user(&Message::user_text("q"), false, json!({}));
        let path = store.session_path(&cwd, ID).unwrap();
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"type\":\"user\",\"uu").unwrap();
        assert_eq!(load(&path, None).unwrap().messages.len(), 1);
    }

    #[test]
    fn no_persistence_writes_nothing() {
        let d = tempfile::tempdir().unwrap();
        let store = SessionStore::new(d.path().join("p"));
        let t = Transcript::create(&store, Path::new("/w"), ID, None, false).unwrap();
        t.append_user(&Message::user_text("q"), false, json!({}));
        assert!(!d.path().join("p").exists());
    }

    #[test]
    fn a_fork_keeps_its_own_leaf_and_survives_a_torn_line() {
        let d = tempfile::tempdir().unwrap();
        let store = SessionStore::new(d.path().join("projects"));
        let cwd = PathBuf::from("/w");
        let t = Transcript::create(&store, &cwd, ID, None, true).unwrap();
        t.append_user(&Message::user_text("one"), false, json!({}));
        t.append_assistant(&assistant("two"));
        // The leaf is a system entry the fork doesn't copy.
        t.append_system("rewind", json!({"to": null}));
        let loaded = load(&store.session_path(&cwd, ID).unwrap(), None).unwrap();
        let fork = Transcript::create(&store, &cwd, ID2, None, true).unwrap();
        fork.write_fork_of(&loaded);
        fork.continue_from(&loaded);
        fork.append_user(&Message::user_text("three"), false, json!({}));
        let path = store.session_path(&cwd, ID2).unwrap();
        let texts: Vec<String> = load(&path, None).unwrap().messages.iter().map(|e| e.message.text()).collect();
        assert_eq!(texts, vec!["one", "two", "three"], "the copied history stays on the chain");

        // A crash left half a line; the next write starts on a new line, and the file still loads.
        use std::io::Write as _;
        std::fs::OpenOptions::new().append(true).open(&path).unwrap().write_all(br#"{"type":"user","uu"#).unwrap();
        assert_eq!(load(&path, None).unwrap().messages.len(), 3, "a torn last line is skipped");
        let again = Transcript::create(&store, &cwd, ID2, None, true).unwrap();
        again.continue_from(&load(&path, None).unwrap());
        again.append_user(&Message::user_text("four"), false, json!({}));
        let texts: Vec<String> = load(&path, None).unwrap().messages.iter().map(|e| e.message.text()).collect();
        assert_eq!(texts, vec!["one", "two", "three", "four"]);
    }
}
