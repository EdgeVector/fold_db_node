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
    /// Base64-encoded AES-256 encryption key (32 bytes)
    pub encryption_key: String,
}

fn credentials_path() -> Result<PathBuf, String> {
    Ok(folddb_home()?.join(CREDENTIALS_FILE))
}

// ============================================================
// os-keychain ENABLED — encrypt credentials via OS keychain
// ============================================================

#[cfg(feature = "os-keychain")]
pub fn store_credentials(creds: &ExememCredentials) -> Result<(), String> {
    let path = credentials_path()?;
    let json = serde_json::to_string_pretty(creds)
        .map_err(|e| format!("Failed to serialize credentials: {}", e))?;
    crate::secure_store::encrypt_and_write(&path, json.as_bytes())
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

// ============================================================
// os-keychain DISABLED — plaintext JSON (dev mode)
// ============================================================

#[cfg(not(feature = "os-keychain"))]
pub fn store_credentials(creds: &ExememCredentials) -> Result<(), String> {
    let path = credentials_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create credentials directory: {}", e))?;
    }

    let json = serde_json::to_string_pretty(creds)
        .map_err(|e| format!("Failed to serialize credentials: {}", e))?;

    // Set restrictive permissions on Unix before writing
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| format!("Failed to open credentials file: {}", e))?;
        use std::io::Write;
        let mut writer = std::io::BufWriter::new(file);
        writer
            .write_all(json.as_bytes())
            .map_err(|e| format!("Failed to write credentials: {}", e))?;
    }

    #[cfg(not(unix))]
    {
        std::fs::write(&path, &json).map_err(|e| format!("Failed to write credentials: {}", e))?;
    }

    Ok(())
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
pub fn delete_credentials() -> Result<(), String> {
    let path = credentials_path()?;
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| format!("Failed to delete credentials: {}", e))?;
    }
    // Also clean up the other filename variant if it exists (e.g. after toggling the feature)
    let alt_path = folddb_home()?.join(if cfg!(feature = "os-keychain") {
        "credentials.json"
    } else {
        "credentials.enc"
    });
    if alt_path.exists() {
        let _ = std::fs::remove_file(&alt_path);
    }
    Ok(())
}

/// Check if credentials exist without loading them.
pub fn has_credentials() -> bool {
    credentials_path().map(|p| p.exists()).unwrap_or(false)
}
