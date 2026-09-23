//! Typed client for a recurse daemon.

mod error;
mod spawn;
mod sse;

use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use recurse_protocol::{HealthResult, PROTOCOL_VERSION, RpcError, RpcRequest, RpcResponse, Target};
use serde::Serialize;
use serde::de::DeserializeOwned;

pub use error::ClientError;
pub use spawn::connect_or_spawn;
pub use sse::EventStream;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

pub fn default_url() -> String {
    std::env::var("RECURSE_DAEMON_URL").unwrap_or_else(|_| {
        let port = std::env::var("RECURSE_DAEMON_PORT")
            .ok()
            .and_then(|port| port.parse::<u16>().ok())
            .unwrap_or(recurse_protocol::DEFAULT_PORT);

        format!("http://127.0.0.1:{port}")
    })
}

/// `base64url(JSON target)` for `GET /events?target=`.
pub fn encode_target(target: &Target) -> String {
    let json = serde_json::to_vec(target).unwrap_or_default();

    URL_SAFE_NO_PAD.encode(json)
}

#[derive(Clone)]
pub struct DaemonClient {
    base_url: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl DaemonClient {
    pub fn new(base_url: impl Into<String>, token: Option<String>) -> Result<Self, ClientError> {
        let http = reqwest::Client::builder()
            .no_proxy()
            .build()
            .map_err(|error| ClientError::Transport(format!("build HTTP client: {error}")))?;

        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_owned(),
            token,
            http,
        })
    }

    pub fn from_env() -> Result<Self, ClientError> {
        Self::new(default_url(), std::env::var("RECURSE_DAEMON_TOKEN").ok())
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Calls `health` and rejects a daemon speaking another protocol version.
    pub async fn health(&self) -> Result<HealthResult, ClientError> {
        let health: HealthResult = self
            .call_with_timeout("health", &serde_json::json!({}), Duration::from_secs(2))
            .await?;

        if health.protocol != PROTOCOL_VERSION {
            return Err(ClientError::Protocol(format!(
                "daemon speaks protocol {}, this client speaks {PROTOCOL_VERSION}",
                health.protocol
            )));
        }

        Ok(health)
    }

    pub async fn call<P, R>(&self, method: &str, params: &P) -> Result<R, ClientError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        self.call_with_timeout(method, params, DEFAULT_TIMEOUT)
            .await
    }

    pub async fn call_with_timeout<P, R>(
        &self,
        method: &str,
        params: &P,
        timeout: Duration,
    ) -> Result<R, ClientError>
    where
        P: Serialize,
        R: DeserializeOwned,
    {
        let params = serde_json::to_value(params)
            .map_err(|error| ClientError::Protocol(format!("encode params: {error}")))?;
        let body = RpcRequest {
            id: Some(serde_json::json!(1)),
            method: method.to_owned(),
            params: Some(params),
        };
        let request = self
            .authorize(self.http.post(format!("{}/rpc", self.base_url)))
            .timeout(timeout)
            .json(&body);
        let response = request
            .send()
            .await
            .map_err(|error| ClientError::Transport(format!("call daemon: {error}")))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|error| ClientError::Transport(format!("read daemon response: {error}")))?;

        decode_response(status, &text)
    }

    /// Opens `GET /events` for `target`.
    pub async fn subscribe(&self, target: &Target) -> Result<EventStream, ClientError> {
        let url = format!("{}/events?target={}", self.base_url, encode_target(target));
        let response = self
            .authorize(self.http.get(url))
            .send()
            .await
            .map_err(|error| ClientError::Transport(format!("subscribe: {error}")))?;
        let status = response.status();

        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();

            return Err(decode_response::<serde_json::Value>(status, &text)
                .err()
                .unwrap_or_else(|| ClientError::Transport(format!("subscribe: HTTP {status}"))));
        }

        Ok(EventStream::new(response))
    }

    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }
}

fn decode_response<R: DeserializeOwned>(
    status: reqwest::StatusCode,
    text: &str,
) -> Result<R, ClientError> {
    let envelope: RpcResponse = serde_json::from_str(text).map_err(|error| {
        ClientError::Protocol(format!("decode daemon response (HTTP {status}): {error}"))
    })?;

    if let Some(error) = envelope.error {
        return Err(ClientError::Rpc(error));
    }

    let value = envelope.result.ok_or_else(|| {
        ClientError::Protocol(format!("daemon response has no result (HTTP {status})"))
    })?;

    serde_json::from_value(value)
        .map_err(|error| ClientError::Protocol(format!("decode daemon result: {error}")))
}

impl From<RpcError> for ClientError {
    fn from(error: RpcError) -> Self {
        Self::Rpc(error)
    }
}
