//! Unified read/write for sensitive files (identity keys, E2E keys, credentials).
//!
//! - `os-keychain` enabled: encrypts via OS keychain master key (AES-256-GCM)
//! - `os-keychain` disabled: writes plaintext with 0o600 Unix permissions
//!
//! All writes go through [`write_atomic_0600`], which delegates to
//! [`crate::utils::fs_atomic::write_atomic`] (tmpfile + fsync + rename, plus
//! a best-effort parent-dir fsync). Power loss between steps therefore leaves
//! either the previous good file or the staged tmpfile — never a half-written
//! final path. AES-GCM auth-tag failures from a torn write would force the
//! user to re-enter the credential, so the rename atomicity is what protects
//! them.

use std::fs;
use std::path::Path;

/// Write sensitive data to disk, encrypted if `os-keychain` is enabled.
pub fn write_sensitive(path: &Path, data: &[u8]) -> Result<(), String> {
    #[cfg(feature = "os-keychain")]
    {
        crate::secure_store::encrypt_and_write(path, data)
    }
    #[cfg(not(feature = "os-keychain"))]
    {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Failed to create directory: {}", e))?;
        }
        write_atomic_0600(path, data)
    }
}

/// Read sensitive data from disk, decrypting if `os-keychain` is enabled.
pub fn read_sensitive(path: &Path) -> Result<Vec<u8>, String> {
    #[cfg(feature = "os-keychain")]
    {
        crate::secure_store::read_and_decrypt(path)
    }
    #[cfg(not(feature = "os-keychain"))]
    {
        fs::read(path).map_err(|e| format!("Failed to read file: {}", e))
    }
}

/// Atomically write `data` to `path`, with mode 0o600 on Unix.
///
/// Thin wrapper over [`crate::utils::fs_atomic::write_atomic`] that pins the
/// Unix mode at 0o600 for sensitive files. Callers are responsible for
/// ensuring the parent directory exists.
pub(crate) fn write_atomic_0600(path: &Path, data: &[u8]) -> Result<(), String> {
    crate::utils::fs_atomic::write_atomic(path, data, Some(0o600))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn write_atomic_0600_sets_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secret");
        write_atomic_0600(&path, b"hello").unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0o600, got {:o}", mode);
    }

    #[test]
    fn write_atomic_0600_leaves_no_tmp_after_success() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secret");
        write_atomic_0600(&path, b"payload").unwrap();
        let tmp_sibling = path.with_file_name("secret.tmp");
        assert!(path.exists());
        assert!(!tmp_sibling.exists(), "stale tmp file at {tmp_sibling:?}");
    }

    #[test]
    fn write_atomic_0600_overwrites_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secret");
        write_atomic_0600(&path, b"first").unwrap();
        write_atomic_0600(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
    }

    #[cfg(not(feature = "os-keychain"))]
    #[test]
    fn write_sensitive_round_trips_via_read_sensitive() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nested").join("secret");
        let payload = b"sensitive bytes \x00\x01\x02";
        write_sensitive(&path, payload).unwrap();
        assert_eq!(read_sensitive(&path).unwrap(), payload);
    }
}
