//! Router, bearer auth, idle exit, and signal handling.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use recurse_protocol::{ErrorCode, RpcError};

use crate::app::App;

pub fn router(app: Arc<App>) -> Router {
    Router::new()
        .route("/rpc", post(crate::rpc::handle))
        .route("/events", get(crate::sse::handle))
        .with_state(app)
}

pub fn authorized(headers: &HeaderMap, token: Option<&str>) -> bool {
    let Some(token) = token else {
        return true;
    };

    headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .is_some_and(|given| constant_time_eq(given.as_bytes(), token.as_bytes()))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |diff, (a, b)| diff | (a ^ b))
            == 0
}

pub fn unauthorized() -> Response {
    let mut response = crate::rpc::reply(
        None,
        Err(RpcError::new(
            ErrorCode::Unauthorized,
            "missing or incorrect bearer token",
        )),
    );

    *response.status_mut() = StatusCode::UNAUTHORIZED;
    response.into_response()
}

/// Requests shutdown once no RPC arrived and no kernel lived for `RECURSE_IDLE_EXIT_SEC`.
pub async fn idle_watch(app: Arc<App>) {
    let limit = Duration::from_secs(app.options.idle_exit_sec);
    let mut shutdown = app.shutdown.subscribe();

    if limit.is_zero() {
        return;
    }

    loop {
        tokio::select! {
            () = tokio::time::sleep(Duration::from_secs(1)) => {}
            _ = shutdown.changed() => return,
        }

        if app.kernels.live_count() > 0 {
            app.touch();
        } else if app.idle_for() >= limit {
            app.log.info("idle exit");
            app.request_shutdown();
            return;
        }
    }
}

/// Resolves on SIGTERM or SIGINT.
pub async fn termination() {
    use tokio::signal::unix::{SignalKind, signal};

    let (Ok(mut term), Ok(mut int)) = (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) else {
        std::future::pending::<()>().await;
        return;
    };

    tokio::select! {
        _ = term.recv() => {}
        _ = int.recv() => {}
    }
}
