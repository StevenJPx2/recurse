//! End-to-end: the binary auto-spawns a daemon, runs cells via `exec` and `mcp`, and the daemon
//! shuts down cleanly on SIGTERM.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Env {
    root: PathBuf,
    port: u16,
}

impl Env {
    fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!("recurse-cli-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();

        Self { root, port }
    }

    fn command(&self, args: &[&str]) -> Command {
        let kernel =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../daemon/tests/fixtures/fake_kernel");
        let mut command = Command::new(env!("CARGO_BIN_EXE_recurse"));

        command
            .args(args)
            .current_dir(&self.root)
            .env_remove("RECURSE_DAEMON_URL")
            .env_remove("RECURSE_DAEMON_TOKEN")
            .env("RECURSE_DAEMON_PORT", self.port.to_string())
            .env("RECURSE_STATE_DIR", self.root.join("state"))
            .env("RECURSE_CONFIG_DIR", self.root.join("config"))
            .env("RECURSE_KERNEL_PATH", kernel)
            .env("RECURSE_SKILLS_DIR", self.root.join("skills"));
        command
    }

    fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    fn daemon_pid(&self) -> Option<i32> {
        let output = self.run(&["health"]);
        let health: Value = serde_json::from_slice(&output.stdout).ok()?;

        health["pid"]
            .as_i64()
            .and_then(|pid| i32::try_from(pid).ok())
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        if let Some(pid) = self.daemon_pid() {
            // SAFETY: sending SIGTERM to the daemon this test spawned.
            unsafe { libc::kill(pid, libc::SIGTERM) };
            wait_until(|| !self.root.join("state/daemon.lock").exists());
        }

        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn wait_until(mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);

    while Instant::now() < deadline {
        if done() {
            return true;
        }

        std::thread::sleep(Duration::from_millis(50));
    }

    false
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[test]
fn exec_auto_spawns_the_daemon_and_sigterm_stops_it() {
    let env = Env::new("exec");

    assert!(!env.run(&["health"]).status.success(), "no daemon yet");

    let output = env.run(&["exec", "--target-id", "box", "-c", "PRINT hi"]);

    assert!(output.status.success(), "{}", text(&output.stderr));
    assert_eq!(text(&output.stdout), "hi\n");

    let failed = env.run(&["exec", "--target-id", "box", "-c", "HOST nope.method {}"]);

    assert_eq!(failed.status.code(), Some(1));
    assert!(
        text(&failed.stderr).contains("invalid_request"),
        "{}",
        text(&failed.stderr)
    );

    let status: Value =
        serde_json::from_slice(&env.run(&["kernel", "status", "--target-id", "box"]).stdout)
            .unwrap();

    assert_eq!(status["state"], "idle");
    assert_eq!(status["execution_count"], 2);

    let events: Value =
        serde_json::from_slice(&env.run(&["events", "list", "--target-id", "box"]).stdout).unwrap();

    assert_eq!(events["events"], json!([]));

    let pid = env.daemon_pid().unwrap();

    // SAFETY: sending SIGTERM to the daemon this test spawned.
    unsafe { libc::kill(pid, libc::SIGTERM) };
    assert!(
        wait_until(|| !env.root.join("state/daemon.lock").exists()),
        "lock released"
    );
    assert!(
        std::fs::read_to_string(env.root.join("state/daemon.log"))
            .unwrap()
            .contains("daemon stopped")
    );
}

#[test]
fn skills_commands_list_get_and_path() {
    let env = Env::new("skills");
    let skill = env.root.join("skills/core");

    std::fs::create_dir_all(skill.join("references")).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: core\ndescription: Core usage.\n---\nBody.\n",
    )
    .unwrap();
    std::fs::write(skill.join("references/api.md"), "API notes.\n").unwrap();

    let hidden = env.root.join(".recurse/skills/secret");

    std::fs::create_dir_all(&hidden).unwrap();
    std::fs::write(
        hidden.join("SKILL.md"),
        "---\nname: secret\ndescription: x\nhidden: true\n---\n",
    )
    .unwrap();

    assert_eq!(
        text(&env.run(&["skills", "list"]).stdout),
        "core\tCore usage.\n"
    );

    let get = text(&env.run(&["skills", "get", "core"]).stdout);

    assert!(get.starts_with("# core\n\n---\nname: core"));
    assert!(!get.contains("API notes"));

    let full = text(&env.run(&["skills", "get", "core", "--full"]).stdout);

    assert!(full.contains("## references/api.md\n\nAPI notes."));
    assert!(
        text(&env.run(&["skills", "path", "secret"]).stdout)
            .trim_end()
            .ends_with("secret")
    );
    assert!(!env.run(&["skills", "get", "missing"]).status.success());
}

fn mcp_call(stdin: &mut impl Write, reader: &mut impl BufRead, request: &Value) -> Value {
    writeln!(stdin, "{request}").unwrap();
    stdin.flush().unwrap();

    let mut line = String::new();

    reader.read_line(&mut line).unwrap();
    serde_json::from_str(&line).unwrap()
}

#[test]
fn mcp_runs_ipython_against_its_own_target() {
    let env = Env::new("mcp");
    let mut child = env
        .command(&["mcp"])
        .env("RECURSE_TARGET_ID", "mcp-test")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let init = mcp_call(
        &mut stdin,
        &mut reader,
        &json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
    );

    assert_eq!(init["result"]["serverInfo"]["name"], "recurse");

    let call = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
        "params": {"name": "ipython", "arguments": {"code": "SPAWN kid task"}}});
    let reply = mcp_call(&mut stdin, &mut reader, &call);
    let rendered = reply["result"]["content"][0]["text"].as_str().unwrap();

    assert_eq!(reply["result"]["isError"], true);
    assert!(rendered.contains("error: unsupported_host"), "{rendered}");

    let status = json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
        "params": {"name": "kernel_status", "arguments": {}}});
    let reply = mcp_call(&mut stdin, &mut reader, &status);

    assert!(
        reply["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("\"state\": \"idle\"")
    );

    drop(stdin);
    assert!(child.wait().unwrap().success());
    assert!(Path::new(&env.root).join("state/registry.json").exists());
}
