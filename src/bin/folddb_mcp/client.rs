use std::process::Command;

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::McpError;

/// Maximum number of retries when auto-starting the daemon.
const DAEMON_START_RETRIES: u32 = 5;
/// Delay between retries in milliseconds.
const DAEMON_RETRY_DELAY_MS: u64 = 2000;

/// HTTP client that talks to the FoldDB server.
pub struct FoldDbClient {
    http: reqwest::Client,
    base_url: String,
    user_hash: String,
}

impl FoldDbClient {
    /// Connect to the running FoldDB server, fetch public key, derive identity.
    /// If the daemon is not running, attempts to auto-start it via `folddb daemon start`.
    pub async fn connect(port: u16) -> Result<Self, McpError> {
        let base_url = format!("http://127.0.0.1:{}", port);
        // trace-egress: loopback (MCP client -> local daemon; .send() wrapped with inject_w3c below)
        let http = reqwest::Client::new();

        // Health check — if it fails, try to auto-start the daemon
        let status_url = format!("{}/api/system/status", base_url);
        let probe = observability::propagation::inject_w3c(http.get(&status_url));
        if probe.send().await.is_err() {
            eprintln!(
                "[folddb-mcp] Daemon not running on port {}. Attempting to auto-start...",
                port
            );
            Self::try_start_daemon(port)?;

            // Retry health check with backoff
            let mut connected = false;
            for attempt in 1..=DAEMON_START_RETRIES {
                tokio::time::sleep(std::time::Duration::from_millis(DAEMON_RETRY_DELAY_MS)).await;
                let retry = observability::propagation::inject_w3c(http.get(&status_url));
                if retry.send().await.is_ok() {
                    eprintln!(
                        "[folddb-mcp] Daemon started successfully (attempt {}/{})",
                        attempt, DAEMON_START_RETRIES
                    );
                    connected = true;
                    break;
                }
                eprintln!(
                    "[folddb-mcp] Waiting for daemon to start (attempt {}/{})",
                    attempt, DAEMON_START_RETRIES
                );
            }
            if !connected {
                return Err(McpError::ServerNotRunning(format!(
                    "Failed to start FoldDB daemon on port {} after {} retries",
                    port, DAEMON_START_RETRIES
                )));
            }
        }

        // Fetch public key and derive user_hash
        let pubkey_request = observability::propagation::inject_w3c(
            http.get(format!("{}/api/system/public-key", base_url))
                .header("x-user-hash", "mcp_bootstrap"),
        );
        let pub_resp: Value = pubkey_request.send().await?.json().await?;

        // Response format: {"public_key": "<base64>", "success": true, ...}
        let pub_b64 = pub_resp
            .get("public_key")
            .or_else(|| pub_resp.pointer("/data/key"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Failed to extract public key from response: {}", pub_resp),
                ))
            })?;

        let pub_bytes = BASE64.decode(pub_b64).map_err(|e| {
            McpError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Invalid base64 in public key: {}", e),
            ))
        })?;

        let user_hash = derive_user_hash(&pub_bytes);

        eprintln!(
            "[folddb-mcp] Connected to {} (user_hash: {})",
            base_url, user_hash
        );

        Ok(Self {
            http,
            base_url,
            user_hash,
        })
    }

    /// Attempt to start the FoldDB daemon via the CLI.
    fn try_start_daemon(port: u16) -> Result<(), McpError> {
        // Resolve the folddb binary — prefer sibling binary next to folddb_mcp
        let folddb_bin = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|dir| dir.join("folddb")))
            .filter(|p| p.exists())
            .unwrap_or_else(|| std::path::PathBuf::from("folddb"));

        eprintln!(
            "[folddb-mcp] Starting daemon: {} daemon start --port {}",
            folddb_bin.display(),
            port
        );

        Command::new(&folddb_bin)
            .args(["daemon", "start", "--port", &port.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| {
                McpError::Io(std::io::Error::new(
                    e.kind(),
                    format!("Failed to spawn '{}': {}", folddb_bin.display(), e),
                ))
            })?;

        Ok(())
    }

    /// GET request.
    pub async fn get(&self, path: &str) -> Result<Value, McpError> {
        let request = observability::propagation::inject_w3c(
            self.http
                .get(format!("{}{}", self.base_url, path))
                .header("x-user-hash", &self.user_hash),
        );
        let resp = request.send().await?.json().await?;
        Ok(resp)
    }

    /// POST request.
    pub async fn post(&self, path: &str, body: &Value) -> Result<Value, McpError> {
        let request = observability::propagation::inject_w3c(
            self.http
                .post(format!("{}{}", self.base_url, path))
                .header("x-user-hash", &self.user_hash)
                .header("Content-Type", "application/json")
                .json(body),
        );
        let resp = request.send().await?.json().await?;
        Ok(resp)
    }
}

/// Derive user_hash from raw public key bytes.
/// Algorithm: SHA-256(raw_bytes) -> first 16 bytes -> hex-encode -> 32-char string.
/// Must match fold_db_node/src/utils/crypto.rs:user_hash_from_pubkey
fn derive_user_hash(pub_key_bytes: &[u8]) -> String {
    let digest = Sha256::digest(pub_key_bytes);
    digest[..16].iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_user_hash_length() {
        let key = [0x42u8; 32];
        let hash = derive_user_hash(&key);
        assert_eq!(hash.len(), 32);
        assert!(hash
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn test_derive_user_hash_deterministic() {
        let key = [0x42u8; 32];
        assert_eq!(derive_user_hash(&key), derive_user_hash(&key));
    }

    #[test]
    fn test_derive_user_hash_different_keys() {
        let h1 = derive_user_hash(&[0x01u8; 32]);
        let h2 = derive_user_hash(&[0x02u8; 32]);
        assert_ne!(h1, h2);
    }
}
