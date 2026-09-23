//! One kernel per target: lazy start, one cell at a time, restart, and shutdown.

mod handle;
mod launch;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use recurse_protocol::{
    CellResult, ErrorCode, ExecuteParams, KernelState, KernelStatus, RpcError, Target, bounds,
};

use crate::app::App;
use crate::registry::{TargetRecord, limit, not_found};
use handle::{Kernel, unavailable};

#[derive(Debug, Default)]
struct Slot {
    busy: AtomicBool,
    kernel: Mutex<Option<Arc<Kernel>>>,
}

impl Slot {
    fn current(&self) -> Option<Arc<Kernel>> {
        self.kernel
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn replace(&self, kernel: Option<Arc<Kernel>>) -> Option<Arc<Kernel>> {
        std::mem::replace(
            &mut *self.kernel.lock().unwrap_or_else(PoisonError::into_inner),
            kernel,
        )
    }
}

/// Clears the slot's busy flag when the execute call ends, however it ends.
struct BusyGuard(Arc<Slot>);

impl BusyGuard {
    fn acquire(slot: &Arc<Slot>) -> Option<Self> {
        slot.busy
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self(slot.clone()))
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

#[derive(Debug, Default)]
pub struct KernelManager {
    slots: Mutex<HashMap<String, Arc<Slot>>>,
}

impl KernelManager {
    fn slots(&self) -> MutexGuard<'_, HashMap<String, Arc<Slot>>> {
        self.slots.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn slot(&self, target: &Target) -> Arc<Slot> {
        self.slots().entry(target.key()).or_default().clone()
    }

    fn existing(&self, target: &Target) -> Option<Arc<Kernel>> {
        self.slots()
            .get(&target.key())
            .and_then(|slot| slot.current())
    }

    pub async fn execute(
        &self,
        app: &Arc<App>,
        params: ExecuteParams,
    ) -> Result<CellResult, RpcError> {
        let target = params.target;
        let (record, depth) = registration(app, &target)?;

        if params.code.len() > bounds::CELL_CODE {
            return Err(limit("cell code exceeds 256 KiB"));
        }

        let timeout_sec = params
            .timeout_sec
            .unwrap_or(app.options.cell_timeout_sec)
            .clamp(1, bounds::MAX_TIMEOUT_SEC);
        let slot = self.slot(&target);
        let _busy = BusyGuard::acquire(&slot).ok_or_else(|| {
            RpcError::new(
                ErrorCode::Busy,
                format!("a cell is already running for {target}"),
            )
        })?;
        let kernel = ensure_running(app, &slot, &target, &record, depth).await?;

        kernel
            .run_cell(
                params.code,
                Duration::from_secs(timeout_sec),
                app.options.interrupt_grace,
            )
            .await
    }

    pub fn interrupt(&self, target: &Target) -> bool {
        self.existing(target)
            .is_some_and(|kernel| kernel.interrupt())
    }

    /// Stops the target's kernel; the next execute starts a fresh one.
    pub async fn restart(&self, target: &Target, grace: Duration) {
        let kernel = self
            .slots()
            .get(&target.key())
            .and_then(|slot| slot.replace(None));

        if let Some(kernel) = kernel {
            kernel.stop(grace).await;
        }
    }

    /// Stops the kernel and forgets the slot (used when a child is deleted).
    pub async fn shutdown_target(&self, target: &Target, grace: Duration) {
        let slot = self.slots().remove(&target.key());

        if let Some(kernel) = slot.and_then(|slot| slot.replace(None)) {
            kernel.stop(grace).await;
        }
    }

    pub fn status(&self, target: &Target, cwd: String) -> KernelStatus {
        let Some(kernel) = self.existing(target) else {
            return KernelStatus {
                state: KernelState::Absent,
                pid: None,
                execution_count: 0,
                started_at: None,
                cwd,
                handles: Vec::new(),
            };
        };
        let (state, pid, execution_count, handles) = kernel.snapshot();

        KernelStatus {
            state,
            pid: Some(pid),
            execution_count,
            started_at: Some(kernel.started_at),
            cwd,
            handles,
        }
    }

    pub fn live_count(&self) -> usize {
        self.all().iter().filter(|kernel| kernel.is_alive()).count()
    }

    fn all(&self) -> Vec<Arc<Kernel>> {
        self.slots()
            .values()
            .filter_map(|slot| slot.current())
            .collect()
    }

    /// Sends `shutdown` to every kernel, waits up to `grace`, then kills stragglers.
    pub async fn stop_all(&self, grace: Duration) {
        let kernels: Vec<Arc<Kernel>> = self
            .all()
            .into_iter()
            .filter(|kernel| kernel.is_alive())
            .collect();

        for kernel in &kernels {
            kernel.begin_stop();
        }

        let deadline = tokio::time::Instant::now() + grace;

        for kernel in &kernels {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());

            if !kernel.wait_exit(remaining).await {
                kernel.kill();
            }
        }

        for kernel in &kernels {
            kernel.wait_exit(Duration::from_secs(1)).await;
        }
    }
}

fn registration(app: &App, target: &Target) -> Result<(TargetRecord, u32), RpcError> {
    app.state
        .read(|core| {
            core.registry
                .record(target)
                .cloned()
                .map(|record| (record, core.registry.depth(target)))
        })
        .ok_or_else(|| not_found(format!("target {target} is not registered")))
}

async fn ensure_running(
    app: &Arc<App>,
    slot: &Slot,
    target: &Target,
    record: &TargetRecord,
    depth: u32,
) -> Result<Arc<Kernel>, RpcError> {
    if let Some(kernel) = slot.current().filter(|kernel| kernel.is_ready()) {
        return Ok(kernel);
    }

    let (kernel, ready) = launch::launch(app, target, record, depth)?;

    slot.replace(Some(kernel.clone()));

    match tokio::time::timeout(app.options.ready_timeout, ready).await {
        Ok(Ok(())) => Ok(kernel),
        Ok(Err(_)) => Err(unavailable(
            "the kernel exited before it was ready (see daemon.log)",
        )),
        Err(_) => {
            kernel.kill();
            Err(unavailable(format!(
                "the kernel did not send ready within {}s",
                app.options.ready_timeout.as_secs()
            )))
        }
    }
}
