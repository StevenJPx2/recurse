use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CellResult, RpcError};

/// Daemon → kernel protocol line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToKernel {
    Execute {
        id: String,
        code: String,
        timeout_ms: u64,
    },
    Interrupt,
    HostResponse {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        result: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<RpcError>,
    },
    Shutdown,
}

/// Kernel → daemon protocol line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FromKernel {
    Ready {
        pid: u32,
        python: String,
        protocol: u32,
    },
    ExecuteResult {
        id: String,
        result: CellResult,
    },
    HostRequest {
        id: String,
        method: String,
        #[serde(default)]
        params: Value,
    },
    Handles {
        handles: Vec<HandleInfo>,
    },
    Log {
        level: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandleInfo {
    pub handle_id: String,
    pub pid: u32,
    pub command: String,
    pub running: bool,
}
