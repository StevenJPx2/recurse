//! Default locations for state, config, and the bundled kernel and skills.

use std::path::{Path, PathBuf};

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// `RECURSE_STATE_DIR` > `$XDG_STATE_HOME/recurse` > `~/.local/state/recurse`.
pub fn state_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("RECURSE_STATE_DIR").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }

    std::env::var_os("XDG_STATE_HOME")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| home().join(".local/state"), PathBuf::from)
        .join("recurse")
}

/// `RECURSE_CONFIG_DIR` > `~/.config/recurse`.
pub fn config_dir() -> PathBuf {
    std::env::var_os("RECURSE_CONFIG_DIR")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| home().join(".config/recurse"), PathBuf::from)
}

/// Finds a shipped directory: `<exe>/../share/recurse/<name>`, else (debug builds) the repository's
/// `<name>/` found by walking up from the executable or this crate. `marker` must exist inside it.
pub fn bundled(name: &str, marker: &str) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf));

    if let Some(share) = exe_dir
        .as_ref()
        .and_then(|dir| dir.parent())
        .map(|prefix| prefix.join("share/recurse").join(name))
        .filter(|dir| dir.join(marker).exists())
    {
        return Some(share);
    }

    if !cfg!(debug_assertions) {
        return None;
    }

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    exe_dir
        .into_iter()
        .chain(std::iter::once(manifest))
        .find_map(|start| repo_dir(&start, name, marker))
}

fn repo_dir(start: &Path, name: &str, marker: &str) -> Option<PathBuf> {
    start
        .ancestors()
        .filter(|dir| dir.join("docs/protocol.md").is_file())
        .map(|dir| dir.join(name))
        .find(|dir| dir.join(marker).exists())
}
