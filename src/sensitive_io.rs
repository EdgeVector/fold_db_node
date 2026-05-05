//! Unified read/write for sensitive files (identity keys, E2E keys, credentials).
//!
//! - `os-keychain` enabled: encrypts via OS keychain master key (AES-256-GCM)
//! - `os-keychain` disabled: writes plaintext with 0o600 Unix permissions
//!
//! All writes go through [`write_atomic_0600`], which stages bytes into
//! `<path>.tmp`, fsyncs, then renames onto the target. Power loss between
//! steps therefore leaves either the previous good file or the staged
//! tmpfile — never a half-written final path. AES-GCM auth-tag failures
//! from a torn write would force the user to re-enter the credential, so
//! the rename atomicity is what protects them.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

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
/// Stages the bytes in `<path>.tmp`, fsyncs the file, renames onto `path`,
/// then best-effort fsyncs the parent directory. On any error, the tmp
/// file is removed so a retry starts clean. Callers are responsible for
/// ensuring the parent directory exists.
pub(crate) fn write_atomic_0600(path: &Path, data: &[u8]) -> Result<(), String> {
    let tmp_path = {
        let mut s: OsString = path.as_os_str().into();
        s.push(".tmp");
        PathBuf::from(s)
    };

    let result: Result<(), String> = (|| {
        #[cfg(unix)]
        let mut file = {
            use std::fs::OpenOptions;
            use std::os::unix::fs::OpenOptionsExt;
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)
                .map_err(|e| format!("Failed to open temp file {}: {}", tmp_path.display(), e))?
        };
        #[cfg(not(unix))]
        let mut file = fs::File::create(&tmp_path)
            .map_err(|e| format!("Failed to create temp file {}: {}", tmp_path.display(), e))?;

        file.write_all(data)
            .map_err(|e| format!("Failed to write temp file {}: {}", tmp_path.display(), e))?;
        file.sync_all()
            .map_err(|e| format!("Failed to fsync temp file {}: {}", tmp_path.display(), e))?;
        drop(file);

        fs::rename(&tmp_path, path).map_err(|e| {
            format!(
                "Failed to rename {} -> {}: {}",
                tmp_path.display(),
                path.display(),
                e
            )
        })?;

        #[cfg(unix)]
        if let Some(parent) = path.parent() {
            // Best-effort: parent-dir fsync hardens the rename against power
            // loss. A failure here does not unwind the successful rename.
            let _ = fs::File::open(parent).and_then(|d| d.sync_all());
        }

        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(&tmp_path);
    }
    result
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
