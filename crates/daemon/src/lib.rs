//! The recurse daemon: one Python kernel per target, the child registry, and the event queue,
//! served on loopback as `POST /rpc` and `GET /events`.

mod app;
mod events;
mod host;
mod kernel;
mod lock;
mod log;
mod options;
pub mod paths;
mod queue;
mod registry;
mod rpc;
mod server;
pub mod skills;
mod sse;
mod state;
mod store;
mod time;

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;

use tokio::task::JoinHandle;

pub use options::DaemonOptions;

use app::App;
use lock::StateLock;
use state::State;
use store::Store;

/// A running daemon. Dropping it does not stop it; call [`Daemon::stop`] or [`Daemon::wait`].
pub struct Daemon {
    addr: SocketAddr,
    app: Arc<App>,
    task: JoinHandle<()>,
}

impl Daemon {
    /// Takes the state lock, loads persisted state, and binds `127.0.0.1:<port>`.
    pub async fn start(options: DaemonOptions) -> Result<Self, String> {
        let store = Store::open(&options.state_dir)?;
        let lock = StateLock::acquire(store.dir())?;
        let log = log::Logger::open(store.dir(), options.log_to_stderr);
        let state = State::load(store)?;
        let address = SocketAddr::from((Ipv4Addr::LOCALHOST, options.port));
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(|error| format!("bind daemon to {address}: {error}"))?;
        let addr = listener
            .local_addr()
            .map_err(|error| format!("read bound address: {error}"))?;
        let app = Arc::new(App::new(options, state, log, time::now_ms()));

        app.log.info(&format!(
            "recurse daemon {} listening on http://{addr} (pid {})",
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        ));

        let task = tokio::spawn(run(app.clone(), listener, lock));

        Ok(Self { addr, app, task })
    }

    pub const fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// Requests graceful shutdown and waits for it to finish.
    pub async fn stop(self) {
        self.app.request_shutdown();
        self.wait().await;
    }

    /// Waits until the daemon shuts down (signal, idle exit, or [`Daemon::stop`]).
    pub async fn wait(self) {
        let _ = self.task.await;
    }

    fn shutdown_handle(&self) -> Arc<App> {
        self.app.clone()
    }
}

/// Runs a daemon in the foreground until SIGTERM/SIGINT or idle exit.
pub async fn serve(options: DaemonOptions) -> Result<(), String> {
    let daemon = Daemon::start(options).await?;
    let app = daemon.shutdown_handle();

    tokio::spawn(async move {
        server::termination().await;
        app.log.info("received termination signal");
        app.request_shutdown();
    });

    daemon.wait().await;

    Ok(())
}

async fn run(app: Arc<App>, listener: tokio::net::TcpListener, lock: StateLock) {
    let mut shutdown = app.shutdown.subscribe();
    let (stopped_tx, stopped_rx) = tokio::sync::oneshot::channel();
    let stopper = app.clone();

    tokio::spawn(server::idle_watch(app.clone()));

    // Kernels stop while in-flight requests drain, so a long cell cannot hold shutdown open.
    let served = axum::serve(listener, server::router(app.clone()))
        .with_graceful_shutdown(async move {
            let _ = shutdown.wait_for(|stop| *stop).await;

            tokio::spawn(async move {
                stopper
                    .kernels
                    .stop_all(stopper.options.shutdown_grace)
                    .await;
                let _ = stopped_tx.send(());
            });
        })
        .await;

    if let Err(error) = served {
        app.log.error(&format!("serve: {error}"));
    }

    if stopped_rx.await.is_err() {
        app.kernels.stop_all(app.options.shutdown_grace).await;
    }
    app.state.flush(&app.log);
    app.log.info("daemon stopped");
    drop(lock);
}
