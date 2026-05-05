//! Persistent on-disk store for the Brave Search API key.
//!
//! Mirrors the storage model used for other sensitive secrets in this crate:
//! plaintext with `0o600` permissions in dev, OS-keychain encrypted in
//! `os-keychain` (Tauri release) builds. Lives at
//! `<config_dir>/web_search.key.{json,enc}`.

use std::path::{Path, PathBuf};

use crate::sensitive_io::{read_sensitive, write_sensitive};

#[cfg(feature = "os-keychain")]
const KEY_FILE_NAME: &str = "web_search.key.enc";
#[cfg(not(feature = "os-keychain"))]
const KEY_FILE_NAME: &str = "web_search.key.json";

fn key_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(KEY_FILE_NAME)
}

/// Load the saved key. Returns `Ok(None)` when the file does not exist.
/// Trims whitespace and treats an empty payload as "no key".
pub fn load(config_dir: &Path) -> Result<Option<String>, String> {
    let path = key_file_path(config_dir);
    if !path.exists() {
        return Ok(None);
    }
    let bytes = read_sensitive(&path)?;
    let key = String::from_utf8(bytes)
        .map_err(|e| format!("Web search key file is not valid UTF-8: {}", e))?;
    let trimmed = key.trim();
    if trimmed.is_empty() {
        Ok(None)
    } else {
        Ok(Some(trimmed.to_string()))
    }
}

/// Persist `key` to disk, encrypting it when the `os-keychain` feature is on.
/// An empty key is treated as a delete request.
pub fn save(config_dir: &Path, key: &str) -> Result<(), String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return delete(config_dir);
    }
    let path = key_file_path(config_dir);
    write_sensitive(&path, trimmed.as_bytes())
}

/// Remove the saved key. No-op when the file does not exist.
///
/// Cleans both `web_search.key.json` and `web_search.key.enc` so a user who
/// toggles the `os-keychain` feature between writes can't end up with the
/// inactive variant orphaned next to the active one — mirrors
/// [`crate::ingestion::anthropic_key_store::delete`] and
/// [`crate::keychain::delete_credentials`].
pub fn delete(config_dir: &Path) -> Result<(), String> {
    let path = key_file_path(config_dir);
    if path.exists() {
        std::fs::remove_file(&path)
            .map_err(|e| format!("Failed to delete web search key file: {}", e))?;
    }
    let alt_path = config_dir.join(if cfg!(feature = "os-keychain") {
        "web_search.key.json"
    } else {
        "web_search.key.enc"
    });
    if alt_path.exists() {
        let _ = std::fs::remove_file(&alt_path);
    }
    Ok(())
}

/// Cheap presence check used by the GET /api/web_search/key endpoint.
pub fn has_key(config_dir: &Path) -> bool {
    matches!(load(config_dir), Ok(Some(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Tests that exercise the encrypted read/write path must hold the
    /// global env-var lock and provide a `FOLDDB_MASTER_KEY` — see
    /// [`crate::secure_store::test_master_key`] for the rationale.
    fn with_master_key() -> crate::secure_store::test_master_key::WithMasterKey {
        crate::secure_store::test_master_key::with_set()
    }

    #[test]
    fn save_load_roundtrip() {
        let _g = with_master_key();
        let dir = TempDir::new().expect("tempdir");
        assert!(!has_key(dir.path()));
        assert_eq!(load(dir.path()).unwrap(), None);

        save(dir.path(), "BRAVE_TEST_KEY_123").expect("save");
        assert!(has_key(dir.path()));
        assert_eq!(
            load(dir.path()).unwrap().as_deref(),
            Some("BRAVE_TEST_KEY_123")
        );
    }

    #[test]
    fn save_overwrites_previous() {
        let _g = with_master_key();
        let dir = TempDir::new().expect("tempdir");
        save(dir.path(), "first").unwrap();
        save(dir.path(), "second").unwrap();
        assert_eq!(load(dir.path()).unwrap().as_deref(), Some("second"));
    }

    #[test]
    fn save_empty_deletes() {
        let _g = with_master_key();
        let dir = TempDir::new().expect("tempdir");
        save(dir.path(), "abc").unwrap();
        assert!(has_key(dir.path()));
        save(dir.path(), "").unwrap();
        assert!(!has_key(dir.path()));
    }

    #[test]
    fn save_trims_whitespace() {
        let _g = with_master_key();
        let dir = TempDir::new().expect("tempdir");
        save(dir.path(), "  padded  ").unwrap();
        assert_eq!(load(dir.path()).unwrap().as_deref(), Some("padded"));
    }

    #[test]
    fn delete_when_missing_is_ok() {
        let dir = TempDir::new().expect("tempdir");
        delete(dir.path()).expect("delete on missing file should be a no-op");
    }

    /// Toggling the `os-keychain` feature between two writes can leave the
    /// inactive variant sitting next to the active one. `delete` must clean
    /// both so the next `load` can't read a stale value.
    #[cfg(not(feature = "os-keychain"))]
    #[test]
    fn delete_cleans_alt_extension_after_feature_toggle() {
        let dir = TempDir::new().expect("tempdir");
        save(dir.path(), "k").unwrap();
        let alt = dir.path().join("web_search.key.enc");
        std::fs::write(&alt, b"stale-encrypted-bytes").unwrap();

        delete(dir.path()).unwrap();

        assert!(
            !key_file_path(dir.path()).exists(),
            "active-feature key file should be removed"
        );
        assert!(!alt.exists(), "alt-extension key file should be removed");
    }

    /// "Empty file means no key" only makes sense for the dev-mode
    /// plaintext encoding. Under `os-keychain`, the on-disk format is an
    /// AES-GCM envelope and an empty file is invalid ciphertext (rightly
    /// surfaced as a decrypt error by `secure_store::read_and_decrypt`).
    #[cfg(not(feature = "os-keychain"))]
    #[test]
    fn load_treats_empty_file_as_no_key() {
        let dir = TempDir::new().expect("tempdir");
        let path = key_file_path(dir.path());
        std::fs::write(&path, b"").unwrap();
        assert_eq!(load(dir.path()).unwrap(), None);
        assert!(!has_key(dir.path()));
    }

    // 0o600 perms are only enforced on the dev (no `os-keychain`) write path —
    // see sensitive_io::write_sensitive. The encrypted release path delegates
    // to secure_store and has its own coverage there.
    #[cfg(all(unix, not(feature = "os-keychain")))]
    #[test]
    fn dev_mode_writes_owner_only_perms() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().expect("tempdir");
        save(dir.path(), "k").unwrap();
        let meta = std::fs::metadata(key_file_path(dir.path())).unwrap();
        let mode = meta.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0o600 perms, got {:o}", mode);
    }
}
