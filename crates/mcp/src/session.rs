use std::time::Duration;

use recurse_client::{ClientError, DaemonClient};
use recurse_protocol::{ErrorCode, HostInfo, HostName, RegisterParams, Target, TargetKind};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::OnceCell;

/// The MCP process's target and its lazily connected, registered daemon client.
pub struct Session {
    pub target: Target,
    cwd: String,
    client: OnceCell<DaemonClient>,
}

impl Session {
    pub fn new(target: Target) -> Self {
        let cwd = std::env::current_dir()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_else(|_| "/".into());

        Self {
            target,
            cwd,
            client: OnceCell::new(),
        }
    }

    /// Target `{kind: "mcp", id: RECURSE_TARGET_ID or "mcp-<pid>-<start ms>"}`.
    pub fn from_env() -> Self {
        let id = std::env::var("RECURSE_TARGET_ID")
            .ok()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| {
                let started = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_millis());

                format!("mcp-{}-{started}", std::process::id())
            });

        Self::new(Target::new(TargetKind::Mcp, id))
    }

    async fn client(&self) -> Result<&DaemonClient, ClientError> {
        self.client
            .get_or_try_init(|| async {
                let client = recurse_client::connect_or_spawn().await?;

                self.register(&client).await?;

                Ok(client)
            })
            .await
    }

    async fn register(&self, client: &DaemonClient) -> Result<(), ClientError> {
        let params = RegisterParams {
            target: self.target.clone(),
            cwd: self.cwd.clone(),
            host: HostInfo {
                name: HostName::Mcp,
                version: Some(env!("CARGO_PKG_VERSION").into()),
                supports_children: false,
            },
            model: None,
        };
        let _: serde_json::Value = client.call("target.register", &params).await?;

        Ok(())
    }

    /// Calls the daemon, re-registering once if it forgot this target.
    pub async fn call<P, R>(
        &self,
        method: &str,
        params: &P,
        timeout: Duration,
    ) -> Result<R, ClientError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let client = self.client().await?;

        match client.call_with_timeout(method, params, timeout).await {
            Err(error)
                if error.code() == Some(ErrorCode::NotFound) && method.starts_with("kernel.") =>
            {
                self.register(client).await?;
                client.call_with_timeout(method, params, timeout).await
            }
            outcome => outcome,
        }
    }
}
