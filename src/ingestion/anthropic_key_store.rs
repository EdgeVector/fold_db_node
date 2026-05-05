//! Sensitive-io-backed storage for the Anthropic API key.
//!
//! - `os-keychain` enabled (release): encrypted at rest as `anthropic.key.enc`.
//! - `os-keychain` disabled (dev): plaintext JSON at `anthropic.key.json`,
//!   0o600 on Unix.
//!
//! Public API takes an explicit `config_dir` so tests (and any callers that
//! need to override `~/.folddb`) stay self-contained — mirrors
//! [`crate::ingestion::config::IngestionConfig::load`].

use crate::sensitive_io::{read_sensitive, write_sensitive};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[cfg(not(feature = "os-keychain"))]
const KEY_FILE: &str = "anthropic.key.json";
#[cfg(feature = "os-keychain")]
const KEY_FILE: &str = "anthropic.key.enc";

#[derive(Serialize, Deserialize)]
struct StoredKey {
    api_key: String,
}

/// Path of the on-disk key file for the given config dir.
pub fn key_path(config_dir: &Path) -> PathBuf {
    config_dir.join(KEY_FILE)
}

/// Returns `Ok(Some(key))` when the file exists and parses, `Ok(None)` when
/// the file is absent. Errors propagate read/parse failures so callers can
/// fail fast instead of silently degrading.
pub fn load(config_dir: &Path) -> Result<Option<String>, String> {
    let path = key_path(config_dir);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = read_sensitive(&path)?;
    let stored: StoredKey = serde_json::from_slice(&bytes).map_err(|e| {
        format!(
            "Failed to parse anthropic key store at {}: {e}",
            path.display()
        )
    })?;
    if stored.api_key.is_empty() {
        Ok(None)
    } else {
        Ok(Some(stored.api_key))
    }
}

/// Persist the API key. Empty input is rejected — callers wanting to clear
/// the key should call [`delete`] explicitly.
pub fn save(config_dir: &Path, api_key: &str) -> Result<(), String> {
    if api_key.is_empty() {
        return Err("Refusing to save empty Anthropic API key; use delete() instead".into());
    }
    let path = key_path(config_dir);
    let bytes = serde_json::to_vec(&StoredKey {
        api_key: api_key.to_string(),
    })
    .map_err(|e| format!("Failed to serialize anthropic key: {e}"))?;
    write_sensitive(&path, &bytes)
}

/// Remove the on-disk key file. No-op if it doesn't exist.
pub fn delete(config_dir: &Path) -> Result<(), String> {
    let path = key_path(config_dir);
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("Failed to delete anthropic key at {}: {e}", path.display()))?;
    }
    Ok(())
}

/// Cheap existence check — does not decrypt or parse the file.
pub fn has_key(config_dir: &Path) -> bool {
    key_path(config_dir).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_returns_none_when_file_absent() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(load(tmp.path()).unwrap(), None);
        assert!(!has_key(tmp.path()));
    }

    #[test]
    fn save_then_load_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        save(tmp.path(), "sk-ant-roundtrip").unwrap();
        assert!(has_key(tmp.path()));
        assert_eq!(load(tmp.path()).unwrap(), Some("sk-ant-roundtrip".into()));
    }

    #[test]
    fn save_overwrites_existing_key() {
        let tmp = tempfile::tempdir().unwrap();
        save(tmp.path(), "first").unwrap();
        save(tmp.path(), "second").unwrap();
        assert_eq!(load(tmp.path()).unwrap(), Some("second".into()));
    }

    #[test]
    fn save_rejects_empty_input() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(save(tmp.path(), "").is_err());
        assert!(!has_key(tmp.path()));
    }

    #[test]
    fn delete_removes_file_and_is_noop_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        delete(tmp.path()).unwrap(); // no-op
        save(tmp.path(), "sk-ant-delete-me").unwrap();
        delete(tmp.path()).unwrap();
        assert!(!has_key(tmp.path()));
        assert_eq!(load(tmp.path()).unwrap(), None);
    }

    /// Defense-in-depth: anyone who finds the file on disk shouldn't be able
    /// to read it without going through a privileged keychain prompt. In dev
    /// mode that means the OS file-permission bits are the only barrier.
    #[cfg(all(unix, not(feature = "os-keychain")))]
    #[test]
    fn dev_mode_writes_with_0o600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        save(tmp.path(), "sk-ant-perm-check").unwrap();
        let mode = std::fs::metadata(key_path(tmp.path()))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "expected 0o600, got {:o}",
            mode & 0o777
        );
    }
}
