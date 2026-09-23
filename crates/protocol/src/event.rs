use serde::{Deserialize, Serialize};

use crate::{ChildInfo, Target};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub target: Target,
    #[serde(flatten)]
    pub body: EventBody,
    pub actionable: bool,
    pub at: u64,
    pub text: String,
}

impl Event {
    pub const fn kind(&self) -> &'static str {
        self.body.kind()
    }
}

/// The `kind` + `payload` pair of an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "payload")]
pub enum EventBody {
    #[serde(rename = "child.spawn")]
    ChildSpawn(ChildSpawnPayload),
    #[serde(rename = "agent.message")]
    AgentMessage(AgentMessagePayload),
    #[serde(rename = "bash.finished")]
    BashFinished(BashFinishedPayload),
    #[serde(rename = "kernel.exited")]
    KernelExited(KernelExitedPayload),
}

impl EventBody {
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::ChildSpawn(_) => "child.spawn",
            Self::AgentMessage(_) => "agent.message",
            Self::BashFinished(_) => "bash.finished",
            Self::KernelExited(_) => "kernel.exited",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildSpawnPayload {
    pub child: ChildInfo,
    pub prompt: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Parent,
    Child,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageFrom {
    pub role: Role,
    pub name: Option<String>,
    pub child_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentMessagePayload {
    pub from: MessageFrom,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BashFinishedPayload {
    pub handle_id: String,
    pub pid: u32,
    pub exit_code: Option<i32>,
    pub command: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KernelExitedPayload {
    pub pid: u32,
    pub exit_code: Option<i32>,
    pub signal: Option<i32>,
}
