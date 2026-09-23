mod common;

use std::path::PathBuf;

use common::{Harness, TempDir, code_of, opencode, options};
use recurse_client::DaemonClient;
use recurse_daemon::Daemon;
use recurse_protocol::{
    CellStatus, ChildStatus, ChildrenListResult, ErrorCode, HealthResult, HostName,
    SkillsListResult, Target, TargetKind,
};
use serde_json::json;

#[tokio::test]
async fn health_reports_the_protocol() {
    let harness = Harness::start("health").await;
    let health: HealthResult = harness.client.health().await.unwrap();

    assert!(health.ok);
    assert_eq!((health.name.as_str(), health.protocol), ("recurse", 1));
    assert_eq!(health.pid, std::process::id());
    harness.stop().await;
}

#[tokio::test]
async fn registry_and_events_survive_a_restart() {
    let harness = Harness::start("persist").await;
    let parent = opencode("ses_persist");

    harness.register(&parent, HostName::Opencode, true).await;
    harness.host(&parent, "SPAWN keeper task").await.unwrap();
    harness.host(&parent, "NOTICE h9").await.unwrap();

    let before = harness.events(&parent).await;
    let dir = harness.stop().await;
    let state = dir.path().join("state");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mode = |path: PathBuf| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;

        assert_eq!(mode(state.clone()), 0o700);
        assert_eq!(mode(state.join("registry.json")), 0o600);
        assert_eq!(mode(state.join("events.json")), 0o600);
        assert!(!state.join("daemon.lock").exists(), "lock released");
    }

    let restarted = options(dir.path());
    let harness = Harness::start_with(dir, restarted).await;
    let children: ChildrenListResult = harness
        .client
        .call("children.list", &json!({"target": parent}))
        .await
        .unwrap();

    assert_eq!(children.children.len(), 1);
    assert_eq!(children.children[0].status, ChildStatus::Pending);
    assert_eq!(harness.events(&parent).await, before);
    assert_eq!(
        harness
            .exec(&parent, "still registered")
            .await
            .unwrap()
            .status,
        CellStatus::Ok
    );
    harness.stop().await;
}

#[tokio::test]
async fn a_second_daemon_on_the_same_state_dir_is_refused() {
    let harness = Harness::start("lock").await;
    let error = Daemon::start(options(harness.dir.path()))
        .await
        .err()
        .unwrap();

    assert!(error.contains("another recurse daemon"), "{error}");
    harness.stop().await;
}

#[tokio::test]
async fn bearer_token_is_required_when_configured() {
    let dir = TempDir::new("token");
    let mut options = options(dir.path());

    options.token = Some("s3cret".into());

    let harness = Harness::start_with(dir, options).await;
    let anonymous = DaemonClient::new(harness.daemon.url(), None).unwrap();
    let error = anonymous.health().await.unwrap_err();

    assert_eq!(code_of(&error), ErrorCode::Unauthorized);

    let target = Target::new(TargetKind::Cli, "x");
    let error = anonymous.subscribe(&target).await.err().unwrap();

    assert_eq!(code_of(&error), ErrorCode::Unauthorized);
    assert!(harness.client.health().await.is_ok());
    assert!(harness.client.subscribe(&target).await.is_ok());
    harness.stop().await;
}

#[tokio::test]
async fn malformed_requests_are_invalid() {
    let harness = Harness::start("invalid").await;
    let url = format!("{}/rpc", harness.daemon.url());
    let http = reqwest::Client::new();

    for (body, message) in [
        ("{", "malformed JSON"),
        (r#"{"id": 3, "method": "nope"}"#, "unknown method nope"),
        (
            r#"{"id": 3, "method": "health", "params": {"x": 1}}"#,
            "invalid params",
        ),
        (
            r#"{"id": 3, "method": "health", "extra": 1}"#,
            "invalid request",
        ),
        (
            r#"{"method": "kernel.status", "params": {"target": {"kind": "cli", "id": ""}}}"#,
            "1-256",
        ),
    ] {
        let response = http.post(&url).body(body).send().await.unwrap();

        assert_eq!(response.status(), 400, "{body}");

        let reply: serde_json::Value = response.json().await.unwrap();

        assert_eq!(reply["error"]["code"], "invalid_request", "{body}");
        assert!(
            reply["error"]["message"]
                .as_str()
                .unwrap()
                .contains(message),
            "{reply}"
        );
    }

    let huge = format!(
        r#"{{"method": "health", "pad": "{}"}}"#,
        "x".repeat(1024 * 1024)
    );
    let reply: serde_json::Value = http
        .post(&url)
        .body(huge)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    assert_eq!(reply["error"]["code"], "limit_exceeded");
    harness.stop().await;
}

#[tokio::test]
async fn skills_are_discovered_with_project_shadowing() {
    let harness = Harness::start("skills").await;
    let target = opencode("ses_skills");
    let write = |root: PathBuf, description: &str| {
        let dir = root.join("audit");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: audit\ndescription: {description}\npython: audit:main\n---\n"),
        )
        .unwrap();
    };

    write(harness.dir.path().join("config/skills"), "user");
    write(harness.dir.path().join(".recurse/skills"), "project");
    harness.register(&target, HostName::Opencode, true).await;

    let global: SkillsListResult = harness
        .client
        .call("skills.list", &json!({}))
        .await
        .unwrap();
    let project: SkillsListResult = harness
        .client
        .call("skills.list", &json!({"target": target}))
        .await
        .unwrap();

    assert_eq!(global.skills[0].description, "user");
    assert_eq!(project.skills[0].description, "project");
    assert_eq!(project.skills[0].python.as_deref(), Some("audit:main"));

    let from_kernel = harness.host(&target, "HOST skills.list {}").await.unwrap();

    assert_eq!(from_kernel["skills"][0]["description"], "project");
    harness.stop().await;
}

#[tokio::test]
async fn idle_exit_stops_the_daemon_and_releases_the_lock() {
    let dir = TempDir::new("idle");
    let mut options = options(dir.path());

    options.idle_exit_sec = 1;

    let daemon = Daemon::start(options).await.unwrap();

    tokio::time::timeout(std::time::Duration::from_secs(10), daemon.wait())
        .await
        .expect("idle exit within 10s");
    assert!(!dir.path().join("state/daemon.lock").exists());
}

#[tokio::test]
async fn real_kernel_runs_a_cell_when_present() {
    let python = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../python");

    if !python.join("recurse_kernel/__main__.py").is_file() {
        eprintln!("skipping: python/recurse_kernel is not in the repository yet");
        return;
    }

    let dir = TempDir::new("real");
    let mut options = options(dir.path());

    options.kernel_path = Some(python.canonicalize().unwrap());

    let harness = Harness::start_with(dir, options).await;
    let target = Target::new(TargetKind::Cli, "real");

    harness.register(&target, HostName::Cli, false).await;

    let cell = harness.exec(&target, "print(1+1)").await.unwrap();

    assert_eq!(cell.status, CellStatus::Ok, "{cell:?}");
    assert_eq!(cell.stdout, "2\n");
    harness.stop().await;
}
