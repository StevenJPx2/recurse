//! `POST /rpc` envelope, error codes, and method params/results (§1.1, §2).

use std::fmt;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::host::parse_params;
use crate::{ChildInfo, EmptyParams, Event, HandleInfo, Target};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RpcResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<RpcError>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    Unauthorized,
    NotFound,
    Busy,
    KernelUnavailable,
    UnsupportedHost,
    DepthExceeded,
    LimitExceeded,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RpcError {
    pub code: ErrorCode,
    pub message: String,
}

impl RpcError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl fmt::Display for RpcError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = serde_json::to_value(self.code)
            .ok()
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();

        write!(formatter, "{code}: {}", self.message)
    }
}

impl std::error::Error for RpcError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostName {
    Opencode,
    Pi,
    Mcp,
    Cli,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HostInfo {
    pub name: HostName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub supports_children: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetParams {
    pub target: Target,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RegisterParams {
    pub target: Target,
    pub cwd: String,
    pub host: HostInfo,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExecuteParams {
    pub target: Target,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_sec: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildBindParams {
    pub target: Target,
    pub child_id: String,
    pub session: Target,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildFailParams {
    pub target: Target,
    pub child_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChildIdParams {
    pub target: Target,
    pub child_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventsAckParams {
    pub target: Target,
    pub event_ids: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsListParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<Target>,
}

/// A parsed RPC request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Health,
    TargetRegister(RegisterParams),
    KernelExecute(ExecuteParams),
    KernelInterrupt(TargetParams),
    KernelRestart(TargetParams),
    KernelStatus(TargetParams),
    ChildrenList(TargetParams),
    ChildrenBind(ChildBindParams),
    ChildrenFail(ChildFailParams),
    ChildrenDelete(ChildIdParams),
    EventsAck(EventsAckParams),
    EventsList(TargetParams),
    SkillsList(SkillsListParams),
}

impl Request {
    pub fn parse(method: &str, params: Option<Value>) -> Result<Self, RpcError> {
        let params = params.unwrap_or(Value::Null);
        let request = match method {
            "health" => parse_params::<EmptyParams>(params).map(|_| Self::Health)?,
            "target.register" => Self::TargetRegister(parse_params(params)?),
            "kernel.execute" => Self::KernelExecute(parse_params(params)?),
            "kernel.interrupt" => Self::KernelInterrupt(parse_params(params)?),
            "kernel.restart" => Self::KernelRestart(parse_params(params)?),
            "kernel.status" => Self::KernelStatus(parse_params(params)?),
            "children.list" => Self::ChildrenList(parse_params(params)?),
            "children.bind" => Self::ChildrenBind(parse_params(params)?),
            "children.fail" => Self::ChildrenFail(parse_params(params)?),
            "children.delete" => Self::ChildrenDelete(parse_params(params)?),
            "events.ack" => Self::EventsAck(parse_params(params)?),
            "events.list" => Self::EventsList(parse_params(params)?),
            "skills.list" => Self::SkillsList(parse_params(params)?),
            other => {
                return Err(RpcError::new(
                    crate::ErrorCode::InvalidRequest,
                    format!("unknown method {other}"),
                ));
            }
        };

        Ok(request)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthResult {
    pub ok: bool,
    pub name: String,
    pub version: String,
    pub protocol: u32,
    pub pid: u32,
    pub started_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterResult {
    pub target: Target,
    pub depth: u32,
    pub parent: Option<Target>,
    pub child: Option<ChildInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptResult {
    pub interrupted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartResult {
    pub restarted: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KernelState {
    Absent,
    Starting,
    Idle,
    Busy,
    Dead,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelStatus {
    pub state: KernelState,
    pub pid: Option<u32>,
    pub execution_count: u64,
    pub started_at: Option<u64>,
    pub cwd: String,
    pub handles: Vec<HandleInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildrenListResult {
    pub children: Vec<ChildInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildResult {
    pub child: ChildInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AckResult {
    pub acked: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventsListResult {
    pub events: Vec<Event>,
}
