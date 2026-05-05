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
use std::io::{Seek, SeekFrom, Write};
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

/// Overwrite `path` with zeros, fsync, then unlink.
///
/// Best-effort defense against filesystem forensics recovering plaintext
/// after a logical delete. The encrypted-credentials variant doesn't need
/// this — its master key in the OS keychain is the canonical secret — but
/// we run the same path on both variants for consistency. If `path`
/// doesn't exist, returns `Ok(())`.
pub fn shred_and_remove(path: &Path) -> Result<(), String> {
    let metadata = match fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("Failed to stat {}: {}", path.display(), e)),
    };
    let len = metadata.len();

    if len > 0 {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(path)
            .map_err(|e| format!("Failed to open {} for shredding: {}", path.display(), e))?;
        file.seek(SeekFrom::Start(0))
            .map_err(|e| format!("Failed to seek {}: {}", path.display(), e))?;
        // Stream zeros in 8 KiB chunks so we don't allocate len bytes for huge files.
        let zeros = [0u8; 8192];
        let mut remaining = len;
        while remaining > 0 {
            let chunk = std::cmp::min(remaining, zeros.len() as u64) as usize;
            file.write_all(&zeros[..chunk])
                .map_err(|e| format!("Failed to overwrite {}: {}", path.display(), e))?;
            remaining -= chunk as u64;
        }
        file.sync_all()
            .map_err(|e| format!("Failed to fsync {}: {}", path.display(), e))?;
        drop(file);
    }

    fs::remove_file(path).map_err(|e| format!("Failed to remove {}: {}", path.display(), e))?;
    Ok(())
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

    #[test]
    fn shred_and_remove_is_noop_when_file_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("does-not-exist");
        shred_and_remove(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn shred_and_remove_unlinks_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secret");
        fs::write(&path, b"plaintext-secret").unwrap();
        shred_and_remove(&path).unwrap();
        assert!(!path.exists());
    }

    /// On Unix, hardlinking the file before shred_and_remove gives us a
    /// second name pointing to the same inode. After the original is
    /// unlinked, the hardlink lets us inspect the inode's contents — they
    /// must be zeros, not the original plaintext.
    #[cfg(unix)]
    #[test]
    fn shred_and_remove_overwrites_inode_before_unlink() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secret");
        let observer = tmp.path().join("observer");
        let plaintext = b"super-secret-plaintext-bytes";
        fs::write(&path, plaintext).unwrap();
        fs::hard_link(&path, &observer).unwrap();

        shred_and_remove(&path).unwrap();
        assert!(!path.exists(), "primary path should be unlinked");
        assert!(observer.exists(), "hardlink keeps inode alive");

        let contents = fs::read(&observer).unwrap();
        assert_eq!(contents.len(), plaintext.len(), "size preserved");
        assert!(
            contents.iter().all(|&b| b == 0),
            "inode bytes were not zeroed before unlink"
        );
        assert_ne!(contents, plaintext, "plaintext leaked through hardlink");
    }

    #[test]
    fn shred_and_remove_propagates_open_error_on_directory_target() {
        let tmp = tempfile::tempdir().unwrap();
        let dir_path = tmp.path().join("not-a-file");
        fs::create_dir(&dir_path).unwrap();
        // Put a file inside so directory size > 0 on filesystems that report it.
        fs::write(dir_path.join("inner"), b"x").unwrap();
        let err = shred_and_remove(&dir_path).expect_err("opening a directory for write must fail");
        assert!(
            err.contains("Failed to open") || err.contains("Failed to remove"),
            "unexpected error: {err}"
        );
    }
}
