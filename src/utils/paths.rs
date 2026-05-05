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
/// Mirrors upstream `observability::init::default_node_log_path` exactly so
/// the path we surface to users (banner, `daemon start` output) matches
/// where the FMT writer actually appends. Note this honours `$HOME` and
/// **not** `FOLDDB_HOME` — that's an upstream quirk; if it ever changes,
/// update both sides together.
pub fn observability_log_path() -> PathBuf {
    if let Ok(p) = std::env::var("OBS_FILE_PATH") {
        return PathBuf::from(p);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".folddb").join("observability.jsonl")
}
