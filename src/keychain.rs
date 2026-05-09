//! Cross-platform credential storage.
//!
//! Stores Exemem credentials at `$FOLDDB_HOME/credentials.json` (or `.enc`
//! when the `os-keychain` feature is enabled).
//!
//! - **`os-keychain` enabled** (release builds): credentials are encrypted at
//!   rest with a master key stored in the OS keychain (macOS Keychain / Windows
//!   Credential Manager / Linux Secret Service).
//! - **`os-keychain` disabled** (dev mode): credentials are stored as plaintext
//!   JSON with 0o600 file permissions, matching the SSH key security model.

use crate::utils::paths::folddb_home;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[cfg(not(feature = "os-keychain"))]
const CREDENTIALS_FILE: &str = "credentials.json";
#[cfg(feature = "os-keychain")]
const CREDENTIALS_FILE: &str = "credentials.enc";

/// Credentials stored locally
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExememCredentials {
    pub user_hash: String,
    pub session_token: String,
    pub api_key: String,
}

fn credentials_path() -> Result<PathBuf, String> {
    Ok(folddb_home()?.join(CREDENTIALS_FILE))
}

pub fn store_credentials(creds: &ExememCredentials) -> Result<(), String> {
    let path = credentials_path()?;
    let json = serde_json::to_string_pretty(creds)
        .map_err(|e| format!("Failed to serialize credentials: {}", e))?;
    crate::sensitive_io::write_sensitive(&path, json.as_bytes())
}

#[cfg(feature = "os-keychain")]
pub fn load_credentials() -> Result<Option<ExememCredentials>, String> {
    let path = credentials_path()?;
    if !path.exists() {
        return Ok(None);
    }
    let plaintext = crate::secure_store::read_and_decrypt(&path)?;
    let json = String::from_utf8(plaintext)
        .map_err(|e| format!("Decrypted credentials are not valid UTF-8: {}", e))?;
    let creds: ExememCredentials = serde_json::from_str(&json)
        .map_err(|e| format!("Failed to deserialize credentials: {}", e))?;
    Ok(Some(creds))
}

#[cfg(not(feature = "os-keychain"))]
pub fn load_credentials() -> Result<Option<ExememCredentials>, String> {
    let path = credentials_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let json =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read credentials: {}", e))?;

    let creds: ExememCredentials = serde_json::from_str(&json)
        .map_err(|e| format!("Failed to deserialize credentials: {}", e))?;

    Ok(Some(creds))
}

/// Delete Exemem credentials from disk.
///
/// Both file variants (`credentials.json` plaintext, `credentials.enc`
/// encrypted) are overwritten with zeros + fsynced before unlink. The
/// encrypted variant doesn't strictly need shredding (the master key in
/// the OS keychain stays the canonical secret), but we run the same path
/// for consistency.
///
/// The two unlinks are intentionally non-atomic: if a crash happens
/// between them after a feature-flag toggle, the surviving variant is
/// reaped on the next call. Worst case is one extra round-trip, not
/// data loss, so the temp-marker dance isn't worth it.
pub fn delete_credentials() -> Result<(), String> {
    let path = credentials_path()?;
    crate::sensitive_io::shred_and_remove(&path)?;

    let alt_path = folddb_home()?.join(if cfg!(feature = "os-keychain") {
        "credentials.json"
    } else {
        "credentials.enc"
    });
    crate::sensitive_io::shred_and_remove(&alt_path)?;
    Ok(())
}

/// Check if credentials exist without loading them.
pub fn has_credentials() -> bool {
    credentials_path().map(|p| p.exists()).unwrap_or(false)
}

#[cfg(all(test, not(feature = "os-keychain"), unix))]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn delete_credentials_shreds_plaintext_before_unlink() {
        let _guard = crate::secure_store::test_master_key::lock();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("FOLDDB_HOME", tmp.path());

        let creds = ExememCredentials {
            user_hash: "user-hash-secret".to_string(),
            session_token: "session-token-secret".to_string(),
            api_key: "api-key-secret".to_string(),
        };
        store_credentials(&creds).unwrap();

        let path = tmp.path().join("credentials.json");
        let observer = tmp.path().join("credentials.json.observer");
        fs::hard_link(&path, &observer).unwrap();
        let original_len = fs::metadata(&observer).unwrap().len();
        assert!(original_len > 0);

        delete_credentials().unwrap();

        assert!(!path.exists(), "credentials.json should be gone");
        let leaked = fs::read(&observer).unwrap();
        assert_eq!(leaked.len() as u64, original_len, "size preserved");
        assert!(
            leaked.iter().all(|&b| b == 0),
            "plaintext bytes were not shredded before unlink: {:?}",
            String::from_utf8_lossy(&leaked)
        );

        std::env::remove_var("FOLDDB_HOME");
    }

    #[test]
    fn delete_credentials_propagates_alt_path_error() {
        let _guard = crate::secure_store::test_master_key::lock();
        let tmp = tempfile::tempdir().unwrap();
        std::env::set_var("FOLDDB_HOME", tmp.path());

        // Primary (dev mode) is credentials.json; alt is credentials.enc.
        let creds = ExememCredentials {
            user_hash: "u".to_string(),
            session_token: "s".to_string(),
            api_key: "k".to_string(),
        };
        store_credentials(&creds).unwrap();

        // Plant a directory at the alt path so shred_and_remove fails on it
        // (open-for-write on a directory errors). The fact we get an Err
        // proves the alt-path error is no longer being swallowed.
        let alt = tmp.path().join("credentials.enc");
        fs::create_dir(&alt).unwrap();
        fs::write(alt.join("inner"), b"x").unwrap();

        let result = delete_credentials();
        assert!(result.is_err(), "alt-path failure should propagate, got Ok");

        std::env::remove_var("FOLDDB_HOME");
    }
}
