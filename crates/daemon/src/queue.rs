//! Per-target replay-until-ack event queue with the 256-event drop policy.

use std::collections::{BTreeMap, HashSet};

use recurse_protocol::{Event, Target, bounds};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedEvent {
    pub event: Event,
    /// Set once an SSE listener has been handed the event; `notice.withdraw` then refuses.
    #[serde(default)]
    pub delivered: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EventQueue {
    targets: BTreeMap<String, Vec<QueuedEvent>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct QueueFile {
    #[serde(default)]
    pub events: Vec<QueuedEvent>,
}

impl EventQueue {
    pub fn from_file(file: QueueFile) -> Self {
        let mut queue = Self::default();

        for queued in file.events {
            queue
                .targets
                .entry(queued.event.target.key())
                .or_default()
                .push(queued);
        }

        queue
    }

    pub fn to_file(&self) -> QueueFile {
        QueueFile {
            events: self.targets.values().flatten().cloned().collect(),
        }
    }

    /// Queues `event` unless one with the same id is already queued. Returns the id.
    pub fn push(&mut self, event: Event) -> String {
        let id = event.id.clone();
        let queue = self.targets.entry(event.target.key()).or_default();

        if queue.iter().any(|queued| queued.event.id == id) {
            return id;
        }

        queue.push(QueuedEvent {
            event,
            delivered: false,
        });

        while queue.len() > bounds::QUEUED_EVENTS {
            let drop_at = queue
                .iter()
                .position(|queued| !queued.event.actionable)
                .unwrap_or(0);

            queue.remove(drop_at);
        }

        id
    }

    pub fn list(&self, target: &Target) -> Vec<Event> {
        self.targets
            .get(&target.key())
            .map(|queue| queue.iter().map(|queued| queued.event.clone()).collect())
            .unwrap_or_default()
    }

    pub fn ack(&mut self, target: &Target, ids: &[String]) -> usize {
        let Some(queue) = self.targets.get_mut(&target.key()) else {
            return 0;
        };
        let ids: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let before = queue.len();

        queue.retain(|queued| !ids.contains(queued.event.id.as_str()));

        let acked = before.saturating_sub(queue.len());

        if queue.is_empty() {
            self.targets.remove(&target.key());
        }

        acked
    }

    /// Removes an undelivered event. Returns false if it is gone or a listener already has it.
    pub fn withdraw(&mut self, target: &Target, id: &str) -> bool {
        let Some(queue) = self.targets.get_mut(&target.key()) else {
            return false;
        };
        let Some(index) = queue
            .iter()
            .position(|queued| queued.event.id == id && !queued.delivered)
        else {
            return false;
        };

        queue.remove(index);

        true
    }

    /// Marks every queued event not in `seen` as delivered and returns them, oldest first.
    pub fn hand_out(&mut self, target: &Target, seen: &HashSet<String>) -> Vec<Event> {
        let Some(queue) = self.targets.get_mut(&target.key()) else {
            return Vec::new();
        };

        queue
            .iter_mut()
            .filter(|queued| !seen.contains(&queued.event.id))
            .map(|queued| {
                queued.delivered = true;
                queued.event.clone()
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use recurse_protocol::{Event, EventBody, KernelExitedPayload, Target, TargetKind, bounds};

    use super::EventQueue;

    fn event(id: usize, actionable: bool) -> Event {
        Event {
            id: format!("e{id}"),
            target: Target::new(TargetKind::Cli, "t"),
            body: EventBody::KernelExited(KernelExitedPayload {
                pid: 1,
                exit_code: None,
                signal: None,
            }),
            actionable,
            at: 0,
            text: String::new(),
        }
    }

    #[test]
    fn overflow_drops_oldest_non_actionable_then_oldest() {
        let mut queue = EventQueue::default();
        let target = Target::new(TargetKind::Cli, "t");

        queue.push(event(0, true));
        queue.push(event(1, false));

        for id in 2..=bounds::QUEUED_EVENTS {
            queue.push(event(id, true));
        }

        let ids: Vec<String> = queue.list(&target).into_iter().map(|e| e.id).collect();

        assert_eq!(ids.len(), bounds::QUEUED_EVENTS);
        assert!(!ids.contains(&"e1".to_owned()));
        assert_eq!(ids[0], "e0");

        queue.push(event(999, true));

        let ids: Vec<String> = queue.list(&target).into_iter().map(|e| e.id).collect();

        assert_eq!(ids[0], "e2");
    }

    #[test]
    fn duplicate_ids_are_queued_once_and_withdraw_respects_delivery() {
        let mut queue = EventQueue::default();
        let target = Target::new(TargetKind::Cli, "t");

        queue.push(event(1, true));
        queue.push(event(1, true));
        queue.push(event(2, true));
        assert_eq!(queue.list(&target).len(), 2);

        assert!(queue.withdraw(&target, "e1"));
        assert_eq!(queue.hand_out(&target, &Default::default()).len(), 1);
        assert!(!queue.withdraw(&target, "e2"));
        assert_eq!(queue.ack(&target, &["e2".into(), "nope".into()]), 1);
    }
}
