#![allow(
    dead_code,
    reason = "each integration test binary uses a subset of the helpers"
)]

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use recurse_client::{ClientError, DaemonClient};
use recurse_daemon::{Daemon, DaemonOptions};
use recurse_protocol::{
    CellResult, ErrorCode, Event, EventsListResult, HostInfo, HostName, RegisterParams,
    RegisterResult, Target, TargetKind,
};
use serde_json::{Value, json};

static NEXT: AtomicUsize = AtomicUsize::new(0);

/// A fresh temporary directory, removed on drop.
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(label: &str) -> Self {
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("recurse-test-{}-{n}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();

        Self(path.canonicalize().unwrap())
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn fake_kernel() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/fake_kernel")
}

pub fn options(state: &Path) -> DaemonOptions {
    let mut options = DaemonOptions::new(state.join("state"));

    options.port = 0;
    options.config_dir = state.join("config");
    options.kernel_path = Some(fake_kernel());
    options.skills_dir = None;
    options.log_to_stderr = false;
    options.interrupt_grace = Duration::from_millis(500);
    options.shutdown_grace = Duration::from_secs(2);
    options
}

pub struct Harness {
    pub dir: TempDir,
    pub daemon: Daemon,
    pub client: DaemonClient,
}

impl Harness {
    pub async fn start(label: &str) -> Self {
        let dir = TempDir::new(label);
        let daemon = Daemon::start(options(dir.path())).await.unwrap();
        let client = DaemonClient::new(daemon.url(), None).unwrap();

        Self {
            dir,
            daemon,
            client,
        }
    }

    pub async fn start_with(dir: TempDir, options: DaemonOptions) -> Self {
        let token = options.token.clone();
        let daemon = Daemon::start(options).await.unwrap();
        let client = DaemonClient::new(daemon.url(), token).unwrap();

        Self {
            dir,
            daemon,
            client,
        }
    }

    pub fn cwd(&self) -> String {
        self.dir.path().to_string_lossy().into_owned()
    }

    pub async fn register(
        &self,
        target: &Target,
        host: HostName,
        children: bool,
    ) -> RegisterResult {
        let params = RegisterParams {
            target: target.clone(),
            cwd: self.cwd(),
            host: HostInfo {
                name: host,
                version: None,
                supports_children: children,
            },
            model: Some("anthropic/claude-sonnet-5".into()),
        };

        self.client.call("target.register", &params).await.unwrap()
    }

    pub async fn exec(&self, target: &Target, code: &str) -> Result<CellResult, ClientError> {
        self.exec_timeout(target, code, None).await
    }

    pub async fn exec_timeout(
        &self,
        target: &Target,
        code: &str,
        timeout_sec: Option<u64>,
    ) -> Result<CellResult, ClientError> {
        let mut params = json!({"target": target, "code": code});

        if let Some(timeout) = timeout_sec {
            params["timeout_sec"] = json!(timeout);
        }

        self.client
            .call_with_timeout("kernel.execute", &params, Duration::from_secs(60))
            .await
    }

    /// Runs a host-request cell and returns the parsed JSON result or the error code.
    pub async fn host(&self, target: &Target, code: &str) -> Result<Value, String> {
        let cell = self.exec(target, code).await.unwrap();

        match cell.error {
            Some(error) => Err(error.ename),
            None => Ok(serde_json::from_str(cell.result.as_deref().unwrap()).unwrap()),
        }
    }

    pub async fn events(&self, target: &Target) -> Vec<Event> {
        let result: EventsListResult = self
            .client
            .call("events.list", &json!({"target": target}))
            .await
            .unwrap();

        result.events
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, ClientError> {
        self.client.call(method, &params).await
    }

    pub async fn stop(self) -> TempDir {
        self.daemon.stop().await;
        self.dir
    }
}

pub fn opencode(id: &str) -> Target {
    Target::new(TargetKind::OpencodeSession, id)
}

pub fn code_of(error: &ClientError) -> ErrorCode {
    error
        .code()
        .unwrap_or_else(|| panic!("not an RPC error: {error}"))
}
