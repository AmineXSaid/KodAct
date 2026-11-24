//! End-to-end: what each permission mode asks before, with the real `kodact`
//! binary over stream-json and `--permission-prompt-tool stdio`, as KodAct's
//! desktop drives it. The model reads and edits a file, then deletes another.

use std::path::PathBuf;

use kodact_api::MockTurn;
use kodact_test_host::{command, kodact_bin, Host, MockApi};
use serde_json::{json, Value};

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
    std::fs::write(cwd.join("notes.txt"), "original").unwrap();
    std::fs::write(cwd.join("junk.txt"), "junk").unwrap();
    Env { _dir: dir, cwd, home }
}

/// Run one turn in `mode`: an edit, then `rm junk.txt`. Every question
/// asked is allowed; returns the tools asked about, in order.
async fn asked_in(e: &Env, mode: &str, extra: &[&str]) -> Vec<String> {
    let notes = e.cwd.join("notes.txt");
    // A file is read before it is edited (KodAct refuses an unread edit).
    let api = MockApi::start(vec![
        MockTurn::tool("Read", json!({"file_path": notes})),
        MockTurn::tool("Edit", json!({"file_path": notes, "old_string": "original", "new_string": "changed"})),
        MockTurn::tool("Bash", json!({"command": "rm junk.txt"})),
        MockTurn::text("done"),
    ])
    .await;
    let mut args = vec![
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-prompt-tool",
        "stdio",
        "--permission-mode",
        mode,
    ];
    args.extend_from_slice(extra);
    let mut h = Host::spawn(command(&kodact_bin(), &e.cwd, &e.home, &api.url, &args));
    h.send_user("tidy up").await;
    let mut asked = vec![];
    loop {
        let v: Value = h.until(|v| v["type"] == "result" || v["type"] == "control_request").await;
        if v["type"] == "result" {
            break;
        }
        if v["request"]["subtype"] == "can_use_tool" {
            asked.push(v["request"]["tool_name"].as_str().unwrap_or_default().to_string());
            let id = v["request_id"].as_str().unwrap().to_string();
            let input = v["request"]["input"].clone();
            h.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id,
                "response": {"behavior": "allow", "updatedInput": input}}}))
                .await;
        }
    }
    h.close_stdin();
    h.wait().await;
    asked
}

#[tokio::test]
async fn manual_asks_before_the_edit_and_the_command() {
    let e = env();
    assert_eq!(asked_in(&e, "default", &[]).await, ["Edit", "Bash"]);
    assert_eq!(std::fs::read_to_string(e.cwd.join("notes.txt")).unwrap(), "changed");
    assert!(!e.cwd.join("junk.txt").exists());
}

#[tokio::test]
async fn edit_automatically_edits_without_asking_but_asks_before_a_deletion() {
    let e = env();
    assert_eq!(asked_in(&e, "acceptEdits", &[]).await, ["Bash"]);
    assert_eq!(std::fs::read_to_string(e.cwd.join("notes.txt")).unwrap(), "changed");
}

#[tokio::test]
async fn plan_changes_nothing() {
    let e = env();
    let asked = asked_in(&e, "plan", &[]).await;
    assert!(!asked.contains(&"Edit".to_string()), "{asked:?}");
    assert_eq!(std::fs::read_to_string(e.cwd.join("notes.txt")).unwrap(), "original", "no edit in Plan");
    assert!(e.cwd.join("junk.txt").exists(), "no deletion in Plan");
}

#[tokio::test]
async fn bypass_asks_nothing_once_allowed() {
    let e = env();
    let asked = asked_in(&e, "bypassPermissions", &["--allow-dangerously-skip-permissions"]).await;
    assert!(asked.is_empty(), "{asked:?}");
    assert_eq!(std::fs::read_to_string(e.cwd.join("notes.txt")).unwrap(), "changed");
    assert!(!e.cwd.join("junk.txt").exists());
}

/// The mode changed by the host while a turn runs (Shift+Tab in KodAct's
/// desktop): the next tool call already follows it, and the model is told
/// at its next prompt, when planning starts and once when it ends.
#[tokio::test]
async fn a_mode_change_mid_turn_applies_at_once_and_reaches_the_model() {
    let e = env();
    let notes = e.cwd.join("notes.txt");
    let api = MockApi::start(vec![
        MockTurn::tool("Read", json!({"file_path": notes})),
        MockTurn::tool("Bash", json!({"command": "touch made.txt"})),
        MockTurn::tool("Edit", json!({"file_path": notes, "old_string": "original", "new_string": "changed"})),
        MockTurn::text("first done"),
        MockTurn::text("second done"),
        MockTurn::text("third done"),
    ])
    .await;
    let args = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-prompt-tool",
        "stdio",
        "--permission-mode",
        "default",
    ];
    let mut h = Host::spawn(command(&kodact_bin(), &e.cwd, &e.home, &api.url, &args));
    h.send_user("tidy up").await;
    // Manual: the command asks. While it waits, the person switches to Plan.
    let ask: Value = h.until(|v| v["type"] == "control_request").await;
    assert_eq!(ask["request"]["tool_name"], "Bash");
    h.send(json!({"type": "control_request", "request_id": "m1", "request": {"subtype": "set_permission_mode", "mode": "plan"}}))
        .await;
    h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "m1").await;
    let id = ask["request_id"].as_str().unwrap().to_string();
    h.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id,
        "response": {"behavior": "allow", "updatedInput": ask["request"]["input"]}}}))
        .await;
    // The edit that follows runs in Plan: refused, nothing asked.
    let next: Value = h.until(|v| v["type"] == "result" || v["type"] == "control_request").await;
    assert_eq!(next["type"], "result", "Plan asked instead of refusing: {next}");
    assert_eq!(std::fs::read_to_string(&notes).unwrap(), "original", "no edit once in Plan");
    let refused = api.requests().last().unwrap().to_string();
    assert!(refused.contains("plan"), "the refusal says why: {refused}");

    // The next prompt tells the model it is planning now.
    h.send_user("go on").await;
    h.until_type("result").await;
    let second = api.requests().last().unwrap().to_string();
    assert!(second.contains("Plan mode is active"), "{second}");

    // Back to Edit automatically between prompts: told once that planning ended.
    h.send(json!({"type": "control_request", "request_id": "m2", "request": {"subtype": "set_permission_mode", "mode": "acceptEdits"}}))
        .await;
    h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "m2").await;
    h.send_user("and now?").await;
    h.until_type("result").await;
    let third = api.requests().last().unwrap().to_string();
    assert!(third.contains("Plan mode is off (the permission mode is now acceptEdits)"), "{third}");
    h.close_stdin();
    h.wait().await;
}

/// The desktop's "Bypass permissions while it still asks" (2026-11-24): a chat
/// launched in Auto without `--allow-dangerously-skip-permissions` is switched to
/// Bypass. The switch is refused, and the host is told the mode the engine is
/// really in, so its menu can't say Bypass while `rm` still asks. Launched with
/// the flag, the same switch is granted, reported, and nothing asks.
#[tokio::test]
async fn a_mode_switch_reports_the_mode_the_engine_is_in() {
    for (extra, granted) in [(&[][..], false), (&["--allow-dangerously-skip-permissions"][..], true)] {
        let e = env();
        let api =
            MockApi::start(vec![MockTurn::tool("Bash", json!({"command": "rm junk.txt"})), MockTurn::text("done")])
                .await;
        let mut args = vec![
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            "auto",
        ];
        args.extend_from_slice(extra);
        let mut h = Host::spawn(command(&kodact_bin(), &e.cwd, &e.home, &api.url, &args));
        h.send(json!({"type": "control_request", "request_id": "b1",
            "request": {"subtype": "set_permission_mode", "mode": "bypassPermissions"}}))
            .await;
        let reply: Value = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "b1").await;
        assert_eq!(reply["response"]["subtype"] == "success", granted, "{reply}");
        let status: Value = h.until(|v| v["type"] == "system" && v["subtype"] == "status").await;
        let expected = if granted { "bypassPermissions" } else { "auto" };
        assert_eq!(status["permissionMode"], expected, "{status}");

        h.send_user("tidy up").await;
        let next: Value = h.until(|v| v["type"] == "result" || v["type"] == "control_request").await;
        if granted {
            assert_eq!(next["type"], "result", "Bypass asked: {next}");
            assert!(!e.cwd.join("junk.txt").exists(), "rm ran unasked in Bypass");
        } else {
            assert_eq!(next["type"], "control_request", "Auto still asks before a removal: {next}");
            let id = next["request_id"].as_str().unwrap().to_string();
            h.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id,
                "response": {"behavior": "deny", "message": "no"}}}))
                .await;
            h.until_type("result").await;
        }
        h.close_stdin();
        h.wait().await;
    }
}
