//! Sub-agents run in their own context and report back (docs/GOALS.md, pillar 1).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use kodact_agents::{builtin_agents, AgentRuntime, ParentLink, TaskTool};
use kodact_api::{MockProvider, MockTurn};
use kodact_engine::{DenyPrompter, Engine, EngineConfig, EngineEvent, EngineParts, VecSink};
use kodact_hooks::{HookBase, HookRunner, HooksConfig};
use kodact_permissions::{PermissionMode, RuleSet};
use kodact_session::{FileHistory, SessionStore, Transcript};
use kodact_tools::{ToolContext, ToolRegistry};
use kodact_types::{ContentBlock, MessageContent, MessagesRequest};
use serde_json::json;

const SID: &str = "00000000-0000-4000-8000-0000000000aa";

fn is_child(req: &MessagesRequest) -> bool {
    req.system.first().map(|s| s.text.contains("sub-agent")).unwrap_or(false)
}

struct Setup {
    _dir: tempfile::TempDir,
    cwd: std::path::PathBuf,
    sink: Arc<VecSink>,
    engine: Engine,
    provider: Arc<MockProvider>,
}

fn setup(responder: impl Fn(&MessagesRequest) -> MockTurn + Send + Sync + 'static, hooks: serde_json::Value) -> Setup {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().canonicalize().unwrap().join("proj");
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(cwd.join("lib.rs"), "fn secret_answer() -> u32 { 42 }\n").unwrap();
    let provider = Arc::new(MockProvider::with_responder(responder));
    let sink = Arc::new(VecSink::default());
    let (hcfg, _) = HooksConfig::from_settings(Some(&hooks));
    let hooks = HookRunner::new(
        hcfg,
        HookBase { session_id: SID.into(), transcript_path: None, cwd: cwd.clone(), project_dir: cwd.clone() },
    );
    let tool_ctx = ToolContext::new(&cwd);
    let history = Arc::new(FileHistory::new(dir.path().join("hist")));
    let rt = Arc::new(AgentRuntime {
        provider: provider.clone(),
        agents: builtin_agents(),
        project_dir: cwd.clone(),
        working_dirs: tool_ctx.working_dirs.clone(),
        env: tool_ctx.env.clone(),
        sandbox: Default::default(),
        shell: kodact_platform::shell::detect().clone(),
        line_edits: false,
        unread_edits: false,
        agent_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        extra_tools: vec![],
        store: Some(SessionStore::new(dir.path().join("agents"))),
        session_id: SID.into(),
        hooks: hooks.clone(),
        sink: sink.clone(),
        base: EngineConfig::default(),
        memory_context: None,
        parent: OnceLock::new(),
    });
    let mut tools = ToolRegistry::new();
    kodact_tools::builtin::register_core(&mut tools);
    tools.register(Arc::new(TaskTool { rt: rt.clone() }));
    let store = SessionStore::new(dir.path().join("store"));
    let engine = Engine::new(
        EngineConfig::default(),
        EngineParts {
            provider: provider.clone(),
            tools,
            tool_ctx,
            permissions: kodact_permissions::Engine::new(PermissionMode::Default, RuleSet::default(), &cwd, &[]),
            hooks,
            prompter: Arc::new(DenyPrompter),
            sink: sink.clone(),
            transcript: Arc::new(Transcript::create(&store, &cwd, SID, None, true).unwrap()),
            history: history.clone(),
            system: vec![kodact_types::SystemBlock::text("You are the main agent.")],
        },
    )
    .unwrap();
    let _ = rt.parent.set(ParentLink { handle: engine.handle(), prompter: engine.prompter(), history });
    Setup { _dir: dir, cwd, sink, engine, provider }
}

fn results(e: &Engine) -> Vec<(String, bool)> {
    e.state
        .messages
        .iter()
        .flat_map(|m| m.content.iter())
        .filter_map(|b| match b {
            ContentBlock::ToolResult { content, is_error, .. } => Some((content.to_text(), is_error.unwrap_or(false))),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn explore_agent_reports_back_in_its_own_context() {
    let parent_calls = AtomicUsize::new(0);
    let child_calls = AtomicUsize::new(0);
    let s = setup(
        move |req| {
            if is_child(req) {
                match child_calls.fetch_add(1, Ordering::SeqCst) {
                    0 => MockTurn::tool("Grep", json!({"pattern": "secret_answer", "output_mode": "content"})),
                    _ => MockTurn::text("secret_answer is defined in lib.rs:1 and returns 42."),
                }
            } else {
                match parent_calls.fetch_add(1, Ordering::SeqCst) {
                    0 => MockTurn::tool(
                        "Task",
                        json!({"description": "find the answer", "prompt": "Find where secret_answer is defined and what it returns.", "subagent_type": "Explore"}),
                    ),
                    _ => MockTurn::text("It returns 42."),
                }
            }
        },
        json!({}),
    );
    let mut e = s.engine;
    let r = e.submit(MessageContent::Text("what does secret_answer return?".into())).await;
    assert_eq!(r.result.as_deref(), Some("It returns 42."));

    let reqs = s.provider.requests();
    let child: Vec<&MessagesRequest> = reqs.iter().filter(|r| is_child(r)).collect();
    let parent: Vec<&MessagesRequest> = reqs.iter().filter(|r| !is_child(r)).collect();
    assert_eq!((child.len(), parent.len()), (2, 2));
    let child_tools: Vec<&str> = child[0].tools.iter().map(|t| t.name.as_str()).collect();
    assert!(
        child_tools.contains(&"Grep") && !child_tools.contains(&"Task") && !child_tools.contains(&"Edit"),
        "{child_tools:?}"
    );
    assert!(child[0].messages[0].text().contains("Find where secret_answer"), "the child sees only its prompt");
    // Explore searches at low effort; the parent keeps its own (here, the model default).
    assert_eq!(child[0].output_config.as_ref().map(|c| c["effort"].clone()), Some(json!("low")));
    assert!(parent[0].output_config.is_none(), "{:?}", parent[0].output_config);
    assert_eq!(results(&e)[0].0, "secret_answer is defined in lib.rs:1 and returns 42.");
    assert!(
        !format!("{:?}", parent[1].messages).contains("output_mode"),
        "child tool calls stay out of the parent context"
    );
    // Budgets include sub-agents: four mock calls of 100 input tokens each.
    assert_eq!(e.state.model_usage["claude-opus-5-5"]["inputTokens"].as_u64(), Some(400));
    // The host sees the child's messages tagged with the Task call.
    let events = s.sink.take();
    assert!(events.iter().any(|ev| matches!(ev, EngineEvent::Assistant { parent_tool_use_id: Some(_), .. })));
}

#[tokio::test]
async fn parallel_agents_run_concurrently() {
    let s = setup(
        |req| {
            if is_child(req) {
                MockTurn::text("done").with_delay(Duration::from_millis(60))
            } else if req.messages.len() == 1 {
                MockTurn::tools(&[
                    ("Task", json!({"description": "a", "prompt": "task a"})),
                    ("Task", json!({"description": "b", "prompt": "task b"})),
                    ("Task", json!({"description": "c", "prompt": "task c"})),
                ])
            } else {
                MockTurn::text("all done")
            }
        },
        json!({}),
    );
    let mut e = s.engine;
    let start = Instant::now();
    let r = e.submit(MessageContent::Text("go".into())).await;
    assert_eq!(r.result.as_deref(), Some("all done"));
    // Each child streams 7 events 60 ms apart; three in sequence would take over 1.2 s.
    assert!(start.elapsed() < Duration::from_millis(1100), "{:?}", start.elapsed());
}

#[tokio::test]
async fn unknown_agent_is_rejected_and_subagent_stop_hook_runs() {
    let marker = tempfile::NamedTempFile::new().unwrap();
    let path = marker.path().display().to_string();
    let hooks = json!({"SubagentStop": [{"hooks": [{"type": "command", "command": format!("cat > '{path}'")}]}]});
    let s = setup(
        |req| {
            if is_child(req) {
                MockTurn::text("child finished")
            } else if req.messages.len() == 1 {
                MockTurn::tools(&[
                    ("Task", json!({"description": "x", "prompt": "p", "subagent_type": "Nonexistent"})),
                    ("Task", json!({"description": "y", "prompt": "p", "subagent_type": "Plan"})),
                ])
            } else {
                MockTurn::text("ok")
            }
        },
        hooks,
    );
    let mut e = s.engine;
    e.submit(MessageContent::Text("go".into())).await;
    let r = results(&e);
    assert!(r[0].1 && r[0].0.contains("Unknown subagent_type"), "{r:?}");
    assert_eq!(r[1], ("child finished".to_string(), false));
    let hook_input = std::fs::read_to_string(marker.path()).unwrap();
    assert!(hook_input.contains("\"hook_event_name\":\"SubagentStop\""), "{hook_input}");
}

#[tokio::test]
async fn child_edits_are_checkpointed_in_the_parent_turn() {
    let s = setup(
        |req| {
            if is_child(req) {
                let dir =
                    req.system[0].text.lines().find_map(|l| l.strip_prefix("Working directory: ")).unwrap().to_string();
                if req.messages.len() == 1 {
                    MockTurn::tool("Write", json!({"file_path": format!("{dir}/made_by_child.txt"), "content": "x"}))
                } else {
                    MockTurn::text("wrote it")
                }
            } else if req.messages.len() == 1 {
                MockTurn::tool("Task", json!({"description": "w", "prompt": "write a file"}))
            } else {
                MockTurn::text("ok")
            }
        },
        json!({}),
    );
    let mut e = s.engine;
    e.handle().set_permission_mode(PermissionMode::AcceptEdits);
    e.submit(MessageContent::Text("go".into())).await;
    let f = s.cwd.join("made_by_child.txt");
    assert!(f.exists(), "{:?}", results(&e));
    let turn = e.state.uuids[0].clone();
    e.history().rewind(&turn, false).unwrap();
    assert!(!f.exists(), "rewinding the parent turn removes the child's file");
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Two agents started together each rewrite the same file. With `isolation: "worktree"` each
/// works in its own copy: neither overwrites the other, the session's tree is untouched, and
/// each result says where the work is. An agent that changed nothing leaves no copy behind.
#[tokio::test]
async fn isolated_agents_work_in_their_own_worktrees() {
    let s = setup(
        |req| {
            if is_child(req) {
                let dir =
                    req.system[0].text.lines().find_map(|l| l.strip_prefix("Working directory: ")).unwrap().to_string();
                let task = req.messages[0].text();
                if task.contains("look only") {
                    MockTurn::text("looked")
                } else if req.messages.len() == 1 {
                    MockTurn::tool("Read", json!({"file_path": format!("{dir}/lib.rs")}))
                } else if req.messages.len() == 3 {
                    let who = if task.contains("alpha") { "alpha" } else { "beta" };
                    MockTurn::tool("Write", json!({"file_path": format!("{dir}/lib.rs"), "content": who}))
                } else {
                    MockTurn::text("rewrote lib.rs")
                }
            } else if req.messages.len() == 1 {
                MockTurn::tools(&[
                    ("Task", json!({"description": "a", "prompt": "rewrite as alpha", "isolation": "worktree"})),
                    ("Task", json!({"description": "b", "prompt": "rewrite as beta", "isolation": "worktree"})),
                    ("Task", json!({"description": "c", "prompt": "look only", "isolation": "worktree"})),
                ])
            } else {
                MockTurn::text("ok")
            }
        },
        json!({}),
    );
    git(&s.cwd, &["init", "-q", "-b", "main"]);
    git(&s.cwd, &["-c", "user.email=t@t", "-c", "user.name=t", "add", "-A"]);
    git(&s.cwd, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-q", "-m", "start"]);
    let before = std::fs::read_to_string(s.cwd.join("lib.rs")).unwrap();
    let mut e = s.engine;
    e.handle().set_permission_mode(PermissionMode::AcceptEdits);
    e.submit(MessageContent::Text("go".into())).await;

    assert_eq!(std::fs::read_to_string(s.cwd.join("lib.rs")).unwrap(), before, "the session's tree is untouched");
    let res = results(&e);
    assert_eq!(res.len(), 3, "{res:?}");
    let mut seen = vec![];
    for (text, is_error) in &res[..2] {
        assert!(!is_error && text.contains("worked in its own worktree"), "{text}");
        let path = text.split("worktree: ").nth(1).unwrap().split(" (branch ").next().unwrap();
        seen.push(std::fs::read_to_string(std::path::Path::new(path).join("lib.rs")).unwrap());
    }
    seen.sort();
    assert_eq!(seen, ["alpha", "beta"], "each agent's change is whole, in its own copy");
    assert_eq!(res[2].0, "looked", "nothing changed: nothing to say");
    let left = std::fs::read_dir(s.cwd.join(".kodact/worktrees")).unwrap().count();
    assert_eq!(left, 2, "the unchanged copy was removed");
    assert_eq!(git(&s.cwd, &["branch", "--list", "kodact/agent-*"]).lines().count(), 2);
    assert_eq!(git(&s.cwd, &["status", "--porcelain"]), "", "the copies don't show up as changes");
}

#[tokio::test]
async fn isolation_outside_a_repository_is_refused_with_the_way_out() {
    let s = setup(
        |req| {
            if req.messages.len() == 1 {
                MockTurn::tool("Task", json!({"description": "a", "prompt": "x", "isolation": "worktree"}))
            } else {
                MockTurn::text("ok")
            }
        },
        json!({}),
    );
    let mut e = s.engine;
    e.submit(MessageContent::Text("go".into())).await;
    let res = results(&e);
    assert!(res[0].1 && res[0].0.contains("needs a git repository"), "{res:?}");
}
