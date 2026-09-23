//! Host requests (kernel → daemon, §4.1).

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{ErrorCode, Role, RpcError};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyParams {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnParams {
    pub task: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeleteSubagentParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentMessageParams {
    pub message: String,
    pub receiver_role: Role,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receiver_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BashFinishedParams {
    pub handle_id: String,
    pub pid: u32,
    pub exit_code: Option<i32>,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WithdrawParams {
    pub event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventIdResult {
    pub event_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WithdrawResult {
    pub withdrawn: bool,
}

/// A parsed host request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCall {
    Spawn(SpawnParams),
    ListSubagents,
    DeleteSubagent(DeleteSubagentParams),
    AgentMessage(AgentMessageParams),
    BashFinished(BashFinishedParams),
    Withdraw(WithdrawParams),
    SkillsList,
}

impl HostCall {
    pub fn parse(method: &str, params: Value) -> Result<Self, RpcError> {
        let call = match method {
            "rlm.spawn" => Self::Spawn(parse_params(params)?),
            "rlm.list_subagents" => empty(params).map(|()| Self::ListSubagents)?,
            "rlm.delete_subagent" => Self::DeleteSubagent(parse_params(params)?),
            "agent_message.send" => Self::AgentMessage(parse_params(params)?),
            "notice.bash_finished" => Self::BashFinished(parse_params(params)?),
            "notice.withdraw" => Self::Withdraw(parse_params(params)?),
            "skills.list" => empty(params).map(|()| Self::SkillsList)?,
            other => {
                return Err(RpcError::new(
                    ErrorCode::InvalidRequest,
                    format!("unknown host method {other}"),
                ));
            }
        };

        Ok(call)
    }

    pub const fn method(&self) -> &'static str {
        match self {
            Self::Spawn(_) => "rlm.spawn",
            Self::ListSubagents => "rlm.list_subagents",
            Self::DeleteSubagent(_) => "rlm.delete_subagent",
            Self::AgentMessage(_) => "agent_message.send",
            Self::BashFinished(_) => "notice.bash_finished",
            Self::Withdraw(_) => "notice.withdraw",
            Self::SkillsList => "skills.list",
        }
    }
}

/// Parses method params, treating a missing (`null`) value as `{}`.
pub(crate) fn parse_params<T: DeserializeOwned>(params: Value) -> Result<T, RpcError> {
    let params = if params.is_null() {
        Value::Object(serde_json::Map::new())
    } else {
        params
    };

    serde_json::from_value(params).map_err(|error| {
        RpcError::new(
            ErrorCode::InvalidRequest,
            format!("invalid params: {error}"),
        )
    })
}

fn empty(params: Value) -> Result<(), RpcError> {
    parse_params::<EmptyParams>(params).map(|_| ())
}
