use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TargetKind {
    OpencodeSession,
    PiSession,
    Mcp,
    Cli,
}

impl TargetKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::OpencodeSession => "opencode-session",
            Self::PiSession => "pi-session",
            Self::Mcp => "mcp",
            Self::Cli => "cli",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        [Self::OpencodeSession, Self::PiSession, Self::Mcp, Self::Cli]
            .into_iter()
            .find(|kind| kind.as_str() == value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Target {
    pub kind: TargetKind,
    pub id: String,
}

impl Target {
    pub fn new(kind: TargetKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }

    /// The `"<kind>:<id>"` key that owns a kernel, queue, and children.
    pub fn key(&self) -> String {
        self.to_string()
    }

    pub fn is_valid(&self) -> bool {
        (1..=crate::bounds::TARGET_ID).contains(&self.id.chars().count())
    }
}

impl fmt::Display for Target {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}:{}", self.kind.as_str(), self.id)
    }
}
