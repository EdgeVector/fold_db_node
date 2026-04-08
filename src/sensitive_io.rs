//! Unified read/write for sensitive files (identity keys, E2E keys, credentials).
//!
//! - `os-keychain` enabled: encrypts via OS keychain master key (AES-256-GCM)
//! - `os-keychain` disabled: writes plaintext with 0o600 Unix permissions

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
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        }
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
                .map_err(|e| format!("Failed to open file: {}", e))?;
            let mut w = std::io::BufWriter::new(file);
            w.write_all(data)
                .map_err(|e| format!("Failed to write file: {}", e))?;
        }
        #[cfg(not(unix))]
        {
            std::fs::write(path, data).map_err(|e| format!("Failed to write file: {}", e))?;
        }
        Ok(())
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
        std::fs::read(path).map_err(|e| format!("Failed to read file: {}", e))
    }
}
