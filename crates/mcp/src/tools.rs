//! Tool definitions and calls.

use std::time::Duration;

use recurse_client::{ClientError, DEFAULT_TIMEOUT};
use recurse_protocol::{CellResult, EventsListResult, bounds};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::Session;
use crate::format;

pub fn list() -> Value {
    let target_only = json!({"type": "object", "properties": {}, "additionalProperties": false});

    json!({"tools": [
        {
            "name": "ipython",
            "title": "IPython",
            "description": "Run Python in this session's persistent kernel (top-level await allowed). Returns stdout, stderr, the repr of the last expression, and any error. State persists across calls.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": {"type": "string", "description": "Python source for one cell"},
                    "timeout_sec": {"type": "integer", "minimum": 1, "maximum": bounds::MAX_TIMEOUT_SEC, "description": "Cell timeout (default 600)"}
                },
                "required": ["code"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": false, "openWorldHint": true}
        },
        {
            "name": "kernel_status",
            "title": "Kernel status",
            "description": "Show the kernel state, pid, execution count, cwd, and live bash() handles.",
            "inputSchema": target_only,
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        },
        {
            "name": "kernel_restart",
            "title": "Restart kernel",
            "description": "Discard all Python state; the next ipython call starts a fresh kernel.",
            "inputSchema": target_only,
            "annotations": {"readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false}
        },
        {
            "name": "kernel_interrupt",
            "title": "Interrupt cell",
            "description": "Raise KeyboardInterrupt in the running cell, if any.",
            "inputSchema": target_only,
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        },
        {
            "name": "events_read",
            "title": "Read events",
            "description": "List queued recurse events (background command notices, kernel exits) oldest first. They stay queued until events_ack.",
            "inputSchema": target_only,
            "annotations": {"readOnlyHint": true, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        },
        {
            "name": "events_ack",
            "title": "Acknowledge events",
            "description": "Remove handled events from the queue by id.",
            "inputSchema": {
                "type": "object",
                "properties": {"event_ids": {"type": "array", "items": {"type": "string"}}},
                "required": ["event_ids"],
                "additionalProperties": false
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": true, "openWorldHint": false}
        }
    ]})
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IpythonArgs {
    code: String,
    timeout_sec: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AckArgs {
    event_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CallParams {
    name: String,
    #[serde(default)]
    arguments: Value,
}

pub async fn call(session: &Session, params: Value) -> Result<Value, String> {
    let params: CallParams =
        serde_json::from_value(params).map_err(|error| format!("invalid tools/call: {error}"))?;
    let arguments = if params.arguments.is_null() {
        json!({})
    } else {
        params.arguments
    };
    let target = json!({"target": session.target});
    let outcome = match params.name.as_str() {
        "ipython" => return ipython(session, parse(arguments)?).await,
        "kernel_status" => {
            session
                .call::<_, Value>("kernel.status", &target, DEFAULT_TIMEOUT)
                .await
        }
        "kernel_restart" => {
            session
                .call::<_, Value>("kernel.restart", &target, DEFAULT_TIMEOUT)
                .await
        }
        "kernel_interrupt" => {
            session
                .call::<_, Value>("kernel.interrupt", &target, DEFAULT_TIMEOUT)
                .await
        }
        "events_read" => {
            session
                .call::<_, Value>("events.list", &target, DEFAULT_TIMEOUT)
                .await
        }
        "events_ack" => {
            let args: AckArgs = parse(arguments)?;
            let params = json!({"target": session.target, "event_ids": args.event_ids});

            session
                .call::<_, Value>("events.ack", &params, DEFAULT_TIMEOUT)
                .await
        }
        other => return Err(format!("unknown tool {other}")),
    };

    Ok(match outcome {
        Ok(value) => text(
            &serde_json::to_string_pretty(&value).unwrap_or_default(),
            false,
        ),
        Err(error) => failure(&error),
    })
}

fn parse<T: serde::de::DeserializeOwned>(arguments: Value) -> Result<T, String> {
    serde_json::from_value(arguments).map_err(|error| format!("invalid arguments: {error}"))
}

async fn ipython(session: &Session, args: IpythonArgs) -> Result<Value, String> {
    let timeout_sec = args
        .timeout_sec
        .unwrap_or(600)
        .clamp(1, bounds::MAX_TIMEOUT_SEC);
    let params = json!({"target": session.target, "code": args.code, "timeout_sec": timeout_sec});
    let deadline = Duration::from_secs(timeout_sec.saturating_add(30));
    let cell = match session
        .call::<_, CellResult>("kernel.execute", &params, deadline)
        .await
    {
        Ok(cell) => cell,
        Err(error) => return Ok(failure(&error)),
    };
    let mut rendered = format::cell(&cell);
    let events: Result<EventsListResult, _> = session
        .call(
            "events.list",
            &json!({"target": session.target}),
            DEFAULT_TIMEOUT,
        )
        .await;

    if let Some(notice) = events
        .ok()
        .and_then(|events| format::pending_events(&events.events))
    {
        rendered.push_str("\n\n");
        rendered.push_str(&notice);
    }

    Ok(text(&rendered, format::is_error(&cell)))
}

fn text(text: &str, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

fn failure(error: &ClientError) -> Value {
    text(&format!("recurse: {error}"), true)
}
