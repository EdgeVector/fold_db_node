//! Cross-platform credential storage.
//!
//! Stores Exemem credentials as a JSON file at `~/.folddb/credentials.json`.
//! No OS-specific APIs (keychain, credential manager) — works on macOS, Linux,
//! Windows, Docker, and CI.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

const CREDENTIALS_DIR: &str = ".folddb";
const CREDENTIALS_FILE: &str = "credentials.json";

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
    let home = dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?;
    Ok(home.join(CREDENTIALS_DIR).join(CREDENTIALS_FILE))
}

/// Store Exemem credentials to disk.
pub fn store_credentials(creds: &ExememCredentials) -> Result<(), String> {
    let path = credentials_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
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
        fs::write(&path, &json).map_err(|e| format!("Failed to write credentials: {}", e))?;
    }

    Ok(())
}

/// Load Exemem credentials from disk.
/// Returns None if no credentials are stored.
pub fn load_credentials() -> Result<Option<ExememCredentials>, String> {
    let path = credentials_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let json =
        fs::read_to_string(&path).map_err(|e| format!("Failed to read credentials: {}", e))?;

    let creds: ExememCredentials = serde_json::from_str(&json)
        .map_err(|e| format!("Failed to deserialize credentials: {}", e))?;

    Ok(Some(creds))
}

/// Delete Exemem credentials from disk.
pub fn delete_credentials() -> Result<(), String> {
    let path = credentials_path()?;
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("Failed to delete credentials: {}", e))?;
    }
    Ok(())
}

/// Check if credentials exist without loading them.
pub fn has_credentials() -> bool {
    credentials_path().map(|p| p.exists()).unwrap_or(false)
}
