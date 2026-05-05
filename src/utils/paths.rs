//! FOLDDB_HOME path resolution.
//!
//! A single env var `FOLDDB_HOME` controls where all instance-specific state
//! lives. Default: `~/.folddb` (backward compatible).

use std::path::PathBuf;

/// Resolve the FOLDDB_HOME directory.
///
/// Priority:
/// 1. `FOLDDB_HOME` environment variable (if set)
/// 2. `~/.folddb` (default, backward compatible)
///
/// Returns an error only if `FOLDDB_HOME` is not set AND the home directory
/// cannot be determined.
pub fn folddb_home() -> Result<PathBuf, String> {
    if let Ok(home) = std::env::var("FOLDDB_HOME") {
        return Ok(PathBuf::from(home));
    }
    let home = dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?;
    Ok(home.join(".folddb"))
}

/// Resolve the structured-tracing log file path the daemon writes to.
///
/// Mirrors upstream `observability::init::default_node_log_path` so the
/// banner / `daemon start` output points at the file the FMT writer
/// actually appends to:
///   1. `OBS_FILE_PATH` — used as-is.
///   2. `$FOLDDB_HOME/observability.jsonl` — same env var that scopes the
///      rest of the node's state, so a single override moves both data
///      and logs together.
///   3. `~/.folddb/observability.jsonl` — final fallback when neither is
///      set and `$HOME` is resolvable.
pub fn observability_log_path() -> PathBuf {
    if let Ok(p) = std::env::var("OBS_FILE_PATH") {
        return PathBuf::from(p);
    }
    if let Ok(home) = folddb_home() {
        return home.join("observability.jsonl");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join(".folddb")
        .join("observability.jsonl")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate `FOLDDB_HOME` / `OBS_FILE_PATH`. Both
    /// envs have process-wide effects, so cargo's parallel test runner
    /// would otherwise race.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// `FOLDDB_HOME=...` makes the resolved log path land under that
    /// directory. This is the bug fix: before, the helper hard-coded
    /// `$HOME/.folddb/observability.jsonl` and lied about where the FMT
    /// writer was actually appending.
    #[test]
    fn observability_log_path_honors_folddb_home() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev_home = std::env::var("FOLDDB_HOME").ok();
        let prev_obs = std::env::var("OBS_FILE_PATH").ok();
        std::env::remove_var("OBS_FILE_PATH");
        std::env::set_var("FOLDDB_HOME", "/tmp/folddb-test-XYZ");

        let resolved = observability_log_path();

        // Restore env before any assertion — leaks would poison sibling tests.
        match prev_home {
            Some(v) => std::env::set_var("FOLDDB_HOME", v),
            None => std::env::remove_var("FOLDDB_HOME"),
        }
        if let Some(v) = prev_obs {
            std::env::set_var("OBS_FILE_PATH", v);
        }

        assert_eq!(
            resolved,
            PathBuf::from("/tmp/folddb-test-XYZ/observability.jsonl")
        );
    }

    /// `OBS_FILE_PATH` wins over `FOLDDB_HOME`.
    #[test]
    fn observability_log_path_obs_file_path_wins() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev_home = std::env::var("FOLDDB_HOME").ok();
        let prev_obs = std::env::var("OBS_FILE_PATH").ok();
        std::env::set_var("FOLDDB_HOME", "/tmp/folddb-test-ignored");
        std::env::set_var("OBS_FILE_PATH", "/tmp/folddb-test-explicit.jsonl");

        let resolved = observability_log_path();

        match prev_home {
            Some(v) => std::env::set_var("FOLDDB_HOME", v),
            None => std::env::remove_var("FOLDDB_HOME"),
        }
        match prev_obs {
            Some(v) => std::env::set_var("OBS_FILE_PATH", v),
            None => std::env::remove_var("OBS_FILE_PATH"),
        }

        assert_eq!(
            resolved,
            PathBuf::from("/tmp/folddb-test-explicit.jsonl")
        );
    }
}
