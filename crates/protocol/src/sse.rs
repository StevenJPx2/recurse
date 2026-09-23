use serde::{Deserialize, Serialize};

use crate::{Event, Target};

/// One `data: <json>` frame on `GET /events`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SseFrame {
    Subscribed { target: Target },
    Event { target: Target, events: Vec<Event> },
    Heartbeat { at: u64 },
}
