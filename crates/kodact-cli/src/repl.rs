//! A line-based interactive session (until the full-screen UI lands).

use std::io::Write;
use std::sync::Arc;

use kodact_engine::{EngineEvent, EventSink, NoticeLevel, PermissionAnswer, PermissionPrompt, PermissionPrompter};
use kodact_types::{ContentBlock, Delta, MessageContent, StreamEvent};
use tokio::sync::{mpsc, Mutex};

use crate::args::Opts;

type Lines = Arc<Mutex<mpsc::UnboundedReceiver<String>>>;

fn spawn_stdin() -> Lines {
    let (tx, rx) = mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        let mut line = String::new();
        loop {
            line.clear();
            match stdin.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {
                    if tx.send(line.trim_end_matches(['\n', '\r']).to_string()).is_err() {
                        break;
                    }
                }
            }
        }
    });
    Arc::new(Mutex::new(rx))
}

struct LinePrompter {
    lines: Lines,
}

#[async_trait::async_trait]
impl PermissionPrompter for LinePrompter {
    async fn ask(&self, p: PermissionPrompt) -> PermissionAnswer {
        match p.tool_name.as_str() {
            "AskUserQuestion" => return self.questions(&p).await,
            "ExitPlanMode" => {
                eprintln!("\n{}\n{}", crate::term::bold("Plan:"), p.input["plan"].as_str().unwrap_or(""));
                eprint!("\n  Approve this plan? [y]es / yes, and [a]ccept edits / [n]o, keep planning: ");
                let _ = std::io::stderr().flush();
                let answer = self.lines.lock().await.recv().await.unwrap_or_default();
                return match answer.trim().to_ascii_lowercase().as_str() {
                    "y" | "yes" => PermissionAnswer::Allow { updated_input: None, updated_permissions: vec![] },
                    "a" => PermissionAnswer::Allow {
                        updated_input: None,
                        updated_permissions: vec![
                            serde_json::json!({"type": "setMode", "mode": "acceptEdits", "destination": "session"}),
                        ],
                    },
                    _ => PermissionAnswer::Deny {
                        message: format!(
                            "The user wants to keep planning.{}",
                            if answer.trim().len() > 1 {
                                format!(" They said: {}", answer.trim())
                            } else {
                                String::new()
                            }
                        ),
                        interrupt: false,
                    },
                };
            }
            _ => {}
        }
        let summary = summarize_input(&p.tool_name, &p.input);
        eprint!("\n  Allow {}{}? [y]es / [a]lways / [n]o: ", p.tool_name, summary);
        let _ = std::io::stderr().flush();
        let answer = self.lines.lock().await.recv().await.unwrap_or_default();
        match answer.trim().to_ascii_lowercase().as_str() {
            "y" | "yes" => PermissionAnswer::Allow { updated_input: None, updated_permissions: vec![] },
            "a" | "always" => PermissionAnswer::Allow {
                updated_input: None,
                updated_permissions: p.suggestions.iter().filter_map(|s| serde_json::to_value(s).ok()).collect(),
            },
            _ => PermissionAnswer::Deny { message: String::new(), interrupt: false },
        }
    }
}

impl LinePrompter {
    /// AskUserQuestion: number the options; an answer is option numbers or free text.
    async fn questions(&self, p: &PermissionPrompt) -> PermissionAnswer {
        let mut input = p.input.clone();
        let mut answers = serde_json::Map::new();
        for q in input["questions"].as_array().cloned().unwrap_or_default() {
            let question = q["question"].as_str().unwrap_or("").to_string();
            let options: Vec<String> = q["options"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|o| o["label"].as_str().map(str::to_string))
                .collect();
            eprintln!("\n{}", crate::term::bold(&question));
            for (i, o) in q["options"].as_array().into_iter().flatten().enumerate() {
                eprintln!(
                    "  {}. {} {}",
                    i + 1,
                    o["label"].as_str().unwrap_or(""),
                    crate::term::dim(o["description"].as_str().unwrap_or(""))
                );
            }
            let multi = q["multiSelect"].as_bool().unwrap_or(false);
            eprint!(
                "  {} ",
                if multi { "Numbers (e.g. 1,3) or your own answer:" } else { "Number or your own answer:" }
            );
            let _ = std::io::stderr().flush();
            let line = self.lines.lock().await.recv().await.unwrap_or_default();
            let picked: Option<Vec<String>> = line
                .split(',')
                .map(|t| t.trim().parse::<usize>().ok().and_then(|n| options.get(n.wrapping_sub(1)).cloned()))
                .collect();
            let answer = match picked {
                Some(p) if !p.is_empty() && (multi || p.len() == 1) => p.join(", "),
                _ => line.trim().to_string(),
            };
            if answer.is_empty() {
                return PermissionAnswer::Deny { message: "The user did not answer.".into(), interrupt: false };
            }
            answers.insert(question, serde_json::json!(answer));
        }
        input["answers"] = serde_json::Value::Object(answers);
        PermissionAnswer::Allow { updated_input: Some(input), updated_permissions: vec![] }
    }
}

fn summarize_input(tool: &str, input: &serde_json::Value) -> String {
    let key = match tool {
        "Bash" => "command",
        "Read" | "Write" | "Edit" | "MultiEdit" => "file_path",
        "Glob" | "Grep" => "pattern",
        "WebFetch" => "url",
        "WebSearch" => "query",
        _ => "",
    };
    match input.get(key).and_then(|v| v.as_str()) {
        Some(v) => format!("({})", v.chars().take(120).collect::<String>()),
        None => String::new(),
    }
}

struct PrintSink;

impl EventSink for PrintSink {
    fn emit(&self, event: EngineEvent) {
        let mut so = std::io::stdout();
        match event {
            EngineEvent::Stream {
                event: StreamEvent::ContentBlockDelta { delta: Delta::TextDelta { text }, .. },
                ..
            } => {
                let _ = write!(so, "{text}");
                let _ = so.flush();
            }
            EngineEvent::Stream { event: StreamEvent::MessageStop, .. } => {
                let _ = writeln!(so);
            }
            EngineEvent::Assistant { message, .. } => {
                for b in &message.content {
                    if let ContentBlock::ToolUse { name, input, .. } = b {
                        let _ = writeln!(
                            so,
                            "{} {}{}",
                            crate::term::dim(kodact_core::glyphs::TOOL),
                            crate::term::bold(name),
                            summarize_input(name, input)
                        );
                    }
                }
            }
            EngineEvent::User { message, is_meta: false, .. } => {
                for b in &message.content {
                    if let ContentBlock::ToolResult { content, is_error, .. } = b {
                        let text = content.to_text();
                        let first = text.lines().next().unwrap_or("").chars().take(160).collect::<String>();
                        let more = text.lines().count().saturating_sub(1);
                        let more = if more > 0 { format!(" (+{more} lines)") } else { String::new() };
                        let line = format!("  {} {first}{more}", kodact_core::glyphs::RESULT);
                        let styled =
                            if is_error == &Some(true) { crate::term::red(&line) } else { crate::term::dim(&line) };
                        let _ = writeln!(so, "{styled}");
                    }
                }
            }
            EngineEvent::Notice { level, text } => {
                let tag = match level {
                    NoticeLevel::Info => "",
                    NoticeLevel::Warning => "warning: ",
                    NoticeLevel::Error => "error: ",
                };
                eprintln!("{tag}{text}");
            }
            _ => {}
        }
    }
}

pub async fn run(prompt: Option<String>, o: Opts) -> Result<i32, crate::exit::Fail> {
    let lines = spawn_stdin();
    let mut lo = crate::launch_options(&o)?;
    let (mcp, mcp_warnings) = kodact_core::connect_mcp(&lo).await;
    lo.mcp = Some(mcp.clone());
    let sink: Arc<dyn kodact_engine::EventSink> = Arc::new(PrintSink);
    let prompter: Arc<dyn kodact_engine::PermissionPrompter> = Arc::new(LinePrompter { lines: lines.clone() });
    let rebuild = (lo.clone(), sink.clone(), prompter.clone());
    let mut session = kodact_core::build_session(lo, sink, prompter).map_err(crate::exit::Fail::from)?;
    session.learn_window().await;
    for w in session.warnings.iter().chain(&mcp_warnings) {
        eprintln!("kodact: {w}");
    }
    eprintln!(
        "KodAct {} · {} · {}  (Ctrl-C interrupts a turn; /exit quits)",
        kodact_core::VERSION,
        session.init.model,
        session.init.cwd
    );
    let mut driver = kodact_core::Driver::new(session, kodact_core::commands::Surface::Repl, Some(mcp.clone()));
    driver.set_rebuild(rebuild.0, rebuild.1, rebuild.2);
    let handle = driver.live();
    // Ctrl-C interrupts the running turn instead of killing the process.
    let h2 = handle.clone();
    tokio::spawn(async move {
        while tokio::signal::ctrl_c().await.is_ok() {
            h2.handle().interrupt();
        }
    });
    let mut report = |r: &kodact_engine::TurnResult| {
        if let Some(b) = &r.prompt_blocked {
            eprintln!("{b}");
        } else if r.num_turns == 0 && r.stop_reason.is_none() {
            // Answered locally (a slash command): show it, since nothing streamed.
            let text = r.result.clone().unwrap_or_default();
            if r.is_error {
                eprintln!("{}", crate::term::red(&text));
            } else {
                outln!("{text}");
            }
        }
    };
    let mut next = prompt;
    // Changes each time a subtask finishes (C20).
    let mut finished = driver.subtasks.watch();
    'session: loop {
        let text = match next.take() {
            Some(t) => t,
            None => {
                // Subtasks that finished during the last turn are reported before the prompt.
                let _ = *finished.borrow_and_update();
                driver.deliver_subtasks();
                eprint!("\n> ");
                let _ = std::io::stderr().flush();
                // Scheduled tasks (/loop, CronCreate) run while the prompt waits. Waits are
                // short and re-checked, so the prompt is drawn again only when a task runs.
                loop {
                    let wait = driver.next_wait();
                    let far = std::time::Duration::from_secs(365 * 86_400);
                    let mut rx = lines.lock().await;
                    tokio::select! {
                        l = rx.recv() => match l {
                            Some(l) => break l,
                            None => break 'session,
                        },
                        _ = tokio::time::sleep(wait.unwrap_or(far)), if wait.is_some() => {
                            drop(rx);
                            if driver.task_due() {
                                eprintln!();
                                driver.run_due(&mut report).await;
                                continue 'session;
                            }
                        }
                        // One finished while the prompt waited: the next pass reports it and asks again.
                        _ = finished.changed() => {
                            drop(rx);
                            eprintln!();
                            continue 'session;
                        }
                    }
                }
            }
        };
        let t = text.trim();
        if t.is_empty() {
            continue;
        }
        if driver.input(MessageContent::Text(t.to_string()), &mut report).await == kodact_core::Flow::Exit {
            break;
        }
    }
    driver.shutdown("prompt_input_exit").await;
    Ok(0)
}
