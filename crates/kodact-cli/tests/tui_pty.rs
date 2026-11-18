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
