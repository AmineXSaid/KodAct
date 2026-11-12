//! End-to-end: the real `kodact` binary against a scripted Messages API.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

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
    let cwd = root.join("project");
    let home = root.join("home");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    Env { _dir: dir, cwd, home }
}

async fn run(e: &Env, api: &MockApi, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    let mut c = command(&kodact_bin(), &e.cwd, &e.home, &api.url, args);
    c.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    {
        use tokio::io::AsyncWriteExt;
        let mut si = child.stdin.take().unwrap();
        if let Some(s) = stdin {
            si.write_all(s.as_bytes()).await.unwrap();
        }
    }
    let out = tokio::time::timeout(Duration::from_secs(30), child.wait_with_output()).await.unwrap().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into(),
        String::from_utf8_lossy(&out.stderr).into(),
    )
}

fn stream_args<'a>(extra: &[&'a str]) -> Vec<&'a str> {
    let mut v = vec!["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose"];
    v.extend_from_slice(extra);
    v
}

#[tokio::test]
async fn print_text_and_request_shape() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("Hello from the mock")]).await;
    let (code, out, err) = run(&e, &api, &["-p", "say hello"], None).await;
    assert_eq!(code, 0, "stderr: {err}");
    assert_eq!(out.trim(), "Hello from the mock");
    let req = &api.requests()[0];
    assert_eq!(req["model"], "claude-opus-5-5");
    assert_eq!(req["stream"], true);
    assert!(req["system"][0]["text"].as_str().unwrap().starts_with("You are KodAct"));
    assert!(req["tools"].as_array().unwrap().iter().any(|t| t["name"] == "Edit"));
    assert_eq!(req["messages"][0]["content"].as_array().unwrap().last().unwrap()["text"], "say hello");
}

#[tokio::test]
async fn print_json_and_piped_stdin() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("42")]).await;
    let (code, out, _) =
        run(&e, &api, &["-p", "--output-format", "json", "what is this?"], Some("some piped data")).await;
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["type"], "result");
    assert_eq!(v["subtype"], "success");
    assert_eq!(v["result"], "42");
    assert_eq!(v["num_turns"], 1);
    assert!(v["session_id"].as_str().unwrap().len() == 36);
    let sent = api.requests()[0]["messages"][0]["content"].as_array().unwrap().last().unwrap()["text"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(sent, "some piped data\n\nwhat is this?");
}

#[tokio::test]
async fn print_attaches_at_mentions() {
    let e = env();
    std::fs::write(e.cwd.join("notes.md"), "Ship the parser on Friday.").unwrap();
    let api = MockApi::start(vec![MockTurn::text("Friday.")]).await;
    let (code, out, err) = run(&e, &api, &["-p", "summarize @notes.md"], None).await;
    assert_eq!((code, out.trim()), (0, "Friday."), "stderr: {err}");
    let content = api.requests()[0]["messages"][0]["content"].as_array().unwrap().clone();
    let texts: Vec<&str> = content.iter().filter_map(|b| b["text"].as_str()).collect();
    assert!(texts.contains(&"summarize @notes.md"), "{texts:?}");
    let note = texts.last().unwrap();
    assert!(note.starts_with("<system-reminder>") && note.contains("Ship the parser on Friday."), "{note}");
}

/// `kodact -p --no-input "prompt"` took the prompt as the flag's value ("value was not a
/// boolean"): the flag takes a value only as `--no-input=false`.
#[tokio::test]
async fn no_input_does_not_take_the_prompt_as_its_value() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("done."), MockTurn::text("done.")]).await;
    let (code, out, err) = run(&e, &api, &["-p", "--no-input", "say done"], None).await;
    assert_eq!((code, out.trim()), (0, "done."), "stderr: {err}");
    let (code, out, err) = run(&e, &api, &["-p", "--no-input=false", "say done"], None).await;
    assert_eq!((code, out.trim()), (0, "done."), "stderr: {err}");
}

/// Session transcripts and what else is kept under the state directory were readable by
/// every user of the machine; only the prompt history was private.
#[cfg(unix)]
#[tokio::test]
async fn the_state_directory_is_private_to_its_owner() {
    use std::os::unix::fs::PermissionsExt;
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("ok")]).await;
    let (code, _, err) = run(&e, &api, &["-p", "remember this"], None).await;
    assert_eq!(code, 0, "{err}");
    let mut found = vec![];
    let mut stack = vec![e.home.clone()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|x| x == "jsonl") {
                found.push(path);
            }
        }
    }
    assert!(!found.is_empty(), "a transcript was written under {}", e.home.display());
    for transcript in found {
        // Some folder between the home and the transcript lets no one else in.
        let closed = transcript
            .ancestors()
            .skip(1)
            .take_while(|a| a.starts_with(&e.home))
            .any(|a| std::fs::metadata(a).unwrap().permissions().mode() & 0o077 == 0);
        assert!(closed, "{} is reachable by other users", transcript.display());
    }
}

#[tokio::test]
async fn print_errors() {
    let e = env();
    let api = MockApi::start(vec![]).await;
    let (code, out, err) = run(&e, &api, &["-p"], Some("")).await;
    assert_eq!(code, 2, "usage error");
    assert!(err.contains("no prompt") && err.contains("hint:"), "{err}");
    assert!(out.is_empty(), "nothing on stdout in text mode");
    let (code, _, err) = run(&e, &api, &["-p", "--permission-mode", "yolo", "x"], None).await;
    assert_eq!(code, 2);
    assert!(err.contains("invalid --permission-mode"), "{err}");
    let api = MockApi::start(vec![MockTurn::http_error(401, "authentication_error")]).await;
    let (code, out, err) = run(&e, &api, &["-p", "--output-format", "json", "x"], None).await;
    assert_eq!(code, 3, "a refused key is a configuration problem");
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["type"], "result");
    assert_eq!(v["subtype"], "error_during_execution");
    assert_eq!(v["exit_code"], 3);
    let msg = v["errors"][0].as_str().unwrap();
    assert!(msg.contains("401") && msg.contains("KODACT_API_KEY"), "the error names the next step: {msg}");
    assert!(!err.contains("{"), "stderr carries no JSON: {err}");
}

/// Replace values that change between runs.
fn normalize(mut v: Value) -> Value {
    fn walk(v: &mut Value) {
        match v {
            Value::Object(m) => {
                for (k, val) in m.iter_mut() {
                    match k.as_str() {
                        "uuid" | "session_id" | "id" | "tool_use_id" | "cwd" | "duration_ms" | "duration_api_ms"
                        | "pid" | "total_cost_usd" | "costUSD" | "kodact_version" => {
                            if !val.is_null() {
                                *val = json!(format!("<{k}>"));
                            }
                        }
                        _ => walk(val),
                    }
                }
            }
            Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(&mut v);
    v
}

#[tokio::test]
async fn stream_json_golden_basic() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("Hi!")]).await;
    let (code, out, err) = run(&e, &api, &["-p", "--output-format", "stream-json", "--verbose", "hello"], None).await;
    assert_eq!(code, 0, "{err}");
    let lines: Vec<Value> = out.lines().map(|l| normalize(serde_json::from_str(l).unwrap())).collect();
    let types: Vec<String> = lines
        .iter()
        .map(|l| format!("{}/{}", l["type"].as_str().unwrap(), l["subtype"].as_str().unwrap_or("")))
        .collect();
    assert_eq!(types, vec!["system/init", "assistant/", "result/success"]);
    let golden_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/fixtures/stream-json-basic.jsonl");
    let rendered: String = lines.iter().map(|l| serde_json::to_string(l).unwrap() + "\n").collect();
    if std::env::var_os("KODACT_UPDATE_GOLDEN").is_some() || !golden_path.exists() {
        std::fs::write(&golden_path, &rendered).unwrap();
    }
    let golden = std::fs::read_to_string(&golden_path).unwrap();
    assert_eq!(rendered, golden, "stream-json shape changed; rerun with KODACT_UPDATE_GOLDEN=1 if intended");
}

#[tokio::test]
async fn host_permission_prompt_allow_and_deny() {
    let e = env();
    let api = MockApi::start(vec![
        MockTurn::tool("Bash", json!({"command": "touch created.txt", "description": "make a file"})),
        MockTurn::text("made it"),
        MockTurn::tool("Bash", json!({"command": "touch second.txt"})),
        MockTurn::text("ok, not made"),
    ])
    .await;
    let mut h = Host::spawn(command(
        &kodact_bin(),
        &e.cwd,
        &e.home,
        &api.url,
        &stream_args(&["--permission-prompt-tool", "stdio"]),
    ));
    h.send(json!({"type": "control_request", "request_id": "init_1", "request": {"subtype": "initialize"}})).await;
    let init_resp = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "init_1").await;
    assert_eq!(init_resp["response"]["subtype"], "success");
    assert!(init_resp["response"]["response"]["models"]
        .as_array()
        .unwrap()
        .iter()
        .any(|m| m["value"] == "claude-opus-5-5"));

    h.send_user("create a file").await;
    let req = h.until(|v| v["type"] == "control_request").await;
    assert_eq!(req["request"]["subtype"], "can_use_tool");
    assert_eq!(req["request"]["tool_name"], "Bash");
    assert_eq!(req["request"]["input"]["command"], "touch created.txt");
    assert_eq!(req["request"]["permission_suggestions"][0]["type"], "addRules");
    let id = req["request_id"].as_str().unwrap().to_string();
    h.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {"behavior": "allow", "updatedInput": {"command": "touch created.txt"}}}})).await;
    let tool_result = h.until(|v| v["type"] == "user").await;
    assert_eq!(tool_result["message"]["content"][0]["type"], "tool_result");
    assert!(tool_result["tool_use_result"].is_object());
    let r = h.until_type("result").await;
    assert_eq!(r["result"], "made it");
    assert!(e.cwd.join("created.txt").exists());

    h.send_user("another").await;
    let req = h.until(|v| v["type"] == "control_request").await;
    let id = req["request_id"].as_str().unwrap().to_string();
    h.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": id, "response": {"behavior": "deny", "message": "not today"}}})).await;
    let tr = h.until(|v| v["type"] == "user").await;
    assert_eq!(tr["message"]["content"][0]["is_error"], true);
    assert_eq!(tr["message"]["content"][0]["content"], "not today");
    let r = h.until_type("result").await;
    assert_eq!(r["permission_denials"][0]["tool_name"], "Bash");
    assert!(!e.cwd.join("second.txt").exists());
    let (code, _) = h.wait().await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn host_reads_what_fills_the_context() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("a short answer")]).await;
    let mut h = Host::spawn(command(&kodact_bin(), &e.cwd, &e.home, &api.url, &stream_args(&[])));
    let ask =
        |id: &str| json!({"type": "control_request", "request_id": id, "request": {"subtype": "get_context_usage"}});
    h.send(ask("c1")).await;
    let r = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "c1").await;
    assert_eq!(r["response"]["subtype"], "success");
    let before = &r["response"]["response"];
    let window = before["window"].as_u64().unwrap();
    assert!(window > 0, "{before}");
    // The parts add up to the total, and what is left is free.
    let parts: u64 = before["categories"].as_array().unwrap().iter().map(|c| c["tokens"].as_u64().unwrap()).sum();
    assert_eq!(parts, before["total"].as_u64().unwrap());
    assert_eq!(before["free"].as_u64().unwrap(), window - parts);
    let named = |v: &serde_json::Value, name: &str| {
        v["categories"].as_array().unwrap().iter().find(|c| c["name"] == name).map(|c| c["tokens"].as_u64().unwrap())
    };
    assert!(named(before, "System prompt").unwrap() > 0);
    assert!(named(before, "Built-in tools").unwrap() > 0);
    assert!(!before["tools"].as_array().unwrap().is_empty());
    // After a turn, the conversation counts.
    h.send_user("hello").await;
    h.until_type("result").await;
    h.send(ask("c2")).await;
    let r = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "c2").await;
    let after = &r["response"]["response"];
    assert!(named(after, "Messages").unwrap() > named(before, "Messages").unwrap(), "{after}");
    let (code, _) = h.wait().await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn host_controls_mode_model_and_interrupt() {
    let e = env();
    let api =
        MockApi::start(
            vec![MockTurn::text("a slow answer streaming in pieces").with_delay(Duration::from_millis(300))],
        )
        .await;
    let mut h =
        Host::spawn(command(&kodact_bin(), &e.cwd, &e.home, &api.url, &stream_args(&["--include-partial-messages"])));
    h.send(json!({"type": "control_request", "request_id": "m1", "request": {"subtype": "set_permission_mode", "mode": "plan"}})).await;
    let r = h.until(|v| v["type"] == "control_response").await;
    assert_eq!(r["response"]["subtype"], "success");
    h.send(json!({"type": "control_request", "request_id": "m2", "request": {"subtype": "set_permission_mode", "mode": "nope"}})).await;
    let r = h.until(|v| v["type"] == "control_response").await;
    assert_eq!(r["response"]["subtype"], "error");
    // Bypassing permissions takes --allow-dangerously-skip-permissions at launch.
    h.send(json!({"type": "control_request", "request_id": "m4", "request": {"subtype": "set_permission_mode", "mode": "bypassPermissions"}})).await;
    let r = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "m4").await;
    assert_eq!(r["response"]["subtype"], "error");
    assert!(r["response"]["error"].as_str().unwrap().contains("--allow-dangerously-skip-permissions"));
    h.send(
        json!({"type": "control_request", "request_id": "m3", "request": {"subtype": "set_model", "model": "sonnet"}}),
    )
    .await;
    h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "m3").await;

    h.send_user("talk slowly").await;
    h.until_type("stream_event").await;
    h.send(json!({"type": "control_request", "request_id": "int", "request": {"subtype": "interrupt"}})).await;
    let r = h.until_type("result").await;
    assert_eq!(r["stop_reason"], "interrupted");
    assert_eq!(r["subtype"], "success");
    assert_eq!(api.requests()[0]["model"], "claude-sonnet-5-5");
    // The system prompt names the model the host switched to.
    let system = api.requests()[0]["system"].to_string();
    assert!(system.contains("Sonnet 5.5") && !system.contains("Opus 5.5"), "{system}");
    let unknown = json!({"type": "control_request", "request_id": "u", "request": {"subtype": "teleport"}});
    h.send(unknown).await;
    let r = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "u").await;
    assert_eq!(r["response"]["subtype"], "error");
    let (code, _) = h.wait().await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn host_stops_one_agent_and_the_turn_goes_on() {
    let e = env();
    // The parent starts an agent; the agent says something and lists files, then its next answer
    // streams slowly; the parent then answers.
    let api = MockApi::start(vec![
        MockTurn::tool("Task", json!({"description": "look around", "prompt": "Survey the project."})),
        MockTurn::blocks(
            vec![
                kodact_types::ContentBlock::text("Listing the files first."),
                kodact_types::ContentBlock::ToolUse {
                    id: "toolu_ls".into(),
                    name: "LS".into(),
                    input: json!({"path": "."}),
                    cache_control: None,
                    provider: None,
                },
            ],
            kodact_types::StopReason::ToolUse,
        ),
        MockTurn::text("a long survey streaming in pieces").with_delay(Duration::from_millis(300)),
        MockTurn::text("Carrying on without the survey."),
    ])
    .await;
    let mut h =
        Host::spawn(command(&kodact_bin(), &e.cwd, &e.home, &api.url, &stream_args(&["--include-partial-messages"])));
    // Nothing runs yet: an unknown id is refused, and `all` stops none.
    h.send(json!({"type": "control_request", "request_id": "s1", "request": {"subtype": "stop_task", "task_id": "toolu_nope"}})).await;
    let r = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "s1").await;
    assert_eq!(r["response"]["subtype"], "error");
    assert_eq!(r["response"]["error"], "No running agent toolu_nope");
    h.send(json!({"type": "control_request", "request_id": "s2", "request": {"subtype": "stop_task", "all": true}}))
        .await;
    let r = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "s2").await;
    assert_eq!(r["response"]["subtype"], "success");
    assert_eq!(r["response"]["response"]["stopped"], 0);
    h.send(json!({"type": "control_request", "request_id": "s3", "request": {"subtype": "stop_task"}})).await;
    let r = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "s3").await;
    assert_eq!(r["response"]["subtype"], "error");

    h.send_user("survey it").await;
    // The agent is at work: stop it by its Task call's id.
    h.until(|v| v["type"] == "user" && v["parent_tool_use_id"] == "toolu_00_task").await;
    // Its slow answer has been asked for (the third request): the stop lands mid-answer.
    while api.requests().len() < 3 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    h.send(json!({"type": "control_request", "request_id": "s4", "request": {"subtype": "stop_task", "task_id": "toolu_00_task"}})).await;
    let r = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "s4").await;
    assert_eq!(r["response"]["subtype"], "success", "{r}");
    assert_eq!(r["response"]["response"]["stopped"], 1);
    // The parent sees the stopped result and its turn continues to a normal end.
    let tr = h
        .until(|v| {
            v["type"] == "user"
                && v["parent_tool_use_id"].is_null()
                && v["message"]["content"][0]["tool_use_id"] == "toolu_00_task"
        })
        .await;
    let block = &tr["message"]["content"][0];
    assert_eq!(block["is_error"], true, "{tr}");
    let text = block["content"].to_string();
    assert!(text.contains(kodact_tools::AGENT_STOPPED), "{tr}");
    // Its last words before the stop come along; the slow answer never finished.
    assert!(text.contains("Listing the files first.") && !text.contains("a long survey"), "{tr}");
    assert_eq!(tr["tool_use_result"]["status"], "stopped", "{tr}");
    let r = h.until_type("result").await;
    assert_eq!(r["result"], "Carrying on without the survey.", "{r}");
    assert_eq!(r["subtype"], "success");
    assert_ne!(r["stop_reason"], "interrupted");
    let (code, _) = h.wait().await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn stream_json_answers_immediate_commands_mid_turn() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("A slow answer.").with_delay(Duration::from_millis(100))]).await;
    let mut h = Host::spawn(command(&kodact_bin(), &e.cwd, &e.home, &api.url, &stream_args(&[])));
    let init = h.until(|v| v["type"] == "system" && v["subtype"] == "init").await;
    // The turn streams for a while; the reader answers /status meanwhile.
    h.send_user("take your time").await;
    h.send_user("/status").await;
    // The command's result comes first, marked immediate; the turn's own result follows.
    let first = h.until_type("result").await;
    assert_eq!((first["immediate"].clone(), first["num_turns"].clone()), (json!(true), json!(0)), "{first}");
    assert!(first["result"].as_str().unwrap().contains("Model:"), "{first}");
    assert_eq!(first["session_id"], init["session_id"]);
    let second = h.until_type("result").await;
    assert_eq!(second["result"], "A slow answer.");
    assert!(second.get("immediate").is_none(), "{second}");
    assert_eq!(api.requests().len(), 1, "/status never reached the model");
    let (code, _) = h.wait().await;
    assert_eq!(code, 0);
}

#[tokio::test]
async fn replay_uuid_drives_rewind_files() {
    let e = env();
    let f = e.cwd.join("notes.txt");
    std::fs::write(&f, "original").unwrap();
    let api = MockApi::start(vec![
        MockTurn::tool("Read", json!({"file_path": f})),
        MockTurn::tool("Edit", json!({"file_path": f, "old_string": "original", "new_string": "changed"})),
        MockTurn::text("edited"),
    ])
    .await;
    let mut h = Host::spawn(command(
        &kodact_bin(),
        &e.cwd,
        &e.home,
        &api.url,
        &stream_args(&["--replay-user-messages", "--permission-mode", "acceptEdits"]),
    ));
    h.send_user("edit the notes").await;
    let replay = h.until(|v| v["type"] == "user" && v["isReplay"] == true).await;
    let uuid = replay["uuid"].as_str().unwrap().to_string();
    h.until_type("result").await;
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "changed");
    h.send(json!({"type": "control_request", "request_id": "rw1", "request": {"subtype": "rewind_files", "user_message_id": uuid, "dry_run": true}})).await;
    let r = h.until(|v| v["type"] == "control_response").await;
    assert_eq!(r["response"]["response"]["canRewind"], true);
    assert_eq!(r["response"]["response"]["filesChanged"][0], f.display().to_string());
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "changed", "dry run leaves the file");
    h.send(json!({"type": "control_request", "request_id": "rw2", "request": {"subtype": "rewind_files", "user_message_id": uuid}})).await;
    h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "rw2").await;
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "original");
    h.wait().await;
}

#[tokio::test]
async fn a_message_sent_mid_turn_reaches_the_model_at_its_next_step() {
    let e = env();
    let f = e.cwd.join("notes.txt");
    std::fs::write(&f, "notes").unwrap();
    let api = MockApi::start(vec![
        MockTurn::tool("Read", json!({"file_path": f})).with_delay(Duration::from_millis(400)),
        MockTurn::text("Short, as asked."),
    ])
    .await;
    let mut h =
        Host::spawn(command(&kodact_bin(), &e.cwd, &e.home, &api.url, &stream_args(&["--replay-user-messages"])));
    h.send_user("read the notes").await;
    h.until(|v| v["type"] == "user" && v["isReplay"] == true).await;
    // While the model streams its first step: one message for its next step, one taken back.
    let next = |uuid: &str, text: &str| {
        json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": text}]},
            "parent_tool_use_id": null, "session_id": "", "uuid": uuid, "priority": "next"})
    };
    h.send(next("q1", "Keep it short.")).await;
    h.send(next("q2", "Forget that.")).await;
    h.send(
        json!({"type": "control_request", "request_id": "c1", "request": {"subtype": "cancel_queued", "uuid": "q2"}}),
    )
    .await;
    let r = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "c1").await;
    assert_eq!(r["response"]["response"]["cancelled"], true, "{r}");
    // It lands under its own id, before the turn's result, as one turn.
    let landed = h.until(|v| v["type"] == "user" && v["isReplay"] == true).await;
    assert_eq!(landed["uuid"], "q1", "{landed}");
    let result = h.until_type("result").await;
    assert_eq!(result["result"], "Short, as asked.");
    assert_eq!(api.requests().len(), 2, "no extra turn");
    let second = api.requests()[1]["messages"].to_string();
    assert!(second.contains("Keep it short."), "{second}");
    assert!(!second.contains("Forget that."));
    // Too late to take back now.
    h.send(
        json!({"type": "control_request", "request_id": "c2", "request": {"subtype": "cancel_queued", "uuid": "q1"}}),
    )
    .await;
    let r = h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "c2").await;
    assert_eq!(r["response"]["response"]["cancelled"], false);
    h.wait().await;
}

#[tokio::test]
async fn headless_denies_and_reports() {
    let e = env();
    let api = MockApi::start(vec![
        MockTurn::tool("Write", json!({"file_path": e.cwd.join("x.txt"), "content": "x"})),
        MockTurn::text("could not write"),
    ])
    .await;
    let (code, out, _) = run(&e, &api, &["-p", "--output-format", "json", "write x"], None).await;
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["permission_denials"][0]["tool_name"], "Write");
    assert!(!e.cwd.join("x.txt").exists());
    let api = MockApi::start(vec![
        MockTurn::tool("Write", json!({"file_path": e.cwd.join("y.txt"), "content": "y"})),
        MockTurn::text("wrote"),
    ])
    .await;
    let (code, _, _) = run(&e, &api, &["-p", "write y", "--allowedTools", "Write"], None).await;
    assert_eq!(code, 0);
    assert!(e.cwd.join("y.txt").exists());
}

#[tokio::test]
async fn max_turns_and_continue() {
    let e = env();
    let api = MockApi::start(vec![
        MockTurn::tool("LS", json!({"path": e.cwd})),
        MockTurn::tool("LS", json!({"path": e.cwd})),
    ])
    .await;
    let (code, out, _) = run(&e, &api, &["-p", "--output-format", "json", "--max-turns", "1", "look"], None).await;
    assert_eq!(code, 4, "limits exit with 4");
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["subtype"], "error_max_turns");

    let api = MockApi::start(vec![MockTurn::text("remembered"), MockTurn::text("second")]).await;
    let (_, out, _) = run(&e, &api, &["-p", "--output-format", "json", "remember 7"], None).await;
    let first: Value = serde_json::from_str(out.trim()).unwrap();
    let (_, out, _) = run(&e, &api, &["-p", "--output-format", "json", "-c", "what number?"], None).await;
    let second: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(first["session_id"], second["session_id"]);
    let msgs = api.requests()[1]["messages"].as_array().unwrap().len();
    assert_eq!(msgs, 3, "continued session sends prior turns");
}

#[tokio::test]
async fn print_tells_the_model_its_time_limit() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("ok")]).await;
    let (code, out, _) = run(&e, &api, &["-p", "--max-time", "15m", "hi"], None).await;
    assert_eq!((code, out.trim()), (0, "ok"));
    let first = api.requests()[0]["messages"][0].to_string();
    assert!(first.contains("time limit (--max-time): about 15 minutes left"), "{first}");

    for bad in ["0", "15x", "soon"] {
        let (code, _, err) = run(&e, &api, &["-p", "--max-time", bad, "hi"], None).await;
        assert_eq!(code, 2, "--max-time {bad}: {err}");
    }
    assert_eq!(api.requests().len(), 1, "a bad limit never reaches the API");
}

#[tokio::test]
async fn autonomous_runs_have_no_question_tools_and_attempt_first() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("Not practical."), MockTurn::text("Tried; it can't be done.")]).await;
    // KODACT_REMINDERS=0 leaves the run to the model alone: its first answer stands.
    let quiet = MockApi::start(vec![MockTurn::text("Not practical.")]).await;
    let mut c = command(&kodact_bin(), &e.cwd, &e.home, &quiet.url, &["-p", "--autonomous", "do the hard thing"]);
    c.env("KODACT_REMINDERS", "0").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = tokio::time::timeout(Duration::from_secs(30), c.output()).await.unwrap().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "Not practical.");
    assert_eq!(quiet.requests().len(), 1, "no reminder with KODACT_REMINDERS=0");
    // By default the reminders are on: no giving up before an attempt.
    let (code, out, _) = run(&e, &api, &["-p", "--autonomous", "do the hard thing"], None).await;
    assert_eq!((code, out.trim()), (0, "Tried; it can't be done."));
    let reqs = api.requests();
    let tools: Vec<String> =
        reqs[0]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    for gone in ["AskUserQuestion", "EnterPlanMode", "ExitPlanMode"] {
        assert!(!tools.iter().any(|t| t == gone), "{gone} in {tools:?}");
    }
    assert!(tools.iter().any(|t| t == "Bash"));
    assert!(reqs[0]["system"].to_string().contains("Running unattended"));
    assert!(reqs[1]["messages"].to_string().contains("Make a real attempt first"));

    // KODACT_AUTONOMOUS=1 does the same; =0 doesn't.
    for (val, on) in [("1", true), ("0", false)] {
        let api = MockApi::start(vec![MockTurn::text("a"), MockTurn::text("b")]).await;
        let mut c = command(&kodact_bin(), &e.cwd, &e.home, &api.url, &["-p", "x"]);
        c.env("KODACT_AUTONOMOUS", val).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let out = tokio::time::timeout(Duration::from_secs(30), c.output()).await.unwrap().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(
            api.requests()[0]["system"].to_string().contains("Running unattended"),
            on,
            "KODACT_AUTONOMOUS={val}"
        );
    }

    // Without the flag the question tools stay and nothing is added.
    let api = MockApi::start(vec![MockTurn::text("hi")]).await;
    run(&e, &api, &["-p", "hello"], None).await;
    let req = &api.requests()[0];
    assert!(req["tools"].as_array().unwrap().iter().any(|t| t["name"] == "AskUserQuestion"));
    assert!(!req["system"].to_string().contains("Running unattended"));
}

#[tokio::test]
async fn version_and_help() {
    let out = std::process::Command::new(kodact_bin()).arg("--version").output().unwrap();
    let v = String::from_utf8_lossy(&out.stdout);
    assert!(v.trim().ends_with("(KodAct)"), "{v}");
    let out = std::process::Command::new(kodact_bin()).arg("--help").output().unwrap();
    let h = String::from_utf8_lossy(&out.stdout);
    for flag in [
        "--print",
        "--output-format",
        "--permission-mode",
        "--allowedTools",
        "--resume",
        "--max-budget-usd",
        "--max-time",
        "--autonomous",
        "--json-schema",
    ] {
        assert!(h.contains(flag), "missing {flag}");
    }
}

#[tokio::test]
async fn retries_transient_errors() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::http_error(529, "overloaded_error"), MockTurn::text("after retry")]).await;
    let mut c = command(&kodact_bin(), &e.cwd, &e.home, &api.url, &["-p", "hi"]);
    c.env("KODACT_MAX_RETRIES", "1").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let out = tokio::time::timeout(Duration::from_secs(30), c.output()).await.unwrap().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "after retry");
    assert_eq!(api.requests().len(), 2);
}

/// A spent quota is said at once, in an everyday run too; a 429 asking for a short wait is
/// waited out; one asking for longer than a retry sleeps is told with the wait, not tried
/// again a minute apart.
#[tokio::test]
async fn a_quota_is_not_retried_and_a_long_wait_is_stated() {
    let print = |api: &MockApi, e: &Env, retries: &str| {
        let mut c = command(&kodact_bin(), &e.cwd, &e.home, &api.url, &["-p", "hi"]);
        c.env("KODACT_MAX_RETRIES", retries).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        c
    };
    let e = env();
    let spent = MockTurn::HttpError {
        status: 429,
        kind: "insufficient_quota".into(),
        message: "You exceeded your current quota, please check your plan and billing details".into(),
    };
    let api = MockApi::start(vec![spent, MockTurn::text("never asked")]).await;
    let out = tokio::time::timeout(Duration::from_secs(30), print(&api, &e, "5").output()).await.unwrap().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert_eq!(api.requests().len(), 1, "no retry: {err}");
    assert!(err.contains("credit or spending limit"), "{err}");

    let throttled = |secs| MockTurn::Throttled { message: "slow down".into(), retry_after: Duration::from_secs(secs) };
    let api = MockApi::start(vec![throttled(1), MockTurn::text("after the wait")]).await;
    let out = tokio::time::timeout(Duration::from_secs(30), print(&api, &e, "5").output()).await.unwrap().unwrap();
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "after the wait");
    assert_eq!(api.requests().len(), 2);

    let api = MockApi::start(vec![throttled(48 * 60), MockTurn::text("never asked")]).await;
    let out = tokio::time::timeout(Duration::from_secs(30), print(&api, &e, "5").output()).await.unwrap().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(!out.status.success());
    assert_eq!(api.requests().len(), 1, "not tried again sooner: {err}");
    assert!(err.contains("asks to wait 48m"), "{err}");
}

/// `-p --output-format json`: the result says where the changes stand, and a one-shot run
/// about to end with todo items open is sent back once.
#[tokio::test]
async fn print_reports_verification_and_nudges_open_todos() {
    let e = env();
    let todos = json!({"todos": [
        {"content": "write notes.py", "status": "completed", "activeForm": "writing"},
        {"content": "run it", "status": "pending", "activeForm": "running"},
    ]});
    let api = MockApi::start(vec![
        MockTurn::tools(&[
            ("TodoWrite", todos),
            ("Write", json!({"file_path": e.cwd.join("notes.py"), "content": "print('hi'\n"})),
        ]),
        MockTurn::text("Wrote the file."),
        MockTurn::text("Left: running it."),
        MockTurn::text("Not run here."),
    ])
    .await;
    let args = ["-p", "--output-format", "json", "--permission-mode", "bypassPermissions", "write notes"];
    let (code, out, err) = run(&e, &api, &args, None).await;
    assert_eq!(code, 0, "stderr: {err}");
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["verification"], json!({"status": "unchecked", "files": ["notes.py"]}), "{out}");
    let reqs = api.requests();
    assert_eq!(reqs.len(), 4, "the todo reminder, then the verification reminder");
    let sent = |i: usize| reqs[i]["messages"].as_array().unwrap().last().unwrap().to_string();
    assert!(sent(2).contains("takes no further input") && sent(2).contains("run it"), "{}", sent(2));
    assert!(sent(3).contains("no check has run since the last change"), "{}", sent(3));
    // The edit's own result said the file doesn't parse, when this machine has Python.
    if std::process::Command::new("python3").arg("--version").output().is_ok() {
        assert!(sent(1).contains("notes.py does not parse"), "{}", sent(1));
    }
}

/// Over HTTP, with the real binary: a tool call a model wrote as text is run, and a reply that
/// repeats itself is stopped and the run goes on to an answer.
#[tokio::test]
async fn print_runs_text_form_calls_and_stops_a_repeating_reply() {
    let e = env();
    let call = "<tool_call>{\"name\": \"Bash\", \"arguments\": {\"command\": \"echo from-text-call\"}}</tool_call>";
    let api = MockApi::start(vec![
        MockTurn::text(&"I should think about this once more. ".repeat(300)),
        MockTurn::text(call),
        MockTurn::text("finished"),
    ])
    .await;
    let args = ["-p", "--output-format", "json", "--permission-mode", "bypassPermissions", "go"];
    let (code, out, err) = run(&e, &api, &args, None).await;
    assert_eq!(code, 0, "stderr: {err}");
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["result"], "finished");
    let reqs = api.requests();
    assert_eq!(reqs.len(), 3);
    let sent = |i: usize| reqs[i]["messages"].as_array().unwrap().last().unwrap().to_string();
    assert!(sent(1).contains("was repeating itself"), "{}", sent(1));
    assert!(sent(2).contains("from-text-call"), "the command ran: {}", sent(2));
}

#[tokio::test]
async fn host_sets_thinking_tokens() {
    let e = env();
    let api = MockApi::start(vec![MockTurn::text("one"), MockTurn::text("two")]).await;
    let mut h = Host::spawn(command(&kodact_bin(), &e.cwd, &e.home, &api.url, &stream_args(&["--model", "sonnet"])));
    h.send_user("default thinking").await;
    h.until_type("result").await;
    h.send(json!({"type": "control_request", "request_id": "t0", "request": {"subtype": "set_max_thinking_tokens", "max_thinking_tokens": 0}})).await;
    h.until(|v| v["type"] == "control_response" && v["response"]["request_id"] == "t0").await;
    h.send_user("no thinking").await;
    h.until_type("result").await;
    let reqs = api.requests();
    assert_eq!(reqs[0]["thinking"]["type"], "adaptive");
    assert_eq!(reqs[1]["thinking"]["type"], "between_tools");
    h.wait().await;
}

#[tokio::test]
async fn sandbox_lets_headless_runs_build_without_prompts() {
    if kodact_tools_sandbox_missing() {
        eprintln!("skipped: no sandbox backend on this machine");
        return;
    }
    let e = env();
    let calls = || {
        vec![
            MockTurn::tool("Bash", json!({"command": "mkdir -p build && echo ok > build/out.txt"})),
            MockTurn::text("built"),
        ]
    };
    // Without the sandbox, print mode denies the command (contract C1).
    let api = MockApi::start(calls()).await;
    let (_, out, _) = run(&e, &api, &["-p", "--output-format", "json", "build it"], None).await;
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["permission_denials"][0]["tool_name"], "Bash");
    assert!(!e.cwd.join("build/out.txt").exists());
    // With it, the command runs confined, with no prompt.
    let api = MockApi::start(calls()).await;
    let (code, out, _) =
        run(&e, &api, &["-p", "--output-format", "json", "--sandbox", "workspace-write", "build it"], None).await;
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["permission_denials"], json!([]));
    assert!(e.cwd.join("build/out.txt").exists());
}

fn kodact_tools_sandbox_missing() -> bool {
    !std::process::Command::new("bwrap")
        .args(["--ro-bind", "/", "/", "--dev", "/dev", "--unshare-net", "true"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
