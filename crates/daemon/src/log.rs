//! Line logger writing to `state_dir/daemon.log` and optionally stderr.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use crate::time::now_ms;

#[derive(Debug)]
pub struct Logger {
    file: Mutex<Option<File>>,
    stderr: bool,
}

impl Logger {
    pub fn open(state_dir: &Path, stderr: bool) -> Self {
        let mut options = OpenOptions::new();

        options.create(true).append(true);

        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;

            options.mode(0o600);
        }

        Self {
            file: Mutex::new(options.open(state_dir.join("daemon.log")).ok()),
            stderr,
        }
    }

    pub fn info(&self, message: &str) {
        self.write("info", message);
    }

    pub fn warn(&self, message: &str) {
        self.write("warn", message);
    }

    pub fn error(&self, message: &str) {
        self.write("error", message);
    }

    pub fn write(&self, level: &str, message: &str) {
        let line = format!("{} [{level}] {message}\n", now_ms());

        if self.stderr {
            let _ = std::io::stderr().write_all(line.as_bytes());
        }

        if let Some(file) = self
            .file
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
}
