//! The persisted core (registry + event queue) behind one lock, with change notification.

use std::collections::BTreeSet;
use std::sync::{Mutex, PoisonError};

use recurse_protocol::{Event, RpcError};
use tokio::sync::broadcast;

use crate::log::Logger;
use crate::queue::{EventQueue, QueueFile};
use crate::registry::Registry;
use crate::store::Store;

const REGISTRY_FILE: &str = "registry.json";
const EVENTS_FILE: &str = "events.json";

#[derive(Debug, Default)]
pub struct Core {
    pub registry: Registry,
    pub queue: EventQueue,
    /// Target keys whose queues grew during the current update.
    notify: BTreeSet<String>,
}

impl Core {
    pub fn enqueue(&mut self, event: Event) -> String {
        self.notify.insert(event.target.key());
        self.queue.push(event)
    }
}

#[derive(Debug)]
pub struct State {
    core: Mutex<Core>,
    store: Store,
    changes: broadcast::Sender<String>,
}

impl State {
    pub fn load(store: Store) -> Result<Self, String> {
        let registry = store.load::<Registry>(REGISTRY_FILE)?.unwrap_or_default();
        let queue =
            EventQueue::from_file(store.load::<QueueFile>(EVENTS_FILE)?.unwrap_or_default());
        let (changes, _) = broadcast::channel(1024);

        Ok(Self {
            core: Mutex::new(Core {
                registry,
                queue,
                notify: BTreeSet::new(),
            }),
            store,
            changes,
        })
    }

    pub fn read<R>(&self, read: impl FnOnce(&Core) -> R) -> R {
        read(&self.core.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Applies `change`, persists on success, and wakes listeners of every queue it grew.
    pub fn update<R>(
        &self,
        log: &Logger,
        change: impl FnOnce(&mut Core) -> Result<R, RpcError>,
    ) -> Result<R, RpcError> {
        let mut core = self.core.lock().unwrap_or_else(PoisonError::into_inner);
        let result = change(&mut core);
        let notify = std::mem::take(&mut core.notify);

        if result.is_ok() {
            self.persist(&core, log);
        }

        drop(core);

        for key in notify {
            let _ = self.changes.send(key);
        }

        result
    }

    pub fn subscribe(&self) -> broadcast::Receiver<String> {
        self.changes.subscribe()
    }

    pub fn flush(&self, log: &Logger) {
        self.persist(
            &self.core.lock().unwrap_or_else(PoisonError::into_inner),
            log,
        );
    }

    fn persist(&self, core: &Core, log: &Logger) {
        let saved = self
            .store
            .save(REGISTRY_FILE, &core.registry)
            .and_then(|()| self.store.save(EVENTS_FILE, &core.queue.to_file()));

        if let Err(error) = saved {
            log.error(&format!("persist state: {error}"));
        }
    }
}
