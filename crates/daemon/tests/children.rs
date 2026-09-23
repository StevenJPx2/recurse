mod common;

use common::{Harness, code_of, opencode};
use recurse_protocol::{
    ChildInfo, ChildResult, ChildStatus, ChildrenListResult, ErrorCode, EventBody, HostName,
    KernelState, KernelStatus, RegisterResult, Role, Target, TargetKind,
};
use serde_json::{Value, json};

fn child(value: Value) -> ChildInfo {
    serde_json::from_value(value).unwrap()
}

async fn spawn_and_bind(
    harness: &Harness,
    parent: &Target,
    name: &str,
    session: &Target,
) -> ChildInfo {
    let pending = child(
        harness
            .host(
                parent,
                &format!("SPAWN {name} Review the authentication flow"),
            )
            .await
            .unwrap(),
    );
    let bound: ChildResult = harness
        .client
        .call(
            "children.bind",
            &json!({"target": parent, "child_id": pending.child_id, "session": session}),
        )
        .await
        .unwrap();

    bound.child
}

#[tokio::test]
async fn spawn_bind_register_and_message_round_trip() {
    let harness = Harness::start("spawn").await;
    let parent = opencode("ses_abc");
    let session = opencode("ses_child");

    harness.register(&parent, HostName::Opencode, true).await;

    let pending = child(
        harness
            .host(
                &parent,
                "SPAWN auth-reviewer Review the authentication flow",
            )
            .await
            .unwrap(),
    );

    assert_eq!(pending.status, ChildStatus::Pending);
    assert_eq!(pending.depth, 1);
    assert_eq!(pending.session_dir, harness.cwd());
    assert_eq!(pending.model.as_deref(), Some("anthropic/claude-sonnet-5"));
    assert!(pending.child_id.starts_with("c_") && pending.child_id.len() == 10);

    let events = harness.events(&parent).await;

    assert_eq!(events[0].id, format!("child.spawn:{}", pending.child_id));
    assert!(!events[0].actionable);
    let EventBody::ChildSpawn(payload) = &events[0].body else {
        panic!("expected child.spawn")
    };
    assert!(
        payload
            .prompt
            .starts_with("Review the authentication flow\n\n---\n")
    );
    assert!(payload.prompt.contains("receiver_role=\"parent\""));

    let bound: ChildResult = harness
        .client
        .call(
            "children.bind",
            &json!({"target": parent, "child_id": pending.child_id, "session": session}),
        )
        .await
        .unwrap();

    assert_eq!(bound.child.status, ChildStatus::Running);

    let registered = harness.register(&session, HostName::Opencode, true).await;

    assert_eq!(registered.depth, 1);
    assert_eq!(registered.parent.as_ref(), Some(&parent));
    assert_eq!(
        registered.child.as_ref().map(|c| c.child_id.as_str()),
        Some(pending.child_id.as_str())
    );
    assert_eq!(
        harness
            .exec(&session, "ENV RECURSE_DEPTH")
            .await
            .unwrap()
            .result
            .as_deref(),
        Some("'1'")
    );

    let sent = harness
        .host(&session, "SEND parent Found 2 issues.")
        .await
        .unwrap();
    let expected_id = format!("agent.message:{}:1", pending.child_id);

    assert_eq!(sent["event_id"], expected_id);

    let message = harness
        .events(&parent)
        .await
        .into_iter()
        .find(|e| e.id == expected_id)
        .unwrap();

    assert!(message.actionable);
    assert_eq!(
        message.text,
        format!(
            "[recurse] message from child auth-reviewer ({}):\n\nFound 2 issues.",
            pending.child_id
        )
    );

    let reply = harness
        .host(&parent, "SEND child auth-reviewer Check the test.")
        .await
        .unwrap();

    assert_eq!(
        reply["event_id"],
        "agent.message:opencode-session:ses_abc:1"
    );

    let inbox = harness.events(&session).await;

    assert_eq!(inbox.len(), 1);
    assert!(matches!(&inbox[0].body, EventBody::AgentMessage(p) if p.from.role == Role::Parent));

    let nested = harness
        .host(&session, "SPAWN grandchild nested work")
        .await
        .unwrap_err();

    assert_eq!(nested, "depth_exceeded");
    harness.stop().await;
}

#[tokio::test]
async fn admission_rules_are_enforced() {
    let harness = Harness::start("admission").await;
    let parent = opencode("ses_rules");
    let mcp = Target::new(TargetKind::Mcp, "mcp-1");

    harness.register(&parent, HostName::Opencode, true).await;
    harness.register(&mcp, HostName::Mcp, false).await;

    assert_eq!(
        harness.host(&mcp, "SPAWN x task").await.unwrap_err(),
        "unsupported_host"
    );
    assert_eq!(
        harness
            .host(&parent, "SPAWN Bad_Name task")
            .await
            .unwrap_err(),
        "invalid_request"
    );
    assert_eq!(
        harness.host(&parent, "SEND parent hi").await.unwrap_err(),
        "not_found"
    );
    assert_eq!(
        harness
            .host(&parent, "SEND child nobody hi")
            .await
            .unwrap_err(),
        "not_found"
    );

    harness.host(&parent, "SPAWN dup task").await.unwrap();
    assert_eq!(
        harness.host(&parent, "SPAWN dup task").await.unwrap_err(),
        "invalid_request"
    );

    for n in 1..16 {
        harness
            .host(&parent, &format!("SPAWN c{n} task"))
            .await
            .unwrap();
    }

    assert_eq!(
        harness.host(&parent, "SPAWN c16 task").await.unwrap_err(),
        "limit_exceeded"
    );

    let deleted = harness
        .host(&parent, r#"HOST rlm.delete_subagent {"name": "dup"}"#)
        .await
        .unwrap();

    assert_eq!(deleted["child"]["status"], "deleted");
    harness.host(&parent, "SPAWN dup task").await.unwrap();

    let listed = harness
        .host(&parent, "HOST rlm.list_subagents {}")
        .await
        .unwrap();

    assert_eq!(listed["children"].as_array().unwrap().len(), 17);
    harness.stop().await;
}

#[tokio::test]
async fn fail_and_delete_update_status_and_notify() {
    let harness = Harness::start("fail-delete").await;
    let parent = opencode("ses_parent");
    let session = opencode("ses_kid");

    harness.register(&parent, HostName::Opencode, true).await;

    let doomed = child(harness.host(&parent, "SPAWN doomed task").await.unwrap());
    let failed: ChildResult = harness
        .client
        .call(
            "children.fail",
            &json!({"target": parent, "child_id": doomed.child_id, "reason": "model unavailable"}),
        )
        .await
        .unwrap();

    assert_eq!(failed.child.status, ChildStatus::Failed);

    let notice = harness
        .events(&parent)
        .await
        .into_iter()
        .find(|event| event.id == format!("agent.message:{}:1", doomed.child_id))
        .unwrap();

    assert!(
        notice
            .text
            .ends_with("[recurse] child failed to start: model unavailable")
    );

    let bound = spawn_and_bind(&harness, &parent, "worker", &session).await;

    harness.register(&session, HostName::Opencode, true).await;
    harness.exec(&session, "warm").await.unwrap();

    let deleted: ChildResult = harness
        .client
        .call(
            "children.delete",
            &json!({"target": parent, "child_id": bound.child_id}),
        )
        .await
        .unwrap();

    assert_eq!(deleted.child.status, ChildStatus::Deleted);

    let status: KernelStatus = harness
        .client
        .call("kernel.status", &json!({"target": session}))
        .await
        .unwrap();

    assert_eq!(status.state, KernelState::Absent, "child kernel shut down");

    let registered: RegisterResult = harness.register(&session, HostName::Opencode, true).await;

    assert_eq!(
        (registered.depth, registered.parent),
        (0, None),
        "session unbound"
    );

    let listed: ChildrenListResult = harness
        .client
        .call("children.list", &json!({"target": parent}))
        .await
        .unwrap();

    assert_eq!(listed.children.len(), 2);

    let missing = harness
        .call(
            "children.bind",
            json!({"target": parent, "child_id": "c_nope", "session": session}),
        )
        .await
        .unwrap_err();

    assert_eq!(code_of(&missing), ErrorCode::NotFound);
    harness.stop().await;
}
