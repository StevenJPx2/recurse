use std::sync::{Mutex, PoisonError};
use std::time::Instant;

use tokio::sync::watch;

use crate::DaemonOptions;
use crate::kernel::KernelManager;
use crate::log::Logger;
use crate::state::State;

/// Everything request handlers, SSE listeners, and kernel tasks share.
#[derive(Debug)]
pub struct App {
    pub options: DaemonOptions,
    pub state: State,
    pub kernels: KernelManager,
    pub log: Logger,
    pub started_at: u64,
    pub shutdown: watch::Sender<bool>,
    last_rpc: Mutex<Instant>,
}

impl App {
    pub fn new(options: DaemonOptions, state: State, log: Logger, started_at: u64) -> Self {
        Self {
            options,
            state,
            kernels: KernelManager::default(),
            log,
            started_at,
            shutdown: watch::channel(false).0,
            last_rpc: Mutex::new(Instant::now()),
        }
    }

    pub fn touch(&self) {
        *self.last_rpc.lock().unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    pub fn idle_for(&self) -> std::time::Duration {
        self.last_rpc
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed()
    }

    pub fn request_shutdown(&self) {
        self.shutdown.send_replace(true);
    }
}
