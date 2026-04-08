use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use ed25519_dalek::{Signer, SigningKey};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::McpError;

const KEY_ID: &str = "SYSTEM_WIDE_PUBLIC_KEY";

/// SignedMessage envelope matching fold_db's security::SignedMessage
#[derive(Serialize)]
struct SignedMessage {
    payload: String,
    public_key_id: String,
    signature: String,
    timestamp: i64,
}

/// HTTP client that talks to the FoldDB server with transparent Ed25519 signing.
pub struct FoldDbClient {
    http: reqwest::Client,
    base_url: String,
    signing_key: SigningKey,
    user_hash: String,
}

impl FoldDbClient {
    /// Connect to the running FoldDB server, fetch keys, derive identity.
    pub async fn connect(port: u16) -> Result<Self, McpError> {
        let base_url = format!("http://127.0.0.1:{}", port);
        let http = reqwest::Client::new();

        // Health check
        let status_url = format!("{}/api/system/status", base_url);
        http.get(&status_url).send().await.map_err(|e| {
            McpError::ServerNotRunning(format!(
                "Cannot reach FoldDB at {}. Is the app running? ({})",
                base_url, e
            ))
        })?;

        // Fetch private key (needs x-user-hash header)
        let pk_resp: Value = http
            .get(format!("{}/api/system/private-key", base_url))
            .header("x-user-hash", "mcp_bootstrap")
            .send()
            .await?
            .json()
            .await?;

        // Response format: {"private_key": "<base64>", "success": true, ...}
        // Also handle wrapped format: {"ok": true, "data": {"key": "<base64>"}}
        let key_b64 = pk_resp
            .get("private_key")
            .or_else(|| pk_resp.pointer("/data/key"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::Signing(format!(
                    "Failed to extract private key from response: {}",
                    pk_resp
                ))
            })?;

        let key_bytes = BASE64
            .decode(key_b64)
            .map_err(|e| McpError::Signing(format!("Invalid base64 in private key: {}", e)))?;

        let signing_key = SigningKey::from_bytes(
            key_bytes
                .as_slice()
                .try_into()
                .map_err(|_| McpError::Signing("Private key must be 32 bytes".to_string()))?,
        );

        // Fetch public key and derive user_hash
        let pub_resp: Value = http
            .get(format!("{}/api/system/public-key", base_url))
            .header("x-user-hash", "mcp_bootstrap")
            .send()
            .await?
            .json()
            .await?;

        // Response format: {"public_key": "<base64>", "success": true, ...}
        let pub_b64 = pub_resp
            .get("public_key")
            .or_else(|| pub_resp.pointer("/data/key"))
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                McpError::Signing(format!(
                    "Failed to extract public key from response: {}",
                    pub_resp
                ))
            })?;

        let pub_bytes = BASE64
            .decode(pub_b64)
            .map_err(|e| McpError::Signing(format!("Invalid base64 in public key: {}", e)))?;

        let user_hash = derive_user_hash(&pub_bytes);

        eprintln!(
            "[folddb-mcp] Connected to {} (user_hash: {})",
            base_url, user_hash
        );

        Ok(Self {
            http,
            base_url,
            signing_key,
            user_hash,
        })
    }

    /// GET request (no signing needed).
    pub async fn get(&self, path: &str) -> Result<Value, McpError> {
        let resp = self
            .http
            .get(format!("{}{}", self.base_url, path))
            .header("x-user-hash", &self.user_hash)
            .send()
            .await?
            .json()
            .await?;
        Ok(resp)
    }

    /// POST request to an unsigned/exempt path.
    pub async fn post_unsigned(&self, path: &str, body: &Value) -> Result<Value, McpError> {
        let resp = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .header("x-user-hash", &self.user_hash)
            .header("Content-Type", "application/json")
            .json(body)
            .send()
            .await?
            .json()
            .await?;
        Ok(resp)
    }

    /// POST request to a signature-protected path.
    pub async fn post_signed(&self, path: &str, body: &Value) -> Result<Value, McpError> {
        let signed = self.sign_payload(body)?;
        let resp = self
            .http
            .post(format!("{}{}", self.base_url, path))
            .header("x-user-hash", &self.user_hash)
            .header("Content-Type", "application/json")
            .json(&signed)
            .send()
            .await?
            .json()
            .await?;
        Ok(resp)
    }

    /// Create a SignedMessage envelope matching fold_db's MessageSigner::sign_message.
    ///
    /// Message format: payload_bytes || timestamp_be_bytes(8) || key_id_bytes
    /// Reference: fold_db/src/security/signing.rs:41-44
    fn sign_payload(&self, payload: &Value) -> Result<SignedMessage, McpError> {
        let payload_bytes = serde_json::to_vec(payload)?;
        let timestamp = chrono::Utc::now().timestamp();

        let mut message = payload_bytes.clone();
        message.extend_from_slice(&timestamp.to_be_bytes());
        message.extend_from_slice(KEY_ID.as_bytes());

        let signature = self.signing_key.sign(&message);

        Ok(SignedMessage {
            payload: BASE64.encode(&payload_bytes),
            public_key_id: KEY_ID.to_string(),
            signature: BASE64.encode(signature.to_bytes()),
            timestamp,
        })
    }
}

/// Derive user_hash from raw public key bytes.
/// Algorithm: SHA-256(raw_bytes) → first 16 bytes → hex-encode → 32-char string.
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
