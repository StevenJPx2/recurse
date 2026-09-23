//! Spawning `$RECURSE_PYTHON -u -m recurse_kernel` and wiring its stdio to protocol tasks.

use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use recurse_protocol::{FromKernel, KERNEL_PROTOCOL_VERSION, RpcError, Target, ToKernel};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};

use super::handle::{Kernel, unavailable};
use crate::app::App;
use crate::registry::TargetRecord;
use crate::time::now_ms;

const CHANNEL: usize = 64;

pub fn launch(
    app: &Arc<App>,
    target: &Target,
    record: &TargetRecord,
    depth: u32,
) -> Result<(Arc<Kernel>, oneshot::Receiver<()>), RpcError> {
    let mut command = command(app, target, record, depth)?;
    let mut child = command
        .spawn()
        .map_err(|error| unavailable(format!("start {}: {error}", app.options.python)))?;
    let pid = child.id().unwrap_or_default();
    let (Some(stdin), Some(stdout), Some(stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(unavailable("kernel stdio was not captured"));
    };
    let (tx, rx) = mpsc::channel(CHANNEL);
    let (kernel, ready) = Kernel::new(pid, now_ms(), tx);
    let kernel = Arc::new(kernel);

    app.log.info(&format!(
        "started kernel {pid} for {target} in {}",
        record.cwd
    ));
    tokio::spawn(write_lines(rx, stdin));
    tokio::spawn(log_stderr(app.clone(), target.clone(), stderr));
    tokio::spawn(read_protocol(
        app.clone(),
        kernel.clone(),
        target.clone(),
        stdout,
    ));
    tokio::spawn(wait_exit(
        app.clone(),
        kernel.clone(),
        target.clone(),
        child,
    ));

    Ok((kernel, ready))
}

fn command(
    app: &App,
    target: &Target,
    record: &TargetRecord,
    depth: u32,
) -> Result<Command, RpcError> {
    let kernel_path =
        app.options.kernel_path.as_deref().ok_or_else(|| {
            unavailable("recurse_kernel package not found; set RECURSE_KERNEL_PATH")
        })?;
    let cwd = Path::new(&record.cwd);

    if !cwd.is_dir() {
        return Err(unavailable(format!(
            "target cwd {} is not a directory",
            record.cwd
        )));
    }

    let skills = std::env::join_paths(app.options.skill_dirs(Some(cwd)))
        .map_err(|error| unavailable(format!("skills path: {error}")))?;
    let target_json = serde_json::to_string(target).unwrap_or_default();
    let mut command = Command::new(&app.options.python);

    command
        .args(["-u", "-m", "recurse_kernel"])
        .current_dir(cwd)
        .env("PYTHONPATH", python_path(kernel_path))
        .env("RECURSE_TARGET", target_json)
        .env("RECURSE_DEPTH", depth.to_string())
        .env("RECURSE_MAX_DEPTH", app.options.max_depth.to_string())
        .env("RECURSE_SKILLS_PATH", skills)
        .env(
            "RECURSE_KERNEL_PROTOCOL",
            KERNEL_PROTOCOL_VERSION.to_string(),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);

    Ok(command)
}

fn python_path(kernel_path: &Path) -> OsString {
    let mut value = kernel_path.as_os_str().to_owned();

    if let Some(existing) = std::env::var_os("PYTHONPATH").filter(|path| !path.is_empty()) {
        value.push(":");
        value.push(existing);
    }

    value
}

async fn write_lines(mut rx: mpsc::Receiver<ToKernel>, mut stdin: ChildStdin) {
    while let Some(message) = rx.recv().await {
        let Ok(mut line) = serde_json::to_vec(&message) else {
            continue;
        };

        line.push(b'\n');

        if stdin.write_all(&line).await.is_err() || stdin.flush().await.is_err() {
            break;
        }
    }
}

async fn log_stderr(app: Arc<App>, target: Target, stderr: ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        app.log.write("kernel", &format!("{target}: {line}"));
    }
}

async fn read_protocol(app: Arc<App>, kernel: Arc<Kernel>, target: Target, stdout: ChildStdout) {
    let mut lines = BufReader::new(stdout).lines();

    while let Ok(Some(line)) = lines.next_line().await {
        match serde_json::from_str::<FromKernel>(&line) {
            Ok(message) => dispatch(&app, &kernel, &target, message),
            Err(error) => app
                .log
                .warn(&format!("{target}: invalid kernel protocol line ({error})")),
        }
    }
}

fn dispatch(app: &Arc<App>, kernel: &Arc<Kernel>, target: &Target, message: FromKernel) {
    match message {
        FromKernel::Ready { pid, protocol, .. } => {
            if protocol != KERNEL_PROTOCOL_VERSION {
                app.log
                    .error(&format!("{target}: kernel speaks protocol {protocol}"));
                kernel.kill();
                return;
            }

            kernel.on_ready(pid);
        }
        FromKernel::ExecuteResult { id, result } => kernel.on_result(&id, result),
        FromKernel::HostRequest { id, method, params } => {
            let (app, kernel, target) = (app.clone(), kernel.clone(), target.clone());

            tokio::spawn(async move {
                let outcome =
                    crate::host::handle(&app, &target, kernel.pid(), &method, params).await;
                let (result, error) = match outcome {
                    Ok(value) => (Some(value), None),
                    Err(error) => (None, Some(error)),
                };

                kernel
                    .send(ToKernel::HostResponse { id, result, error })
                    .await;
            });
        }
        FromKernel::Handles { handles } => kernel.on_handles(handles),
        FromKernel::Log { level, message } => {
            app.log.write(&level, &format!("{target}: {message}"))
        }
    }
}

async fn wait_exit(app: Arc<App>, kernel: Arc<Kernel>, target: Target, mut child: Child) {
    let exited = tokio::select! {
        status = child.wait() => Some(status),
        () = kernel.killed() => None,
    };
    let status = match exited {
        Some(status) => status,
        None => {
            let _ = child.start_kill();
            child.wait().await
        }
    };
    let (exit_code, signal) = status.as_ref().map_or((None, None), |status| {
        use std::os::unix::process::ExitStatusExt;

        (status.code(), status.signal())
    });
    let exit = kernel.on_exit();

    app.log.info(&format!(
        "kernel {} for {target} exited (code {exit_code:?}, signal {signal:?})",
        exit.pid
    ));

    if exit.report {
        let event = crate::events::kernel_exited(&target, exit.pid, exit_code, signal);
        let _ = app.state.update(&app.log, |core| Ok(core.enqueue(event)));
    }
}
