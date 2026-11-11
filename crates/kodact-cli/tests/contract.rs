//! The public CLI contract (docs/CLI.md), checked at the process boundary:
//! stdout, stderr and exit status separately, with a clean environment.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use kodact_api::MockTurn;
use kodact_test_host::{kodact_bin, MockApi};
use serde_json::Value;

struct Env {
    _dir: tempfile::TempDir,
    cwd: PathBuf,
    home: PathBuf,
}

fn env_with_cwd(name: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let cwd = root.join(name);
    let home = root.join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    Env { _dir: dir, cwd, home }
}

fn env() -> Env {
    env_with_cwd("project")
}

/// A clean environment: no credentials, no network endpoint, isolated home.
fn kodact(e: &Env, args: &[&str]) -> Command {
    let mut c = Command::new(kodact_bin());
    c.args(args)
        .current_dir(&e.cwd)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &e.home)
        .env("KODACT_HOME", e.home.join(".kodact"))
        .stdin(Stdio::null());
    c
}

fn run<C: std::borrow::BorrowMut<Command>>(mut c: C) -> (i32, String, String) {
    let o: Output = c.borrow_mut().output().unwrap();
    (
        o.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&o.stdout).into(),
        String::from_utf8_lossy(&o.stderr).into(),
    )
}

fn with_api(mut c: Command, api: &MockApi) -> Command {
    c.env("KODACT_BASE_URL", &api.url)
        .env("KODACT_API_KEY", "sk-test-key-0123456789abcdef")
        .env("KODACT_MAX_RETRIES", "0");
    c
}

#[test]
fn help_and_version_need_no_credentials_or_network() {
    let e = env();
    for args in [
        &["--help"][..],
        &["-h"],
        &["config", "--help"],
        &["completion", "--help"],
        &["doctor", "--help"],
        &["help", "config"],
    ] {
        let (code, out, err) = run(kodact(&e, args));
        assert_eq!(code, 0, "{args:?}: {err}");
        assert!(out.contains("Usage"), "{args:?}: help goes to stdout");
        assert!(err.is_empty(), "{args:?}: {err}");
    }
    let (code, out, _) = run(kodact(&e, &["--version"]));
    assert_eq!(code, 0);
    assert!(out.trim().ends_with("(KodAct)"));
}

#[test]
fn usage_errors_exit_2_on_stderr() {
    let e = env();
    for args in [
        &["--bogus"][..],
        &["-p", "--output-format", "yaml", "x"],
        &["-p", "--max-turns", "0", "x"],
        &["-p", "--effort", "extreme", "x"],
        &["-p", "--session-id", "not-a-uuid", "x"],
        &["-p", "--add-dir", "/does/not/exist", "x"],
        &["config", "nope"],
    ] {
        let (code, out, err) = run(kodact(&e, args));
        assert_eq!(code, 2, "{args:?}: {err}");
        assert!(out.is_empty(), "{args:?}: stdout must stay empty, got {out}");
        assert!(!err.is_empty());
    }
}

#[test]
fn missing_credentials_exit_3_with_a_next_step() {
    let e = env();
    let (code, out, err) = run(kodact(&e, &["-p", "hello"]));
    assert_eq!(code, 3);
    assert!(out.is_empty());
    assert!(err.contains("KODACT_API_KEY") && err.contains("kodact doctor"), "{err}");
    // In JSON mode stdout holds exactly one error document.
    let (code, out, _) = run(kodact(&e, &["-p", "--output-format", "json", "hello"]));
    assert_eq!(code, 3);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["type"], "error");
    assert_eq!(v["error"]["exit_code"], 3);
}

#[test]
fn interactive_mode_needs_a_terminal_and_input() {
    let e = env();
    let (code, out, err) = run(kodact(&e, &[]));
    assert_eq!(code, 2);
    assert!(out.is_empty());
    assert!(err.contains("needs a terminal") && err.contains("kodact -p"), "{err}");
    let (code, _, err) = run(kodact(&e, &["--no-input"]));
    assert_eq!(code, 2);
    assert!(err.contains("--no-input"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_results_on_stdout_diagnostics_on_stderr() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("the answer")]).await;
    let (code, out, err) = run(with_api(kodact(&e, &["-p", "q"]), &api));
    assert_eq!((code, out.as_str()), (0, "the answer\n"), "{err}");
    // An error no retry fixes (a 500 or a 429 is waited out first).
    let api = MockApi::start(vec![MockTurn::http_error(400, "invalid_request_error")]).await;
    let (code, out, err) = run(with_api(kodact(&e, &["-p", "q"]), &api));
    assert_eq!(code, 1);
    assert!(out.is_empty(), "errors never go to stdout: {out}");
    assert_eq!(err.matches("API Error").count(), 1, "the error is reported once: {err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn prompts_starting_with_a_dash_and_paths_with_spaces() {
    let e = env_with_cwd("my project dir");
    let api = MockApi::start(vec![MockTurn::text("ok")]).await;
    let (code, out, err) = run(with_api(kodact(&e, &["-p", "--", "-v means verbose?"]), &api));
    assert_eq!((code, out.trim()), (0, "ok"), "{err}");
    let sent = api.requests()[0]["messages"][0]["content"].as_array().unwrap().last().unwrap()["text"].clone();
    assert_eq!(sent, "-v means verbose?");
}

#[test]
fn color_follows_no_color_and_flag() {
    let e = env();
    let (_, _, err) = run(kodact(&e, &["-p", "x"]).env("NO_COLOR", "1"));
    assert!(!err.contains('\x1b'), "{err:?}");
    let (_, _, err) = run(kodact(&e, &["-p", "x", "--color", "always"]));
    assert!(err.contains('\x1b'), "{err:?}");
    let (_, _, err) = run(kodact(&e, &["-p", "x", "--color", "never"]).env("FORCE_COLOR", "1"));
    assert!(!err.contains('\x1b'), "--color never wins over FORCE_COLOR");
}

#[test]
fn config_redacts_secrets_and_shows_origin_and_paths() {
    let e = env();
    std::fs::create_dir_all(e.cwd.join(".kodact")).unwrap();
    std::fs::write(
        e.cwd.join(".kodact/settings.json"),
        r#"{"model": "sonnet", "env": {"KODACT_API_KEY": "sk-secret-value-1234567890", "DEBUG": "1"}}"#,
    )
    .unwrap();
    let (code, out, _) = run(kodact(&e, &["config", "list"]));
    assert_eq!(code, 0);
    assert!(!out.contains("sk-secret-value"), "{out}");
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["env"]["KODACT_API_KEY"], "<redacted>");
    assert_eq!(v["env"]["DEBUG"], "1");
    let (_, out, _) = run(kodact(&e, &["config", "list", "--origin"]));
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["model"]["source"], "projectSettings");
    assert!(v["model"]["file"].as_str().unwrap().ends_with(".kodact/settings.json"));
    let (code, out, _) = run(kodact(&e, &["config", "get", "env.KODACT_API_KEY"]));
    assert_eq!((code, out.trim()), (0, "<redacted>"));
    let (code, _, err) = run(kodact(&e, &["config", "get", "nope.nothing"]));
    assert_eq!(code, 1);
    assert!(err.contains("not set"));
    let (code, out, _) = run(kodact(&e, &["config", "paths"]));
    assert_eq!(code, 0);
    assert!(out.contains(&e.home.join(".kodact").display().to_string()), "{out}");
    assert!(out.contains("state") && out.contains("cache"));
}

#[test]
fn completions_are_generated_from_the_command_definitions() {
    let e = env();
    for shell in ["bash", "zsh", "fish"] {
        let (code, out, _) = run(kodact(&e, &["completion", shell]));
        assert_eq!(code, 0);
        assert!(out.contains("kodact") && out.contains("output-format"), "{shell}");
    }
}

#[test]
fn closed_stdout_pipe_does_not_panic() {
    let e = env();
    let mut c = kodact(&e, &["completion", "bash"]);
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!err.contains("panicked"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ctrl_c_interrupts_cleanly_and_exits_130() {
    let e = env();
    let api =
        MockApi::start(vec![MockTurn::text("slow answer that keeps going").with_delay(Duration::from_millis(400))])
            .await;
    let mut c = with_api(kodact(&e, &["-p", "--output-format", "stream-json", "--verbose", "go"]), &api);
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let child = c.spawn().unwrap();
    tokio::time::sleep(Duration::from_millis(700)).await;
    unsafe { libc_kill(child.id() as i32) };
    let out = tokio::task::spawn_blocking(move || child.wait_with_output()).await.unwrap().unwrap();
    assert_eq!(out.status.code(), Some(130));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last: Value = serde_json::from_str(stdout.lines().last().unwrap()).unwrap();
    assert_eq!(last["type"], "result", "a result is still written: {stdout}");
    assert_eq!(last["stop_reason"], "interrupted");
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

unsafe fn libc_kill(pid: i32) {
    kill(pid, 2);
}

#[test]
fn unreachable_endpoint_names_the_next_step() {
    let e = env();
    let (code, out, err) = run(kodact(&e, &["-p", "q"])
        .env("KODACT_BASE_URL", "http://127.0.0.1:9")
        .env("KODACT_API_KEY", "x")
        .env("KODACT_MAX_RETRIES", "0"));
    assert_eq!(code, 1);
    assert!(out.is_empty());
    assert!(err.contains("could not connect") && err.contains("KODACT_BASE_URL"), "{err}");
}

#[test]
fn doctor_reports_missing_credentials_with_status_3() {
    let e = env();
    let (code, out, _) = run(kodact(&e, &["doctor"]));
    assert_eq!(code, 3);
    assert!(out.contains("FAIL") && out.contains("KODACT_API_KEY"), "{out}");
    let (code, _, _) = run(kodact(&e, &["doctor"]).env("KODACT_API_KEY", "x"));
    assert_eq!(code, 0);
    let _ = Path::new("/");
}

/// A hook that exits without reading its stdin must not kill kodact (SIGPIPE), even when
/// the event is larger than a pipe buffer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_hook_that_ignores_stdin_cannot_kill_kodact() {
    let e = env();
    std::fs::create_dir_all(e.cwd.join(".kodact")).unwrap();
    std::fs::write(
        e.cwd.join(".kodact/settings.json"),
        r#"{"hooks": {"UserPromptSubmit": [{"hooks": [{"type": "command", "command": "true"}]}]}}"#,
    )
    .unwrap();
    let api = MockApi::start(vec![MockTurn::text("fine"), MockTurn::text("fine"), MockTurn::text("fine")]).await;
    let big = "x".repeat(100_000); // more than a pipe buffer, less than an argument limit
    for _ in 0..3 {
        let (code, out, err) = run(with_api(kodact(&e, &["-p", &big]), &api));
        assert_eq!((code, out.as_str()), (0, "fine\n"), "{err}");
    }
}

/// A reader that goes away (`kodact ... | head -c 1`) ends kodact quietly: no panic text.
#[test]
fn closed_stdout_ends_quietly() {
    let e = env();
    let script = format!("set -o pipefail; '{}' completion zsh | head -c 1 >/dev/null", kodact_bin().display());
    let o = Command::new("bash")
        .arg("-c")
        .arg(&script)
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .env("HOME", &e.home)
        .current_dir(&e.cwd)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(o.status.success(), "status {:?}: {err}", o.status);
    assert!(!err.contains("panicked") && !err.contains("Broken pipe"), "{err}");
}

/// An OpenAI-compatible endpoint that refuses: exit 3, and the hint names the
/// variable that endpoint reads, never KODACT_API_KEY, which is not sent there.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_endpoint_401_names_the_right_variable_and_exits_3() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::http_error(401, "invalid_request_error")]).await;
    let mut c = kodact(&e, &["-p", "hello"]);
    c.env("KODACT_OPENAI_BASE_URL", format!("{}/v1", api.url))
        .env("KODACT_API_KEY", "sk-messages-0123456789abcdef")
        .env("KODACT_MAX_RETRIES", "0");
    let (code, out, err) = tokio::task::spawn_blocking(move || run(c)).await.unwrap();
    assert_eq!(code, 3, "{err}");
    assert!(out.is_empty());
    assert!(err.contains("set KODACT_OPENAI_API_KEY") && err.contains("/v1/chat/completions"), "{err}");
    assert!(!err.contains("Check KODACT_API_KEY"), "{err}");
    let head = &api.headers()[0];
    assert!(head.starts_with("post /v1/chat/completions"), "{head}");
    assert!(!head.contains("authorization") && !head.contains("sk-messages"), "no key crosses providers: {head}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn openai_endpoint_sends_only_its_own_key() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::http_error(401, "invalid_request_error")]).await;
    let mut c = kodact(&e, &["-p", "--output-format", "json", "hello"]);
    c.env("KODACT_OPENAI_BASE_URL", format!("{}/v1", api.url))
        .env("KODACT_OPENAI_API_KEY", "sk-openai-0123456789")
        .env("KODACT_API_KEY", "sk-messages-0123456789");
    let (code, out, _) = tokio::task::spawn_blocking(move || run(c)).await.unwrap();
    assert_eq!(code, 3);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["exit_code"], 3);
    let head = &api.headers()[0];
    assert!(head.contains("authorization: bearer sk-openai-0123456789") && !head.contains("sk-messages"), "{head}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn project_settings_cannot_redirect_the_key() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("leaked")]).await;
    std::fs::create_dir_all(e.cwd.join(".kodact")).unwrap();
    std::fs::write(
        e.cwd.join(".kodact/settings.json"),
        serde_json::json!({"openai": {"baseUrl": format!("{}/v1", api.url)}, "apiKeyHelper": "touch ran"}).to_string(),
    )
    .unwrap();
    let mut c = kodact(&e, &["-p", "hello"]);
    c.env("KODACT_OPENAI_API_KEY", "sk-openai-0123456789");
    let (code, _, err) = tokio::task::spawn_blocking(move || run(c)).await.unwrap();
    assert_eq!(code, 3, "{err}");
    assert!(err.contains("Ignored openai.baseUrl") && err.contains("Ignored apiKeyHelper"), "{err}");
    assert!(api.requests().is_empty(), "nothing was sent to the project's URL");
    assert!(!e.cwd.join("ran").exists(), "the project's key helper did not run");
}

#[cfg(unix)]
#[test]
fn key_helper_failure_is_named() {
    let e = env();
    std::fs::create_dir_all(e.home.join(".kodact")).unwrap();
    std::fs::write(e.home.join(".kodact/settings.json"), r#"{"apiKeyHelper": "echo nope >&2; exit 3"}"#).unwrap();
    let (code, _, err) = run(kodact(&e, &["-p", "hello"]).env("KODACT_BASE_URL", "http://127.0.0.1:9"));
    assert_eq!(code, 3);
    assert!(err.contains("apiKeyHelper failed") && err.contains("exited with status 3: nope"), "{err}");
}

#[test]
fn doctor_reports_provider_endpoint_and_masked_key() {
    let e = env();
    let (code, out, _) = run(kodact(&e, &["doctor"])
        .env("KODACT_OPENAI_BASE_URL", "https://gw.example.com/v1")
        .env("KODACT_API_KEY", "x"));
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("OpenAI-compatible") && out.contains("FAIL credentials"), "{out}");
    assert!(out.contains("KODACT_OPENAI_API_KEY") && out.contains("never sent"), "{out}");
    let (code, out, _) = run(kodact(&e, &["doctor"])
        .env("KODACT_OPENAI_BASE_URL", "https://gw.example.com/v1")
        .env("KODACT_OPENAI_API_KEY", "sk-0123456789abwxyz"));
    assert!(out.contains("...wxyz") && !out.contains("sk-0123"), "{out}");
    assert!(out.contains("https://gw.example.com/v1 (from KODACT_OPENAI_BASE_URL)"), "{out}");
    assert!(!out.contains("FAIL credentials") && !out.contains("FAIL endpoint"), "{out}");
    let _ = code;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn doctor_probe_checks_the_key() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::http_error(401, "invalid_request_error")]).await;
    let url = format!("{}/v1", api.url);
    let (u, cwd, home) = (url.clone(), e.cwd.clone(), e.home.clone());
    let plain = move |probe: bool| {
        let mut c = Command::new(kodact_bin());
        c.args(if probe { vec!["doctor", "--probe"] } else { vec!["doctor"] })
            .current_dir(&cwd)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env("HOME", &home)
            .env("KODACT_HOME", home.join(".kodact"))
            .env("KODACT_OPENAI_BASE_URL", &u)
            .env("KODACT_OPENAI_API_KEY", "sk-wrong-0123456789")
            .stdin(Stdio::null());
        run(c)
    };
    let p = plain.clone();
    let (_, out, _) = tokio::task::spawn_blocking(move || p(false)).await.unwrap();
    assert!(api.headers().is_empty(), "plain doctor makes no request: {out}");
    let (code, out, _) = tokio::task::spawn_blocking(move || plain(true)).await.unwrap();
    assert_eq!(code, 3, "{out}");
    assert!(out.contains("FAIL probe") && out.contains("401"), "{out}");
    assert!(api.headers()[0].starts_with("get /v1/models"), "{:?}", api.headers());
}

/// A 429 doesn't fail the run: the request waits, is sent again, and the
/// person is told the concurrency limit went down.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rate_limits_are_waited_out_and_reported() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::http_error(429, "rate_limit_error"), MockTurn::text("answered")]).await;
    let mut c = with_api(kodact(&e, &["-p", "hello"]), &api);
    c.env("KODACT_MAX_CONCURRENT_REQUESTS", "2");
    let (code, out, err) = tokio::task::spawn_blocking(move || run(c)).await.unwrap();
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.trim(), "answered");
    assert!(
        err.contains("The endpoint is limiting requests (HTTP 429). KodAct now sends at most 1 at a time"),
        "{err}"
    );
    assert_eq!(api.requests().len(), 2);
}
