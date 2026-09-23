use serde::{Deserialize, Serialize};

use crate::bounds::{self, clip_tail};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CellStatus {
    Ok,
    Error,
    Timeout,
    Interrupted,
}

impl CellStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Timeout => "timeout",
            Self::Interrupted => "interrupted",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellError {
    pub ename: String,
    pub evalue: String,
    pub traceback: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellResult {
    pub status: CellStatus,
    pub stdout: String,
    pub stderr: String,
    pub result: Option<String>,
    pub error: Option<CellError>,
    pub execution_count: u64,
    pub duration_ms: u64,
    pub truncated: bool,
}

impl CellResult {
    /// A result with no output, used when the kernel never answered.
    pub fn empty(status: CellStatus, execution_count: u64, duration_ms: u64) -> Self {
        Self {
            status,
            stdout: String::new(),
            stderr: String::new(),
            result: None,
            error: None,
            execution_count,
            duration_ms,
            truncated: false,
        }
    }

    /// Clips every field to its bound, setting `truncated` if anything was cut.
    pub fn enforce_bounds(&mut self) {
        let mut clipped = clip_tail(&mut self.stdout, bounds::CELL_STREAM);
        clipped |= clip_tail(&mut self.stderr, bounds::CELL_STREAM);

        if let Some(result) = self.result.as_mut() {
            clipped |= bounds::clip_head(result, bounds::CELL_RESULT);
        }

        if let Some(error) = self.error.as_mut() {
            clipped |= clip_tail(&mut error.traceback, bounds::TRACEBACK);
            clipped |= bounds::clip_head(&mut error.evalue, bounds::CELL_RESULT);
        }

        self.truncated |= clipped;
    }
}
