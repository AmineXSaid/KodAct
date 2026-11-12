//! MCP tools and resources as KodAct tools.

use std::sync::Arc;

use kodact_permissions::Subject;
use kodact_tools::{Tool, ToolContext, ToolOutput, INTERRUPTED};
use kodact_types::{ContentBlock, MediaSource};
use serde_json::{json, Value};

use crate::client::{McpClient, ToolInfo};
use crate::manager::ServerEntry;

/// Tool names the Messages API accepts: `[A-Za-z0-9_-]{1,64}`.
const MAX_NAME: usize = 64;

fn clean(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect()
}

/// `mcp__<server>__<tool>`, cleaned and kept within the API's length limit.
pub fn tool_name(server: &str, tool: &str) -> String {
    let full = format!("mcp__{}__{}", clean(server), clean(tool));
    if full.len() <= MAX_NAME {
        return full;
    }
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    full.hash(&mut h);
    let suffix = format!("_{:08x}", h.finish() as u32);
    format!("{}{suffix}", &full[..MAX_NAME - suffix.len()])
}

/// Turn a `tools/call` (or prompt/resource) content list into tool output.
pub fn convert_content(result: &Value) -> ToolOutput {
    let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);
    let mut blocks: Vec<ContentBlock> = vec![];
    let mut text = String::new();
    let mut push_text = |t: &str, blocks: &mut Vec<ContentBlock>| {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(t);
        blocks.push(ContentBlock::text(t));
    };
    for item in result.get("content").and_then(Value::as_array).into_iter().flatten() {
        match item.get("type").and_then(Value::as_str) {
            Some("text") => push_text(item.get("text").and_then(Value::as_str).unwrap_or_default(), &mut blocks),
            Some("image") => {
                let mime = item.get("mimeType").and_then(Value::as_str).unwrap_or("image/png");
                match item.get("data").and_then(Value::as_str) {
                    Some(data) if mime.starts_with("image/") => blocks.push(ContentBlock::Image {
                        source: MediaSource::Base64 { media_type: mime.into(), data: data.into() },
                        cache_control: None,
                    }),
                    _ => push_text("[image omitted: no data]", &mut blocks),
                }
            }
            Some("resource") => {
                let r = item.get("resource").cloned().unwrap_or(Value::Null);
                let uri = r.get("uri").and_then(Value::as_str).unwrap_or("?");
                match r.get("text").and_then(Value::as_str) {
                    Some(t) => push_text(&format!("[resource {uri}]\n{t}"), &mut blocks),
                    None => push_text(&format!("[resource {uri}: binary content omitted]"), &mut blocks),
                }
            }
            Some("resource_link") => {
                let uri = item.get("uri").and_then(Value::as_str).unwrap_or("?");
                let name = item.get("name").and_then(Value::as_str).unwrap_or(uri);
                push_text(&format!("[resource link: {name} <{uri}>]"), &mut blocks);
            }
            Some(other) => push_text(&format!("[{other} content omitted]"), &mut blocks),
            None => {}
        }
    }
    if blocks.is_empty() {
        if let Some(s) = result.get("structuredContent") {
            push_text(&s.to_string(), &mut blocks);
        }
    }
    let has_media = blocks.iter().any(|b| !matches!(b, ContentBlock::Text { .. }));
    let mut out = if has_media {
        ToolOutput::blocks(blocks)
    } else if text.is_empty() {
        ToolOutput::text("(no output)")
    } else {
        ToolOutput::text(text)
    };
    out.is_error = is_error;
    if let Some(s) = result.get("structuredContent") {
        out.structured = Some(s.clone());
    }
    out
}

/// One server tool. It reads its server's live state at each use, so a
/// reconnect, enable or disable reaches tool objects the session and its
/// sub-agents already hold: hidden while the server is off or no longer lists
/// the tool, with the description and schema the server gives now.
pub struct McpTool {
    pub full_name: String,
    pub server: Arc<ServerEntry>,
    /// The tool as listed when this object was made.
    pub info: ToolInfo,
}

impl McpTool {
    pub fn new(server: Arc<ServerEntry>, info: ToolInfo) -> Self {
        McpTool { full_name: tool_name(&server.name, &info.name), server, info }
    }

    fn current(&self) -> ToolInfo {
        self.server.tool(&self.info.name).unwrap_or_else(|| self.info.clone())
    }
}

#[async_trait::async_trait]
impl Tool for McpTool {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> String {
        let info = self.current();
        let d = if info.description.trim().is_empty() {
            format!("Tool {} from the MCP server {}.", info.name, self.server.name)
        } else {
            info.description
        };
        d.chars().take(2048).collect()
    }

    fn input_schema(&self) -> Value {
        let mut s = self.current().input_schema;
        if s.get("type").is_none() {
            s["type"] = json!("object");
        }
        s
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        self.current().read_only_hint
    }

    /// Hidden while the server is off, failed or restarting, or no longer lists the tool.
    fn is_enabled(&self) -> bool {
        self.server.tool(&self.info.name).is_some()
    }

    fn permission_subject(&self, _input: &Value, _ctx: &ToolContext) -> Subject {
        Subject::None
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let Some(client) = self.server.client() else {
            return ToolOutput::error(format!(
                "MCP server {} isn't connected ({}), so {} didn't run.",
                self.server.name,
                self.server.status().as_str(),
                self.info.name
            ));
        };
        tokio::select! {
            r = client.call_tool(&self.info.name, input) => match r {
                Ok(v) => convert_content(&v),
                // The server is gone: say so everywhere (`/mcp`, the tool list) and how to bring it back.
                Err(e @ crate::transport::McpError::Closed(_)) => {
                    self.server.mark_lost(&e.to_string());
                    ToolOutput::error(format!(
                        "MCP server {0} stopped: {e}. {1} didn't run, and its tools are unavailable until the user runs \
                         /mcp reconnect {0}.",
                        self.server.name, self.info.name
                    ))
                }
                Err(e) => ToolOutput::error(format!("MCP server {} failed to run {}: {e}", self.server.name, self.info.name)),
            },
            _ = ctx.cancel.cancelled() => {
                client.cancelled("interrupted by the user").await;
                ToolOutput::error(INTERRUPTED)
            }
        }
    }
}

/// The connected servers among `servers` that offer resources, read at each call.
fn resource_clients(servers: &[Arc<ServerEntry>]) -> Vec<Arc<McpClient>> {
    servers.iter().filter_map(|s| s.client()).filter(|c| c.has("resources")).collect()
}

/// `ListMcpResourcesTool`: resources across servers.
pub struct ListResources {
    pub servers: Vec<Arc<ServerEntry>>,
}

#[async_trait::async_trait]
impl Tool for ListResources {
    fn name(&self) -> &str {
        "ListMcpResourcesTool"
    }

    fn description(&self) -> String {
        "List the resources (files, records, documents) that connected MCP servers offer. Give `server` to list \
         one server's; read one with ReadMcpResourceTool."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"server": {"type": "string", "description": "Only this server's resources"}},
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    /// Hidden while no connected server offers resources.
    fn is_enabled(&self) -> bool {
        !resource_clients(&self.servers).is_empty()
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> ToolOutput {
        let clients = resource_clients(&self.servers);
        let only = input.get("server").and_then(Value::as_str);
        if let Some(s) = only {
            if !clients.iter().any(|c| c.name == s) {
                let names: Vec<&str> = clients.iter().map(|c| c.name.as_str()).collect();
                return ToolOutput::error(format!("No MCP server named {s:?}. Connected: {}", names.join(", ")));
            }
        }
        let mut out = vec![];
        let mut errors = vec![];
        for c in clients.iter().filter(|c| only.is_none_or(|s| s == c.name)) {
            match c.list_resources().await {
                Ok(list) => out.extend(list.into_iter().map(|mut r| {
                    r["server"] = json!(c.name);
                    r
                })),
                Err(e) => errors.push(format!("{}: {e}", c.name)),
            }
        }
        let mut text = if out.is_empty() {
            "No resources found.".to_string()
        } else {
            serde_json::to_string_pretty(&out).unwrap_or_default()
        };
        if !errors.is_empty() {
            text.push_str(&format!("\n\nErrors: {}", errors.join("; ")));
        }
        ToolOutput::text(text).with_structured(json!(out))
    }
}

/// `ReadMcpResourceTool`: one resource by server and URI.
pub struct ReadResource {
    pub servers: Vec<Arc<ServerEntry>>,
}

#[async_trait::async_trait]
impl Tool for ReadResource {
    fn name(&self) -> &str {
        "ReadMcpResourceTool"
    }

    fn description(&self) -> String {
        "Read one resource from an MCP server by its URI (from ListMcpResourcesTool).".into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "server": {"type": "string", "description": "The MCP server's name"},
                "uri": {"type": "string", "description": "The resource URI"}
            },
            "required": ["server", "uri"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    /// Hidden while no connected server offers resources.
    fn is_enabled(&self) -> bool {
        !resource_clients(&self.servers).is_empty()
    }

    fn permission_subject(&self, input: &Value, _ctx: &ToolContext) -> Subject {
        Subject::Name(input.get("server").and_then(Value::as_str).unwrap_or_default().to_string())
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> ToolOutput {
        let server = input.get("server").and_then(Value::as_str).unwrap_or_default();
        let uri = input.get("uri").and_then(Value::as_str).unwrap_or_default();
        let clients = resource_clients(&self.servers);
        let Some(c) = clients.iter().find(|c| c.name == server) else {
            return ToolOutput::error(format!("No MCP server named {server:?}."));
        };
        match c.read_resource(uri).await {
            Ok(v) => {
                let contents: Vec<Value> = v
                    .get("contents")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|r| match (r.get("text"), r.get("blob")) {
                        (Some(_), _) => json!({"type": "resource", "resource": r}),
                        (None, Some(b))
                            if r.get("mimeType").and_then(Value::as_str).is_some_and(|m| m.starts_with("image/")) =>
                        {
                            json!({"type": "image", "data": b, "mimeType": r["mimeType"]})
                        }
                        _ => json!({"type": "resource", "resource": {"uri": r.get("uri")}}),
                    })
                    .collect();
                convert_content(&json!({"content": contents}))
            }
            Err(e) => ToolOutput::error(format!("Could not read {uri} from {server}: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_clean_and_short() {
        assert_eq!(tool_name("git hub", "create.issue"), "mcp__git_hub__create_issue");
        let long = tool_name("a-very-long-server-name-for-testing", "and_an_even_longer_tool_name_that_overflows");
        assert_eq!(long.len(), 64);
        assert!(long.starts_with("mcp__a-very-long-server-name"));
        assert_ne!(
            long,
            tool_name("a-very-long-server-name-for-testing", "and_an_even_longer_tool_name_that_overflowz")
        );
    }

    #[test]
    fn converts_every_content_type() {
        let out = convert_content(&json!({"content": [
            {"type": "text", "text": "hello"},
            {"type": "resource", "resource": {"uri": "file:///a", "text": "body"}},
            {"type": "resource_link", "uri": "file:///b", "name": "b"},
            {"type": "audio", "data": "x"}
        ]}));
        assert!(!out.is_error);
        assert_eq!(
            out.text_content(),
            "hello\n[resource file:///a]\nbody\n[resource link: b <file:///b>]\n[audio content omitted]"
        );
        let img = convert_content(&json!({"content": [{"type": "image", "data": "aGk=", "mimeType": "image/png"}]}));
        assert!(
            matches!(&img.content, kodact_types::ToolResultContent::Blocks(b) if matches!(b[0], ContentBlock::Image { .. }))
        );
        let err = convert_content(&json!({"isError": true, "content": [{"type": "text", "text": "nope"}]}));
        assert!(err.is_error);
        let structured = convert_content(&json!({"content": [], "structuredContent": {"n": 1}}));
        assert_eq!(structured.text_content(), r#"{"n":1}"#);
        assert_eq!(convert_content(&json!({"content": []})).text_content(), "(no output)");
    }
}
