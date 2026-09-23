//! Serde types for every wire contract in `docs/protocol.md`.

pub mod bounds;
mod cell;
mod child;
mod event;
mod host;
mod kernel;
mod rpc;
mod skill;
mod sse;
mod target;

pub use cell::{CellError, CellResult, CellStatus};
pub use child::{ChildInfo, ChildStatus};
pub use event::{
    AgentMessagePayload, BashFinishedPayload, ChildSpawnPayload, Event, EventBody,
    KernelExitedPayload, MessageFrom, Role,
};
pub use host::{
    AgentMessageParams, BashFinishedParams, DeleteSubagentParams, EmptyParams, EventIdResult,
    HostCall, SpawnParams, WithdrawParams, WithdrawResult,
};
pub use kernel::{FromKernel, HandleInfo, ToKernel};
pub use rpc::{
    AckResult, ChildBindParams, ChildFailParams, ChildIdParams, ChildResult, ChildrenListResult,
    ErrorCode, EventsAckParams, EventsListResult, ExecuteParams, HealthResult, HostInfo, HostName,
    InterruptResult, KernelState, KernelStatus, RegisterParams, RegisterResult, Request,
    RestartResult, RpcError, RpcRequest, RpcResponse, SkillsListParams, TargetParams,
};
pub use skill::{SkillInfo, SkillsListResult};
pub use sse::SseFrame;
pub use target::{Target, TargetKind};

pub const PROTOCOL_VERSION: u32 = 1;
pub const DAEMON_NAME: &str = "recurse";
pub const DEFAULT_PORT: u16 = 18_791;
pub const KERNEL_PROTOCOL_VERSION: u32 = 1;

/// Returns true when `name` matches `^[a-z0-9][a-z0-9-]{0,62}$` (child and skill names).
pub fn is_valid_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    let lower_digit = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();

    name.len() <= 63 && lower_digit(first) && chars.all(|c| lower_digit(c) || c == '-')
}

#[cfg(test)]
mod tests {
    use super::is_valid_name;

    #[test]
    fn names_follow_the_pattern() {
        assert!(is_valid_name("auth-reviewer"));
        assert!(is_valid_name("a"));
        assert!(is_valid_name(&"a".repeat(63)));
        assert!(!is_valid_name(&"a".repeat(64)));
        assert!(!is_valid_name("-x"));
        assert!(!is_valid_name("Upper"));
        assert!(!is_valid_name(""));
        assert!(!is_valid_name("a_b"));
    }
}
