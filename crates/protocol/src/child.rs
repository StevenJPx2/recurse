use serde::{Deserialize, Serialize};

use crate::Target;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildStatus {
    Pending,
    Running,
    Failed,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildInfo {
    pub child_id: String,
    pub name: String,
    pub task: String,
    pub model: Option<String>,
    pub status: ChildStatus,
    pub parent: Target,
    pub session: Option<Target>,
    pub session_dir: String,
    pub depth: u32,
    pub created_at: u64,
}

impl ChildInfo {
    pub fn is_deleted(&self) -> bool {
        self.status == ChildStatus::Deleted
    }

    /// The session this child is bound to, if it is running.
    pub fn bound_session(&self) -> Option<&Target> {
        match self.status {
            ChildStatus::Running => self.session.as_ref(),
            ChildStatus::Pending | ChildStatus::Failed | ChildStatus::Deleted => None,
        }
    }
}
