// todarchy-mcp — a Model Context Protocol server over stdio, exposing the
// todarchy task store as tools an LLM can call.
//
// It drives todarchy-core directly, so every read pulls the latest state and
// every write rides the user's existing sync transports (shared folder +
// relay) — changes made here show up on iOS / other devices, and shared
// projects (e.g. groceries) are encrypted through the same path.
//
// Transport: newline-delimited JSON-RPC 2.0 on stdin/stdout (the MCP stdio
// transport). CRITICAL: stdout carries the protocol, so all logging goes to
// stderr — never println! here.

mod tools;

use anyhow::Result;
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// MCP protocol version we implement (broadly supported by current clients).
const PROTOCOL_VERSION: &str = "2024-11-05";

struct RpcError {
    code: i64,
    message: String,
}

fn rpc_err(code: i64, message: impl Into<String>) -> RpcError {
    RpcError { code, message: message.into() }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Logs to stderr only; stdout is reserved for JSON-RPC frames.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let mut reader = BufReader::new(tokio::io::stdin()).lines();
    let mut stdout = tokio::io::stdout();

    while let Some(line) = reader.next_line().await? {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                write_error(&mut stdout, Value::Null, -32700, &format!("parse error: {e}")).await?;
                continue;
            }
        };

        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("").to_string();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);

        // No id → JSON-RPC notification: act if we care, never respond.
        let Some(id) = id else {
            tracing::debug!("notification: {method}");
            continue;
        };

        match dispatch(&method, params).await {
            Ok(result) => write_result(&mut stdout, id, result).await?,
            Err(e) => write_error(&mut stdout, id, e.code, &e.message).await?,
        }
    }
    Ok(())
}

async fn dispatch(method: &str, params: Value) -> std::result::Result<Value, RpcError> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "todokase", "version": env!("CARGO_PKG_VERSION") },
        })),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools::list() })),
        "tools/call" => {
            let name = params.get("name").and_then(Value::as_str).unwrap_or("");
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            // Tool-level failures are returned as a result with isError=true
            // (per MCP), so the model sees the message instead of a transport
            // fault. Only genuinely unknown tools are a protocol error.
            match tools::call(name, args).await {
                Ok(text) => Ok(json!({ "content": [ { "type": "text", "text": text } ] })),
                Err(tools::ToolError::Unknown) => {
                    Err(rpc_err(-32602, format!("unknown tool: {name}")))
                }
                Err(tools::ToolError::Failed(msg)) => Ok(json!({
                    "content": [ { "type": "text", "text": msg } ],
                    "isError": true,
                })),
            }
        }
        // Tools-only server: politely decline the optional feature methods.
        "resources/list" => Ok(json!({ "resources": [] })),
        "prompts/list" => Ok(json!({ "prompts": [] })),
        other => Err(rpc_err(-32601, format!("method not found: {other}"))),
    }
}

async fn write_result<W: AsyncWriteExt + Unpin>(w: &mut W, id: Value, result: Value) -> Result<()> {
    write_frame(w, json!({ "jsonrpc": "2.0", "id": id, "result": result })).await
}

async fn write_error<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    id: Value,
    code: i64,
    message: &str,
) -> Result<()> {
    write_frame(w, json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })).await
}

async fn write_frame<W: AsyncWriteExt + Unpin>(w: &mut W, v: Value) -> Result<()> {
    let mut line = serde_json::to_string(&v)?;
    line.push('\n');
    w.write_all(line.as_bytes()).await?;
    w.flush().await?;
    Ok(())
}
