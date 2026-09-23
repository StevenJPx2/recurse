use std::process::Stdio;
use std::time::Duration;

use crate::{ClientError, DaemonClient};

const POLL_INTERVAL: Duration = Duration::from_millis(150);
const SPAWN_DEADLINE: Duration = Duration::from_secs(10);

/// Returns a client for a healthy daemon, spawning `<current exe> daemon` detached when none
/// answers and `RECURSE_DAEMON_URL` is unset.
pub async fn connect_or_spawn() -> Result<DaemonClient, ClientError> {
    let client = DaemonClient::from_env()?;

    match client.health().await {
        Ok(_) => return Ok(client),
        Err(error @ ClientError::Protocol(_)) => return Err(error),
        Err(error) if std::env::var_os("RECURSE_DAEMON_URL").is_some() => return Err(error),
        Err(_) => {}
    }

    spawn_daemon()?;

    let deadline = tokio::time::Instant::now() + SPAWN_DEADLINE;
    let mut last_error = None;

    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;

        match client.health().await {
            Ok(_) => return Ok(client),
            Err(error @ ClientError::Protocol(_)) => return Err(error),
            Err(error) => last_error = Some(error),
        }
    }

    let reason = last_error
        .map(|error| error.to_string())
        .unwrap_or_default();

    Err(ClientError::Transport(format!(
        "recurse daemon did not become healthy within 10s at {}: {reason}",
        client.base_url()
    )))
}

fn spawn_daemon() -> Result<(), ClientError> {
    let executable = std::env::current_exe()
        .map_err(|error| ClientError::Transport(format!("locate recurse binary: {error}")))?;
    let mut command = std::process::Command::new(executable);

    command
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;

        command.process_group(0);
    }

    command
        .spawn()
        .map(drop)
        .map_err(|error| ClientError::Transport(format!("spawn recurse daemon: {error}")))
}
