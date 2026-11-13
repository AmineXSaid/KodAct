//! The stream-json host protocol: user messages and control requests on
//! stdin, conversation and control traffic on stdout.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use kodact_engine::{PermissionAnswer, PermissionPrompt, PermissionPrompter};
use kodact_mcp::{Outcome, ServerAction, Status};
use kodact_permissions::PermissionMode;
use kodact_types::sdk::{ControlRequest, ControlRequestBody, PermissionResult, SdkMessage};
use kodact_types::MessageContent;
use serde_json::{json, Value};
use tokio::io::AsyncBufReadExt;
use tokio::sync::{mpsc, oneshot};

use crate::output::Out;

type Pending = Arc<Mutex<Waiters>>;

/// Permission requests waiting for the host's answer.
#[derive(Default)]
pub struct Waiters {
    map: HashMap<String, oneshot::Sender<Result<Value, String>>>,
    /// The host's input has ended: nothing can answer a new request.
    closed: bool,
}

impl Waiters {
    fn remove(&mut self, id: &str) -> Option<oneshot::Sender<Result<Value, String>>> {
        self.map.remove(id)
    }

    /// End of input: drop the waiters (their requests are denied) and refuse new ones.
    fn close(&mut self) {
        self.closed = true;
        self.map.clear();
    }
}

/// Sends `can_use_tool` requests to the host and waits for its answer (contract C1).
pub struct HostPrompter {
    pub out: Arc<Out>,
    pub pending: Pending,
    counter: AtomicU64,
}

impl HostPrompter {
    pub fn new(out: Arc<Out>, pending: Pending) -> Self {
        HostPrompter { out, pending, counter: AtomicU64::new(0) }
    }
}

#[async_trait::async_trait]
impl PermissionPrompter for HostPrompter {
    async fn ask(&self, p: PermissionPrompt) -> PermissionAnswer {
        let id = format!("kodact-req-{}", self.counter.fetch_add(1, Ordering::SeqCst) + 1);
        let (tx, rx) = oneshot::channel();
        {
            let mut w = self.pending.lock().unwrap();
            if w.closed {
                // A scheduled turn after stdin ended: nobody is left to answer.
                return PermissionAnswer::Deny {
                    message: "The host's input has ended, so nobody can approve this; it was not run.".into(),
                    interrupt: false,
                };
            }
            w.map.insert(id.clone(), tx);
        }
        let mut body = json!({
            "tool_name": p.tool_name,
            "input": p.input,
            "tool_use_id": p.tool_use_id,
            "permission_suggestions": p.suggestions,
            "decision_reason": p.reason,
        });
        if let Some(b) = &p.blocked_path {
            body["blocked_path"] = json!(b);
        }
        self.out.line(&SdkMessage::ControlRequest(ControlRequest {
            request_id: id.clone(),
            request: ControlRequestBody::new("can_use_tool", body),
        }));
        let deny = |m: String| PermissionAnswer::Deny { message: m, interrupt: false };
        match rx.await {
            Ok(Ok(v)) => match serde_json::from_value::<PermissionResult>(v) {
                Ok(PermissionResult::Allow { updated_input, updated_permissions }) => PermissionAnswer::Allow {
                    updated_input,
                    updated_permissions: updated_permissions.unwrap_or_default(),
                },
                Ok(PermissionResult::Deny { message, interrupt }) => {
                    PermissionAnswer::Deny { message, interrupt: interrupt.unwrap_or(false) }
                }
                Err(e) => deny(format!("The host sent an invalid permission answer ({e}); the call was not run.")),
            },
            Ok(Err(e)) => deny(format!("The host could not answer the permission request: {e}")),
            Err(_) => deny("The permission request was cancelled.".into()),
        }
    }
}

/// What the reader forwards to the turn loop.
pub enum Input {
    User(MessageContent),
    /// `initialize` carried prompt overrides to apply before the first turn.
    SystemPrompt {
        replace: Option<String>,
        append: Option<String>,
    },
    /// An MCP server was reconnected, enabled or disabled: refresh what the session derives from them.
    McpChanged,
    Eof,
}

/// Everything the reader needs to answer control requests while a turn runs.
pub struct ControlContext {
    pub out: Arc<Out>,
    pub pending: Pending,
    /// The current session (it changes with /clear, /resume, /branch, /cd).
    pub live: kodact_core::driver::Live,
    pub mcp: Option<Arc<kodact_mcp::McpManager>>,
    pub init_response: Value,
    /// `set_permission_mode` may choose `bypassPermissions` (`Driver::bypass_allowed`).
    pub bypass_allowed: bool,
    /// `mcp_reconnect` and `mcp_toggle` still running, and immediate commands; awaited before exit.
    pub tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// What immediate commands read (C17): they are answered here, beside the turn.
    pub view: kodact_core::view::SessionView,
}

impl ControlContext {
    /// Wait for MCP restarts still running, so their answers are written before exit.
    pub async fn finish_tasks(&self) {
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
        for t in tasks {
            let _ = t.await;
        }
    }

    fn answer(&self, id: &str, r: Result<Option<Value>, String>) {
        let msg = match r {
            Ok(v) => SdkMessage::success(id, v),
            Err(e) => SdkMessage::error(id, e),
        };
        self.out.line(&msg);
    }

    /// Answer an immediate command (`/status`, `/usage`, ...) at once, from the session view,
    /// with a `result` line marked `"immediate": true`. It never reaches the turn loop, so a
    /// turn in progress goes on; its own result comes later, without the mark.
    fn answer_immediate(&self, text: String) {
        let (out, view, live) = (self.out.clone(), self.view.clone(), self.live.clone());
        let task = tokio::spawn(async move {
            let cancel = tokio_util::sync::CancellationToken::new();
            let Some(kodact_core::commands::Exec::Local { text, is_error }) =
                kodact_core::commands::execute_immediate(&view, &text, &cancel).await
            else {
                return;
            };
            let mut msg = crate::output::result_message(&view.local_result(text, is_error), &live.session_id());
            msg.immediate = Some(true);
            out.line(&SdkMessage::Result(Box::new(msg)));
        });
        self.tasks.lock().unwrap().push(task);
    }

    /// Handle one control request from the host.
    pub fn handle_request(&self, req: &ControlRequest, tx: &mpsc::UnboundedSender<Input>) {
        let b = &req.request;
        let id = req.request_id.as_str();
        match b.subtype.as_str() {
            "initialize" => {
                let replace = b.get_str("systemPrompt").map(str::to_string);
                let append = b.get_str("appendSystemPrompt").map(str::to_string);
                if replace.is_some() || append.is_some() {
                    let _ = tx.send(Input::SystemPrompt { replace, append });
                }
                self.answer(id, Ok(Some(self.init_response.clone())));
            }
            "interrupt" => {
                self.live.handle().interrupt();
                self.answer(id, Ok(None));
            }
            // Stop one Task agent (`task_id`: its Task call's tool_use_id) or all of them
            // (`all: true`). The turn goes on; the parent model gets each agent's stopped result.
            "stop_task" => {
                let handle = self.live.handle();
                if b.data.get("all").and_then(Value::as_bool) == Some(true) {
                    return self.answer(id, Ok(Some(json!({"stopped": handle.stop_agents()}))));
                }
                match b.get_str("task_id") {
                    Some(task) if handle.stop_agent(task) => self.answer(id, Ok(Some(json!({"stopped": 1})))),
                    Some(task) => self.answer(id, Err(format!("No running agent {task}"))),
                    None => self.answer(id, Err("task_id or all: true is required".into())),
                }
            }
            "set_permission_mode" => match b.get_str("mode").and_then(PermissionMode::parse) {
                Some(PermissionMode::BypassPermissions) if !self.bypass_allowed => self.answer(
                    id,
                    Err("bypassPermissions needs --allow-dangerously-skip-permissions at launch, and managed \
                         settings must not disable it"
                        .into()),
                ),
                Some(m) => {
                    self.live.handle().set_permission_mode(m);
                    self.answer(id, Ok(None));
                }
                None => self.answer(id, Err(format!("invalid permission mode {:?}", b.data.get("mode")))),
            },
            // Take back a message sent for the turn's next step, if it hasn't reached the model.
            "cancel_queued" => {
                let cancelled = b.get_str("uuid").is_some_and(|u| self.live.handle().cancel_steer(u));
                self.answer(id, Ok(Some(json!({"cancelled": cancelled}))));
            }
            "set_model" => {
                self.live.handle().set_model(b.get_str("model").unwrap_or("default"));
                self.answer(id, Ok(None));
            }
            "set_max_thinking_tokens" => {
                let n = b.data.get("max_thinking_tokens").and_then(Value::as_u64).map(|n| n as u32);
                self.live.handle().set_max_thinking_tokens(n);
                self.answer(id, Ok(None));
            }
            "mcp_status" => {
                let servers = self.mcp.as_ref().map(|m| m.status_json()).unwrap_or_else(|| json!([]));
                self.answer(id, Ok(Some(json!({"mcpServers": servers}))))
            }
            // What fills the context window (`/context`'s numbers), from the
            // published view: answered at once, mid-turn too.
            "get_context_usage" => self.answer(id, Ok(Some(kodact_core::commands::context_usage(&self.view)))),
            "rewind_files" => {
                let Some(msg_id) = b.get_str("user_message_id") else {
                    return self.answer(id, Err("user_message_id is required".into()));
                };
                let dry = b.data.get("dry_run").and_then(Value::as_bool).unwrap_or(false);
                match self.live.history().rewind(msg_id, dry) {
                    Ok(plan) => {
                        let files: Vec<String> =
                            plan.restore.iter().chain(plan.delete.iter()).map(|p| p.display().to_string()).collect();
                        self.answer(id, Ok(Some(json!({"canRewind": true, "filesChanged": files}))))
                    }
                    Err(e) => self.answer(id, Ok(Some(json!({"canRewind": false, "error": e})))),
                }
            }
            "mcp_reconnect" | "mcp_toggle" => {
                let Some(m) = self.mcp.clone() else {
                    return self.answer(id, Err("no MCP servers are configured".into()));
                };
                let Some(server) = b.get_str("serverName").map(str::to_string) else {
                    return self.answer(id, Err("serverName is required".into()));
                };
                let action = match (b.subtype.as_str(), b.data.get("enabled").and_then(Value::as_bool)) {
                    ("mcp_reconnect", _) => ServerAction::Reconnect,
                    (_, Some(true)) => ServerAction::Enable,
                    (_, Some(false)) => ServerAction::Disable,
                    (_, None) => return self.answer(id, Err("enabled (true or false) is required".into())),
                };
                // Starting a server can take a while: answer from a task, so this reader
                // keeps handling interrupts and permission answers meanwhile.
                let (out, tx, id) = (self.out.clone(), tx.clone(), id.to_string());
                let task = tokio::spawn(async move {
                    let r = control_result(m.apply(action, &server).await);
                    let _ = tx.send(Input::McpChanged);
                    out.line(&match r {
                        Ok(()) => SdkMessage::success(&id, None),
                        Err(e) => SdkMessage::error(&id, e),
                    });
                });
                self.tasks.lock().unwrap().push(task);
            }
            other => self.answer(id, Err(format!("unsupported control request: {other}"))),
        }
    }
}

/// An MCP action's control answer: an error names every server that refused or failed.
fn control_result(r: Result<Vec<Result<Outcome, String>>, String>) -> Result<(), String> {
    let errors: Vec<String> = r?
        .into_iter()
        .filter_map(|o| match o {
            Ok(Outcome { status: Status::Failed(why), server, .. }) => {
                Some(format!("MCP server {server} failed to start: {why}"))
            }
            Ok(_) => None,
            Err(e) => Some(e),
        })
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("; "))
    }
}

/// Read stdin lines until EOF, answering control traffic inline.
pub async fn read_stdin(ctx: Arc<ControlContext>, tx: mpsc::UnboundedSender<Input>) {
    let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        if line.trim().is_empty() {
            continue;
        }
        let msg: SdkMessage = match serde_json::from_str(&line) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("kodact: ignoring invalid input line ({e}): {}", line.chars().take(200).collect::<String>());
                continue;
            }
        };
        match msg {
            SdkMessage::User(u) => {
                let cat = kodact_core::commands::Catalog::default();
                match kodact_core::commands::command_text(&u.message.content) {
                    Some(text) if kodact_core::commands::immediate(&text, &cat) => ctx.answer_immediate(text),
                    // Sent while a turn runs, for its next step (`priority: "next"`): the model reads
                    // it between tool calls. With no turn running, it is a turn of its own.
                    _ if u.priority.as_deref() == Some("next") && u.uuid.is_some() => {
                        let steer = kodact_engine::Steer {
                            uuid: u.uuid.clone().unwrap_or_default(),
                            content: u.message.content.clone(),
                        };
                        if !ctx.live.handle().steer(steer) {
                            let _ = tx.send(Input::User(u.message.content));
                        }
                    }
                    _ => {
                        let _ = tx.send(Input::User(u.message.content));
                    }
                }
            }
            SdkMessage::ControlRequest(req) => ctx.handle_request(&req, &tx),
            SdkMessage::ControlResponse(resp) => {
                let id = resp.response.request_id().to_string();
                if let Some(waiter) = ctx.pending.lock().unwrap().remove(&id) {
                    let r = match resp.response {
                        kodact_types::sdk::ControlResponseBody::Success { response, .. } => {
                            Ok(response.unwrap_or(Value::Null))
                        }
                        kodact_types::sdk::ControlResponseBody::Error { error, .. } => Err(error),
                    };
                    let _ = waiter.send(r);
                }
            }
            SdkMessage::ControlCancelRequest { request_id } => {
                ctx.pending.lock().unwrap().remove(&request_id);
            }
            _ => {}
        }
    }
    // Unanswered prompts can never be answered now, and new ones are refused.
    ctx.pending.lock().unwrap().close();
    let _ = tx.send(Input::Eof);
}
