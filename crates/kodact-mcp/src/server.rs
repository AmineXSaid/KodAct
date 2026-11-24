//! `kodact mcp serve`: KodAct's built-in tools as an MCP server over stdio.
//!
//! The connecting client decides what to allow (as with any MCP server), so
//! calls run without KodAct's permission prompts. Dangerous-command patterns
//! (contract C14) are still refused, because a client may be an agent being
//! steered by injected text.

use std::sync::Arc;

use kodact_tools::{ToolContext, ToolRegistry};
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::Mutex;

use crate::client::{PROTOCOL_VERSION, SUPPORTED_VERSIONS};

fn reply(id: &Value, result: Result<Value, (i64, String)>) -> Value {
    match result {
        Ok(r) => json!({"jsonrpc": "2.0", "id": id, "result": r}),
        Err((code, message)) => json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
    }
}

async fn call_tool(
    registry: &ToolRegistry,
    ctx: &ToolContext,
    params: &Value,
    id: &Value,
) -> Result<Value, (i64, String)> {
    let name = params.get("name").and_then(Value::as_str).ok_or((-32602, "missing tool name".to_string()))?;
    let args = params.get("arguments").cloned().unwrap_or_else(|| json!({}));
    let tool = registry.get(name).ok_or((-32602, format!("unknown tool: {name}")))?;
    let call_id = format!("mcp-{}", id.to_string().trim_matches('"'));
    let ctx = ctx.for_call(&call_id, ctx.cancel.child_token());
    let error = |text: String| json!({"content": [{"type": "text", "text": text}], "isError": true});
    let subject = tool.permission_subject(&args, &ctx);
    if let Some((url, entry)) = ctx.blocked_urls.blocks(&subject) {
        return Ok(error(kodact_tools::BlockedUrls::refusal(&url, entry)));
    }
    if let kodact_permissions::Subject::Command(cmd) = subject {
        if let Some(t) = kodact_permissions::threat::flagged(&cmd) {
            return Ok(error(format!("Refused: this command {} ({}).", t.description, t.name)));
        }
    }
    if let Err(e) = tool.validate(&args, &ctx) {
        return Ok(error(e));
    }
    let out = tool.call(args, &ctx).await;
    let content: Vec<Value> = match &out.content {
        kodact_types::ToolResultContent::Text(t) => vec![json!({"type": "text", "text": t})],
        kodact_types::ToolResultContent::Blocks(blocks) => blocks
            .iter()
            .map(|b| match b {
                kodact_types::ContentBlock::Image {
                    source: kodact_types::MediaSource::Base64 { media_type, data },
                    ..
                } => {
                    json!({"type": "image", "data": data, "mimeType": media_type})
                }
                other => json!({"type": "text", "text": other.as_text().unwrap_or("[unsupported content]")}),
            })
            .collect(),
    };
    let mut result = json!({"content": content, "isError": out.is_error});
    if let Some(s) = out.structured.filter(Value::is_object) {
        result["structuredContent"] = s;
    }
    Ok(result)
}

/// Serve until `input` closes. Requests run concurrently; answers may come back out of order.
pub async fn serve<R, W>(registry: ToolRegistry, ctx: ToolContext, input: R, output: W) -> std::io::Result<()>
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let registry = Arc::new(registry);
    let out = Arc::new(Mutex::new(output));
    let mut lines = input.lines();
    let mut tasks = tokio::task::JoinSet::new();
    while let Some(line) = lines.next_line().await? {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                let r = reply(&Value::Null, Err((-32700, format!("parse error: {e}"))));
                let mut w = out.lock().await;
                w.write_all(format!("{r}\n").as_bytes()).await?;
                w.flush().await?;
                continue;
            }
        };
        let Some(id) = msg.get("id").cloned().filter(|i| !i.is_null()) else {
            continue; // a notification (initialized, cancelled, ...)
        };
        let method = msg.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let (registry, ctx, out) = (registry.clone(), ctx.clone(), out.clone());
        tasks.spawn(async move {
            let result = match method.as_str() {
                "initialize" => {
                    let asked = params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION);
                    let version = if SUPPORTED_VERSIONS.contains(&asked) { asked } else { PROTOCOL_VERSION };
                    Ok(json!({
                        "protocolVersion": version,
                        "capabilities": {"tools": {"listChanged": false}},
                        "serverInfo": {"name": "kodact", "title": "KodAct", "version": env!("CARGO_PKG_VERSION")},
                        "instructions": "KodAct's coding tools: read, search and edit files, and run shell commands in the server's working directory."
                    }))
                }
                "ping" => Ok(json!({})),
                "tools/list" => Ok(json!({"tools": registry.iter().filter(|t| t.is_enabled()).map(|t| {
                    let mut v = json!({"name": t.name(), "description": t.description(), "inputSchema": t.input_schema()});
                    if t.is_read_only(&json!({})) {
                        v["annotations"] = json!({"readOnlyHint": true});
                    }
                    v
                }).collect::<Vec<_>>()})),
                "tools/call" => call_tool(&registry, &ctx, &params, &id).await,
                m => Err((-32601, format!("method not found: {m}"))),
            };
            let r = reply(&id, result);
            let mut w = out.lock().await;
            let _ = w.write_all(format!("{r}\n").as_bytes()).await;
            let _ = w.flush().await;
        });
    }
    while tasks.join_next().await.is_some() {}
    Ok(())
}
