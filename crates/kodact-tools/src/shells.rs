//! Running shell commands: foreground with timeout / interrupt, and
//! background shells that outlive the call (BashOutput / KillShell).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use kodact_platform::process::ProcessTree;
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};
use tokio_util::sync::CancellationToken;

pub use kodact_platform::shell::{Shell, ShellChoice, ShellKind, ShellMissing};

/// Why a command didn't start.
#[derive(Debug)]
pub enum StartError {
    /// No usable shell in this session (the Bash tool reports it as non-retryable).
    NoShell(ShellMissing),
    /// The shell (or the sandbox wrapper) exists but could not be started.
    Spawn { program: String, error: std::io::Error },
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartError::NoShell(m) => write!(f, "{m}"),
            StartError::Spawn { program, error } => write!(f, "could not start {program}: {error}"),
        }
    }
}

impl StartError {
    /// No command can run until the user fixes the setup.
    pub fn is_setup_problem(&self) -> bool {
        match self {
            StartError::NoShell(_) => true,
            StartError::Spawn { error, .. } => error.kind() == std::io::ErrorKind::NotFound,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CommandResult {
    pub stdout: String,
    pub stderr: String,
    pub code: Option<i32>,
    pub interrupted: bool,
    pub timed_out: bool,
    /// Directory the command finished in, when it could be determined.
    pub final_cwd: Option<PathBuf>,
}

type Sandbox<'a> = Option<&'a (crate::sandbox::Backend, crate::sandbox::SandboxPolicy)>;

struct Prepared {
    command: Command,
    program: String,
    /// Windows: the script file, kept until the process exits.
    script: Option<kodact_platform::shell::ScriptFile>,
}

fn build(
    shell: &ShellChoice,
    command: &str,
    cwd: &Path,
    env: &HashMap<String, String>,
    cwd_file: Option<&Path>,
    sandbox: Sandbox,
) -> Result<Prepared, StartError> {
    let shell = shell.as_ref().map_err(|m| StartError::NoShell(m.clone()))?;
    let script = shell.script(command, cwd_file);
    let program = shell.program.display().to_string();
    let (mut c, program, file) = match sandbox {
        Some((backend, policy)) => {
            let (prog, args) = policy.wrap(*backend, &program, &script, cwd);
            let mut c = std::process::Command::new(&prog);
            c.args(args);
            (c, prog, None)
        }
        None => {
            let (c, file) =
                shell.command(&script).map_err(|error| StartError::Spawn { program: program.clone(), error })?;
            (c, program, file)
        }
    };
    c.current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    c.env("KODACT", "1");
    c.envs(env);
    kodact_platform::process::isolate(&mut c);
    let mut c = Command::from(c);
    c.kill_on_drop(true);
    Ok(Prepared { command: c, program, script: file })
}

fn spawn(p: &mut Prepared) -> Result<(Child, Option<ProcessTree>), StartError> {
    let child = p.command.spawn().map_err(|error| StartError::Spawn { program: p.program.clone(), error })?;
    let tree = child.id().map(ProcessTree::attach);
    Ok((child, tree))
}

/// Output kept from each stream: the first and the last this many bytes.
const KEEP_BYTES: usize = 4 << 20;

/// One stream's output as it arrives. The first and the last `keep` bytes are kept, so a
/// command that prints gigabytes can't fill memory, and what was already read is remembered.
#[derive(Debug)]
struct Captured {
    keep: usize,
    head: Vec<u8>,
    tail: Vec<u8>,
    /// Bytes dropped between `head` and `tail`.
    dropped: u64,
    /// How much of the whole stream `take_new` has returned.
    read: u64,
}

impl Captured {
    fn new(keep: usize) -> Self {
        Captured { keep, head: Vec::new(), tail: Vec::new(), dropped: 0, read: 0 }
    }

    fn push(&mut self, mut data: &[u8]) {
        if self.head.len() < self.keep {
            let take = data.len().min(self.keep - self.head.len());
            self.head.extend_from_slice(&data[..take]);
            data = &data[take..];
        }
        self.tail.extend_from_slice(data);
        if self.tail.len() > 2 * self.keep {
            let cut = self.tail.len() - self.keep;
            self.dropped += cut as u64;
            self.tail.drain(..cut);
        }
    }

    fn total(&self) -> u64 {
        self.head.len() as u64 + self.dropped + self.tail.len() as u64
    }

    /// The stream from offset `from` to its end, with a note where bytes were dropped.
    fn since(&self, from: u64) -> String {
        let head_len = self.head.len() as u64;
        let tail_start = head_len + self.dropped;
        let mut out = Vec::new();
        if from < head_len {
            out.extend_from_slice(&self.head[from as usize..]);
        }
        if self.dropped > 0 && from < tail_start {
            let omitted = tail_start - from.max(head_len);
            out.extend_from_slice(format!("\n[... {omitted} bytes of output omitted ...]\n").as_bytes());
        }
        let skip = (from.saturating_sub(tail_start) as usize).min(self.tail.len());
        out.extend_from_slice(&self.tail[skip..]);
        String::from_utf8_lossy(&out).into_owned()
    }

    /// What arrived since the previous call.
    fn take_new(&mut self) -> String {
        let s = self.since(self.read);
        self.read = self.total();
        s
    }

    /// The whole stream: its start and, after a note on what was dropped, its last `keep` bytes.
    fn whole(&self) -> String {
        let extra = self.tail.len().saturating_sub(self.keep);
        let dropped = self.dropped + extra as u64;
        let mut out = String::from_utf8_lossy(&self.head).into_owned();
        if dropped > 0 {
            out.push_str(&format!("\n[... {dropped} bytes of output omitted ...]\n"));
        }
        out.push_str(&String::from_utf8_lossy(&self.tail[extra..]));
        out
    }
}

/// Read a stream to its end, keeping its start and end: a command that prints
/// gigabytes can't fill memory.
async fn read_all<R: AsyncRead + Unpin>(mut r: R) -> String {
    read_capped(&mut r, KEEP_BYTES).await
}

async fn read_capped<R: AsyncRead + Unpin>(r: &mut R, keep: usize) -> String {
    let mut captured = Captured::new(keep);
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        match r.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => captured.push(&chunk[..n]),
        }
    }
    captured.whole()
}

/// Run a command to completion, honouring timeout and cancellation.
pub async fn run_command(
    shell: &ShellChoice,
    command: &str,
    cwd: &Path,
    env: &HashMap<String, String>,
    timeout: Duration,
    cancel: &CancellationToken,
    sandbox: Sandbox<'_>,
) -> Result<CommandResult, StartError> {
    let cwd_file = std::env::temp_dir().join(format!("kodact-cwd-{}", uuid::Uuid::new_v4()));
    let mut prepared = build(shell, command, cwd, env, Some(&cwd_file), sandbox)?;
    let (mut child, tree) = spawn(&mut prepared)?;
    let kill = || {
        if let Some(t) = &tree {
            t.kill();
        }
    };
    let out = tokio::spawn(read_all(child.stdout.take().expect("piped")));
    let err = tokio::spawn(read_all(child.stderr.take().expect("piped")));
    let mut result = CommandResult::default();
    tokio::select! {
        status = child.wait() => { result.code = status.ok().and_then(|s| s.code()); }
        _ = tokio::time::sleep(timeout) => { result.timed_out = true; kill(); let _ = child.wait().await; }
        _ = cancel.cancelled() => { result.interrupted = true; kill(); let _ = child.wait().await; }
    }
    // Background children of the command may keep the pipes open; do not wait forever.
    let grace = Duration::from_millis(if result.timed_out || result.interrupted { 200 } else { 2000 });
    result.stdout = tokio::time::timeout(grace, out).await.ok().and_then(|r| r.ok()).unwrap_or_default();
    result.stderr = tokio::time::timeout(grace, err).await.ok().and_then(|r| r.ok()).unwrap_or_default();
    if let (Ok(written), Ok(shell)) = (std::fs::read_to_string(&cwd_file), shell) {
        result.final_cwd = shell.parse_cwd(&written);
    }
    if cfg!(windows) {
        result.stdout = result.stdout.replace("\r\n", "\n");
        result.stderr = result.stderr.replace("\r\n", "\n");
    }
    drop(prepared);
    let _ = std::fs::remove_file(&cwd_file);
    Ok(result)
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShellStatus {
    Running,
    Completed(Option<i32>),
    Killed,
}

impl ShellStatus {
    pub fn label(&self) -> &'static str {
        match self {
            ShellStatus::Running => "running",
            ShellStatus::Completed(_) => "completed",
            ShellStatus::Killed => "killed",
        }
    }
}

#[derive(Debug)]
pub struct BackgroundShell {
    /// Set when the shell is listed in its session's [`ShellManager`].
    id: OnceLock<String>,
    pub command: String,
    pub started: Instant,
    ended: Mutex<Option<Instant>>,
    stdout: Mutex<Captured>,
    stderr: Mutex<Captured>,
    status: Mutex<ShellStatus>,
    /// Becomes true when the process has exited and its output has been read.
    done: tokio::sync::watch::Receiver<bool>,
    /// The model knows how it ended (an exit notice, BashOutput, KillShell), or needn't.
    reported: AtomicBool,
    /// A Bash call stopped waiting for it at its timeout: the model wants its result.
    awaited: AtomicBool,
    /// Where the shell writes its final directory (Bash calls only).
    cwd_file: Option<PathBuf>,
    tree: Option<ProcessTree>,
    /// The directory it started in.
    pub cwd: PathBuf,
    /// It installs or removes packages and was asked to stop with SIGINT: it may still be
    /// finishing the step it is on.
    stop_asked: AtomicBool,
    /// It was stopped at once while it installed or removed packages: their database may be
    /// half-updated.
    forced_mid_change: AtomicBool,
}

/// How [`BackgroundShell::stop`] stopped a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// It had already ended.
    NotRunning,
    /// Stopped at once (SIGTERM, then SIGKILL), as every command is that doesn't change packages,
    /// and an install stopped a second time. `change`: it was installing or removing packages, so
    /// their database may be half-updated.
    Stopped { change: Option<PackageChange> },
    /// It was installing or removing packages, so it was asked to stop with SIGINT: it finishes
    /// the step it is on, then ends by itself.
    Asked { change: PackageChange },
}

pub use crate::builtin::exit_hints::PackageChange;

impl BackgroundShell {
    /// Its ID in the session (`bash_3`); empty while it runs as a Bash call.
    pub fn id(&self) -> &str {
        self.id.get().map(String::as_str).unwrap_or_default()
    }

    pub fn status(&self) -> ShellStatus {
        self.status.lock().unwrap().clone()
    }

    /// How long it ran, or has been running.
    pub fn runtime(&self) -> Duration {
        self.ended.lock().unwrap().unwrap_or_else(Instant::now).duration_since(self.started)
    }

    /// Wait until the process has exited and its output has been read.
    pub async fn wait(&self) {
        let mut done = self.done.clone();
        let _ = done.wait_for(|d| *d).await;
    }

    /// Output produced since the previous call.
    pub fn take_new_output(&self) -> (String, String) {
        (self.stdout.lock().unwrap().take_new(), self.stderr.lock().unwrap().take_new())
    }

    /// Output not read yet, without marking it read.
    pub fn unread_output(&self) -> (String, String) {
        let unread = |c: &Mutex<Captured>| {
            let c = c.lock().unwrap();
            c.since(c.read)
        };
        (unread(&self.stdout), unread(&self.stderr))
    }

    /// The result of a finished command, as [`run_command`] gives it.
    pub fn result(&self, shell: &ShellChoice) -> CommandResult {
        let mut result = CommandResult {
            stdout: self.stdout.lock().unwrap().whole(),
            stderr: self.stderr.lock().unwrap().whole(),
            ..Default::default()
        };
        match self.status() {
            ShellStatus::Completed(code) => result.code = code,
            ShellStatus::Killed => result.interrupted = true,
            ShellStatus::Running => {}
        }
        if let (Some(file), Ok(shell)) = (&self.cwd_file, shell) {
            if let Ok(written) = std::fs::read_to_string(file) {
                result.final_cwd = shell.parse_cwd(&written);
            }
        }
        if cfg!(windows) {
            result.stdout = result.stdout.replace("\r\n", "\n");
            result.stderr = result.stderr.replace("\r\n", "\n");
        }
        result
    }

    /// Stop it at once: SIGTERM to its process group, SIGKILL 300 ms later.
    pub fn kill(&self) {
        let mut st = self.status.lock().unwrap();
        if *st == ShellStatus::Running {
            if let Some(t) = &self.tree {
                t.kill();
            }
            *st = ShellStatus::Killed;
        }
    }

    /// Stop it. A command that installs or removes packages is asked to stop with SIGINT, the
    /// signal a person's Ctrl-C sends: apt and dpkg stop on it without breaking their database,
    /// where SIGTERM and SIGKILL leave it half-updated, so it finishes the step it is on and then
    /// ends by itself, with nothing forcing it. Asked again, it is stopped at once. Any other
    /// command is stopped at once, as before (SIGTERM, then SIGKILL).
    pub fn stop(&self) -> Stop {
        self.stop_inner(true)
    }

    /// As [`stop`](Self::stop), but an install already asked to stop is left to finish: the end
    /// of a session doesn't force what a stop already asked.
    pub fn stop_for_exit(&self) -> Stop {
        self.stop_inner(false)
    }

    fn stop_inner(&self, force_if_asked: bool) -> Stop {
        if self.status() != ShellStatus::Running {
            return Stop::NotRunning;
        }
        let change = self.package_change();
        if let Some(c) = change {
            let asked = self.stop_asked.load(Ordering::SeqCst);
            // Windows has no SIGINT for a windowless console: an install is stopped at once there.
            if !asked && cfg!(unix) {
                self.stop_asked.store(true, Ordering::SeqCst);
                if let Some(t) = &self.tree {
                    t.interrupt();
                }
                return Stop::Asked { change: c };
            }
            if asked && !force_if_asked {
                return Stop::Asked { change: c };
            }
            self.forced_mid_change.store(true, Ordering::SeqCst);
        }
        self.kill();
        Stop::Stopped { change }
    }

    /// The packages it installs or removes, if it does: read from its command line, or on Linux
    /// from the programs running in it (a `./setup.sh` that runs apt-get).
    pub fn package_change(&self) -> Option<PackageChange> {
        crate::builtin::exit_hints::package_change(&self.command).or_else(|| {
            let tree = self.tree.as_ref()?;
            let members = tree.members();
            members
                .iter()
                .filter(|p| p.pid != tree.pid())
                .find_map(|p| crate::builtin::exit_hints::package_change(&p.cmdline))
        })
    }

    /// Asked to stop with SIGINT, and maybe still finishing.
    pub fn stop_asked(&self) -> bool {
        self.stop_asked.load(Ordering::SeqCst)
    }

    /// The process group it runs in (its first process).
    pub fn pid(&self) -> Option<u32> {
        self.tree.as_ref().map(|t| t.pid())
    }

    /// The model knows how it ended: no exit notice is due.
    pub fn mark_reported(&self) {
        self.reported.store(true, Ordering::SeqCst);
    }

    /// Moved to the background by a Bash call that stopped waiting for it.
    pub fn is_awaited(&self) -> bool {
        self.awaited.load(Ordering::SeqCst)
    }

    pub fn set_awaited(&self) {
        self.awaited.store(true, Ordering::SeqCst);
    }
}

impl Drop for BackgroundShell {
    fn drop(&mut self) {
        if let Some(file) = &self.cwd_file {
            let _ = std::fs::remove_file(file);
        }
    }
}

/// How long KillShell and the end of a session wait for an install asked to stop to finish the
/// step it is on, before saying it is still doing that. apt stops at the end of the dpkg run it is
/// in, which for an install of a few packages takes seconds.
pub const STOP_WAIT: Duration = Duration::from_secs(10);

/// Background shells of a session.
#[derive(Debug)]
pub struct ShellManager {
    shells: Mutex<HashMap<String, Arc<BackgroundShell>>>,
    counter: AtomicU32,
    /// How long a stop waits for an install asked to stop ([`STOP_WAIT`]; shorter in tests).
    pub stop_wait: Duration,
}

impl Default for ShellManager {
    fn default() -> Self {
        ShellManager { shells: Mutex::default(), counter: AtomicU32::new(0), stop_wait: STOP_WAIT }
    }
}

impl ShellManager {
    /// Start a command in the background and list it (Bash `run_in_background`).
    pub fn spawn(
        &self,
        shell: &ShellChoice,
        command: &str,
        cwd: &Path,
        env: &HashMap<String, String>,
        sandbox: Sandbox<'_>,
    ) -> Result<Arc<BackgroundShell>, StartError> {
        let sh = start(shell, command, cwd, env, sandbox, false)?;
        self.adopt(&sh);
        Ok(sh)
    }

    /// List a shell started with [`start`] (a Bash call that stopped waiting for it) and give it
    /// its ID.
    pub fn adopt(&self, sh: &Arc<BackgroundShell>) -> String {
        let id = sh.id.get_or_init(|| format!("bash_{}", self.counter.fetch_add(1, Ordering::SeqCst) + 1)).clone();
        self.shells.lock().unwrap().insert(id.clone(), sh.clone());
        id
    }

    pub fn get(&self, id: &str) -> Option<Arc<BackgroundShell>> {
        self.shells.lock().unwrap().get(id).cloned()
    }

    pub fn list(&self) -> Vec<Arc<BackgroundShell>> {
        let mut v: Vec<_> = self.shells.lock().unwrap().values().cloned().collect();
        v.sort_by_key(|s| s.started);
        v
    }

    /// Shells that exited since the last call and that the model hasn't been told about, now
    /// marked as told. Stopped ones are left out: whoever stopped them knows.
    pub fn take_exited(&self) -> Vec<Arc<BackgroundShell>> {
        let mut out = vec![];
        for s in self.list() {
            match s.status() {
                ShellStatus::Running => {}
                ShellStatus::Killed => s.mark_reported(),
                ShellStatus::Completed(_) => {
                    if !s.reported.swap(true, Ordering::SeqCst) {
                        out.push(s);
                    }
                }
            }
        }
        out
    }

    /// Commands still running after a Bash call stopped waiting for them.
    pub fn awaited_running(&self) -> Vec<Arc<BackgroundShell>> {
        self.list().into_iter().filter(|s| s.is_awaited() && s.status() == ShellStatus::Running).collect()
    }

    /// Stop every running shell, without waiting (a sub-agent that ended): an install is asked to
    /// stop and finishes the step it is on; see [`BackgroundShell::stop_for_exit`].
    pub fn stop_all(&self) {
        for s in self.list() {
            s.stop_for_exit();
        }
    }

    /// The end of the session: stop every running shell, wait up to [`ShellManager::stop_wait`]
    /// for installs asked to stop to finish the step they are on, and say, for the person, what
    /// was cut short and what may be left: an install that ended or is still finishing, and git
    /// lock files a stopped git command left. Nothing for anything else.
    pub async fn end_session(&self) -> Vec<String> {
        let mut installs = vec![];
        let mut gits = vec![];
        for s in self.list() {
            let git = s.status() == ShellStatus::Running && crate::builtin::exit_hints::runs_git(&s.command);
            match s.stop_for_exit() {
                Stop::Asked { change } => installs.push((s, change)),
                Stop::Stopped { change: Some(change) } => installs.push((s, change)),
                Stop::Stopped { change: None } if git => gits.push(s),
                _ => {}
            }
        }
        let deadline = tokio::time::Instant::now() + self.stop_wait;
        for (s, _) in &installs {
            let _ = tokio::time::timeout_at(deadline, s.wait()).await;
        }
        for s in &gits {
            let _ = tokio::time::timeout(Duration::from_secs(1), s.wait()).await;
        }
        let mut notes = vec![];
        for (s, change) in &installs {
            let what = format!("`{}` was installing or removing packages when the session ended", one_line(&s.command));
            notes.push(if s.status() != ShellStatus::Running {
                if s.forced_mid_change.load(Ordering::SeqCst) {
                    format!("{what}, and it was stopped at once. {}", change.forced_note())
                } else {
                    format!(
                        "{what}. It was stopped with SIGINT, which {} stops on without breaking its package database: \
                         it finished the step it was on, so packages it hadn't reached are not installed.",
                        change.manager
                    )
                }
            } else {
                format!(
                    "{what}. It was sent SIGINT and is still finishing the step it is on (process {}); it ends by \
                     itself and holds the package lock until then. Stopping it by force can leave the package \
                     database half-updated; if that happens, {}.",
                    s.pid().map(|p| p.to_string()).unwrap_or_else(|| "?".into()),
                    change.repair
                )
            });
        }
        for s in &gits {
            let locks = crate::builtin::exit_hints::git_locks_left(&s.cwd);
            if let Some(n) = crate::builtin::exit_hints::git_locks_note(&locks) {
                notes.push(format!("`{}` was stopped when the session ended. {n}", one_line(&s.command)));
            }
        }
        notes
    }

    /// The ID of this session's running shell a process belongs to: in its process group, or
    /// started from one (Linux).
    pub fn owner_of(&self, pid: u32) -> Option<String> {
        self.list().into_iter().find_map(|s| {
            let group = s.pid()?;
            (s.status() == ShellStatus::Running && kodact_platform::process::descends_from_group(pid, group))
                .then(|| s.id().to_string())
        })
    }

    /// What a command's output says is broken or locked on this machine, with what this session
    /// knows about it ([`exit_hints::state_hint`](crate::builtin::exit_hints::state_hint)).
    /// Nothing is looked up unless the output has one of the messages it reads.
    pub fn state_hint(&self, text: &str) -> Option<String> {
        if !crate::builtin::exit_hints::mentions_state(text) {
            return None;
        }
        let shells = self.list();
        let running: Vec<(String, String)> = shells
            .iter()
            .filter(|s| s.status() == ShellStatus::Running && !s.id().is_empty())
            .map(|s| (s.id().to_string(), s.command.clone()))
            .collect();
        let forced: Vec<String> =
            shells.iter().filter(|s| s.forced_mid_change.load(Ordering::SeqCst)).map(|s| s.command.clone()).collect();
        crate::builtin::exit_hints::state_hint(text, &running, |pid| self.owner_of(pid), &forced)
    }
}

/// A command line on one line, at most 100 characters.
fn one_line(command: &str) -> String {
    let first = command.trim().lines().next().unwrap_or_default();
    let mut s: String = first.chars().take(100).collect();
    if s.len() < first.len() || command.trim().lines().count() > 1 {
        s.push_str(" …");
    }
    s
}

/// Start a command whose output is collected as it arrives, without listing it in a session:
/// a Bash call waits for it and gives it to [`ShellManager::adopt`] if it outlives the wait.
/// `track_cwd` records the directory it ends in, for [`BackgroundShell::result`].
pub fn start(
    shell: &ShellChoice,
    command: &str,
    cwd: &Path,
    env: &HashMap<String, String>,
    sandbox: Sandbox<'_>,
    track_cwd: bool,
) -> Result<Arc<BackgroundShell>, StartError> {
    let cwd_file = track_cwd.then(|| std::env::temp_dir().join(format!("kodact-cwd-{}", uuid::Uuid::new_v4())));
    let mut prepared = build(shell, command, cwd, env, cwd_file.as_deref(), sandbox)?;
    // The waiting task owns the child; a shell outlives the call that started it.
    prepared.command.kill_on_drop(false);
    let (mut child, tree) = spawn(&mut prepared)?;
    let script = prepared.script.take();
    let (done_tx, done) = tokio::sync::watch::channel(false);
    let sh = Arc::new(BackgroundShell {
        id: OnceLock::new(),
        command: command.to_string(),
        started: Instant::now(),
        ended: Mutex::new(None),
        stdout: Mutex::new(Captured::new(KEEP_BYTES)),
        stderr: Mutex::new(Captured::new(KEEP_BYTES)),
        status: Mutex::new(ShellStatus::Running),
        done,
        reported: AtomicBool::new(false),
        awaited: AtomicBool::new(false),
        cwd_file,
        tree,
        cwd: cwd.to_path_buf(),
        stop_asked: AtomicBool::new(false),
        forced_mid_change: AtomicBool::new(false),
    });
    let pump = |mut r: Box<dyn AsyncRead + Unpin + Send>, sh: Arc<BackgroundShell>, is_err: bool| async move {
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            match r.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let target = if is_err { &sh.stderr } else { &sh.stdout };
                    target.lock().unwrap().push(&buf[..n]);
                }
            }
        }
    };
    let out = tokio::spawn(pump(Box::new(child.stdout.take().expect("piped")), sh.clone(), false));
    let err = tokio::spawn(pump(Box::new(child.stderr.take().expect("piped")), sh.clone(), true));
    let waiter = sh.clone();
    tokio::spawn(async move {
        let code = child.wait().await.ok().and_then(|s| s.code());
        *waiter.ended.lock().unwrap() = Some(Instant::now());
        // Background children of the command may keep the pipes open; do not wait forever.
        let killed = waiter.status() == ShellStatus::Killed;
        let grace = Duration::from_millis(if killed { 200 } else { 2000 });
        let _ = tokio::time::timeout(grace, async {
            let _ = out.await;
            let _ = err.await;
        })
        .await;
        drop(script);
        {
            let mut st = waiter.status.lock().unwrap();
            if *st == ShellStatus::Running {
                *st = ShellStatus::Completed(code);
            }
        }
        let _ = done_tx.send(true);
    });
    Ok(sh)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn long_output_keeps_its_start_and_end() {
        let data: Vec<u8> = (0..1000u32).flat_map(|i| format!("{i:04}\n").into_bytes()).collect();
        let out = read_capped(&mut data.as_slice(), 100).await;
        assert!(out.starts_with("0000\n0001\n"), "{out}");
        assert!(out.ends_with("0998\n0999\n"), "{out}");
        assert!(out.contains("[... 4800 bytes of output omitted ...]"), "{out}");
        let short = read_capped(&mut b"hi\n".as_slice(), 100).await;
        assert_eq!(short, "hi\n");
    }

    #[test]
    fn captured_output_is_read_in_parts_and_notes_what_was_dropped() {
        let mut c = Captured::new(10);
        c.push(b"0123456789abc");
        assert_eq!(c.take_new(), "0123456789abc");
        assert_eq!(c.take_new(), "");
        c.push(b"defghijklmnopqrstuvwxyz");
        // The tail keeps at most twice `keep` before it drops its oldest part; what was read
        // already is not shown again.
        let next = c.take_new();
        assert!(next.contains("bytes of output omitted") && next.ends_with("qrstuvwxyz"), "{next}");
        let whole = c.whole();
        assert!(whole.starts_with("0123456789") && whole.ends_with("qrstuvwxyz"), "{whole}");
        assert!(whole.contains("[... 16 bytes of output omitted ...]"), "{whole}");
    }
}
