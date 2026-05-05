//! Crash-safe file writes via tmpfile + fsync + rename.
//!
//! Stages bytes in `<path>.tmp`, fsyncs the tmp file, renames onto `path`,
//! and best-effort fsyncs the parent directory. Power loss (or a daemon
//! crash) between steps therefore leaves either the previous good file or
//! the staged tmpfile — never a half-written final path. Callers are
//! responsible for ensuring the parent directory exists.
//!
//! [`crate::sensitive_io::write_atomic_0600`] delegates here with
//! `mode = Some(0o600)`; plaintext config writers pass `mode = None` so
//! the umask applies (typically 0o644).

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Atomically write `data` to `path`.
///
/// On Unix, when `mode` is `Some(m)` the tmp file is opened with that mode
/// (which then survives the rename). When `mode` is `None`, the file is
/// created with default options so the process umask applies. On non-Unix
/// platforms the `mode` argument is ignored.
pub fn write_atomic(path: &Path, data: &[u8], mode: Option<u32>) -> Result<(), String> {
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
            let mut opts = OpenOptions::new();
            opts.write(true).create(true).truncate(true);
            if let Some(m) = mode {
                opts.mode(m);
            }
            opts.open(&tmp_path)
                .map_err(|e| format!("Failed to open temp file {}: {}", tmp_path.display(), e))?
        };
        #[cfg(not(unix))]
        let mut file = {
            let _ = mode;
            fs::File::create(&tmp_path)
                .map_err(|e| format!("Failed to create temp file {}: {}", tmp_path.display(), e))?
        };

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
    fn write_atomic_with_explicit_mode_sets_unix_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secret");
        write_atomic(&path, b"hello", Some(0o600)).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0o600, got {:o}", mode);
    }

    #[cfg(unix)]
    #[test]
    fn write_atomic_without_mode_honors_umask() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("plain.json");
        write_atomic(&path, b"hello", None).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        // Whatever the umask gave us, it must not be the locked-down 0o600
        // that sensitive_io uses — that's the regression this guards against.
        assert_ne!(mode, 0o600, "plaintext write must not use 0o600");
        // The Unix default with the standard 0o022 umask is 0o644; allow the
        // group/other bits to be looser if a tighter umask is in effect.
        assert!(
            mode & 0o600 == 0o600,
            "owner read+write missing, got {:o}",
            mode
        );
    }

    #[test]
    fn write_atomic_leaves_no_tmp_after_success() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("plain.json");
        write_atomic(&path, b"payload", None).unwrap();
        let tmp_sibling = path.with_file_name("plain.json.tmp");
        assert!(path.exists());
        assert!(!tmp_sibling.exists(), "stale tmp file at {tmp_sibling:?}");
    }

    #[test]
    fn write_atomic_overwrites_preserve_atomicity() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("plain.json");
        write_atomic(&path, b"first", None).unwrap();
        write_atomic(&path, b"second", None).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
    }
}
