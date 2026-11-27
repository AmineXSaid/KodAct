//! The terminal UI as a person uses it: the real binary in a pseudo-terminal, keys and pastes
//! in, the screen read back (`kodact_test_host::pty`). What only the TUI does is tested here:
//! `-p` runs and in-memory render tests don't go through its input or its screen.

use std::path::PathBuf;
use std::time::Duration;

use kodact_api::MockTurn;
use kodact_test_host::{kodact_bin, pty::Pty, MockApi};

const WAIT: Duration = Duration::from_secs(15);

struct Dirs {
    _tmp: tempfile::TempDir,
    cwd: PathBuf,
    home: PathBuf,
}

fn dirs() -> Dirs {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let (cwd, home) = (root.join("project"), root.join("home"));
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    Dirs { _tmp: tmp, cwd, home }
}

/// The user message the model was sent, as JSON text.
fn prompt_sent(api: &MockApi) -> String {
    let req = &api.requests()[0];
    req["messages"].as_array().unwrap().last().unwrap()["content"].to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_typed_prompt_reaches_the_model_and_its_answer_is_shown() {
    let d = dirs();
    let api = MockApi::start(vec![MockTurn::text("Hello from the mock")]).await;
    let mut tui = Pty::spawn(&kodact_bin(), &d.cwd, &d.home, &api.url, &[], 100, 30);
    tui.wait_for("What shall we build?", WAIT).unwrap_or_else(|s| panic!("no front page:\n{s}"));
    tui.keys("say hello\r");
    let screen = tui.wait_for("Hello from the mock", WAIT).unwrap_or_else(|s| panic!("no answer:\n{s}"));
    assert!(screen.contains("say hello"), "the prompt stays on screen:\n{screen}");
    assert!(prompt_sent(&api).contains("say hello"));
}

/// A pasted block is one prompt: its newlines are part of it, not Enter presses. (The baseline
/// for a paste guard: an Enter that arrives with or right after a paste.)
#[tokio::test(flavor = "multi_thread")]
async fn a_multi_line_paste_is_one_prompt() {
    let d = dirs();
    let api = MockApi::start(vec![MockTurn::text("Got all three lines")]).await;
    let mut tui = Pty::spawn(&kodact_bin(), &d.cwd, &d.home, &api.url, &[], 100, 30);
    tui.wait_for("What shall we build?", WAIT).unwrap_or_else(|s| panic!("no front page:\n{s}"));
    tui.paste("first line\nsecond line\nthird line");
    std::thread::sleep(Duration::from_millis(500));
    assert!(api.requests().is_empty(), "nothing is sent before Enter:\n{}", tui.screen());
    tui.keys("\r");
    tui.wait_for("Got all three lines", WAIT).unwrap_or_else(|s| panic!("no answer:\n{s}"));
    assert_eq!(api.requests().len(), 1);
    let sent = prompt_sent(&api);
    for line in ["first line", "second line", "third line"] {
        assert!(sent.contains(line), "{line} missing from {sent}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn exit_quits() {
    let d = dirs();
    let api = MockApi::start(vec![]).await;
    let mut tui = Pty::spawn(&kodact_bin(), &d.cwd, &d.home, &api.url, &[], 100, 30);
    tui.wait_for("What shall we build?", WAIT).unwrap_or_else(|s| panic!("no front page:\n{s}"));
    tui.keys("/exit\r");
    let end = std::time::Instant::now() + WAIT;
    while !tui.exited() {
        assert!(std::time::Instant::now() < end, "still running:\n{}", tui.screen());
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn mouse_wheel_scrolls_the_discussion_without_changing_the_chatbox() {
    let d = dirs();
    let long_reply =
        (1..=60).map(|i| format!("discussion-line-{i:02}")).collect::<Vec<_>>().join("\n") + "\nBOTTOM-OF-DISCUSSION";
    let api = MockApi::start(vec![MockTurn::text(&long_reply)]).await;
    let mut tui = Pty::spawn(&kodact_bin(), &d.cwd, &d.home, &api.url, &[], 100, 24);
    tui.wait_for("What shall we build?", WAIT).unwrap_or_else(|s| panic!("no front page:\n{s}"));
    tui.keys("my-first-prompt\r");
    tui.wait_for("BOTTOM-OF-DISCUSSION", WAIT).unwrap_or_else(|s| panic!("no bottom:\n{s}"));
    // Type a draft in the chatbox, then roll the mouse wheel up (?1007h sends Up bursts).
    tui.keys("keep-this-draft");
    tui.wait_for("keep-this-draft", WAIT).unwrap_or_else(|s| panic!("no draft:\n{s}"));
    for _ in 0..20 {
        tui.keys("\x1b[A\x1b[A\x1b[A");
    }
    let top_screen = tui.wait_for("discussion-line-01", WAIT).unwrap_or_else(|s| panic!("did not scroll up:\n{s}"));
    assert!(top_screen.contains("keep-this-draft"), "draft in chatbox must stay untouched:\n{top_screen}");
    // Roll the mouse wheel back down to the newest lines of the discussion.
    for _ in 0..20 {
        tui.keys("\x1b[B\x1b[B\x1b[B");
    }
    let bottom_screen =
        tui.wait_for("BOTTOM-OF-DISCUSSION", WAIT).unwrap_or_else(|s| panic!("did not scroll down:\n{s}"));
    assert!(bottom_screen.contains("keep-this-draft"), "draft in chatbox must stay untouched:\n{bottom_screen}");
}

/// Shift+Tab through several modes writes one note, for the mode it lands on, and only once a
/// prompt goes out: not a banner per step of the cycle.
#[tokio::test(flavor = "multi_thread")]
async fn cycling_modes_leaves_one_note_in_the_transcript() {
    let d = dirs();
    let api = MockApi::start(vec![MockTurn::text("Noted")]).await;
    let mut tui = Pty::spawn(&kodact_bin(), &d.cwd, &d.home, &api.url, &[], 120, 30);
    tui.wait_for("What shall we build?", WAIT).unwrap_or_else(|s| panic!("no front page:\n{s}"));
    for _ in 0..3 {
        tui.keys("\x1b[Z");
        std::thread::sleep(Duration::from_millis(150));
    }
    std::thread::sleep(Duration::from_millis(300));
    let screen = tui.screen();
    assert_eq!(screen.matches("── ◆").count(), 0, "no banner while cycling:\n{screen}");
    tui.keys("hi\r");
    let screen = tui.wait_for("Noted", WAIT).unwrap_or_else(|s| panic!("no answer:\n{s}"));
    assert_eq!(screen.matches("── ◆").count(), 1, "one note for where the cycle landed:\n{screen}");
}

/// `kodact setup` from the first key to the saved file: provider, key, model, review.
#[tokio::test(flavor = "multi_thread")]
async fn setup_walks_to_a_saved_settings_file() {
    let d = dirs();
    let mut tui = Pty::spawn(&kodact_bin(), &d.cwd, &d.home, "http://127.0.0.1:9", &["setup"], 100, 30);
    tui.wait_for("Choose a provider", WAIT).unwrap_or_else(|s| panic!("no wizard:\n{s}"));
    tui.keys("\r");
    tui.wait_for("KODACT_API_KEY", WAIT).unwrap_or_else(|s| panic!("no key step:\n{s}"));
    // The test's terminal has a KODACT_API_KEY, which the step would take;
    // a typed key wins and is the one saved.
    tui.paste("sk-ant-test-0000-1234\n");
    let screen = tui.wait_for("21 chars", WAIT).unwrap_or_else(|s| panic!("key not taken:\n{s}"));
    assert!(!screen.contains("sk-ant-test"), "the key is masked:\n{screen}");
    tui.keys("\r");
    tui.wait_for("Pick a default model", WAIT).unwrap_or_else(|s| panic!("no model step:\n{s}"));
    // No suggested list (the endpoint lists the models, and here it doesn't answer): the ID typed is the one used.
    tui.keys("claude-opus-4-6");
    tui.wait_for("Use “claude-opus-4-6”", WAIT).unwrap_or_else(|s| panic!("no typed model:\n{s}"));
    tui.keys("\r");
    let screen = tui.wait_for("Review & save", WAIT).unwrap_or_else(|s| panic!("no review:\n{s}"));
    assert!(screen.contains("claude-opus-4-6") && screen.contains("••••1234"), "{screen}");
    tui.keys("\r");
    let end = std::time::Instant::now() + WAIT;
    while !tui.exited() {
        assert!(std::time::Instant::now() < end, "still running:\n{}", tui.screen());
        std::thread::sleep(Duration::from_millis(50));
    }
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(d.home.join(".kodact/settings.json")).unwrap()).unwrap();
    assert_eq!(saved["model"], "claude-opus-4-6");
    assert_eq!(saved["env"]["KODACT_API_KEY"], "sk-ant-test-0000-1234");
}

/// The welcome page's cards: Tab gives the keyboard to the first, Enter sends its prompt.
#[tokio::test(flavor = "multi_thread")]
async fn a_welcome_card_sends_its_prompt() {
    let d = dirs();
    let api = MockApi::start(vec![MockTurn::text("Here is the map")]).await;
    let mut tui = Pty::spawn(&kodact_bin(), &d.cwd, &d.home, &api.url, &[], 120, 40);
    tui.wait_for("Explain this project", WAIT).unwrap_or_else(|s| panic!("no cards:\n{s}"));
    tui.keys("\t");
    tui.wait_for("back to the prompt", WAIT).unwrap_or_else(|s| panic!("the card has no keyboard:\n{s}"));
    tui.keys("\r");
    tui.wait_for("Here is the map", WAIT).unwrap_or_else(|s| panic!("no answer:\n{s}"));
    assert!(prompt_sent(&api).contains("Explain this project"), "{}", prompt_sent(&api));
}

/// AskUserQuestion in a real terminal: multi-select, typed free-form answer, Esc cancel,
/// and `--dangerously-skip-permissions` (which approves plans without skipping user questions).
#[tokio::test(flavor = "multi_thread")]
async fn ask_user_question_works_in_the_terminal_including_bypass_mode() {
    use serde_json::json;

    // 1. Two questions (5-option multi-select + typed answer), even under --dangerously-skip-permissions.
    let d = dirs();
    let q = json!({
        "questions": [
            {
                "question": "Which stack components?",
                "multi_select": true,
                "options": ["Postgres", "Redis", "SQLite", "ClickHouse", "Kafka"]
            },
            {
                "question": "Service name?",
                "options": ["api"]
            }
        ]
    });
    let api = MockApi::start(vec![MockTurn::tool("AskUserQuestion", q), MockTurn::text("Configured the stack.")]).await;
    let mut tui = Pty::spawn(&kodact_bin(), &d.cwd, &d.home, &api.url, &["--dangerously-skip-permissions"], 100, 30);
    tui.wait_for("What shall we build?", WAIT).unwrap_or_else(|s| panic!("no front page:\n{s}"));
    tui.keys("set up the service\r");
    tui.wait_for("Which stack components? (1/2)", WAIT).unwrap_or_else(|s| panic!("question 1 never opened:\n{s}"));
    // Toggle options 1 (Postgres) and 5 (Kafka), then confirm with Enter.
    tui.keys("15\r");
    tui.wait_for("Service name? (2/2)", WAIT).unwrap_or_else(|s| panic!("question 2 never opened:\n{s}"));
    // Choose "2. Type an answer", type "billing-svc", and press Enter.
    tui.keys("2");
    tui.wait_for("Enter to submit", WAIT).unwrap_or_else(|s| panic!("typing mode never opened:\n{s}"));
    tui.keys("billing-svc\r");
    let screen = tui.wait_for("Configured the stack.", WAIT).unwrap_or_else(|s| panic!("turn never finished:\n{s}"));
    assert!(screen.contains("Asked Which stack components?"), "built-in header shown:\n{screen}");
    let second_req = api.requests()[1].to_string();
    assert!(second_req.contains("Postgres, Kafka") && second_req.contains("billing-svc"), "{second_req}");

    // 2. Esc cancels an open AskUserQuestion dialog and returns control to the prompt box.
    let d = dirs();
    let api = MockApi::start(vec![MockTurn::tool(
        "AskUserQuestion",
        json!({"question": "Which region?", "options": ["eu-west-1", "us-east-1"]}),
    )])
    .await;
    let mut tui = Pty::spawn(&kodact_bin(), &d.cwd, &d.home, &api.url, &[], 100, 30);
    tui.wait_for("What shall we build?", WAIT).unwrap_or_else(|s| panic!("no front page:\n{s}"));
    tui.keys("deploy it\r");
    tui.wait_for("Which region?", WAIT).unwrap_or_else(|s| panic!("dialog never opened:\n{s}"));
    tui.keys("\x1b");
    tui.wait_for("Interrupted", WAIT).unwrap_or_else(|s| panic!("Esc did not cancel question:\n{s}"));
}
