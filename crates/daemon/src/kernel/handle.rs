//! One running kernel process: its protocol channel, shared status, and cell execution.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use recurse_protocol::{
    CellResult, CellStatus, ErrorCode, HandleInfo, KernelState, RpcError, ToKernel,
};
use tokio::sync::{Notify, mpsc, oneshot, watch};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Starting,
    Ready,
    Dead,
}

#[derive(Debug)]
struct Shared {
    phase: Phase,
    pid: u32,
    running_cell: Option<String>,
    execution_count: u64,
    handles: Vec<HandleInfo>,
    pending: HashMap<String, oneshot::Sender<CellResult>>,
    ready: Option<oneshot::Sender<()>>,
    /// The daemon asked this kernel to stop; its exit is not reported as `kernel.exited`.
    intentional: bool,
}

/// What the waiter learns when the process ends.
#[derive(Debug, Clone, Copy)]
pub struct Exit {
    pub pid: u32,
    pub report: bool,
}

#[derive(Debug)]
pub struct Kernel {
    pub started_at: u64,
    tx: mpsc::Sender<ToKernel>,
    shared: Mutex<Shared>,
    kill: Notify,
    exited: watch::Sender<bool>,
    cells: AtomicU64,
}

pub fn unavailable(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::KernelUnavailable, message)
}

impl Kernel {
    pub fn new(
        pid: u32,
        started_at: u64,
        tx: mpsc::Sender<ToKernel>,
    ) -> (Self, oneshot::Receiver<()>) {
        let (ready_tx, ready_rx) = oneshot::channel();
        let shared = Shared {
            phase: Phase::Starting,
            pid,
            running_cell: None,
            execution_count: 0,
            handles: Vec::new(),
            pending: HashMap::new(),
            ready: Some(ready_tx),
            intentional: false,
        };
        let kernel = Self {
            started_at,
            tx,
            shared: Mutex::new(shared),
            kill: Notify::new(),
            exited: watch::channel(false).0,
            cells: AtomicU64::new(0),
        };

        (kernel, ready_rx)
    }

    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn pid(&self) -> u32 {
        self.lock().pid
    }

    pub fn is_ready(&self) -> bool {
        self.lock().phase == Phase::Ready
    }

    pub fn is_alive(&self) -> bool {
        self.lock().phase != Phase::Dead
    }

    pub fn snapshot(&self) -> (KernelState, u32, u64, Vec<HandleInfo>) {
        let shared = self.lock();
        let state = match (shared.phase, &shared.running_cell) {
            (Phase::Starting, _) => KernelState::Starting,
            (Phase::Ready, Some(_)) => KernelState::Busy,
            (Phase::Ready, None) => KernelState::Idle,
            (Phase::Dead, _) => KernelState::Dead,
        };

        (
            state,
            shared.pid,
            shared.execution_count,
            shared.handles.clone(),
        )
    }

    pub fn on_ready(&self, pid: u32) {
        let mut shared = self.lock();

        if shared.phase == Phase::Starting {
            shared.phase = Phase::Ready;
            shared.pid = pid;

            if let Some(ready) = shared.ready.take() {
                let _ = ready.send(());
            }
        }
    }

    pub fn on_result(&self, id: &str, result: CellResult) {
        if let Some(pending) = self.lock().pending.remove(id) {
            let _ = pending.send(result);
        }
    }

    pub fn on_handles(&self, handles: Vec<HandleInfo>) {
        self.lock().handles = handles;
    }

    /// Records the process exit and fails every waiter.
    pub fn on_exit(&self) -> Exit {
        let mut shared = self.lock();
        let report = shared.phase == Phase::Ready && !shared.intentional;

        shared.phase = Phase::Dead;
        shared.pending.clear();
        shared.ready = None;
        shared.handles.clear();
        drop(shared);
        self.exited.send_replace(true);

        Exit {
            pid: self.pid(),
            report,
        }
    }

    pub async fn send(&self, message: ToKernel) -> bool {
        self.tx.send(message).await.is_ok()
    }

    pub async fn killed(&self) {
        self.kill.notified().await;
    }

    pub fn kill(&self) {
        self.kill.notify_one();
    }

    pub async fn wait_exit(&self, timeout: Duration) -> bool {
        let mut exited = self.exited.subscribe();

        tokio::time::timeout(timeout, exited.wait_for(|dead| *dead))
            .await
            .is_ok()
    }

    /// Sends `interrupt` if a cell is running.
    pub fn interrupt(&self) -> bool {
        let running = self.lock().running_cell.is_some();

        running && self.tx.try_send(ToKernel::Interrupt).is_ok()
    }

    /// Asks the kernel to exit, killing it after `grace`.
    pub async fn stop(&self, grace: Duration) {
        self.lock().intentional = true;

        let _ = self.tx.try_send(ToKernel::Shutdown);

        if !self.wait_exit(grace).await {
            self.kill();
            self.wait_exit(Duration::from_secs(2)).await;
        }
    }

    pub fn begin_stop(&self) {
        self.lock().intentional = true;

        let _ = self.tx.try_send(ToKernel::Shutdown);
    }

    pub async fn run_cell(
        &self,
        code: String,
        timeout: Duration,
        grace: Duration,
    ) -> Result<CellResult, RpcError> {
        let id = format!(
            "x{}",
            self.cells.fetch_add(1, Ordering::Relaxed).saturating_add(1)
        );
        let (result_tx, mut result_rx) = oneshot::channel();

        {
            let mut shared = self.lock();

            if shared.phase != Phase::Ready {
                return Err(unavailable("kernel is not running"));
            }

            shared.pending.insert(id.clone(), result_tx);
            shared.running_cell = Some(id.clone());
        }

        let started = Instant::now();
        let timeout_ms = u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX);
        let execute = ToKernel::Execute {
            id: id.clone(),
            code,
            timeout_ms,
        };
        let outcome = if self.send(execute).await {
            match tokio::time::timeout(timeout, &mut result_rx).await {
                Ok(Ok(result)) => Ok(result),
                Ok(Err(_)) => Err(unavailable("kernel exited while running the cell")),
                Err(_) => Ok(self.time_out(result_rx, grace, started).await),
            }
        } else {
            Err(unavailable("kernel stdin is closed"))
        };

        self.finish_cell(&id, outcome)
    }

    async fn time_out(
        &self,
        result_rx: oneshot::Receiver<CellResult>,
        grace: Duration,
        started: Instant,
    ) -> CellResult {
        let _ = self.send(ToKernel::Interrupt).await;

        if let Ok(Ok(mut result)) = tokio::time::timeout(grace, result_rx).await {
            result.status = CellStatus::Timeout;
            return result;
        }

        self.kill();
        self.wait_exit(Duration::from_secs(5)).await;

        let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut result =
            CellResult::empty(CellStatus::Timeout, self.lock().execution_count, elapsed);

        result.stderr = "[recurse] the cell ignored the interrupt; the kernel was killed and its state is gone\n".into();
        result
    }

    fn finish_cell(
        &self,
        id: &str,
        outcome: Result<CellResult, RpcError>,
    ) -> Result<CellResult, RpcError> {
        let mut shared = self.lock();

        shared.pending.remove(id);

        if shared.running_cell.as_deref() == Some(id) {
            shared.running_cell = None;
        }

        let mut result = outcome?;

        result.enforce_bounds();
        shared.execution_count = shared.execution_count.max(result.execution_count);

        Ok(result)
    }
}
