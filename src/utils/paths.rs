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
