use std::path::{Path, PathBuf};
use std::time::Duration;

use recurse_protocol::DEFAULT_PORT;

use crate::paths;

/// Everything the daemon needs; `from_env` reads the documented environment variables.
#[derive(Debug, Clone)]
pub struct DaemonOptions {
    /// Loopback port; 0 binds an ephemeral port.
    pub port: u16,
    pub token: Option<String>,
    pub state_dir: PathBuf,
    pub config_dir: PathBuf,
    /// Directory containing the `recurse_kernel` package.
    pub kernel_path: Option<PathBuf>,
    /// Built-in skills directory.
    pub skills_dir: Option<PathBuf>,
    pub python: String,
    pub max_depth: u32,
    pub cell_timeout_sec: u64,
    pub idle_exit_sec: u64,
    pub log_to_stderr: bool,
    pub ready_timeout: Duration,
    pub interrupt_grace: Duration,
    pub shutdown_grace: Duration,
    pub heartbeat: Duration,
}

impl DaemonOptions {
    /// Defaults that ignore the environment, rooted at `state_dir`.
    pub fn new(state_dir: impl Into<PathBuf>) -> Self {
        Self {
            port: DEFAULT_PORT,
            token: None,
            state_dir: state_dir.into(),
            config_dir: paths::home().join(".config/recurse"),
            kernel_path: paths::bundled("python", "recurse_kernel"),
            skills_dir: paths::bundled("skills", ""),
            python: "python3".into(),
            max_depth: 1,
            cell_timeout_sec: 600,
            idle_exit_sec: 0,
            log_to_stderr: true,
            ready_timeout: Duration::from_secs(10),
            interrupt_grace: Duration::from_secs(5),
            shutdown_grace: Duration::from_secs(5),
            heartbeat: Duration::from_secs(15),
        }
    }

    pub fn from_env() -> Result<Self, String> {
        let mut options = Self::new(paths::state_dir());

        if let Some(port) = env("RECURSE_DAEMON_PORT") {
            options.port = port
                .parse()
                .map_err(|_| format!("RECURSE_DAEMON_PORT is not a port: {port}"))?;
        }

        options.token = env("RECURSE_DAEMON_TOKEN");
        options.config_dir = paths::config_dir();

        if let Some(path) = env("RECURSE_KERNEL_PATH") {
            options.kernel_path = Some(PathBuf::from(path));
        }

        if let Some(path) = env("RECURSE_SKILLS_DIR") {
            options.skills_dir = Some(PathBuf::from(path));
        }

        if let Some(python) = env("RECURSE_PYTHON") {
            options.python = python;
        }

        options.max_depth = parse_env("RECURSE_MAX_DEPTH", options.max_depth)?;
        options.cell_timeout_sec = parse_env("RECURSE_CELL_TIMEOUT_SEC", options.cell_timeout_sec)?;
        options.idle_exit_sec = parse_env("RECURSE_IDLE_EXIT_SEC", options.idle_exit_sec)?;

        Ok(options)
    }

    pub fn skill_dirs(&self, cwd: Option<&Path>) -> Vec<PathBuf> {
        crate::skills::search_dirs(self.skills_dir.as_deref(), &self.config_dir, cwd)
    }
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn parse_env<T: std::str::FromStr>(name: &str, default: T) -> Result<T, String> {
    match env(name) {
        Some(value) => value
            .parse()
            .map_err(|_| format!("{name} is not a valid number: {value}")),
        None => Ok(default),
    }
}
