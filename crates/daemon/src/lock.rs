//! `state_dir/daemon.lock`: a pid file with stale-pid takeover.

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

const ATTEMPTS: usize = 5;

/// Held for the daemon's lifetime; removes the lock file on drop if it is still ours.
#[derive(Debug)]
pub struct StateLock {
    path: PathBuf,
}

impl StateLock {
    pub fn acquire(state_dir: &Path) -> Result<Self, String> {
        let path = state_dir.join("daemon.lock");
        let pid = std::process::id();

        for _ in 0..ATTEMPTS {
            match create_exclusive(&path, pid) {
                Ok(()) => return Ok(Self { path }),
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(format!("create {}: {error}", path.display())),
            }

            let holder = read_pid(&path);

            if holder.is_none_or(process_alive) {
                let holder = holder.map_or_else(|| "unknown".to_owned(), |pid| pid.to_string());

                return Err(format!(
                    "another recurse daemon (pid {holder}) is using {}; stop it or set RECURSE_STATE_DIR",
                    state_dir.display()
                ));
            }

            // Renaming is atomic, so exactly one contender removes a given stale lock.
            let contender = path.with_extension(format!("lock.{pid}.stale"));

            if std::fs::rename(&path, &contender).is_ok() {
                let _ = std::fs::remove_file(&contender);
            }
        }

        Err(format!(
            "could not acquire the recurse state lock at {}",
            path.display()
        ))
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        if read_pid(&self.path) == Some(std::process::id()) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

fn create_exclusive(path: &Path, pid: u32) -> std::io::Result<()> {
    let mut options = OpenOptions::new();

    options.write(true).create_new(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }

    let mut file = options.open(path)?;

    file.write_all(format!("{pid}\n").as_bytes())?;
    file.sync_all()
}

fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse()
        .ok()
        .filter(|pid| *pid > 0)
}

pub fn process_alive(pid: u32) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return false;
    };

    // SAFETY: signal 0 performs only the existence and permission check; no signal is sent.
    let result = unsafe { libc::kill(pid, 0) };

    result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::StateLock;

    #[test]
    fn second_acquire_fails_and_stale_lock_is_taken_over() {
        let dir = std::env::temp_dir().join(format!("recurse-lock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let first = StateLock::acquire(&dir).unwrap();
        let error = StateLock::acquire(&dir).unwrap_err();

        assert!(error.contains("another recurse daemon"), "{error}");
        drop(first);
        assert!(!dir.join("daemon.lock").exists());

        std::fs::write(dir.join("daemon.lock"), "999999999\n").unwrap();
        let taken = StateLock::acquire(&dir).unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.join("daemon.lock"))
                .unwrap()
                .trim(),
            std::process::id().to_string()
        );
        drop(taken);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
