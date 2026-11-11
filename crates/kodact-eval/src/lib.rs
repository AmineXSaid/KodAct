//! `kodact-eval`: measure the harness (docs/GOALS.md, "measure, don't assume").
//!
//! A task is a directory:
//!
//! ```text
//! evals/tasks/<id>/
//!   task.json   {"instruction": "...", "check": "python3 -m unittest -q", "pillar": "verification", ...}
//!   repo/       files copied into a fresh workspace for every run
//! ```
//!
//! Each run copies `repo/` into a new workspace, initialises git, runs the
//! optional `setup` command, runs `kodact -p <instruction>` in stream-json mode,
//! then runs `check`. Exit code 0 means the task passed. The stream is parsed
//! for cost, turns and tool errors.
//!
//! `setup` and `check` see `KODACT_EVAL_TASK_DIR`, so checks can use hidden
//! files (e.g. `$KODACT_EVAL_TASK_DIR/hidden/`) the agent never sees.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{bail, Context};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
use tokio::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSpec {
    #[serde(default)]
    pub id: String,
    pub instruction: String,
    /// Shell command run in the workspace after the agent; exit 0 = pass.
    pub check: String,
    /// Shell command run before the agent (installing fixtures, breaking things).
    #[serde(default)]
    pub setup: Option<String>,
    /// Which GOALS.md pillar the task exercises.
    #[serde(default)]
    pub pillar: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    /// Extra `kodact` arguments for this task.
    #[serde(default)]
    pub args: Vec<String>,
    /// A message sent while the agent works, as a desktop host sends one.
    #[serde(default)]
    pub steer: Option<SteerSpec>,
    /// Kill kodact mid-turn, then continue the session in a new process.
    #[serde(default)]
    pub crash: Option<CrashSpec>,
}

/// Kill kodact (as a crash or a closed terminal does) once the agent has made
/// `after_tool_calls` tool calls, then run `kodact -p <resume> --continue` in the same
/// workspace. A run that ends before that many calls is not killed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrashSpec {
    pub after_tool_calls: usize,
    #[serde(default = "default_resume")]
    pub resume: String,
}

fn default_resume() -> String {
    "Continue.".into()
}

/// A message sent mid-turn (`priority: "next"`) once the agent has made `after_tool_calls`
/// tool calls; if the turn ends before that, it is sent as the next turn instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SteerSpec {
    pub after_tool_calls: usize,
    pub text: String,
}

/// A stream-json user line; a steer is marked for the running turn's next step.
fn user_line(text: &str, steer: bool) -> String {
    let mut v = serde_json::json!({
        "type": "user",
        "message": { "role": "user", "content": [{ "type": "text", "text": text }] },
        "parent_tool_use_id": null,
        "session_id": "",
    });
    if steer {
        v["priority"] = "next".into();
        v["uuid"] = "kodact-eval-steer".into();
    }
    format!("{v}\n")
}

/// Tool calls the main agent made in one stream line (a sub-agent's are not counted).
fn tool_uses(v: &Value) -> usize {
    if v["type"] != "assistant" || v.get("parent_tool_use_id").is_some_and(|p| !p.is_null()) {
        return 0;
    }
    v["message"]["content"].as_array().map_or(0, |c| c.iter().filter(|b| b["type"] == "tool_use").count())
}

fn default_timeout() -> u64 {
    900
}

#[derive(Debug, Clone)]
pub struct Task {
    pub spec: TaskSpec,
    pub dir: PathBuf,
}

/// Load every task under `root` (directories containing `task.json`), sorted by id.
pub fn load_tasks(root: &Path, filter: &[String]) -> anyhow::Result<Vec<Task>> {
    let mut tasks = vec![];
    let rd = std::fs::read_dir(root).with_context(|| format!("cannot read task directory {}", root.display()))?;
    for e in rd.flatten() {
        let f = e.path().join("task.json");
        if !f.exists() {
            continue;
        }
        let mut spec: TaskSpec =
            serde_json::from_str(&std::fs::read_to_string(&f)?).with_context(|| format!("invalid {}", f.display()))?;
        if spec.id.is_empty() {
            spec.id = e.file_name().to_string_lossy().into_owned();
        }
        let keep = filter.is_empty()
            || filter.iter().any(|f| spec.id.contains(f.as_str()) || spec.tags.contains(f) || &spec.pillar == f);
        if keep {
            tasks.push(Task { spec, dir: e.path() });
        }
    }
    tasks.sort_by(|a, b| a.spec.id.cmp(&b.spec.id));
    Ok(tasks)
}

#[derive(Debug, Clone)]
pub struct RunOptions {
    /// The `kodact` binary under test.
    pub kodact: PathBuf,
    /// Extra arguments for every run (the configuration being measured).
    pub args: Vec<String>,
    /// Extra environment (credentials and endpoints are inherited from the caller).
    pub env: Vec<(String, String)>,
    pub label: String,
    pub out_dir: PathBuf,
    /// Runs per task.
    pub repeat: u32,
    /// Tasks run in parallel.
    pub jobs: usize,
}

/// One run of one task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RunRecord {
    pub task: String,
    pub pillar: String,
    pub run: u32,
    pub passed: bool,
    /// The agent finished without error but the checker failed.
    pub false_finish: bool,
    /// Some tool call or API request failed during the run.
    pub had_error: bool,
    pub cost_usd: f64,
    pub turns: u64,
    pub wall_secs: f64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    /// Times the verification loop reminded the model to run checks.
    #[serde(default)]
    pub verify_reminders: u64,
    /// Times the loop guard warned the model it was repeating itself.
    #[serde(default)]
    pub loop_guards: u64,
    /// Replies that were all reasoning, each answered with a reminder to act (`act_reminder`).
    #[serde(default)]
    pub act_reminders: u64,
    /// Edits whose result said the file no longer parses (`diagnostics`).
    #[serde(default)]
    pub edit_diagnostics: u64,
    /// Compactions of the conversation (`compact_boundary`).
    #[serde(default)]
    pub compactions: u64,
    /// Summary attempts that failed (`compact_failed`); each costs a model call.
    #[serde(default)]
    pub compact_failures: u64,
    /// Compactions done with KodAct's own note after two failed summaries (`compact_fallback`).
    #[serde(default)]
    pub compact_fallbacks: u64,
    pub agent_error: bool,
    pub stop_reason: Option<String>,
    /// What the result said of the changes (`verification.status`): `passed`, `failed`,
    /// `unchecked`, `read_back` or `no_changes`. Beside `passed` it shows whether a false
    /// finish was announced by KodAct itself (`unchecked`, `failed`) or slipped past a check.
    #[serde(default)]
    pub verification: Option<String>,
    pub timed_out: bool,
    pub check_output: String,
    pub workspace: PathBuf,
}

/// Aggregates for one configuration (docs/GOALS.md metrics).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Summary {
    pub runs: usize,
    pub pass_rate: f64,
    pub mean_cost_usd: f64,
    pub cost_per_pass_usd: Option<f64>,
    pub mean_turns: f64,
    pub mean_wall_secs: f64,
    /// Of runs that hit an error, the share that still passed.
    pub recovery_rate: Option<f64>,
    pub false_finish_rate: f64,
    /// Tool errors per tool call.
    pub invalid_call_rate: f64,
    /// KodAct's own reminders per run: each is either needed or a false positive, so a change
    /// that cuts them must keep the pass rate.
    #[serde(default)]
    pub verify_reminders_per_run: f64,
    #[serde(default)]
    pub loop_guards_per_run: f64,
    /// Replies that were all reasoning per run: each took up to a whole output budget for nothing.
    #[serde(default)]
    pub act_reminders_per_run: f64,
    /// Edits per run that left a file unparseable and were told so with their result.
    #[serde(default)]
    pub edit_diagnostics_per_run: f64,
    /// Compactions per run, and failed summary attempts per run (each a model call for nothing).
    #[serde(default)]
    pub compactions_per_run: f64,
    #[serde(default)]
    pub compact_failures_per_run: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub label: String,
    pub created: String,
    pub args: Vec<String>,
    pub summary: Summary,
    pub by_pillar: BTreeMap<String, Summary>,
    pub runs: Vec<RunRecord>,
}

pub fn summarize(runs: &[RunRecord]) -> Summary {
    if runs.is_empty() {
        return Summary::default();
    }
    let n = runs.len() as f64;
    let passed = runs.iter().filter(|r| r.passed).count();
    let total_cost: f64 = runs.iter().map(|r| r.cost_usd).sum();
    let errored: Vec<&RunRecord> = runs.iter().filter(|r| r.had_error).collect();
    let calls: u64 = runs.iter().map(|r| r.tool_calls).sum();
    let errors: u64 = runs.iter().map(|r| r.tool_errors).sum();
    Summary {
        runs: runs.len(),
        pass_rate: passed as f64 / n,
        mean_cost_usd: total_cost / n,
        cost_per_pass_usd: (passed > 0).then(|| total_cost / passed as f64),
        mean_turns: runs.iter().map(|r| r.turns as f64).sum::<f64>() / n,
        mean_wall_secs: runs.iter().map(|r| r.wall_secs).sum::<f64>() / n,
        recovery_rate: (!errored.is_empty())
            .then(|| errored.iter().filter(|r| r.passed).count() as f64 / errored.len() as f64),
        false_finish_rate: runs.iter().filter(|r| r.false_finish).count() as f64 / n,
        invalid_call_rate: if calls == 0 { 0.0 } else { errors as f64 / calls as f64 },
        verify_reminders_per_run: runs.iter().map(|r| r.verify_reminders as f64).sum::<f64>() / n,
        loop_guards_per_run: runs.iter().map(|r| r.loop_guards as f64).sum::<f64>() / n,
        act_reminders_per_run: runs.iter().map(|r| r.act_reminders as f64).sum::<f64>() / n,
        edit_diagnostics_per_run: runs.iter().map(|r| r.edit_diagnostics as f64).sum::<f64>() / n,
        compactions_per_run: runs.iter().map(|r| r.compactions as f64).sum::<f64>() / n,
        compact_failures_per_run: runs.iter().map(|r| r.compact_failures as f64).sum::<f64>() / n,
    }
}

pub fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            copy_dir(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), &target)?;
        }
    }
    Ok(())
}

/// The shell task checks run in: a POSIX shell (tasks are shell scripts),
/// `KODACT_SHELL` if set. Found once.
fn eval_shell() -> &'static kodact_platform::shell::ShellChoice {
    static S: std::sync::OnceLock<kodact_platform::shell::ShellChoice> = std::sync::OnceLock::new();
    S.get_or_init(|| {
        kodact_platform::shell::resolve(&kodact_platform::shell::ShellConfig {
            posix_only: true,
            ..kodact_platform::shell::ShellConfig::from_env()
        })
    })
}

async fn sh(cmd: &str, dir: &Path, task_dir: &Path, timeout: Duration) -> (bool, String) {
    let shell = match eval_shell() {
        Ok(s) => s,
        Err(m) => return (false, m.to_string()),
    };
    let (std_cmd, _script) = match shell.command(&shell.script(cmd, None)) {
        Ok(c) => c,
        Err(e) => return (false, format!("could not run {cmd}: {e}")),
    };
    let mut std_cmd = std_cmd;
    kodact_platform::process::no_window(&mut std_cmd);
    let child = Command::from(std_cmd)
        .current_dir(dir)
        .env("KODACT_EVAL_TASK_DIR", task_dir)
        // Checks must see the files as they are now, never stale bytecode.
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();
    let Ok(child) = child else { return (false, format!("could not run {cmd}")) };
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(o)) => {
            let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&o.stderr));
            let tail: String = text.chars().rev().take(4000).collect::<Vec<_>>().into_iter().rev().collect();
            (o.status.success(), tail)
        }
        Ok(Err(e)) => (false, e.to_string()),
        Err(_) => (false, format!("timed out after {timeout:?}")),
    }
}

/// Check a task's quality: its check must fail on the starting files and pass
/// after `solution.sh` (kept beside `task.json`, never shown to the agent).
pub async fn validate_task(task: &Task, scratch: &Path) -> Result<(), String> {
    let ws = scratch.join(&task.spec.id);
    let _ = std::fs::remove_dir_all(&ws);
    let repo = task.dir.join("repo");
    if repo.exists() {
        copy_dir(&repo, &ws).map_err(|e| e.to_string())?;
    } else {
        std::fs::create_dir_all(&ws).map_err(|e| e.to_string())?;
    }
    let task_dir = kodact_platform::path::canonicalize(&task.dir).map_err(|e| e.to_string())?;
    let init = "git init -q && git add -A && git -c user.email=eval@kodact -c user.name=kodact-eval commit -q --allow-empty -m fixture";
    sh(init, &ws, &task_dir, Duration::from_secs(60)).await;
    if let Some(setup) = &task.spec.setup {
        let (ok, out) = sh(setup, &ws, &task_dir, Duration::from_secs(300)).await;
        if !ok {
            return Err(format!("setup failed: {out}"));
        }
    }
    let (before, _) = sh(&task.spec.check, &ws, &task_dir, Duration::from_secs(300)).await;
    if before {
        return Err("the check already passes on the starting files".into());
    }
    let solution = task_dir.join("solution.sh");
    if !solution.exists() {
        return Err("no solution.sh".into());
    }
    // Forward slashes: the POSIX shell on Windows (Git Bash) reads `\` as an escape.
    let solution = solution.display().to_string().replace('\\', "/");
    let (ok, out) = sh(&format!("sh '{solution}'"), &ws, &task_dir, Duration::from_secs(300)).await;
    if !ok {
        return Err(format!("solution.sh failed: {out}"));
    }
    let (after, out) = sh(&task.spec.check, &ws, &task_dir, Duration::from_secs(300)).await;
    if !after {
        return Err(format!("the check fails after solution.sh: {out}"));
    }
    Ok(())
}

/// Facts parsed from a stream-json transcript.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct StreamFacts {
    pub cost_usd: f64,
    pub turns: u64,
    pub tool_calls: u64,
    pub tool_errors: u64,
    pub verify_reminders: u64,
    pub loop_guards: u64,
    pub act_reminders: u64,
    pub edit_diagnostics: u64,
    pub compactions: u64,
    pub compact_failures: u64,
    pub compact_fallbacks: u64,
    pub api_error: bool,
    pub is_error: bool,
    pub stop_reason: Option<String>,
    pub verification: Option<String>,
    pub saw_result: bool,
}

pub fn parse_stream(lines: &[Value]) -> StreamFacts {
    let mut f = StreamFacts::default();
    for v in lines {
        match v["type"].as_str() {
            Some("assistant") => {
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    if b["type"] == "tool_use" {
                        f.tool_calls += 1;
                    }
                }
            }
            Some("user") => {
                for b in v["message"]["content"].as_array().into_iter().flatten() {
                    if b["type"] == "tool_result" && b["is_error"] == true {
                        f.tool_errors += 1;
                    }
                }
            }
            Some("system") if v["subtype"] == "verification" => f.verify_reminders += 1,
            Some("system") if v["subtype"] == "loop_guard" => f.loop_guards += 1,
            Some("system") if v["subtype"] == "act_reminder" => f.act_reminders += 1,
            Some("system") if v["subtype"] == "diagnostics" => f.edit_diagnostics += 1,
            Some("system") if v["subtype"] == "compact_boundary" => f.compactions += 1,
            Some("system") if v["subtype"] == "compact_failed" => f.compact_failures += 1,
            Some("system") if v["subtype"] == "compact_fallback" => f.compact_fallbacks += 1,
            Some("system") if v["subtype"] == "model_fallback" || v["subtype"] == "api_retry" => f.api_error = true,
            Some("result") => {
                f.saw_result = true;
                f.cost_usd = v["total_cost_usd"].as_f64().unwrap_or(0.0);
                f.turns = v["num_turns"].as_u64().unwrap_or(0);
                f.is_error = v["is_error"].as_bool().unwrap_or(true);
                f.stop_reason = v["stop_reason"].as_str().map(str::to_string);
                f.verification = v["verification"]["status"].as_str().map(str::to_string);
                if v["subtype"] == "error_during_execution" {
                    f.api_error = true;
                }
            }
            _ => {}
        }
    }
    f
}

/// Run one task once.
pub async fn run_task(task: &Task, run: u32, opts: &RunOptions) -> anyhow::Result<RunRecord> {
    let run_dir = opts.out_dir.join(format!("{}-{run}", task.spec.id));
    let ws = run_dir.join("workspace");
    if run_dir.exists() {
        std::fs::remove_dir_all(&run_dir)?;
    }
    let repo = task.dir.join("repo");
    if repo.exists() {
        copy_dir(&repo, &ws)?;
    } else {
        std::fs::create_dir_all(&ws)?;
    }
    let ws = kodact_platform::path::canonicalize(&ws)?;
    let init = "git init -q && git add -A && git -c user.email=eval@kodact -c user.name=kodact-eval commit -q --allow-empty -m fixture";
    let task_dir = kodact_platform::path::canonicalize(&task.dir)?;
    sh(init, &ws, &task_dir, Duration::from_secs(60)).await;
    if let Some(setup) = &task.spec.setup {
        let (ok, out) = sh(setup, &ws, &task_dir, Duration::from_secs(300)).await;
        if !ok {
            bail!("setup failed for {}: {out}", task.spec.id);
        }
    }

    let steer = task.spec.steer.as_ref();
    let crash = task.spec.crash.as_ref();
    let mut cmd = Command::new(&opts.kodact);
    cmd.arg("-p");
    // A task that steers talks to kodact over stdin, as a host does.
    match steer {
        Some(_) => cmd.args(["--input-format", "stream-json"]),
        None => cmd.arg(&task.spec.instruction),
    };
    cmd.args(["--output-format", "stream-json", "--verbose"])
        .args(&opts.args)
        .args(&task.spec.args)
        .current_dir(&ws)
        .stdin(if steer.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (k, v) in &opts.env {
        cmd.env(k, v);
    }
    let started = Instant::now();
    let mut child = cmd.spawn().with_context(|| format!("cannot start {}", opts.kodact.display()))?;
    let stdout = child.stdout.take().expect("piped");
    let stderr = child.stderr.take().expect("piped");
    let mut stdin = child.stdin.take();
    if let Some(w) = stdin.as_mut() {
        w.write_all(user_line(&task.spec.instruction, false).as_bytes()).await?;
    }
    let transcript_path = run_dir.join("transcript.jsonl");
    let mut transcript = tokio::fs::File::create(&transcript_path).await?;
    let mut killed = false;
    let collect = async {
        let mut lines = tokio::io::BufReader::new(stdout).lines();
        let mut parsed = vec![];
        let mut calls = 0;
        while let Ok(Some(l)) = lines.next_line().await {
            let _ = transcript.write_all(format!("{l}\n").as_bytes()).await;
            if let Ok(v) = serde_json::from_str::<Value>(&l) {
                calls += tool_uses(&v);
                if crash.is_some_and(|c| calls >= c.after_tool_calls) && v["type"] != "result" {
                    parsed.push(v);
                    killed = true;
                    break;
                }
                // The steer goes once the agent is that far in, or as the next turn if the
                // turn ended first; then stdin closes and kodact exits when the work is done.
                if let (Some(s), Some(mut w)) = (steer, stdin.take()) {
                    let ended = v["type"] == "result";
                    if calls >= s.after_tool_calls || ended {
                        let _ = w.write_all(user_line(&s.text, !ended).as_bytes()).await;
                    } else {
                        stdin = Some(w);
                    }
                }
                parsed.push(v);
            }
        }
        parsed
    };
    let err_task = tokio::spawn(async move {
        let mut s = String::new();
        let mut r = tokio::io::BufReader::new(stderr);
        let _ = tokio::io::AsyncReadExt::read_to_string(&mut r, &mut s).await;
        s
    });
    let timeout = Duration::from_secs(task.spec.timeout_secs);
    let (mut parsed, mut timed_out) = match tokio::time::timeout(timeout, collect).await {
        Ok(p) => (p, false),
        Err(_) => {
            let _ = child.kill().await;
            (vec![], true)
        }
    };
    if killed {
        // Mid-turn, with a tool call under way: nothing gets to wind down.
        let _ = child.kill().await;
    }
    let _ = child.wait().await;
    let mut stderr_text = err_task.await.unwrap_or_default();
    if let (true, Some(c)) = (killed, crash) {
        let mut cmd = Command::new(&opts.kodact);
        cmd.arg("-p")
            .arg(&c.resume)
            .args(["--continue", "--output-format", "stream-json", "--verbose"])
            .args(&opts.args)
            .args(&task.spec.args)
            .current_dir(&ws)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in &opts.env {
            cmd.env(k, v);
        }
        let left = timeout.saturating_sub(started.elapsed());
        let child = cmd.spawn().with_context(|| format!("cannot start {}", opts.kodact.display()))?;
        match tokio::time::timeout(left, child.wait_with_output()).await {
            Ok(Ok(out)) => {
                let text = String::from_utf8_lossy(&out.stdout);
                let _ = transcript.write_all(text.as_bytes()).await;
                parsed.extend(text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()));
                stderr_text.push_str(&String::from_utf8_lossy(&out.stderr));
            }
            _ => timed_out = true,
        }
    }
    let _ = std::fs::write(run_dir.join("stderr.txt"), &stderr_text);
    let wall_secs = started.elapsed().as_secs_f64();
    let facts = parse_stream(&parsed);

    let (passed, check_output) = sh(&task.spec.check, &ws, &task_dir, Duration::from_secs(300)).await;
    let agent_error = timed_out || !facts.saw_result || facts.is_error;
    Ok(RunRecord {
        task: task.spec.id.clone(),
        pillar: task.spec.pillar.clone(),
        run,
        passed,
        false_finish: !passed && !agent_error,
        had_error: facts.tool_errors > 0 || facts.api_error,
        cost_usd: facts.cost_usd,
        turns: facts.turns,
        wall_secs,
        tool_calls: facts.tool_calls,
        tool_errors: facts.tool_errors,
        verify_reminders: facts.verify_reminders,
        loop_guards: facts.loop_guards,
        act_reminders: facts.act_reminders,
        edit_diagnostics: facts.edit_diagnostics,
        compactions: facts.compactions,
        compact_failures: facts.compact_failures,
        compact_fallbacks: facts.compact_fallbacks,
        agent_error,
        stop_reason: facts.stop_reason,
        verification: facts.verification,
        timed_out,
        check_output,
        workspace: ws,
    })
}

/// Run every task `repeat` times with up to `jobs` in parallel.
pub async fn run_suite(tasks: &[Task], opts: &RunOptions) -> Report {
    use futures::stream::{self, StreamExt};
    let jobs: Vec<(usize, u32)> = (0..tasks.len()).flat_map(|t| (1..=opts.repeat).map(move |r| (t, r))).collect();
    let mut runs: Vec<RunRecord> = stream::iter(jobs)
        .map(|(t, r)| async move {
            match run_task(&tasks[t], r, opts).await {
                Ok(rec) => rec,
                Err(e) => RunRecord {
                    task: tasks[t].spec.id.clone(),
                    pillar: tasks[t].spec.pillar.clone(),
                    run: r,
                    passed: false,
                    false_finish: false,
                    had_error: true,
                    cost_usd: 0.0,
                    turns: 0,
                    wall_secs: 0.0,
                    tool_calls: 0,
                    tool_errors: 0,
                    verify_reminders: 0,
                    loop_guards: 0,
                    act_reminders: 0,
                    edit_diagnostics: 0,
                    compactions: 0,
                    compact_failures: 0,
                    compact_fallbacks: 0,
                    agent_error: true,
                    stop_reason: None,
                    verification: None,
                    timed_out: false,
                    check_output: format!("harness error: {e:#}"),
                    workspace: PathBuf::new(),
                },
            }
        })
        .buffer_unordered(opts.jobs.max(1))
        .collect()
        .await;
    runs.sort_by(|a, b| a.task.cmp(&b.task).then(a.run.cmp(&b.run)));
    let mut by_pillar: BTreeMap<String, Vec<RunRecord>> = BTreeMap::new();
    for r in &runs {
        by_pillar
            .entry(if r.pillar.is_empty() { "other".into() } else { r.pillar.clone() })
            .or_default()
            .push(r.clone());
    }
    Report {
        label: opts.label.clone(),
        created: chrono::Utc::now().to_rfc3339(),
        args: opts.args.clone(),
        summary: summarize(&runs),
        by_pillar: by_pillar.into_iter().map(|(k, v)| (k, summarize(&v))).collect(),
        runs,
    }
}

fn pct(x: f64) -> String {
    format!("{:.0}%", x * 100.0)
}

fn opt_pct(x: Option<f64>) -> String {
    x.map(pct).unwrap_or_else(|| "–".into())
}

/// Markdown report for one configuration.
pub fn render_markdown(r: &Report) -> String {
    let s = &r.summary;
    let mut out = format!(
        "# kodact-eval: {}\n\n{} runs · args: `{}`\n\n| Metric | Value |\n| --- | --- |\n\
         | Pass rate | {} |\n| Cost per task | ${:.4} |\n| Cost per passed task | {} |\n| Turns per task | {:.1} |\n\
         | Wall time per task | {:.1}s |\n| Recovery rate | {} |\n| False-finish rate | {} |\n| Invalid-call rate | {} |\n\
         | Verification reminders per task | {:.2} |\n| Loop warnings per task | {:.2} |\n\
         | Reasoning-only replies per task | {:.2} |\n| Edits told they broke the syntax, per task | {:.2} |\n\
         | Compactions per task | {:.2} |\n\
         | Failed summaries per task | {:.2} |\n\n",
        r.label,
        s.runs,
        r.args.join(" "),
        pct(s.pass_rate),
        s.mean_cost_usd,
        s.cost_per_pass_usd.map(|c| format!("${c:.4}")).unwrap_or_else(|| "–".into()),
        s.mean_turns,
        s.mean_wall_secs,
        opt_pct(s.recovery_rate),
        pct(s.false_finish_rate),
        pct(s.invalid_call_rate),
        s.verify_reminders_per_run,
        s.loop_guards_per_run,
        s.act_reminders_per_run,
        s.edit_diagnostics_per_run,
        s.compactions_per_run,
        s.compact_failures_per_run,
    );
    out.push_str("## By pillar\n\n| Pillar | Runs | Pass | False finish | Turns |\n| --- | --- | --- | --- | --- |\n");
    for (p, s) in &r.by_pillar {
        out.push_str(&format!(
            "| {p} | {} | {} | {} | {:.1} |\n",
            s.runs,
            pct(s.pass_rate),
            pct(s.false_finish_rate),
            s.mean_turns
        ));
    }
    out.push_str("\n## Runs\n\n| Task | Run | Pass | False finish | Turns | Cost | Tool errors | Time | Reminders (verify/loop/act) | Compactions (done/failed/note) | Changes |\n| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |\n");
    for x in &r.runs {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | ${:.4} | {}/{} | {:.1}s | {}/{}/{} | {}/{}/{} | {} |\n",
            x.task,
            x.run,
            if x.passed { "yes" } else { "no" },
            if x.false_finish { "yes" } else { "" },
            x.turns,
            x.cost_usd,
            x.tool_errors,
            x.tool_calls,
            x.wall_secs,
            x.verify_reminders,
            x.loop_guards,
            x.act_reminders,
            x.compactions,
            x.compact_failures,
            x.compact_fallbacks,
            x.verification.as_deref().unwrap_or("")
        ));
    }
    out
}

/// A/B comparison of two reports (B relative to A).
pub fn render_compare(a: &Report, b: &Report) -> String {
    let (x, y) = (&a.summary, &b.summary);
    let row = |name: &str, va: String, vb: String, better: &str| format!("| {name} | {va} | {vb} | {better} |\n");
    let delta = |da: f64, db: f64, higher_better: bool| {
        let d = db - da;
        if d.abs() < 1e-9 {
            "=".to_string()
        } else if (d > 0.0) == higher_better {
            "B better".into()
        } else {
            "A better".into()
        }
    };
    let mut out =
        format!("# A/B: {} (A) vs {} (B)\n\n| Metric | A | B | |\n| --- | --- | --- | --- |\n", a.label, b.label);
    out.push_str(&row("Pass rate", pct(x.pass_rate), pct(y.pass_rate), &delta(x.pass_rate, y.pass_rate, true)));
    out.push_str(&row(
        "Cost per task",
        format!("${:.4}", x.mean_cost_usd),
        format!("${:.4}", y.mean_cost_usd),
        &delta(x.mean_cost_usd, y.mean_cost_usd, false),
    ));
    out.push_str(&row(
        "Turns per task",
        format!("{:.1}", x.mean_turns),
        format!("{:.1}", y.mean_turns),
        &delta(x.mean_turns, y.mean_turns, false),
    ));
    out.push_str(&row(
        "Wall time",
        format!("{:.1}s", x.mean_wall_secs),
        format!("{:.1}s", y.mean_wall_secs),
        &delta(x.mean_wall_secs, y.mean_wall_secs, false),
    ));
    out.push_str(&row(
        "False-finish rate",
        pct(x.false_finish_rate),
        pct(y.false_finish_rate),
        &delta(x.false_finish_rate, y.false_finish_rate, false),
    ));
    out.push_str(&row(
        "Invalid-call rate",
        pct(x.invalid_call_rate),
        pct(y.invalid_call_rate),
        &delta(x.invalid_call_rate, y.invalid_call_rate, false),
    ));
    out.push_str(&row(
        "Verification reminders per task",
        format!("{:.2}", x.verify_reminders_per_run),
        format!("{:.2}", y.verify_reminders_per_run),
        &delta(x.verify_reminders_per_run, y.verify_reminders_per_run, false),
    ));
    out.push_str(&row(
        "Loop warnings per task",
        format!("{:.2}", x.loop_guards_per_run),
        format!("{:.2}", y.loop_guards_per_run),
        &delta(x.loop_guards_per_run, y.loop_guards_per_run, false),
    ));
    out.push_str(&row(
        "Reasoning-only replies per task",
        format!("{:.2}", x.act_reminders_per_run),
        format!("{:.2}", y.act_reminders_per_run),
        &delta(x.act_reminders_per_run, y.act_reminders_per_run, false),
    ));
    out.push_str(&row(
        "Compactions per task",
        format!("{:.2}", x.compactions_per_run),
        format!("{:.2}", y.compactions_per_run),
        &delta(x.compactions_per_run, y.compactions_per_run, false),
    ));
    out.push_str(&row(
        "Failed summaries per task",
        format!("{:.2}", x.compact_failures_per_run),
        format!("{:.2}", y.compact_failures_per_run),
        &delta(x.compact_failures_per_run, y.compact_failures_per_run, false),
    ));
    out.push_str(&row(
        "Recovery rate",
        opt_pct(x.recovery_rate),
        opt_pct(y.recovery_rate),
        &match (x.recovery_rate, y.recovery_rate) {
            (Some(p), Some(q)) => delta(p, q, true),
            _ => "–".into(),
        },
    ));
    out.push_str(&format!(
        "\n{} runs (A) and {} runs (B). Differences within a few points are noise at small sample sizes; use `--repeat` to tighten them.\n",
        x.runs, y.runs
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_steer_is_a_user_line_for_the_next_step() {
        let v: Value = serde_json::from_str(&user_line("Also do d.", true)).unwrap();
        assert_eq!((v["type"].as_str(), v["priority"].as_str()), (Some("user"), Some("next")));
        assert_eq!(v["message"]["content"][0]["text"], "Also do d.");
        assert!(v["uuid"].is_string());
        let plain: Value = serde_json::from_str(&user_line("Go.", false)).unwrap();
        assert!(plain.get("priority").is_none());
    }

    #[test]
    fn only_the_main_agents_tool_calls_count() {
        let main = json!({"type": "assistant", "parent_tool_use_id": null, "message": {"content": [
            {"type": "text", "text": "x"}, {"type": "tool_use"}, {"type": "tool_use"}]}});
        let sub =
            json!({"type": "assistant", "parent_tool_use_id": "t1", "message": {"content": [{"type": "tool_use"}]}});
        assert_eq!((tool_uses(&main), tool_uses(&sub), tool_uses(&json!({"type": "result"}))), (2, 0, 0));
    }

    fn rec(passed: bool, had_error: bool, false_finish: bool, cost: f64) -> RunRecord {
        RunRecord {
            task: "t".into(),
            pillar: "verification".into(),
            run: 1,
            passed,
            false_finish,
            had_error,
            cost_usd: cost,
            turns: 4,
            wall_secs: 10.0,
            tool_calls: 10,
            tool_errors: if had_error { 2 } else { 0 },
            verify_reminders: 0,
            loop_guards: 0,
            act_reminders: 0,
            edit_diagnostics: 0,
            compactions: 0,
            compact_failures: 0,
            compact_fallbacks: 0,
            agent_error: false,
            stop_reason: Some("end_turn".into()),
            verification: None,
            timed_out: false,
            check_output: String::new(),
            workspace: PathBuf::new(),
        }
    }

    #[test]
    fn summary_metrics() {
        let runs = vec![
            rec(true, false, false, 0.1),
            rec(true, true, false, 0.2),
            rec(false, true, true, 0.3),
            rec(false, false, true, 0.4),
        ];
        let s = summarize(&runs);
        assert_eq!(s.runs, 4);
        assert!((s.pass_rate - 0.5).abs() < 1e-9);
        assert!((s.mean_cost_usd - 0.25).abs() < 1e-9);
        assert!((s.cost_per_pass_usd.unwrap() - 0.5).abs() < 1e-9);
        assert_eq!(s.recovery_rate, Some(0.5));
        assert!((s.false_finish_rate - 0.5).abs() < 1e-9);
        assert!((s.invalid_call_rate - 0.1).abs() < 1e-9);
    }

    #[test]
    fn stream_parsing() {
        let lines = vec![
            json!({"type": "system", "subtype": "init"}),
            json!({"type": "assistant", "message": {"content": [{"type": "tool_use"}, {"type": "tool_use"}]}}),
            json!({"type": "user", "message": {"content": [{"type": "tool_result", "is_error": true}, {"type": "tool_result"}]}}),
            json!({"type": "system", "subtype": "verification", "kind": "unchecked"}),
            json!({"type": "system", "subtype": "loop_guard", "kind": "repeated_failure", "after_changes": true}),
            json!({"type": "system", "subtype": "act_reminder", "kind": "reasoning_cutoff", "reasoning_chars": 8000}),
            json!({"type": "system", "subtype": "diagnostics", "file": "a.py", "parser": "python3"}),
            json!({"type": "system", "subtype": "compact_failed", "trigger": "auto", "reason": "empty_summary"}),
            json!({"type": "system", "subtype": "compact_failed", "trigger": "auto", "reason": "empty_summary"}),
            json!({"type": "system", "subtype": "compact_fallback", "trigger": "auto"}),
            json!({"type": "system", "subtype": "compact_boundary", "compact_metadata": {"trigger": "auto"}}),
            json!({"type": "result", "subtype": "success", "is_error": false, "total_cost_usd": 0.05, "num_turns": 3, "stop_reason": "end_turn", "verification": {"status": "unchecked", "files": ["a.py"]}}),
        ];
        let f = parse_stream(&lines);
        assert_eq!((f.verify_reminders, f.loop_guards), (1, 1));
        assert_eq!((f.compactions, f.compact_failures, f.compact_fallbacks), (1, 2, 1));
        assert_eq!((f.act_reminders, f.edit_diagnostics), (1, 1));
        assert_eq!((f.tool_calls, f.tool_errors, f.turns), (2, 1, 3));
        assert!(f.saw_result && !f.is_error && !f.api_error);
        assert_eq!(f.cost_usd, 0.05);
        assert_eq!(f.verification.as_deref(), Some("unchecked"));
    }

    #[test]
    fn markdown_and_compare_render() {
        let mk = |label: &str, runs: Vec<RunRecord>| Report {
            label: label.into(),
            created: String::new(),
            args: vec![],
            summary: summarize(&runs),
            by_pillar: BTreeMap::from([("verification".into(), summarize(&runs))]),
            runs,
        };
        let a = mk("base", vec![rec(false, false, true, 0.2)]);
        let b = mk("verify-loop", vec![rec(true, false, false, 0.3)]);
        let md = render_markdown(&b);
        assert!(md.contains("| Pass rate | 100% |"));
        let cmp = render_compare(&a, &b);
        assert!(cmp.contains("| Pass rate | 0% | 100% | B better |"));
        assert!(cmp.contains("| Cost per task | $0.2000 | $0.3000 | A better |"));
    }
}
