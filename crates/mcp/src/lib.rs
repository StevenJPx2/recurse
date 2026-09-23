//! Stdio MCP server: a client of the recurse daemon bound to one `mcp` target.

mod format;
mod session;
mod tools;

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

pub use session::Session;

pub const PROTOCOL_VERSION: &str = "2025-11-25";

/// Serves MCP over stdin/stdout until stdin closes. Requests run concurrently so `ping` and
/// cancellation stay responsive during a long cell.
pub async fn run_stdio(session: Session) -> Result<(), String> {
    let session = Arc::new(session);
    let (tx, mut rx) = mpsc::channel::<Value>(64);
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();

        while let Some(message) = rx.recv().await {
            let mut line = serde_json::to_vec(&message).unwrap_or_default();

            line.push(b'\n');

            if stdout.write_all(&line).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });
    let mut lines = BufReader::new(tokio::io::stdin()).lines();

    while let Some(line) = lines.next_line().await.map_err(|error| error.to_string())? {
        if line.trim().is_empty() {
            continue;
        }

        let (session, tx) = (session.clone(), tx.clone());

        tokio::spawn(async move {
            if let Some(response) = handle_line(&session, &line).await {
                let _ = tx.send(response).await;
            }
        });
    }

    drop(tx);
    let _ = writer.await;

    Ok(())
}

async fn handle_line(session: &Session, line: &str) -> Option<Value> {
    let request: Value = match serde_json::from_str(line) {
        Ok(request) => request,
        Err(error) => {
            return Some(error_response(
                Value::Null,
                -32700,
                &format!("parse error: {error}"),
            ));
        }
    };
    let id = request.get("id").cloned()?;
    let method = request
        .get("method")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));
    let result = match method {
        "initialize" => Ok(initialize()),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools::list()),
        "tools/call" => tools::call(session, params).await,
        other => {
            return Some(error_response(
                id,
                -32601,
                &format!("method not found: {other}"),
            ));
        }
    };

    Some(match result {
        Ok(result) => json!({"jsonrpc": "2.0", "id": id, "result": result}),
        Err(message) => error_response(id, -32602, &message),
    })
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn initialize() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {"tools": {}},
        "serverInfo": {"name": "recurse", "version": env!("CARGO_PKG_VERSION")},
        "instructions": "Use the ipython tool to work in a persistent Python kernel. State persists across calls; the last expression's repr is returned. Call events_read when told events are pending and events_ack after handling them."
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::{Session, handle_line};

    #[tokio::test]
    async fn answers_initialize_and_lists_tools_without_a_daemon() {
        let session = Session::new(recurse_protocol::Target::new(
            recurse_protocol::TargetKind::Mcp,
            "test",
        ));
        let init = handle_line(
            &session,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#,
        )
        .await
        .unwrap();

        assert_eq!(init["result"]["protocolVersion"], "2025-11-25");

        let tools = handle_line(
            &session,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        )
        .await
        .unwrap();
        let names: Vec<&str> = tools["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();

        assert_eq!(
            names,
            [
                "ipython",
                "kernel_status",
                "kernel_restart",
                "kernel_interrupt",
                "events_read",
                "events_ack"
            ]
        );
        assert_eq!(
            tools["result"]["tools"][1]["annotations"]["readOnlyHint"],
            json!(true)
        );
        assert!(
            handle_line(
                &session,
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
            )
            .await
            .is_none()
        );
        assert_eq!(
            handle_line(&session, r#"{"jsonrpc":"2.0","id":3,"method":"nope"}"#)
                .await
                .unwrap()["error"]["code"],
            -32601
        );
    }
}
