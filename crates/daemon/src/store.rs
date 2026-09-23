//! Atomic JSON persistence in the state directory (dir 0700, files 0600).

use std::fs::OpenOptions;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

#[derive(Debug, Clone)]
pub struct Store {
    dir: PathBuf,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(dir)
            .map_err(|error| format!("create {}: {error}", dir.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| format!("chmod {}: {error}", dir.display()))?;
        }

        Ok(Self {
            dir: dir.to_path_buf(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Loads `name`, returning `None` when it does not exist. Corrupt files are an error so the
    /// daemon never silently overwrites state it could not read.
    pub fn load<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, String> {
        let path = self.dir.join(name);

        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).map(Some).map_err(|error| {
                format!(
                    "invalid state at {}: {error}; refusing to overwrite",
                    path.display()
                )
            }),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(format!("read {}: {error}", path.display())),
        }
    }

    pub fn save<T: Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        let path = self.dir.join(name);
        let temporary = self.dir.join(format!("{name}.{}.tmp", std::process::id()));
        let mut bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;

        bytes.push(b'\n');
        write_private(&temporary, &bytes)
            .and_then(|()| std::fs::rename(&temporary, &path))
            .map_err(|error| format!("write {}: {error}", path.display()))
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = OpenOptions::new();

    options.write(true).create(true).truncate(true);

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;

        options.mode(0o600);
    }

    let mut file = options.open(path)?;

    file.write_all(bytes)?;
    file.sync_all()
}
