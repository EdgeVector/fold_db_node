//! OS keychain-backed encryption for sensitive files.
//!
//! When the `os-keychain` feature is enabled, a random 32-byte master key is
//! stored in the OS keychain (macOS Keychain / Windows Credential Manager /
//! Linux Secret Service). All sensitive files on disk (node_identity, e2e key,
//! credentials) are encrypted with this master key using AES-256-GCM.
//!
//! When the feature is disabled (dev mode), this module is not compiled and
//! files remain plaintext — matching the current SSH-like security model.

use fold_db::crypto::envelope::{decrypt_envelope, encrypt_envelope};
use rand::RngCore;

const KEYCHAIN_SERVICE: &str = "com.folddb.node";
const KEYCHAIN_MASTER_KEY: &str = "master-key";

/// Retrieve the master key from the OS keychain, or generate and store a new one.
pub fn get_or_create_master_key() -> Result<[u8; 32], String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_MASTER_KEY)
        .map_err(|e| format!("Failed to access OS keychain: {}", e))?;

    // Try to load existing key
    match entry.get_secret() {
        Ok(bytes) => {
            if bytes.len() != 32 {
                return Err(format!(
                    "Master key in OS keychain has invalid length: {} (expected 32)",
                    bytes.len()
                ));
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            Ok(key)
        }
        Err(keyring::Error::NoEntry) => {
            // Generate a new random master key
            let mut key = [0u8; 32];
            rand::rngs::OsRng.fill_bytes(&mut key);
            entry
                .set_secret(&key)
                .map_err(|e| format!("Failed to store master key in OS keychain: {}", e))?;
            tracing::info!("Generated and stored new master key in OS keychain");
            Ok(key)
        }
        Err(e) => Err(format!("Failed to read master key from OS keychain: {}", e)),
    }
}

/// Encrypt data with the master key and write to disk.
pub fn encrypt_and_write(path: &std::path::Path, plaintext: &[u8]) -> Result<(), String> {
    let master_key = get_or_create_master_key()?;
    let envelope = encrypt_envelope(&master_key, plaintext)
        .map_err(|e| format!("Failed to encrypt: {}", e))?;

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
            .map_err(|e| format!("Failed to open file for writing: {}", e))?;
        let mut writer = std::io::BufWriter::new(file);
        writer
            .write_all(&envelope)
            .map_err(|e| format!("Failed to write encrypted file: {}", e))?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(path, &envelope)
            .map_err(|e| format!("Failed to write encrypted file: {}", e))?;
    }

    Ok(())
}

/// Read an encrypted file from disk and decrypt with the master key.
pub fn read_and_decrypt(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let master_key = get_or_create_master_key()?;
    let envelope =
        std::fs::read(path).map_err(|e| format!("Failed to read encrypted file: {}", e))?;
    decrypt_envelope(&master_key, &envelope).map_err(|e| format!("Failed to decrypt: {}", e))
}

/// Delete the master key from the OS keychain (e.g. on node reset).
pub fn delete_master_key() -> Result<(), String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_MASTER_KEY)
        .map_err(|e| format!("Failed to access OS keychain: {}", e))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // Already gone
        Err(e) => Err(format!(
            "Failed to delete master key from OS keychain: {}",
            e
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        // Test the envelope encrypt/decrypt directly (doesn't need OS keychain)
        let key = [0x42u8; 32];
        let plaintext = b"test credentials json";
        let envelope = encrypt_envelope(&key, plaintext).unwrap();
        let decrypted = decrypt_envelope(&key, &envelope).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_encrypt_and_write_read_and_decrypt() {
        // This test requires OS keychain access — skip in CI
        if std::env::var("CI").is_ok() {
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test_creds.enc");
        let plaintext = b"sensitive data here";

        encrypt_and_write(&path, plaintext).unwrap();
        assert!(path.exists());

        // File on disk should NOT be plaintext
        let raw = std::fs::read(&path).unwrap();
        assert_ne!(raw, plaintext);

        // Decrypted should match
        let decrypted = read_and_decrypt(&path).unwrap();
        assert_eq!(decrypted, plaintext);
    }
}
