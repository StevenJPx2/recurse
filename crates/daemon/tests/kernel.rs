mod common;

use std::time::Duration;

use common::{Harness, code_of, opencode};
use recurse_protocol::{
    CellStatus, ErrorCode, EventBody, HostName, InterruptResult, KernelState, KernelStatus,
    RestartResult, Target, TargetKind,
};
use serde_json::json;

async fn status(harness: &Harness, target: &Target) -> KernelStatus {
    harness
        .client
        .call("kernel.status", &json!({"target": target}))
        .await
        .unwrap()
}

#[tokio::test]
async fn execute_runs_in_the_target_cwd_with_the_documented_env() {
    let harness = Harness::start("execute").await;
    let target = Target::new(TargetKind::Cli, "laptop");

    harness.register(&target, HostName::Cli, false).await;
    assert_eq!(status(&harness, &target).await.state, KernelState::Absent);

    let cell = harness.exec(&target, "1 + 1").await.unwrap();

    assert_eq!(cell.status, CellStatus::Ok);
    assert_eq!(cell.result.as_deref(), Some("'1 + 1'"));
    assert_eq!(cell.execution_count, 1);

    let cwd = harness.exec(&target, "CWD").await.unwrap();

    assert_eq!(
        cwd.result,
        Some(format!("{:?}", harness.cwd()).replace('"', "'"))
    );

    for (name, value) in [
        ("RECURSE_DEPTH", "'0'"),
        ("RECURSE_MAX_DEPTH", "'1'"),
        ("RECURSE_KERNEL_PROTOCOL", "'1'"),
        ("RECURSE_TARGET", r#"'{"kind":"cli","id":"laptop"}'"#),
    ] {
        let cell = harness.exec(&target, &format!("ENV {name}")).await.unwrap();

        assert_eq!(cell.result.as_deref(), Some(value), "{name}");
    }

    let status = status(&harness, &target).await;

    assert_eq!(status.state, KernelState::Idle);
    assert_eq!(status.execution_count, 6);
    assert_eq!(status.cwd, harness.cwd());
    assert!(status.pid.is_some());
    harness.stop().await;
}

#[tokio::test]
async fn unregistered_targets_and_oversized_code_are_rejected() {
    let harness = Harness::start("reject").await;
    let target = opencode("ses_unknown");
    let error = harness.exec(&target, "1").await.unwrap_err();

    assert_eq!(code_of(&error), ErrorCode::NotFound);

    harness.register(&target, HostName::Opencode, true).await;

    let error = harness
        .exec(&target, &"x".repeat(256 * 1024 + 1))
        .await
        .unwrap_err();

    assert_eq!(code_of(&error), ErrorCode::LimitExceeded);

    let big = harness.exec(&target, "BIG").await.unwrap();

    assert!(big.truncated);
    assert!(big.stdout.len() <= 64 * 1024);
    harness.stop().await;
}

#[tokio::test]
async fn concurrent_execute_is_busy_and_interrupt_stops_the_cell() {
    let harness = Harness::start("busy").await;
    let target = opencode("ses_busy");

    harness.register(&target, HostName::Opencode, true).await;
    harness.exec(&target, "warm").await.unwrap();

    let (slow, busy) = tokio::join!(harness.exec(&target, "SLEEP 30"), async {
        tokio::time::sleep(Duration::from_millis(300)).await;

        let busy = harness.exec(&target, "2").await.unwrap_err();

        assert_eq!(status(&harness, &target).await.state, KernelState::Busy);

        let interrupted: InterruptResult = harness
            .client
            .call("kernel.interrupt", &json!({"target": target}))
            .await
            .unwrap();

        assert!(interrupted.interrupted);
        busy
    });

    assert_eq!(code_of(&busy), ErrorCode::Busy);
    assert_eq!(
        busy.to_string(),
        "busy: a cell is already running for opencode-session:ses_busy"
    );
    assert_eq!(slow.unwrap().status, CellStatus::Interrupted);

    let idle: InterruptResult = harness
        .client
        .call("kernel.interrupt", &json!({"target": target}))
        .await
        .unwrap();

    assert!(!idle.interrupted);
    harness.stop().await;
}

#[tokio::test]
async fn timeout_interrupts_and_keeps_the_kernel_or_kills_a_stuck_one() {
    let harness = Harness::start("timeout").await;
    let target = opencode("ses_timeout");

    harness.register(&target, HostName::Opencode, true).await;
    harness.exec(&target, "warm").await.unwrap();

    let pid = status(&harness, &target).await.pid;
    let cell = harness
        .exec_timeout(&target, "SLEEP 30", Some(1))
        .await
        .unwrap();

    assert_eq!(cell.status, CellStatus::Timeout);
    assert_eq!(cell.stdout, "slept partially\n");
    assert_eq!(status(&harness, &target).await.pid, pid, "kernel survives");

    let stuck = harness
        .exec_timeout(&target, "HANG 30", Some(1))
        .await
        .unwrap();

    assert_eq!(stuck.status, CellStatus::Timeout);
    assert_eq!(status(&harness, &target).await.state, KernelState::Dead);

    let fresh = harness.exec(&target, "again").await.unwrap();

    assert_eq!(fresh.execution_count, 1);
    assert_ne!(status(&harness, &target).await.pid, pid, "respawned");
    harness.stop().await;
}

#[tokio::test]
async fn kernel_exit_queues_kernel_exited_and_the_next_execute_respawns() {
    let harness = Harness::start("exit").await;
    let target = opencode("ses_exit");

    harness.register(&target, HostName::Opencode, true).await;
    harness.exec(&target, "warm").await.unwrap();

    let pid = status(&harness, &target).await.pid.unwrap();
    let error = harness.exec(&target, "EXIT").await.unwrap_err();

    assert_eq!(code_of(&error), ErrorCode::KernelUnavailable);

    for _ in 0..50 {
        if !harness.events(&target).await.is_empty() {
            break;
        }

        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let events = harness.events(&target).await;

    assert_eq!(events.len(), 1);
    assert_eq!(events[0].id, format!("kernel.exited:{pid}"));
    assert!(!events[0].actionable);
    assert!(matches!(
        &events[0].body,
        EventBody::KernelExited(payload) if payload.exit_code == Some(3) && payload.signal.is_none()
    ));
    assert_eq!(status(&harness, &target).await.state, KernelState::Dead);
    assert_eq!(
        harness.exec(&target, "ok").await.unwrap().status,
        CellStatus::Ok
    );
    harness.stop().await;
}

#[tokio::test]
async fn restart_discards_the_kernel_without_an_exit_event() {
    let harness = Harness::start("restart").await;
    let target = opencode("ses_restart");

    harness.register(&target, HostName::Opencode, true).await;
    harness.exec(&target, "HANDLES").await.unwrap();

    let before = status(&harness, &target).await;

    assert_eq!(before.handles.len(), 1);

    let restarted: RestartResult = harness
        .client
        .call("kernel.restart", &json!({"target": target}))
        .await
        .unwrap();

    assert!(restarted.restarted);
    assert_eq!(status(&harness, &target).await.state, KernelState::Absent);

    let cell = harness.exec(&target, "fresh").await.unwrap();

    assert_eq!(cell.execution_count, 1);
    assert_ne!(status(&harness, &target).await.pid, before.pid);
    assert!(harness.events(&target).await.is_empty());
    harness.stop().await;
}
