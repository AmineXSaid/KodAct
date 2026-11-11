use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::files::FileState;
use crate::shells::ShellManager;

/// The session's OS sandbox policy, shared with sub-agents and changeable
/// while the session runs (`/sandbox`). `None` = commands run unconfined.
pub type SandboxCell = Arc<RwLock<Option<Arc<crate::sandbox::SandboxPolicy>>>>;

/// Saves a file's prior content before a tool writes it (contract C4).
pub trait Checkpointer: Send + Sync {
    fn before_write(&self, path: &Path);
}

/// Everything a tool call may use. Cheap to clone; shared state sits behind `Arc`s.
#[derive(Clone)]
pub struct ToolContext {
    /// The session's project directory (where `kodact` was started).
    pub project_dir: PathBuf,
    /// The Bash tool's current directory; persists between calls.
    pub shell_cwd: Arc<Mutex<PathBuf>>,
    /// Project dir plus `--add-dir` directories.
    pub working_dirs: Arc<RwLock<Vec<PathBuf>>>,
    pub cancel: CancellationToken,
    pub session_id: String,
    pub tool_use_id: String,
    pub files: Arc<FileState>,
    pub checkpointer: Option<Arc<dyn Checkpointer>>,
    pub shells: Arc<ShellManager>,
    pub todos: Arc<Mutex<Vec<Value>>>,
    /// The model's notes (the Notes tool), re-attached after compaction.
    pub notes: Arc<crate::builtin::NotesStore>,
    /// The model's own checkpoints of its edits (the Checkpoint tool).
    pub checkpoints: Arc<crate::builtin::Checkpoints>,
    /// Extra environment for spawned commands.
    pub env: Arc<HashMap<String, String>>,
    /// Upper bound for Bash output kept in a result.
    pub max_output_chars: usize,
    /// OS sandbox for shell commands.
    pub sandbox: SandboxCell,
    /// Where output too long for a result is saved in full (`None` = not saved).
    pub spill_dir: Option<PathBuf>,
    /// The session's transcript file (`None` without session persistence), for
    /// tools that look at the conversation so far (Advisor).
    pub transcript_path: Option<PathBuf>,
    /// The shell commands run in (Bash, `!` commands); `Err` when none was found.
    pub shell: crate::shells::ShellChoice,
}

impl ToolContext {
    pub fn new(project_dir: &Path) -> Self {
        ToolContext {
            project_dir: project_dir.to_path_buf(),
            shell_cwd: Arc::new(Mutex::new(project_dir.to_path_buf())),
            working_dirs: Arc::new(RwLock::new(vec![project_dir.to_path_buf()])),
            cancel: CancellationToken::new(),
            session_id: String::new(),
            tool_use_id: String::new(),
            files: Arc::new(FileState::default()),
            checkpointer: None,
            shells: Arc::new(ShellManager::default()),
            todos: Arc::new(Mutex::new(vec![])),
            notes: Default::default(),
            checkpoints: Default::default(),
            env: Arc::new(HashMap::new()),
            max_output_chars: 30_000,
            sandbox: Arc::new(RwLock::new(None)),
            spill_dir: None,
            transcript_path: None,
            shell: kodact_platform::shell::detect().clone(),
        }
    }

    /// A per-call copy with its own id and cancellation.
    pub fn for_call(&self, tool_use_id: &str, cancel: CancellationToken) -> Self {
        let mut c = self.clone();
        c.tool_use_id = tool_use_id.to_string();
        c.cancel = cancel;
        c
    }

    pub fn shell_cwd(&self) -> PathBuf {
        self.shell_cwd.lock().unwrap().clone()
    }

    pub fn in_working_dirs(&self, p: &Path) -> bool {
        let p = kodact_permissions::normalize(p, &self.project_dir);
        self.working_dirs.read().unwrap().iter().any(|d| p.starts_with(d))
    }

    /// The sandbox policy for a command now, with the current working directories writable.
    pub fn sandbox_now(&self) -> Option<(crate::sandbox::Backend, crate::sandbox::SandboxPolicy)> {
        let policy = self.sandbox_policy()?;
        let backend = crate::sandbox::backend()?;
        let mut p = (*policy).clone();
        p.writable_roots = self.working_dirs.read().unwrap().clone();
        Some((backend, p))
    }

    /// The configured sandbox policy, if any.
    pub fn sandbox_policy(&self) -> Option<Arc<crate::sandbox::SandboxPolicy>> {
        self.sandbox.read().unwrap().clone()
    }

    pub fn set_sandbox(&self, policy: Option<crate::sandbox::SandboxPolicy>) {
        *self.sandbox.write().unwrap() = policy.map(Arc::new);
    }

    /// The session's notes, as the Notes tool last left them.
    pub fn notes_text(&self) -> String {
        self.notes.get(self.transcript_path.as_deref())
    }

    pub fn checkpoint(&self, path: &Path) {
        self.checkpoints.before_write(path);
        if let Some(c) = &self.checkpointer {
            c.before_write(path);
        }
    }
}
