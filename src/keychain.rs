use serde::{Deserialize, Serialize};

const SERVICE_NAME: &str = "com.exemem.folddb";
const CREDENTIALS_KEY: &str = "credentials";

/// Credentials stored in the OS keychain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExememCredentials {
    pub user_hash: String,
    pub session_token: String,
    pub api_key: String,
    /// Base64-encoded AES-256 encryption key (32 bytes)
    pub encryption_key: String,
}

/// Store Exemem credentials in the OS keychain.
pub fn store_credentials(creds: &ExememCredentials) -> Result<(), String> {
    let json = serde_json::to_string(creds)
        .map_err(|e| format!("Failed to serialize credentials: {}", e))?;

    let entry = keyring::Entry::new(SERVICE_NAME, CREDENTIALS_KEY)
        .map_err(|e| format!("Failed to create keyring entry: {}", e))?;

    entry
        .set_password(&json)
        .map_err(|e| format!("Failed to store in keychain: {}", e))?;

    Ok(())
}

/// Load Exemem credentials from the OS keychain.
/// Returns None if no credentials are stored.
pub fn load_credentials() -> Result<Option<ExememCredentials>, String> {
    let entry = keyring::Entry::new(SERVICE_NAME, CREDENTIALS_KEY)
        .map_err(|e| format!("Failed to create keyring entry: {}", e))?;

    match entry.get_password() {
        Ok(json) => {
            let creds: ExememCredentials = serde_json::from_str(&json)
                .map_err(|e| format!("Failed to deserialize credentials: {}", e))?;
            Ok(Some(creds))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Failed to load from keychain: {}", e)),
    }
}

/// Delete Exemem credentials from the OS keychain.
pub fn delete_credentials() -> Result<(), String> {
    let entry = keyring::Entry::new(SERVICE_NAME, CREDENTIALS_KEY)
        .map_err(|e| format!("Failed to create keyring entry: {}", e))?;

    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // Already deleted
        Err(e) => Err(format!("Failed to delete from keychain: {}", e)),
    }
}

/// Check if credentials exist in the OS keychain without loading them.
pub fn has_credentials() -> bool {
    load_credentials().ok().flatten().is_some()
}
