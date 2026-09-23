//! `POST /rpc`: envelope parsing and method dispatch (§2).

use std::sync::Arc;

use axum::Json;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use recurse_protocol::{
    AckResult, ChildResult, ChildrenListResult, DeleteSubagentParams, ErrorCode, EventsListResult,
    HealthResult, InterruptResult, Request, RestartResult, RpcError, RpcRequest, RpcResponse,
    SkillsListResult, Target, bounds,
};
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::registry::{invalid, not_found};
use crate::{events, host};

pub async fn handle(State(app): State<Arc<App>>, headers: HeaderMap, body: Body) -> Response {
    if !crate::server::authorized(&headers, app.options.token.as_deref()) {
        return crate::server::unauthorized();
    }

    app.touch();

    let (id, outcome) = match axum::body::to_bytes(body, bounds::RPC_BODY).await {
        Ok(bytes) => parse_and_dispatch(&app, &bytes).await,
        Err(_) => (
            None,
            Err(RpcError::new(
                ErrorCode::LimitExceeded,
                "request body exceeds 1 MiB",
            )),
        ),
    };

    reply(id, outcome)
}

async fn parse_and_dispatch(
    app: &Arc<App>,
    bytes: &[u8],
) -> (Option<Value>, Result<Value, RpcError>) {
    let raw: Value = match serde_json::from_slice(bytes) {
        Ok(raw) => raw,
        Err(error) => return (None, Err(invalid(format!("malformed JSON: {error}")))),
    };
    let id = raw.get("id").cloned();
    let envelope: RpcRequest = match serde_json::from_value(raw) {
        Ok(envelope) => envelope,
        Err(error) => return (id, Err(invalid(format!("invalid request: {error}")))),
    };
    let outcome = match Request::parse(&envelope.method, envelope.params) {
        Ok(request) => dispatch(app, request).await,
        Err(error) => Err(error),
    };

    (envelope.id, outcome)
}

pub fn reply(id: Option<Value>, outcome: Result<Value, RpcError>) -> Response {
    let (status, response) = match outcome {
        Ok(result) => (
            StatusCode::OK,
            RpcResponse {
                id,
                result: Some(result),
                error: None,
            },
        ),
        Err(error) => (
            status_for(error.code),
            RpcResponse {
                id,
                result: None,
                error: Some(error),
            },
        ),
    };

    (status, Json(response)).into_response()
}

const fn status_for(code: ErrorCode) -> StatusCode {
    match code {
        ErrorCode::Unauthorized => StatusCode::UNAUTHORIZED,
        ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCode::InvalidRequest
        | ErrorCode::NotFound
        | ErrorCode::Busy
        | ErrorCode::KernelUnavailable
        | ErrorCode::UnsupportedHost
        | ErrorCode::DepthExceeded
        | ErrorCode::LimitExceeded => StatusCode::BAD_REQUEST,
    }
}

fn json<T: Serialize>(value: &T) -> Result<Value, RpcError> {
    serde_json::to_value(value)
        .map_err(|error| RpcError::new(ErrorCode::Internal, error.to_string()))
}

pub fn check_target(target: &Target) -> Result<(), RpcError> {
    if target.is_valid() {
        Ok(())
    } else {
        Err(invalid("target id must be 1-256 characters"))
    }
}

fn request_target(request: &Request) -> Option<&Target> {
    match request {
        Request::Health => None,
        Request::TargetRegister(params) => Some(&params.target),
        Request::KernelExecute(params) => Some(&params.target),
        Request::KernelInterrupt(params)
        | Request::KernelRestart(params)
        | Request::KernelStatus(params)
        | Request::ChildrenList(params)
        | Request::EventsList(params) => Some(&params.target),
        Request::ChildrenBind(params) => Some(&params.target),
        Request::ChildrenFail(params) => Some(&params.target),
        Request::ChildrenDelete(params) => Some(&params.target),
        Request::EventsAck(params) => Some(&params.target),
        Request::SkillsList(params) => params.target.as_ref(),
    }
}

async fn dispatch(app: &Arc<App>, request: Request) -> Result<Value, RpcError> {
    if let Some(target) = request_target(&request) {
        check_target(target)?;
    }

    match request {
        Request::Health => json(&health(app)),
        Request::TargetRegister(params) => {
            if params.cwd.is_empty() {
                return Err(invalid("cwd must not be empty"));
            }

            json(
                &app.state
                    .update(&app.log, |core| core.registry.register(params))?,
            )
        }
        Request::KernelExecute(params) => json(&app.kernels.execute(app, params).await?),
        Request::KernelInterrupt(params) => {
            registered_cwd(app, &params.target)?;
            json(&InterruptResult {
                interrupted: app.kernels.interrupt(&params.target),
            })
        }
        Request::KernelRestart(params) => {
            registered_cwd(app, &params.target)?;
            app.kernels
                .restart(&params.target, app.options.shutdown_grace)
                .await;
            json(&RestartResult { restarted: true })
        }
        Request::KernelStatus(params) => {
            let cwd = registered_cwd(app, &params.target)?;

            json(&app.kernels.status(&params.target, cwd))
        }
        other => dispatch_children_and_events(app, other).await,
    }
}

async fn dispatch_children_and_events(app: &Arc<App>, request: Request) -> Result<Value, RpcError> {
    match request {
        Request::ChildrenList(params) => json(&ChildrenListResult {
            children: app
                .state
                .read(|core| core.registry.children_of(&params.target)),
        }),
        Request::ChildrenBind(params) => {
            check_target(&params.session)?;

            let child = app.state.update(&app.log, |core| {
                core.registry
                    .bind(&params.target, &params.child_id, &params.session)
            })?;

            json(&ChildResult { child })
        }
        Request::ChildrenFail(params) => {
            let mut reason = params.reason;

            bounds::clip_head(&mut reason, bounds::MESSAGE);

            let child = app.state.update(&app.log, |core| {
                let child = core.registry.fail(&params.target, &params.child_id)?;
                let sequence = core.registry.next_sequence(&child.child_id);
                let message = format!("[recurse] child failed to start: {reason}");

                core.enqueue(events::message_to_parent(&child, sequence, message));

                Ok(child)
            })?;

            json(&ChildResult { child })
        }
        Request::ChildrenDelete(params) => {
            let selector = DeleteSubagentParams {
                child_id: Some(params.child_id),
                name: None,
            };

            json(&ChildResult {
                child: host::delete_child(app, &params.target, &selector).await?,
            })
        }
        Request::EventsAck(params) => {
            let acked = app.state.update(&app.log, |core| {
                Ok(core.queue.ack(&params.target, &params.event_ids))
            })?;

            json(&AckResult { acked })
        }
        Request::EventsList(params) => json(&EventsListResult {
            events: app.state.read(|core| core.queue.list(&params.target)),
        }),
        Request::SkillsList(params) => json(&SkillsListResult {
            skills: host::skills_for(app, params.target.as_ref()),
        }),
        Request::Health
        | Request::TargetRegister(_)
        | Request::KernelExecute(_)
        | Request::KernelInterrupt(_)
        | Request::KernelRestart(_)
        | Request::KernelStatus(_) => Err(RpcError::new(ErrorCode::Internal, "misrouted request")),
    }
}

fn health(app: &App) -> HealthResult {
    HealthResult {
        ok: true,
        name: recurse_protocol::DAEMON_NAME.into(),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: recurse_protocol::PROTOCOL_VERSION,
        pid: std::process::id(),
        started_at: app.started_at,
    }
}

fn registered_cwd(app: &App, target: &Target) -> Result<String, RpcError> {
    app.state
        .read(|core| {
            core.registry
                .record(target)
                .map(|record| record.cwd.clone())
        })
        .ok_or_else(|| not_found(format!("target {target} is not registered")))
}
