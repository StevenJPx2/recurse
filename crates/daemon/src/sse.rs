//! `GET /events`: subscribed, replay of unacked events, live events, heartbeats.

use std::collections::HashSet;
use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::sse::{Event as SseEvent, Sse};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use base64::engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD};
use recurse_protocol::{Event, RpcError, SseFrame, Target, bounds};
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc};
use tokio_stream::wrappers::ReceiverStream;

use crate::app::App;
use crate::registry::invalid;
use crate::time::now_ms;

type FrameSender = mpsc::Sender<Result<SseEvent, Infallible>>;

#[derive(Debug, Deserialize)]
pub struct EventsQuery {
    target: Option<String>,
}

pub async fn handle(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    Query(query): Query<EventsQuery>,
) -> Response {
    if !crate::server::authorized(&headers, app.options.token.as_deref()) {
        return crate::server::unauthorized();
    }

    let target = match decode_target(query.target.as_deref()) {
        Ok(target) => target,
        Err(error) => return crate::rpc::reply(None, Err(error)),
    };
    let (tx, rx) = mpsc::channel(16);

    tokio::spawn(listen(app, target, tx));

    Sse::new(ReceiverStream::new(rx)).into_response()
}

fn decode_target(encoded: Option<&str>) -> Result<Target, RpcError> {
    let encoded = encoded.ok_or_else(|| invalid("missing ?target="))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded.trim_end_matches('='))
        .or_else(|_| URL_SAFE.decode(encoded))
        .map_err(|error| invalid(format!("target is not base64url: {error}")))?;
    let target: Target = serde_json::from_slice(&bytes)
        .map_err(|error| invalid(format!("target is not a JSON target: {error}")))?;

    crate::rpc::check_target(&target)?;

    Ok(target)
}

/// One listener: owns the set of event ids it has been handed (its claim).
async fn listen(app: Arc<App>, target: Target, tx: FrameSender) {
    let mut changes = app.state.subscribe();
    let mut shutdown = app.shutdown.subscribe();
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + app.options.heartbeat,
        app.options.heartbeat,
    );
    let mut claimed = HashSet::new();
    let key = target.key();

    if *shutdown.borrow_and_update()
        || !send(
            &tx,
            &SseFrame::Subscribed {
                target: target.clone(),
            },
        )
        .await
        || !deliver(&app, &target, &mut claimed, &tx, true).await
    {
        return;
    }

    loop {
        let open = tokio::select! {
            change = changes.recv() => match change {
                Ok(changed) if changed != key => true,
                Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {
                    deliver(&app, &target, &mut claimed, &tx, false).await
                }
                Err(broadcast::error::RecvError::Closed) => false,
            },
            _ = heartbeat.tick() => send(&tx, &SseFrame::Heartbeat { at: now_ms() }).await,
            _ = shutdown.changed() => false,
            () = tx.closed() => false,
        };

        if !open {
            return;
        }
    }
}

/// Hands every queued event this listener has not seen, marking them delivered.
async fn deliver(
    app: &App,
    target: &Target,
    claimed: &mut HashSet<String>,
    tx: &FrameSender,
    replay: bool,
) -> bool {
    let unseen = app.state.read(|core| {
        core.queue
            .list(target)
            .iter()
            .any(|event| !claimed.contains(&event.id))
    });

    if !unseen && !replay {
        return true;
    }

    let fresh = app.state.update(&app.log, |core| {
        let queued: HashSet<String> = core.queue.list(target).into_iter().map(|e| e.id).collect();

        claimed.retain(|id| queued.contains(id));

        Ok(core.queue.hand_out(target, claimed))
    });
    let fresh = fresh.unwrap_or_default();

    claimed.extend(fresh.iter().map(|event| event.id.clone()));

    if fresh.is_empty() && !replay {
        return true;
    }

    for events in chunk(fresh) {
        let frame = SseFrame::Event {
            target: target.clone(),
            events,
        };

        if !send(tx, &frame).await {
            return false;
        }
    }

    true
}

/// Splits events into frames that stay under the SSE frame bound (always at least one frame).
fn chunk(events: Vec<Event>) -> Vec<Vec<Event>> {
    let budget = bounds::SSE_FRAME.saturating_sub(4096);
    let mut frames = vec![Vec::new()];
    let mut size = 0usize;

    for event in events {
        let length = serde_json::to_vec(&event).map_or(0, |bytes| bytes.len());

        if size.saturating_add(length) > budget
            && frames.last().is_some_and(|frame| !frame.is_empty())
        {
            frames.push(Vec::new());
            size = 0;
        }

        size = size.saturating_add(length);

        if let Some(frame) = frames.last_mut() {
            frame.push(event);
        }
    }

    frames
}

async fn send(tx: &FrameSender, frame: &SseFrame) -> bool {
    let Ok(data) = serde_json::to_string(frame) else {
        return true;
    };

    tx.send(Ok(SseEvent::default().data(data))).await.is_ok()
}
