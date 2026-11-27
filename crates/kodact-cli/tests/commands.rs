//! Built-in slash commands in print mode (contract C17): every one answers
//! locally, with no model call, and failures exit 1.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use kodact_api::MockTurn;
use kodact_test_host::{command, kodact_bin, MockApi};
use serde_json::Value;

struct Env {
    _dir: tempfile::TempDir,
    cwd: PathBuf,
    home: PathBuf,
}

fn env() -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let (cwd, home) = (root.join("project"), root.join("home"));
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    Env { _dir: dir, cwd, home }
}

fn write(path: PathBuf, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

async fn kodact(e: &Env, api: &str, args: &[&str]) -> (i32, String, String) {
    let mut c = command(&kodact_bin(), &e.cwd, &e.home, api, args);
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = tokio::time::timeout(Duration::from_secs(60), c.output()).await.unwrap().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn info_commands_answer_locally() {
    let e = env();
    write(e.cwd.join("KODACT.md"), "Use tabs.");
    write(e.cwd.join(".kodact/skills/pdf/SKILL.md"), "---\ndescription: Read PDF files\n---\nUse pdftotext.");
    write(
        e.cwd.join(".kodact/settings.json"),
        r#"{"hooks": {"PreToolUse": [{"matcher": "Bash", "hooks": [{"type": "command", "command": "true"}]}]}}"#,
    );
    let api = MockApi::start(vec![]).await;
    let run = |args: &'static [&'static str]| {
        let (e, url) = (&e, api.url.clone());
        async move { kodact(e, &url, args).await }
    };

    let (code, out, _) = run(&["-p", "/help"]).await;
    assert_eq!(code, 0);
    for want in [
        "/usage - Show this session's cost, token use and activity (also /cost, /stats)",
        "/clear [name] - Start a new conversation; the current one stays resumable (also /reset, /new)",
        "/compact [instructions] -",
        "/pdf - Read PDF files (skill)",
    ] {
        assert!(out.contains(want), "/help lacks {want:?}:\n{out}");
    }

    let (code, out, _) = run(&["-p", "/status"]).await;
    assert_eq!(code, 0);
    for want in
        ["KodAct ", "Session:", "Directory:", "Model:", "Permissions:    default mode", "Memory:         1 file(s)"]
    {
        assert!(out.contains(want), "/status lacks {want:?}:\n{out}");
    }

    for cmd in [&["-p", "/usage"][..], &["-p", "/cost"], &["-p", "/stats"]] {
        let (code, out, _) = kodact(&e, &api.url, cmd).await;
        assert_eq!(code, 0);
        assert!(out.contains("Total cost:     $0.0000") && out.contains("Usage by model: none yet"), "{out}");
    }

    let (code, out, _) = run(&["-p", "/skills"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("12 skill(s):") && out.contains("pdf - Read PDF files"), "{out}");
    assert!(
        out.contains("verify - Prove a change works by running it, not by reading it [bundled; you and KodAct;"),
        "{out}"
    );

    let (code, out, _) = run(&["-p", "/memory"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("KODACT.md"), "{out}");

    let (code, out, _) = run(&["-p", "/hooks"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("PreToolUse:") && out.contains("[Bash] true"), "{out}");

    let (code, out, _) = run(&["-p", "/agents"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("Subagents:") && out.contains("To add one"), "{out}");

    let (code, out, _) = run(&["-p", "/plugin"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("No plugins loaded"), "{out}");

    let (code, out, _) = run(&["-p", "/mcp"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("No MCP servers configured"), "{out}");

    let (code, out, _) = run(&["-p", "/tasks"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("No background tasks."), "{out}");

    let (code, out, _) = run(&["-p", "/release-notes"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("0.1.0"), "{out}");

    let (code, out, _) = run(&["-p", "/clear"]).await;
    assert_eq!(code, 0);
    assert!(out.starts_with("Conversation cleared. The previous one is saved: /resume "), "{out}");

    let (code, out, _) = run(&["-p", "/doctor"]).await;
    assert!(out.contains("settings") && out.contains("credentials") && out.contains("model"), "{out}");
    assert!(code == 0 || code == 1, "{code}");

    let (code, out, _) = run(&["-p", "/exit"]).await;
    assert_eq!((code, out.as_str()), (0, ""));

    assert!(api.requests().is_empty(), "no model calls for local commands");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_commands_fail_with_exit_1() {
    let e = env();
    let api = MockApi::start(vec![]).await;
    // Account and cloud commands are out of scope: they don't exist.
    for name in ["nope", "login", "logout", "upgrade", "remote-control", "teleport"] {
        let (code, out, err) = kodact(&e, &api.url, &["-p", &format!("/{name}")]).await;
        assert_eq!((code, out.as_str()), (1, ""), "/{name}");
        assert!(err.contains(&format!("Unknown command: /{name}")), "{err}");
    }
    let (code, _, err) = kodact(&e, &api.url, &["-p", "/tasks stop nope"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("No background task nope."), "{err}");
    let (code, _, err) = kodact(&e, &api.url, &["-p", "/plugin install x"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("marketplaces aren't supported"), "{err}");
    assert!(api.requests().is_empty());

    // A path or a slash inside a name is a prompt, not a command.
    let api = MockApi::start(vec![MockTurn::text("a"), MockTurn::text("b")]).await;
    let (code, out, _) = kodact(&e, &api.url, &["-p", "/tmp is full"]).await;
    assert_eq!((code, out.trim()), (0, "a"));
    let (code, out, _) = kodact(&e, &api.url, &["-p", "/a/b c"]).await;
    assert_eq!((code, out.trim()), (0, "b"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn json_output_marks_local_results() {
    let e = env();
    let api = MockApi::start(vec![]).await;
    let (code, out, _) = kodact(&e, &api.url, &["-p", "/usage", "--output-format", "json"]).await;
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["num_turns"], 0);
    assert_eq!(v["is_error"], false);
    assert!(v["result"].as_str().unwrap().contains("Total cost:"));

    let (code, out, _) = kodact(&e, &api.url, &["-p", "/login", "--output-format", "json"]).await;
    assert_eq!(code, 1);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!((v["is_error"].as_bool(), v["exit_code"].as_i64()), (Some(true), Some(1)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_commands_persist_and_reach_the_api() {
    let e = env();
    let api = MockApi::start(vec![]).await;
    // /config saves to user settings; the next run starts with it.
    let (code, out, err) =
        kodact(&e, &api.url, &["-p", "/config model=sonnet effortLevel=low autoCompactWindow=300k"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("model = \"sonnet\". Saved in user settings"), "{out}");
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(e.home.join(".kodact/settings.json")).unwrap()).unwrap();
    assert_eq!((saved["model"].as_str(), saved["autoCompactWindow"].as_u64()), (Some("sonnet"), Some(300_000)));
    let (_, out, _) = kodact(&e, &api.url, &["-p", "/status"]).await;
    assert!(out.contains("Model:          claude-sonnet-5-5 · effort low"), "{out}");

    // Over stream-json, /model lasts for the session only, and reaches the request.
    let api = MockApi::start(vec![MockTurn::text("hi")]).await;
    let mut c = command(
        &kodact_bin(),
        &e.cwd,
        &e.home,
        &api.url,
        &["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"],
    );
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child.stdin.take().unwrap();
        for text in ["/model opus", "/fast on", "hello"] {
            let line = serde_json::json!({"type": "user", "message": {"role": "user", "content": text}});
            stdin.write_all(format!("{line}\n").as_bytes()).await.unwrap();
        }
    }
    let out = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output()).await.unwrap().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Model: claude-opus-5-5 · 1M window. (This session only.)"), "{stdout}");
    let reqs = api.requests();
    assert_eq!(reqs.len(), 1);
    assert_eq!((reqs[0]["model"].as_str(), reqs[0]["speed"].as_str()), (Some("claude-opus-5-5"), Some("fast")));
    assert!(api.headers()[0].contains("fast-mode-2026-02-01"), "fast mode sends its beta flag");
    let saved: Value =
        serde_json::from_str(&std::fs::read_to_string(e.home.join(".kodact/settings.json")).unwrap()).unwrap();
    assert_eq!(saved["model"], "sonnet", "-p never changes the saved default");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debug_turns_on_a_session_log() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("Looks like a timeout.")]).await;
    let (code, out, _) = kodact(&e, &api.url, &["-p", "/debug"]).await;
    assert_eq!(code, 0);
    assert!(out.starts_with("Debug logging is on, writing to ") && out.contains("/debug/"), "{out}");
    let (code, out, _) = kodact(&e, &api.url, &["-p", "/debug the request hangs"]).await;
    assert_eq!((code, out.trim()), (0, "Looks like a timeout."));
    let prompt = api.requests()[0].to_string();
    assert!(prompt.contains("The user reports this problem") && prompt.contains("the request hangs"), "{prompt}");
    let logs: Vec<_> = std::fs::read_dir(e.home.join(".kodact/state/debug")).unwrap().flatten().collect();
    assert_eq!(logs.len(), 2, "one log per session");
    assert!(logs.iter().any(|f| std::fs::metadata(f.path()).unwrap().len() > 0), "the turn was logged");
}

fn verdict(v: &str, reason: &str) -> MockTurn {
    // A pass must point at tool output.
    let evidence: Vec<&str> = if v == "met" { vec![reason] } else { vec![] };
    MockTurn::text(&serde_json::json!({"verdict": v, "evidence": evidence, "reason": reason}).to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn goal_runs_to_completion_in_print_mode() {
    let e = env();
    let glob = || MockTurn::tool("Glob", serde_json::json!({"pattern": "*"}));
    let script = || {
        vec![
            glob(),
            MockTurn::text("Started."),
            verdict("not_met", "not yet"),
            glob(),
            MockTurn::text("Done."),
            verdict("met", "it's done"),
        ]
    };
    let api = MockApi::start(script()).await;
    let (code, out, err) =
        kodact(&e, &api.url, &["-p", "/goal finish the work", "--output-format", "stream-json", "--verbose"]).await;
    assert_eq!(code, 0, "{err}");
    let lines: Vec<Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let results: Vec<&Value> = lines.iter().filter(|l| l["type"] == "result").collect();
    assert_eq!(results.len(), 2, "one result per turn");
    assert_eq!(results[1]["result"], "Done.");
    let goals: Vec<&str> = lines
        .iter()
        .filter(|l| l["type"] == "system" && l["subtype"] == "goal")
        .filter_map(|l| l["status"].as_str())
        .collect();
    assert_eq!(goals, ["active", "active", "achieved"], "{out}");
    // Results come in order: the first turn's result before the second turn starts.
    let first_result = lines.iter().position(|l| l["type"] == "result").unwrap();
    let done = lines.iter().position(|l| l.to_string().contains("\"Done.\"")).unwrap();
    assert!(first_result < done);

    // Text mode prints each turn's answer; success is quiet unless --verbose.
    let api = MockApi::start(script()).await;
    let (code, out, err) = kodact(&e, &api.url, &["-p", "/goal finish the work"]).await;
    assert_eq!((code, out.as_str(), err.as_str()), (0, "Started.\nDone.\n", ""));
    let api = MockApi::start(script()).await;
    let (_, _, err) = kodact(&e, &api.url, &["-p", "/goal finish the work", "--verbose"]).await;
    assert!(err.contains("Goal achieved: it's done"), "{err}");

    // A goal that can't be met says so, even without --verbose.
    let api = MockApi::start(vec![MockTurn::text("Hmm."), verdict("impossible", "no such file")]).await;
    let (code, _, err) = kodact(&e, &api.url, &["-p", "/goal fix missing.rs"]).await;
    assert_eq!(code, 1, "an unmet goal fails the run");
    assert!(err.contains("Goal can't be met: no such file"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn clear_starts_a_new_session_for_stream_hosts() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("one"), MockTurn::text("two")]).await;
    let mut c = command(
        &kodact_bin(),
        &e.cwd,
        &e.home,
        &api.url,
        &["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"],
    );
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child.stdin.take().unwrap();
        for text in ["first", "/clear", "second"] {
            let line = serde_json::json!({"type": "user", "message": {"role": "user", "content": text}});
            stdin.write_all(format!("{line}\n").as_bytes()).await.unwrap();
        }
    }
    let out = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output()).await.unwrap().unwrap();
    let lines: Vec<Value> =
        String::from_utf8_lossy(&out.stdout).lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let inits: Vec<&str> = lines
        .iter()
        .filter(|l| l["type"] == "system" && l["subtype"] == "init")
        .filter_map(|l| l["session_id"].as_str())
        .collect();
    assert_eq!(inits.len(), 2, "a new init after /clear");
    assert_ne!(inits[0], inits[1]);
    let results: Vec<&Value> = lines.iter().filter(|l| l["type"] == "result").collect();
    assert_eq!(results.len(), 3);
    assert_eq!(
        (results[0]["session_id"].as_str(), results[2]["session_id"].as_str()),
        (Some(inits[0]), Some(inits[1]))
    );
    // The second conversation starts empty.
    let reqs = api.requests();
    assert_eq!(reqs[1]["messages"].as_array().unwrap().len(), 1);
}

async fn kodact_env(e: &Env, api: &str, args: &[&str], vars: &[(&str, &str)]) -> (i32, String, String) {
    let mut c = command(&kodact_bin(), &e.cwd, &e.home, api, args);
    for (k, v) in vars {
        c.env(k, v);
    }
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = tokio::time::timeout(Duration::from_secs(60), c.output()).await.unwrap().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn print_mode_keeps_running_for_scheduled_tasks() {
    let e = env();
    // A minute of schedule time is a tenth of a second here.
    let fast = [("KODACT_TEST_TIME_SCALE", "600")];
    let api = MockApi::start(vec![MockTurn::text("hi 1"), MockTurn::text("hi 2"), MockTurn::text("hi 3")]).await;
    let started = std::time::Instant::now();
    let (code, out, err) = kodact_env(&e, &api.url, &["-p", "/loop 1m say hi", "--max-turns", "3"], &fast).await;
    // The task was still scheduled when --max-turns ended the run: exit 4.
    assert_eq!((code, out.as_str()), (4, "hi 1\nhi 2\nhi 3\n"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(30));
    let reqs = api.requests();
    assert_eq!(reqs.len(), 3, "it stops at --max-turns");
    assert!(reqs.iter().all(|r| r.to_string().contains("say hi")));

    // stream-json reports each run, with a system event naming the task.
    let api = MockApi::start(vec![MockTurn::text("a"), MockTurn::text("b")]).await;
    let (code, out, _) = kodact_env(
        &e,
        &api.url,
        &["-p", "/loop 2m ping", "--max-turns", "2", "--output-format", "stream-json", "--verbose"],
        &fast,
    )
    .await;
    assert_eq!(code, 4);
    let lines: Vec<Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(lines.iter().filter(|l| l["type"] == "result").count(), 2);
    assert!(lines.iter().any(|l| l["subtype"] == "scheduled" && l["cron"] == "*/2 * * * *"), "{out}");
    assert!(lines.iter().any(|l| l["subtype"] == "scheduled_task" && l["prompt"] == "ping"), "{out}");

    // Scheduling can be turned off.
    let (code, _, err) = kodact_env(&e, &api.url, &["-p", "/loop 5m x"], &[("KODACT_DISABLE_CRON", "1")]).await;
    assert_eq!(code, 1);
    assert!(err.contains("Scheduling is off"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn feedback_writes_a_local_bundle_with_secrets_masked() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("noted")]).await;
    let mut c = command(
        &kodact_bin(),
        &e.cwd,
        &e.home,
        &api.url,
        &["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"],
    );
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    {
        use tokio::io::AsyncWriteExt;
        let mut stdin = child.stdin.take().unwrap();
        for text in ["deploy with key sk-abcdefghijklmnopqrstu and password=hunter2", "/bug it hangs"] {
            let line = serde_json::json!({"type": "user", "message": {"role": "user", "content": text}});
            stdin.write_all(format!("{line}\n").as_bytes()).await.unwrap();
        }
    }
    let out = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output()).await.unwrap().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("Saved a feedback bundle in ") && stdout.contains("Nothing was sent anywhere"), "{stdout}");
    let root = e.home.join(".kodact/state/feedback");
    let dirs: Vec<_> = std::fs::read_dir(&root).unwrap().flatten().map(|d| d.path()).collect();
    assert_eq!(dirs.len(), 1);
    let dir = &dirs[0];
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir).unwrap().permissions().mode() & 0o777, 0o700);
    }
    let report = std::fs::read_to_string(dir.join("report.txt")).unwrap();
    assert!(report.starts_with("Description:\nit hangs") && report.contains("KodAct:"), "{report}");
    assert!(std::fs::read_to_string(dir.join("doctor.txt")).unwrap().contains("credentials"));
    let transcript = std::fs::read_to_string(dir.join("transcript.jsonl")).unwrap();
    assert!(transcript.contains("deploy with key <redacted> and password=<redacted>"), "{transcript}");
    assert!(!transcript.contains("hunter2") && !transcript.contains("sk-abcdefghijklmnopqrstu"));
    assert_eq!(api.requests().len(), 1, "no model call for /feedback");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_shows_a_plan_then_applies_it() {
    let e = env();
    write(e.home.join(".codex/config.toml"), "[mcp_servers.docs]\ncommand = \"docs-mcp\"\nargs = [\"--stdio\"]\n");
    write(e.cwd.join("GEMINI.md"), "Run pnpm test before committing.");
    let api = MockApi::start(vec![]).await;
    let (code, out, err) = kodact(&e, &api.url, &["-p", "/import"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.contains("MCP server docs from Codex: docs-mcp --stdio -> your user settings"), "{out}");
    assert!(
        out.contains("Imported from Gemini CLI (project instructions)") && out.contains("Run /import --yes to apply.")
    );
    assert!(!e.home.join(".kodact/settings.json").exists(), "the plan alone changes nothing");

    let (code, out, _) = kodact(&e, &api.url, &["-p", "/import codex gemini --yes"]).await;
    assert_eq!(code, 0);
    assert!(out.contains("Added MCP server docs (stdio)") && out.contains("next session"), "{out}");
    let settings: Value =
        serde_json::from_str(&std::fs::read_to_string(e.home.join(".kodact/settings.json")).unwrap()).unwrap();
    assert_eq!(settings["mcpServers"]["docs"]["args"], serde_json::json!(["--stdio"]));
    assert!(std::fs::read_to_string(e.cwd.join("KODACT.md")).unwrap().contains("Run pnpm test before committing."));

    let (_, out, _) = kodact(&e, &api.url, &["-p", "/import"]).await;
    assert!(out.starts_with("Nothing to import"), "{out}");
    let (code, _, err) = kodact(&e, &api.url, &["-p", "/import vscode"]).await;
    assert_eq!(code, 1);
    assert!(err.contains("Unknown source \"vscode\""), "{err}");
    assert!(api.requests().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn print_mode_waits_for_subtasks_then_answers_with_their_reports() {
    let e = env();
    let api =
        MockApi::start(vec![MockTurn::text("There are 3 TODOs."), MockTurn::text("The subtask found 3 TODOs.")]).await;
    let (code, out, err) = kodact(&e, &api.url, &["-p", "/subtask count the TODOs"]).await;
    assert_eq!(code, 0, "{err}");
    assert!(out.starts_with("Started subtask_1 in the background: count the TODOs."), "{out}");
    assert!(out.trim_end().ends_with("The subtask found 3 TODOs."), "{out}");
    let reqs = api.requests();
    assert_eq!(reqs.len(), 2, "the fork's turn, then one turn with its report");
    let last = reqs[1]["messages"].to_string();
    assert!(
        last.contains("Background subtask subtask_1 (count the TODOs) has finished")
            && last.contains("There are 3 TODOs."),
        "{last}"
    );

    // JSON output stays one object: the turn that handed the report back, with the fork's cost in it.
    let api = MockApi::start(vec![MockTurn::text("found it"), MockTurn::text("done")]).await;
    let (code, out, _) = kodact(&e, &api.url, &["-p", "/subtask look", "--output-format", "json"]).await;
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["result"], "done");
}
