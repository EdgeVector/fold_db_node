//! Shared bootstrap helpers used by both `POST /api/setup/bootstrap`
//! (the canonical server-side path) and the CLI wizard wrapper.
//!
//! These helpers do not touch HTTP/Actix types — the route module
//! and the CLI wrap them.

use base64::Engine;
use fold_db::security::{Ed25519KeyPair, KeyUtils};
use serde::Deserialize;

/// Response body returned by the Exemem `POST /api/auth/cli/register`
/// endpoint. Only the fields fold_db_node consumes are deserialized;
/// extras are ignored so a future Exemem revision doesn't break us.
#[derive(Debug, Deserialize)]
pub struct ExememRegisterResponse {
    pub ok: bool,
    #[serde(default)]
    pub user_hash: Option<String>,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub message: Option<String>,
}

/// Hex-encode raw bytes (lower-case, no separator).
pub fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Sign the canonical CLI-register payload `"{public_key_hex}:{timestamp}"`
/// with the node's base64-encoded Ed25519 private key. Returns the base64
/// signature.
///
/// Must stay in sync with:
/// - `fold_db_node/src/server/routes/auth.rs::sign_payload`
/// - `exemem-infra/lambdas/auth_service/src/cli/types.rs::verify_ed25519_signature`
pub fn sign_cli_register_payload(
    private_key_b64: &str,
    public_key_hex: &str,
    timestamp: i64,
) -> Result<String, String> {
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(private_key_b64)
        .map_err(|e| format!("Failed to decode private key: {}", e))?;
    let key_pair = Ed25519KeyPair::from_secret_key(&key_bytes)
        .map_err(|e| format!("Failed to create key pair: {}", e))?;
    let payload = format!("{}:{}", public_key_hex, timestamp);
    let signature = key_pair.sign(payload.as_bytes());
    Ok(KeyUtils::signature_to_base64(&signature))
}

/// Register the node's public key with the Exemem API, optionally passing
/// an invite code.
///
/// Always sends a signed request — the caller must provide the node's
/// base64-encoded Ed25519 private key so the payload can be signed.
///
/// Synchronous (uses `reqwest::blocking`); the route handler wraps this in
/// `tokio::task::spawn_blocking`. Kept blocking so the CLI binary — which
/// has no tokio runtime around the call — can use it directly.
pub fn register_with_exemem_and_invite(
    api_url: &str,
    public_key_hex: &str,
    private_key_b64: &str,
    invite_code: Option<&str>,
) -> Result<ExememRegisterResponse, String> {
    let url = format!("{}/api/auth/cli/register", api_url.trim_end_matches('/'));
    let timestamp = chrono::Utc::now().timestamp();
    let signature = sign_cli_register_payload(private_key_b64, public_key_hex, timestamp)?;
    let mut body = serde_json::json!({
        "public_key": public_key_hex,
        "timestamp": timestamp,
        "signature": signature,
    });
    if let Some(code) = invite_code {
        body["invite_code"] = serde_json::Value::String(code.to_string());
    }

    // Run the blocking client on a dedicated thread so a tokio runtime
    // (when this is called from inside a `spawn_blocking`) isn't pinned by
    // the synchronous reqwest worker.
    let result = std::thread::spawn(move || {
        // skip-3p: outbound to Exemem cloud API; trace propagation is
        // injected at the route layer when we move to async reqwest.
        let client = reqwest::blocking::Client::new();
        client
            .post(&url)
            .json(&body)
            .timeout(std::time::Duration::from_secs(30))
            .send()
    })
    .join()
    .map_err(|_| "Registration request thread panicked".to_string())?
    .map_err(|e| format!("Failed to reach Exemem API: {}", e))?;

    let status = result.status();
    let body_text = result
        .text()
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    if !status.is_success() {
        let msg = serde_json::from_str::<serde_json::Value>(&body_text)
            .ok()
            .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
            .unwrap_or(body_text);
        return Err(format!(
            "Exemem registration failed (HTTP {}): {}",
            status, msg
        ));
    }

    let resp: ExememRegisterResponse = serde_json::from_str(&body_text)
        .map_err(|e| format!("Failed to parse registration response: {}", e))?;

    if !resp.ok {
        let msg = resp.message.unwrap_or_else(|| "Unknown error".to_string());
        return Err(format!("Exemem registration failed: {}", msg));
    }

    Ok(resp)
}

/// Reconstruct an Ed25519 keypair (NodeIdentity) from a 24-word BIP39
/// recovery phrase. Mirrors `handlers::auth::restore_from_phrase`'s
/// derivation so a phrase produced by [`derive_recovery_phrase`] yields
/// the original keypair.
pub fn identity_from_phrase(words: &str) -> Result<crate::identity::NodeIdentity, String> {
    let mnemonic = bip39::Mnemonic::parse(words.trim())
        .map_err(|e| format!("Invalid recovery phrase: {}", e))?;
    let entropy = mnemonic.to_entropy();
    if entropy.len() != 32 {
        return Err(format!(
            "Recovery phrase must encode 32 bytes, got {}",
            entropy.len()
        ));
    }
    let key_pair = Ed25519KeyPair::from_secret_key(&entropy)
        .map_err(|e| format!("Failed to derive keypair from phrase: {}", e))?;
    let private_key = base64::engine::general_purpose::STANDARD.encode(&entropy);
    let public_key = base64::engine::general_purpose::STANDARD.encode(key_pair.public_key_bytes());
    Ok(crate::identity::NodeIdentity {
        private_key,
        public_key,
    })
}

/// Derive a 24-word BIP39 recovery phrase from an Ed25519 private key.
///
/// Uses the first 32 bytes of the secret-key as entropy. Returns
/// `Err` if the decoded private key is shorter than 32 bytes.
pub fn derive_recovery_phrase(private_key_b64: &str) -> Result<Vec<String>, String> {
    let key_bytes = base64::engine::general_purpose::STANDARD
        .decode(private_key_b64)
        .map_err(|e| format!("Failed to decode private key: {}", e))?;

    if key_bytes.len() < 32 {
        return Err("Private key too short for recovery phrase".to_string());
    }
    let entropy = &key_bytes[..32];

    let mnemonic = bip39::Mnemonic::from_entropy(entropy)
        .map_err(|e| format!("Failed to generate mnemonic: {}", e))?;

    Ok(mnemonic.words().map(|w| w.to_string()).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_db::security::Ed25519KeyPair;

    #[test]
    fn sign_cli_register_payload_round_trips() {
        let key_pair = Ed25519KeyPair::generate().expect("generate keypair");
        let private_key_b64 = key_pair.secret_key_base64();
        let public_key_b64 = key_pair.public_key_base64();
        let pub_key_bytes = base64::engine::general_purpose::STANDARD
            .decode(&public_key_b64)
            .expect("decode pub key");
        let public_key_hex = hex_encode(&pub_key_bytes);

        let timestamp: i64 = 1_700_000_000;
        let sig_b64 = sign_cli_register_payload(&private_key_b64, &public_key_hex, timestamp)
            .expect("sign payload");
        let signature = KeyUtils::signature_from_base64(&sig_b64).expect("sig from b64");
        let payload = format!("{}:{}", public_key_hex, timestamp);
        assert!(
            key_pair.verify(payload.as_bytes(), &signature),
            "signature should verify against canonical payload"
        );
    }

    #[test]
    fn sign_cli_register_payload_rejects_invalid_private_key() {
        assert!(sign_cli_register_payload("not-valid-base64!!!", "deadbeef", 123).is_err());
    }

    #[test]
    fn derive_recovery_phrase_returns_24_words() {
        let key_pair = Ed25519KeyPair::generate().expect("generate keypair");
        let words = derive_recovery_phrase(&key_pair.secret_key_base64()).expect("derive");
        assert_eq!(words.len(), 24, "BIP39 256-bit entropy yields 24 words");
    }

    #[test]
    fn derive_recovery_phrase_rejects_short_key() {
        let short_b64 = base64::engine::general_purpose::STANDARD.encode([0u8; 16]);
        assert!(derive_recovery_phrase(&short_b64).is_err());
    }
}
