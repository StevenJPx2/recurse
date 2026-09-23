mod common;

use std::time::Duration;

use common::{Harness, opencode};
use recurse_client::EventStream;
use recurse_protocol::{AckResult, Event, HostName, KernelStatus, SseFrame, Target};
use serde_json::json;

async fn frame(stream: &mut EventStream) -> SseFrame {
    tokio::time::timeout(Duration::from_secs(10), stream.next_frame())
        .await
        .expect("frame within 10s")
        .unwrap()
        .expect("stream open")
}

/// Subscribes and returns the stream plus the replayed events.
async fn subscribe(harness: &Harness, target: &Target) -> (EventStream, Vec<Event>) {
    let mut stream = harness.client.subscribe(target).await.unwrap();

    assert_eq!(
        frame(&mut stream).await,
        SseFrame::Subscribed {
            target: target.clone()
        }
    );

    let SseFrame::Event { events, .. } = frame(&mut stream).await else {
        panic!("expected the replay frame")
    };

    (stream, events)
}

async fn kernel_pid(harness: &Harness, target: &Target) -> u32 {
    let status: KernelStatus = harness
        .client
        .call("kernel.status", &json!({"target": target}))
        .await
        .unwrap();

    status.pid.unwrap()
}

#[tokio::test]
async fn bash_notice_is_withdrawable_until_a_listener_takes_it() {
    let harness = Harness::start("notice").await;
    let target = opencode("ses_notice");

    harness.register(&target, HostName::Opencode, true).await;

    let queued = harness.host(&target, "NOTICE h1").await.unwrap();
    let pid = kernel_pid(&harness, &target).await;
    let first = format!("bash.finished:{pid}:h1");

    assert_eq!(queued["event_id"], first);

    let events = harness.events(&target).await;

    assert!(events[0].actionable);
    assert!(
        events[0]
            .text
            .starts_with("[recurse] Background command finished: pid 777, exit 1 (npm test).")
    );
    assert_eq!(
        harness
            .host(&target, &format!("WITHDRAW {first}"))
            .await
            .unwrap()["withdrawn"],
        true
    );
    assert!(harness.events(&target).await.is_empty());
    assert_eq!(
        harness
            .host(&target, &format!("WITHDRAW {first}"))
            .await
            .unwrap()["withdrawn"],
        false
    );

    harness.host(&target, "NOTICE h2").await.unwrap();

    let second = format!("bash.finished:{pid}:h2");
    let (_stream, replay) = subscribe(&harness, &target).await;

    assert_eq!(
        replay.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        [second.as_str()]
    );
    assert_eq!(
        harness
            .host(&target, &format!("WITHDRAW {second}"))
            .await
            .unwrap()["withdrawn"],
        false
    );
    assert_eq!(
        harness.events(&target).await.len(),
        1,
        "delivered notice stays queued"
    );
    harness.stop().await;
}

#[tokio::test]
async fn sse_replays_until_ack_and_streams_live_events() {
    let harness = Harness::start("sse").await;
    let target = opencode("ses_sse");

    harness.register(&target, HostName::Opencode, true).await;

    let (mut stream, replay) = subscribe(&harness, &target).await;

    assert!(replay.is_empty());

    harness.host(&target, "NOTICE live").await.unwrap();

    let SseFrame::Event { events, .. } = frame(&mut stream).await else {
        panic!("expected a live event frame")
    };
    let id = events[0].id.clone();

    assert!(id.ends_with(":live"));
    drop(stream);

    let (stream, replay) = subscribe(&harness, &target).await;

    assert_eq!(replay[0].id, id, "unacked events replay on reconnect");
    drop(stream);

    let acked: AckResult = harness
        .client
        .call(
            "events.ack",
            &json!({"target": target, "event_ids": [id, "unknown"]}),
        )
        .await
        .unwrap();

    assert_eq!(acked.acked, 1);

    let (_stream, replay) = subscribe(&harness, &target).await;

    assert!(replay.is_empty(), "acked events are gone");
    harness.stop().await;
}

#[tokio::test]
async fn events_require_a_valid_target_query() {
    let harness = Harness::start("sse-bad").await;
    let url = format!("{}/events?target=not-base64!", harness.daemon.url());
    let response = reqwest::get(url).await.unwrap();

    assert_eq!(response.status(), 400);

    let body: serde_json::Value = response.json().await.unwrap();

    assert_eq!(body["error"]["code"], "invalid_request");
    harness.stop().await;
}
