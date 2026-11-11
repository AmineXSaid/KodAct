//! Connects every configured server at startup, exposes their tools, and
//! reconnects, enables or disables a server while a session runs (`/mcp`,
//! and the stream-json `mcp_reconnect` / `mcp_toggle` requests).
//!
//! Each server's changing state sits behind its own lock, shared with the
//! tool objects made from it, so a change reaches the session and its
//! sub-agents at once. A tool name a server didn't list when the session was
//! built has no tool object until the session is rebuilt (`/reload-plugins`,
//! `/clear`, a new run): the engine's tool registry is fixed per session.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use kodact_tools::Tool;
use serde_json::{json, Value};

use crate::client::{ConnectOptions, McpClient, ToolInfo};
use crate::config::{self, NamedServer, Resolved, Scope};
use crate::tools::{tool_name, ListResources, McpTool, ReadResource};

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Connected,
    Failed(String),
    /// Not running: a `.mcp.json` server the user has not approved, or one the
    /// user turned off ([`config::DISABLED`]).
    Skipped(String),
    /// Starting again (`/mcp reconnect`, `/mcp enable`).
    Pending,
}

impl Status {
    /// The SDK status word: `connected`, `failed`, `needs-auth`, `pending`, or `disabled`.
    pub fn as_str(&self) -> &'static str {
        match self {
            Status::Connected => "connected",
            Status::Failed(_) => "failed",
            Status::Skipped(_) => "disabled",
            Status::Pending => "pending",
        }
    }
}

/// What `/mcp reconnect|enable|disable` and `mcp_reconnect` / `mcp_toggle` do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerAction {
    /// Stop the server and start it again from the same config.
    Reconnect,
    /// Start a disabled server, and take it off the disabled list.
    Enable,
    /// Stop it, hide its tools and prompts, and put it on the disabled list.
    Disable,
}

impl ServerAction {
    pub fn parse(word: &str) -> Option<ServerAction> {
        match word {
            "reconnect" => Some(ServerAction::Reconnect),
            "enable" => Some(ServerAction::Enable),
            "disable" => Some(ServerAction::Disable),
            _ => None,
        }
    }
}

/// What one action did to one server.
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub server: String,
    /// The status afterwards.
    pub status: Status,
    /// It already was as asked (enabling an enabled server, disabling a disabled one).
    pub unchanged: bool,
    /// The settings file holding the choice; `None` for `--mcp-config` and
    /// plugin servers, which are chosen per run, and for a reconnect.
    pub saved: Option<PathBuf>,
    /// While connected: how many tools and prompts it offers.
    pub tools: usize,
    pub prompts: usize,
    /// Tools it offers that the session has no tool object for yet (they join
    /// when the session is rebuilt).
    pub new_tools: Vec<String>,
    /// Tools the session offered that the server no longer has (hidden now).
    pub gone_tools: Vec<String>,
}

/// One server's changing state. Its lock is never held across `.await`.
#[derive(Clone)]
struct ServerState {
    status: Status,
    client: Option<Arc<McpClient>>,
    /// The last lists the server gave; kept while it is off, so its tool objects survive.
    tools: Vec<ToolInfo>,
    prompts: Vec<Value>,
    /// Full names of the tool objects the latest [`McpManager::tools`] made.
    offered: Vec<String>,
}

impl ServerState {
    fn off(status: Status) -> Self {
        ServerState { status, client: None, tools: vec![], prompts: vec![], offered: vec![] }
    }

    fn connected(client: McpClient, tools: Vec<ToolInfo>, prompts: Vec<Value>) -> Self {
        ServerState { status: Status::Connected, client: Some(Arc::new(client)), tools, prompts, offered: vec![] }
    }
}

pub struct ServerEntry {
    pub name: String,
    pub scope: &'static str,
    pub transport: &'static str,
    /// What it starts from (`${VAR}` not expanded yet); `None` for a
    /// `.mcp.json` server that still needs the user's approval.
    pub config: Option<NamedServer>,
    state: RwLock<ServerState>,
    /// One reconnect, enable or disable at a time for this server.
    op: tokio::sync::Mutex<()>,
}

impl ServerEntry {
    fn new(
        name: String,
        scope: &'static str,
        transport: &'static str,
        config: Option<NamedServer>,
        state: ServerState,
    ) -> Self {
        ServerEntry { name, scope, transport, config, state: RwLock::new(state), op: tokio::sync::Mutex::new(()) }
    }

    fn snapshot(&self) -> ServerState {
        self.state.read().unwrap().clone()
    }

    pub fn status(&self) -> Status {
        self.state.read().unwrap().status.clone()
    }

    /// The server went away under a call (its process died, the connection dropped): it
    /// is no longer connected, and everything that reads the status says so. Before, it
    /// stayed "connected" in `/mcp` while every call to it failed. `/mcp reconnect`
    /// starts it again from its config.
    pub fn mark_lost(&self, why: &str) {
        let mut s = self.state.write().unwrap();
        if s.status == Status::Connected {
            s.status = Status::Failed(why.to_string());
            s.client = None;
        }
    }

    /// The client, while connected.
    pub fn client(&self) -> Option<Arc<McpClient>> {
        let s = self.state.read().unwrap();
        if s.status == Status::Connected {
            s.client.clone()
        } else {
            None
        }
    }

    /// The server's last tool list (kept while it is off).
    pub fn tools(&self) -> Vec<ToolInfo> {
        self.state.read().unwrap().tools.clone()
    }

    pub fn prompts(&self) -> Vec<Value> {
        self.state.read().unwrap().prompts.clone()
    }

    /// Off: disabled, or waiting for approval.
    pub fn is_off(&self) -> bool {
        matches!(self.state.read().unwrap().status, Status::Skipped(_))
    }

    /// `tool` as the server lists it now; `None` (hidden) while the server
    /// isn't connected or no longer has it.
    pub fn tool(&self, tool: &str) -> Option<ToolInfo> {
        let s = self.state.read().unwrap();
        if s.status != Status::Connected {
            return None;
        }
        s.tools.iter().find(|t| t.name == tool).cloned()
    }
}

#[derive(Default)]
pub struct McpManager {
    pub servers: Vec<Arc<ServerEntry>>,
    /// How the servers were connected; restarts use the same.
    opts: Option<ConnectOptions>,
    /// Set by [`McpManager::shutdown`]: a restart that ends later stops its server at once.
    closed: AtomicBool,
}

/// Start one server: connect, then read its tools and prompts.
async fn start(s: &NamedServer, opts: &ConnectOptions) -> Result<(McpClient, Vec<ToolInfo>, Vec<Value>), String> {
    let env = |k: &str| std::env::var(k).ok();
    let config = s.config.expanded(&env)?;
    let c = McpClient::connect(&s.name, &config, opts).await.map_err(|e| e.to_string())?;
    match c.list_tools().await {
        Ok(tools) => {
            let prompts = c.list_prompts().await.unwrap_or_default();
            Ok((c, tools, prompts))
        }
        Err(e) => {
            c.close().await;
            Err(format!("tools/list: {e}"))
        }
    }
}

/// Record an enable or disable where the next session's resolver reads it:
/// `disabledMcpjsonServers` in the launch project's local settings.
/// `--mcp-config` and plugin servers are chosen per run: nothing is saved.
fn save(s: &NamedServer, opts: &ConnectOptions, disable: bool) -> std::io::Result<Option<PathBuf>> {
    if s.scope == Scope::Flag {
        return Ok(None);
    }
    let file = kodact_config::project_dir(&opts.cwd).join("settings.local.json");
    config::set_disabled(&file, &s.name, disable)?;
    Ok(Some(file))
}

/// What `s` looks like after an action.
fn outcome(s: &ServerEntry, unchanged: bool, saved: Option<PathBuf>) -> Outcome {
    let st = s.snapshot();
    let on = st.status == Status::Connected;
    let names: Vec<String> = if on { st.tools.iter().map(|t| tool_name(&s.name, &t.name)).collect() } else { vec![] };
    Outcome {
        server: s.name.clone(),
        unchanged,
        saved,
        tools: names.len(),
        prompts: if on { st.prompts.len() } else { 0 },
        new_tools: names.iter().filter(|n| !st.offered.contains(n)).cloned().collect(),
        gone_tools: if on { st.offered.iter().filter(|n| !names.contains(n)).cloned().collect() } else { vec![] },
        status: st.status,
    }
}

impl McpManager {
    /// Connect to every server at once; a failing server is reported, not fatal.
    pub async fn connect(resolved: &Resolved, opts: &ConnectOptions) -> Self {
        let futs = resolved.servers.iter().map(|s| async move {
            let state = match start(s, opts).await {
                Ok((c, tools, prompts)) => ServerState::connected(c, tools, prompts),
                Err(e) => ServerState::off(Status::Failed(e)),
            };
            ServerEntry::new(s.name.clone(), s.scope.as_str(), s.config.transport(), Some(s.clone()), state)
        });
        let mut servers: Vec<Arc<ServerEntry>> =
            futures::future::join_all(futs).await.into_iter().map(Arc::new).collect();
        for s in &resolved.skipped {
            let (scope, transport) = match &s.config {
                Some(c) => (c.scope.as_str(), c.config.transport()),
                None => ("project", ""),
            };
            let state = ServerState::off(Status::Skipped(s.reason.clone()));
            servers.push(Arc::new(ServerEntry::new(s.name.clone(), scope, transport, s.config.clone(), state)));
        }
        McpManager { servers, opts: Some(opts.clone()), closed: AtomicBool::new(false) }
    }

    /// The connected servers' clients.
    pub fn clients(&self) -> Vec<Arc<McpClient>> {
        self.servers.iter().filter_map(|s| s.client()).collect()
    }

    /// A tool object for every tool a server has listed (hidden while it is
    /// off), plus the resource tools (hidden while no connected server offers
    /// resources).
    pub fn tools(&self) -> Vec<Arc<dyn Tool>> {
        let mut out: Vec<Arc<dyn Tool>> = vec![];
        for s in &self.servers {
            let list = {
                let mut st = s.state.write().unwrap();
                st.offered = st.tools.iter().map(|t| tool_name(&s.name, &t.name)).collect();
                st.tools.clone()
            };
            out.extend(list.into_iter().map(|t| Arc::new(McpTool::new(s.clone(), t)) as Arc<dyn Tool>));
        }
        // Servers that could offer resources: any with a config (a disabled one may come back).
        let servers: Vec<Arc<ServerEntry>> = self.servers.iter().filter(|s| s.config.is_some()).cloned().collect();
        if !servers.is_empty() {
            out.push(Arc::new(ListResources { servers: servers.clone() }));
            out.push(Arc::new(ReadResource { servers }));
        }
        out
    }

    /// Slash-command names of every connected server's prompts: `mcp__<server>__<prompt>`.
    pub fn prompt_names(&self) -> Vec<String> {
        self.servers
            .iter()
            .filter(|s| s.status() == Status::Connected)
            .flat_map(|s| {
                s.prompts()
                    .iter()
                    .filter_map(|p| p.get("name").and_then(Value::as_str))
                    .map(|p| tool_name(&s.name, p))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Run `/mcp__<server>__<prompt> args`: the prompt's messages as one text. Arguments
    /// fill the prompt's declared arguments in order; the last one takes the rest.
    pub async fn get_prompt(&self, command: &str, args: &str) -> Option<Result<String, String>> {
        for s in &self.servers {
            let Some(c) = s.client() else { continue };
            for p in &s.prompts() {
                let Some(name) = p.get("name").and_then(Value::as_str) else { continue };
                if crate::tools::tool_name(&s.name, name) != command {
                    continue;
                }
                let declared: Vec<String> = p
                    .get("arguments")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter().filter_map(|x| x.get("name").and_then(Value::as_str).map(str::to_string)).collect()
                    })
                    .unwrap_or_default();
                let words: Vec<&str> = args.split_whitespace().collect();
                let mut map = serde_json::Map::new();
                for (i, d) in declared.iter().enumerate() {
                    let v = if i + 1 == declared.len() {
                        words.get(i..).map(|w| w.join(" "))
                    } else {
                        words.get(i).map(|w| w.to_string())
                    };
                    if let Some(v) = v.filter(|v| !v.is_empty()) {
                        map.insert(d.clone(), json!(v));
                    }
                }
                let r = c.get_prompt(name, Value::Object(map)).await.map_err(|e| e.to_string()).map(|v| {
                    v.get("messages")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|m| {
                            crate::tools::convert_content(
                                &json!({"content": [m.get("content").cloned().unwrap_or(Value::Null)]}),
                            )
                            .text_content()
                        })
                        .collect::<Vec<_>>()
                        .join("\n\n")
                });
                return Some(r);
            }
        }
        None
    }

    /// `[{name, status}]` for `system/init` and `mcp_status`.
    pub fn status(&self) -> Vec<(String, String)> {
        self.servers.iter().map(|s| (s.name.clone(), s.status().as_str().to_string())).collect()
    }

    /// The `mcp_status` control response's `mcpServers` list.
    pub fn status_json(&self) -> Value {
        json!(self
            .servers
            .iter()
            .map(|s| {
                let st = s.snapshot();
                let mut v = json!({"name": s.name, "status": st.status.as_str(), "scope": s.scope});
                if let (Status::Connected, Some(c)) = (&st.status, &st.client) {
                    v["serverInfo"] = c.server_info.clone();
                    v["tools"] = json!(st.tools.iter().map(|t| json!({"name": t.name})).collect::<Vec<_>>());
                }
                if let Status::Failed(e) | Status::Skipped(e) = &st.status {
                    v["error"] = json!(e);
                }
                v
            })
            .collect::<Vec<_>>())
    }

    /// Server instructions for the system prompt, from connected servers only.
    pub fn instructions(&self) -> Option<String> {
        let parts: Vec<String> = self
            .servers
            .iter()
            .filter_map(|s| {
                let c = s.client()?;
                let text = c.instructions.as_ref()?.trim().to_string();
                (!text.is_empty()).then(|| format!("## {}\n{}", s.name, text.chars().take(4000).collect::<String>()))
            })
            .collect();
        (!parts.is_empty()).then(|| format!("# MCP server instructions\n\n{}", parts.join("\n\n")))
    }

    /// Warnings for the person: servers that failed or were skipped.
    pub fn warnings(&self) -> Vec<String> {
        self.servers
            .iter()
            .filter_map(|s| match s.status() {
                Status::Failed(e) => Some(format!("MCP server {} failed to start: {e}", s.name)),
                Status::Skipped(r) if r != config::DISABLED => Some(format!("MCP server {} (.mcp.json) {r}", s.name)),
                _ => None,
            })
            .collect()
    }

    /// The servers `target` names: one by name, or all of them for `all`.
    fn find(&self, target: &str) -> Result<Vec<Arc<ServerEntry>>, String> {
        if target == "all" {
            return Ok(self.servers.clone());
        }
        if let Some(s) = self.servers.iter().find(|s| s.name == target) {
            return Ok(vec![s.clone()]);
        }
        let names: Vec<&str> = self.servers.iter().map(|s| s.name.as_str()).collect();
        Err(if names.is_empty() {
            format!("No MCP server named {target:?}: none are configured.")
        } else {
            format!("No MCP server named {target:?}. Servers: {}.", names.join(", "))
        })
    }

    /// Run `action` on `target` (a server's name, or `all`), servers at once.
    /// `Err` when `target` names no server; otherwise one result per server,
    /// where `Err` means nothing changed for it. `all` leaves out servers
    /// waiting for approval, and disabled ones for a reconnect.
    pub async fn apply(&self, action: ServerAction, target: &str) -> Result<Vec<Result<Outcome, String>>, String> {
        let all = target == "all";
        let servers: Vec<Arc<ServerEntry>> = self
            .find(target)?
            .into_iter()
            .filter(|s| !all || (s.config.is_some() && !(action == ServerAction::Reconnect && s.is_off())))
            .collect();
        Ok(futures::future::join_all(servers.iter().map(|s| self.apply_one(action, s))).await)
    }

    async fn apply_one(&self, action: ServerAction, s: &ServerEntry) -> Result<Outcome, String> {
        let _one_at_a_time = s.op.lock().await;
        let Some(named) = &s.config else {
            return Err(format!(
                "MCP server {0} (.mcp.json) needs approval: run `kodact mcp approve {0}`, then start a new session.",
                s.name
            ));
        };
        let Some(opts) = &self.opts else { return Err("MCP servers can't be restarted here.".into()) };
        if self.closed.load(Ordering::SeqCst) {
            return Err("The session is ending.".into());
        }
        let off = s.is_off();
        match action {
            ServerAction::Enable if !off => return Ok(outcome(s, true, None)),
            ServerAction::Disable if off => return Ok(outcome(s, true, None)),
            ServerAction::Reconnect if off => {
                return Err(format!("MCP server {} is disabled; enable it first.", s.name))
            }
            _ => {}
        }
        // Saved first: when the choice can't be recorded, nothing changes.
        let saved = match action {
            ServerAction::Reconnect => None,
            _ => save(named, opts, action == ServerAction::Disable)
                .map_err(|e| format!("Could not save the change for {}: {e}", s.name))?,
        };
        // The old process stops before a new one starts: a server may hold a lock, a port or a database.
        let old = {
            let mut st = s.state.write().unwrap();
            st.status = if action == ServerAction::Disable {
                Status::Skipped(config::DISABLED.into())
            } else {
                Status::Pending
            };
            st.client.take()
        };
        if let Some(c) = old {
            c.close().await;
        }
        if action != ServerAction::Disable {
            let started = start(named, opts).await;
            if self.closed.load(Ordering::SeqCst) {
                if let Ok((c, _, _)) = &started {
                    c.close().await;
                }
                return Err("The session is ending.".into());
            }
            let mut st = s.state.write().unwrap();
            match started {
                Ok((c, tools, prompts)) => {
                    st.status = Status::Connected;
                    st.client = Some(Arc::new(c));
                    st.tools = tools;
                    st.prompts = prompts;
                }
                Err(e) => st.status = Status::Failed(e),
            }
        }
        Ok(outcome(s, false, saved))
    }

    pub async fn shutdown(&self) {
        self.closed.store(true, Ordering::SeqCst);
        for s in &self.servers {
            let client = s.state.write().unwrap().client.take();
            if let Some(c) = client {
                c.close().await;
            }
        }
    }
}
