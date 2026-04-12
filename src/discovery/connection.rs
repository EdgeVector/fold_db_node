//! Connection request encryption and local state management.
//!
//! Uses X25519 ECDH + AES-256-GCM for E2E encrypted connection requests.
//! Each pseudonym derives its own X25519 key pair from the master key,
//! preserving unlinkability across pseudonyms.

use aes_gcm::{
    aead::{Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use fold_db::storage::traits::KvStore;
use hkdf::Hkdf;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use uuid::Uuid;
use x25519_dalek::{PublicKey, StaticSecret};

const CONN_REQ_PREFIX: &str = "discovery:conn_req:";
const CONN_SENT_PREFIX: &str = "discovery:conn_sent:";

/// Derive an X25519 key pair for a specific pseudonym.
/// This produces a unique key pair per pseudonym, preserving unlinkability.
pub fn derive_pseudonym_keypair(master_key: &[u8], pseudonym: &Uuid) -> (StaticSecret, PublicKey) {
    let hk = Hkdf::<Sha256>::new(Some(pseudonym.as_bytes()), master_key);
    let mut seed = [0u8; 32];
    hk.expand(b"discovery-x25519-keypair", &mut seed)
        .expect("32 bytes is a valid HKDF output length");
    let secret = StaticSecret::from(seed);
    let public = PublicKey::from(&secret);
    (secret, public)
}

/// Identity information included in accept messages so the requester
/// can create a trust relationship using the acceptor's real node key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityCardPayload {
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_hint: Option<String>,
    /// The node's Ed25519 public key (base64) — used for trust, NOT the pseudonym key
    pub node_public_key: String,
}

/// Plaintext payload inside an encrypted connection message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionPayload {
    /// "request", "accept", or "decline"
    pub message_type: String,
    /// Human-readable intro/response message
    pub message: String,
    /// Sender's persistent public key (base64) for future communication
    pub sender_public_key: String,
    /// Sender's pseudonym (so recipient can look up their public key to respond)
    pub sender_pseudonym: String,
    /// Sender's reply public key (base64) — encrypt responses with this
    pub reply_public_key: String,
    /// Identity card (included in accept messages for trust creation)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_card: Option<IdentityCardPayload>,
    /// Role the requester wants to assign to the acceptor (default: "acquaintance").
    /// Only meaningful in "request" messages; carried through to the requester's
    /// `process_accepted_connection` when the accept comes back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_role: Option<String>,
}

/// Encrypt a connection payload for a target's X25519 public key.
///
/// Format: `[ephemeral_public_key: 32B] [nonce: 12B] [ciphertext+tag]`
pub fn encrypt_connection_message(
    target_public_key: &[u8; 32],
    payload: &ConnectionPayload,
) -> Result<Vec<u8>, String> {
    let target_pk = PublicKey::from(*target_public_key);

    // Generate ephemeral key pair
    let ephemeral_secret = StaticSecret::random_from_rng(OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);

    // ECDH shared secret
    let shared_secret = ephemeral_secret.diffie_hellman(&target_pk);

    // Derive AES key from shared secret via HKDF
    let hk = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
    let mut aes_key = [0u8; 32];
    hk.expand(b"connection-request-aes", &mut aes_key)
        .map_err(|e| format!("HKDF expand failed: {}", e))?;

    // Encrypt payload
    let plaintext =
        serde_json::to_vec(payload).map_err(|e| format!("Failed to serialize payload: {}", e))?;

    let cipher =
        Aes256Gcm::new_from_slice(&aes_key).map_err(|e| format!("Invalid AES key: {}", e))?;

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_slice())
        .map_err(|e| format!("Encryption failed: {}", e))?;

    // Build output: ephemeral_public || nonce || ciphertext+tag
    let mut output = Vec::with_capacity(32 + 12 + ciphertext.len());
    output.extend_from_slice(ephemeral_public.as_bytes());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);

    Ok(output)
}

/// Decrypt a connection message using our pseudonym's private key.
///
/// Input format: `[ephemeral_public_key: 32B] [nonce: 12B] [ciphertext+tag]`
pub fn decrypt_connection_message(
    our_secret: &StaticSecret,
    encrypted: &[u8],
) -> Result<ConnectionPayload, String> {
    if encrypted.len() < 32 + 12 + 16 {
        return Err("Encrypted message too short".to_string());
    }

    // Extract ephemeral public key
    let mut epk_bytes = [0u8; 32];
    epk_bytes.copy_from_slice(&encrypted[..32]);
    let ephemeral_public = PublicKey::from(epk_bytes);

    // ECDH shared secret
    let shared_secret = our_secret.diffie_hellman(&ephemeral_public);

    // Derive AES key
    let hk = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
    let mut aes_key = [0u8; 32];
    hk.expand(b"connection-request-aes", &mut aes_key)
        .map_err(|e| format!("HKDF expand failed: {}", e))?;

    // Decrypt
    let nonce_bytes = &encrypted[32..44];
    let ciphertext = &encrypted[44..];

    let cipher =
        Aes256Gcm::new_from_slice(&aes_key).map_err(|e| format!("Invalid AES key: {}", e))?;

    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "Decryption failed (wrong key or tampered message)".to_string())?;

    serde_json::from_slice(&plaintext).map_err(|e| format!("Failed to deserialize payload: {}", e))
}

/// Encrypt any serializable payload for a target's X25519 public key.
///
/// Same crypto as `encrypt_connection_message` but works with any serializable type.
/// Format: `[ephemeral_public_key: 32B] [nonce: 12B] [ciphertext+tag]`
pub fn encrypt_message<T: Serialize>(
    target_public_key: &[u8; 32],
    payload: &T,
) -> Result<Vec<u8>, String> {
    let target_pk = PublicKey::from(*target_public_key);

    let ephemeral_secret = StaticSecret::random_from_rng(OsRng);
    let ephemeral_public = PublicKey::from(&ephemeral_secret);

    let shared_secret = ephemeral_secret.diffie_hellman(&target_pk);

    let hk = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
    let mut aes_key = [0u8; 32];
    hk.expand(b"connection-request-aes", &mut aes_key)
        .map_err(|e| format!("HKDF expand failed: {}", e))?;

    let plaintext =
        serde_json::to_vec(payload).map_err(|e| format!("Failed to serialize payload: {}", e))?;

    let cipher =
        Aes256Gcm::new_from_slice(&aes_key).map_err(|e| format!("Invalid AES key: {}", e))?;

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_slice())
        .map_err(|e| format!("Encryption failed: {}", e))?;

    let mut output = Vec::with_capacity(32 + 12 + ciphertext.len());
    output.extend_from_slice(ephemeral_public.as_bytes());
    output.extend_from_slice(&nonce_bytes);
    output.extend_from_slice(&ciphertext);

    Ok(output)
}

/// Decrypt a message and return the raw JSON value.
///
/// Same crypto as `decrypt_connection_message` but returns `serde_json::Value`
/// so callers can inspect `message_type` and deserialize to the appropriate type.
pub fn decrypt_message_raw(
    our_secret: &StaticSecret,
    encrypted: &[u8],
) -> Result<serde_json::Value, String> {
    if encrypted.len() < 32 + 12 + 16 {
        return Err("Encrypted message too short".to_string());
    }

    let mut epk_bytes = [0u8; 32];
    epk_bytes.copy_from_slice(&encrypted[..32]);
    let ephemeral_public = PublicKey::from(epk_bytes);

    let shared_secret = our_secret.diffie_hellman(&ephemeral_public);

    let hk = Hkdf::<Sha256>::new(None, shared_secret.as_bytes());
    let mut aes_key = [0u8; 32];
    hk.expand(b"connection-request-aes", &mut aes_key)
        .map_err(|e| format!("HKDF expand failed: {}", e))?;

    let nonce_bytes = &encrypted[32..44];
    let ciphertext = &encrypted[44..];

    let cipher =
        Aes256Gcm::new_from_slice(&aes_key).map_err(|e| format!("Invalid AES key: {}", e))?;

    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| "Decryption failed (wrong key or tampered message)".to_string())?;

    serde_json::from_slice(&plaintext).map_err(|e| format!("Failed to deserialize payload: {}", e))
}

/// Referral query — "do you know this pseudonym?"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferralQueryPayload {
    pub message_type: String,
    pub query_id: String,
    pub subject_pseudonym: String,
    pub subject_public_key: String,
    pub sender_pseudonym: String,
    pub reply_public_key: String,
}

/// Referral response — "yes, I know them as Bob"
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReferralResponsePayload {
    pub message_type: String,
    pub query_id: String,
    pub known_as: String,
    pub sender_pseudonym: String,
    pub reply_public_key: String,
}

/// A vouch from a trusted contact about a connection requester.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Vouch {
    pub voucher_display_name: String,
    pub known_as: String,
    pub received_at: String,
}

/// A decrypted, locally stored connection request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalConnectionRequest {
    pub request_id: String,
    pub message_id: String,
    pub target_pseudonym: String,
    pub sender_pseudonym: String,
    pub sender_public_key: String,
    pub reply_public_key: String,
    pub message: String,
    /// "pending", "accepted", "declined"
    pub status: String,
    pub created_at: String,
    pub responded_at: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vouches: Vec<Vouch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub referral_query_id: Option<String>,
    #[serde(default)]
    pub referral_contacts_queried: u32,
}

/// A locally stored sent connection request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalSentRequest {
    pub request_id: String,
    pub target_pseudonym: String,
    pub sender_pseudonym: String,
    pub message: String,
    /// "pending", "accepted", "declined"
    pub status: String,
    pub created_at: String,
    /// Role the requester chose to assign the acceptor (default: "acquaintance")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_role: Option<String>,
}

/// Save a received connection request to local Sled store.
pub async fn save_received_request(
    store: &dyn KvStore,
    request: &LocalConnectionRequest,
) -> Result<(), String> {
    let key = format!("{}{}", CONN_REQ_PREFIX, request.request_id);
    let value =
        serde_json::to_vec(request).map_err(|e| format!("Failed to serialize request: {}", e))?;
    store
        .put(key.as_bytes(), value)
        .await
        .map_err(|e| format!("Failed to save request: {}", e))
}

/// Load all received connection requests.
pub async fn list_received_requests(
    store: &dyn KvStore,
) -> Result<Vec<LocalConnectionRequest>, String> {
    let entries = store
        .scan_prefix(CONN_REQ_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("Failed to scan requests: {}", e))?;

    let mut requests = Vec::new();
    for (_key, value) in entries {
        match serde_json::from_slice(&value) {
            Ok(req) => requests.push(req),
            Err(e) => log::warn!("Failed to deserialize connection request: {}", e),
        }
    }

    requests.sort_by(|a: &LocalConnectionRequest, b: &LocalConnectionRequest| {
        b.created_at.cmp(&a.created_at)
    });
    Ok(requests)
}

/// Update the status of a received connection request.
pub async fn update_request_status(
    store: &dyn KvStore,
    request_id: &str,
    status: &str,
) -> Result<LocalConnectionRequest, String> {
    let key = format!("{}{}", CONN_REQ_PREFIX, request_id);
    let value = store
        .get(key.as_bytes())
        .await
        .map_err(|e| format!("Failed to get request: {}", e))?
        .ok_or_else(|| format!("Request {} not found", request_id))?;

    let mut request: LocalConnectionRequest = serde_json::from_slice(&value)
        .map_err(|e| format!("Failed to deserialize request: {}", e))?;

    request.status = status.to_string();
    request.responded_at = Some(chrono::Utc::now().to_rfc3339());

    let updated =
        serde_json::to_vec(&request).map_err(|e| format!("Failed to serialize request: {}", e))?;
    store
        .put(key.as_bytes(), updated)
        .await
        .map_err(|e| format!("Failed to save request: {}", e))?;

    Ok(request)
}

/// Save a sent connection request to local Sled store.
pub async fn save_sent_request(
    store: &dyn KvStore,
    request: &LocalSentRequest,
) -> Result<(), String> {
    let key = format!("{}{}", CONN_SENT_PREFIX, request.request_id);
    let value = serde_json::to_vec(request)
        .map_err(|e| format!("Failed to serialize sent request: {}", e))?;
    store
        .put(key.as_bytes(), value)
        .await
        .map_err(|e| format!("Failed to save sent request: {}", e))
}

/// Load all sent connection requests.
pub async fn list_sent_requests(store: &dyn KvStore) -> Result<Vec<LocalSentRequest>, String> {
    let entries = store
        .scan_prefix(CONN_SENT_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("Failed to scan sent requests: {}", e))?;

    let mut requests = Vec::new();
    for (_key, value) in entries {
        match serde_json::from_slice(&value) {
            Ok(req) => requests.push(req),
            Err(e) => log::warn!("Failed to deserialize sent request: {}", e),
        }
    }

    requests.sort_by(|a: &LocalSentRequest, b: &LocalSentRequest| b.created_at.cmp(&a.created_at));
    Ok(requests)
}

/// Update a sent request status (e.g., when we receive an acceptance).
/// Returns the updated `LocalSentRequest` if one was found, so callers
/// can inspect `preferred_role` etc.
pub async fn update_sent_request_status(
    store: &dyn KvStore,
    target_pseudonym: &str,
    status: &str,
) -> Result<Option<LocalSentRequest>, String> {
    let entries = store
        .scan_prefix(CONN_SENT_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("Failed to scan sent requests: {}", e))?;

    for (key, value) in entries {
        if let Ok(mut req) = serde_json::from_slice::<LocalSentRequest>(&value) {
            if req.target_pseudonym == target_pseudonym && req.status == "pending" {
                req.status = status.to_string();
                let updated =
                    serde_json::to_vec(&req).map_err(|e| format!("Failed to serialize: {}", e))?;
                store
                    .put(&key, updated)
                    .await
                    .map_err(|e| format!("Failed to update: {}", e))?;
                return Ok(Some(req));
            }
        }
    }

    Ok(None)
}

/// Get the base64-encoded X25519 public key for a pseudonym.
pub fn get_pseudonym_public_key_b64(master_key: &[u8], pseudonym: &Uuid) -> String {
    let (_secret, public) = derive_pseudonym_keypair(master_key, pseudonym);
    B64.encode(public.as_bytes())
}

/// A batch of records shared via the encrypted bulletin board.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataSharePayload {
    /// Always "data_share"
    pub message_type: String,
    /// Sender's node public key (Ed25519, base64) — used as the mutation pub_key
    pub sender_public_key: String,
    /// Sender's display name
    pub sender_display_name: String,
    /// The records being shared
    pub records: Vec<SharedRecord>,
}

/// A single shared record — schema + field values + optional file data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedRecord {
    /// Schema name (e.g., "Photography")
    pub schema_name: String,
    /// Schema definition as JSON (for creating the schema on the recipient if it doesn't exist)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_definition: Option<serde_json::Value>,
    /// The record's field values
    pub fields: std::collections::HashMap<String, serde_json::Value>,
    /// Record key
    pub key: SharedRecordKey,
    /// Optional base64-encoded file data (e.g., photo bytes)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_data_base64: Option<String>,
    /// Original filename (for file_data)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
}

/// Key for a shared record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharedRecordKey {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_derivation_deterministic() {
        let master_key = [0x42u8; 32];
        let pseudonym = Uuid::new_v4();

        let (s1, p1) = derive_pseudonym_keypair(&master_key, &pseudonym);
        let (s2, p2) = derive_pseudonym_keypair(&master_key, &pseudonym);

        assert_eq!(p1.as_bytes(), p2.as_bytes());
        // StaticSecret doesn't expose bytes directly, but the public keys match
        // proves the secrets are the same
        let _ = (s1, s2);
    }

    #[test]
    fn test_different_pseudonyms_different_keys() {
        let master_key = [0x42u8; 32];
        let p1 = Uuid::new_v4();
        let p2 = Uuid::new_v4();

        let (_, pk1) = derive_pseudonym_keypair(&master_key, &p1);
        let (_, pk2) = derive_pseudonym_keypair(&master_key, &p2);

        assert_ne!(pk1.as_bytes(), pk2.as_bytes());
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let master_key = [0x42u8; 32];
        let target_pseudonym = Uuid::new_v4();

        let (secret, public) = derive_pseudonym_keypair(&master_key, &target_pseudonym);

        let payload = ConnectionPayload {
            message_type: "request".to_string(),
            message: "Hello, I'd love to connect!".to_string(),
            sender_public_key: "sender_pk_base64".to_string(),
            sender_pseudonym: Uuid::new_v4().to_string(),
            reply_public_key: "reply_pk_base64".to_string(),
            identity_card: None,
            preferred_role: None,
        };

        let encrypted = encrypt_connection_message(public.as_bytes(), &payload).unwrap();
        let decrypted = decrypt_connection_message(&secret, &encrypted).unwrap();

        assert_eq!(decrypted.message_type, "request");
        assert_eq!(decrypted.message, "Hello, I'd love to connect!");
        assert_eq!(decrypted.sender_public_key, "sender_pk_base64");
    }

    #[test]
    fn test_wrong_key_fails_decrypt() {
        let master_key = [0x42u8; 32];
        let target_pseudonym = Uuid::new_v4();
        let wrong_pseudonym = Uuid::new_v4();

        let (_, public) = derive_pseudonym_keypair(&master_key, &target_pseudonym);
        let (wrong_secret, _) = derive_pseudonym_keypair(&master_key, &wrong_pseudonym);

        let payload = ConnectionPayload {
            message_type: "request".to_string(),
            message: "Secret message".to_string(),
            sender_public_key: "pk".to_string(),
            sender_pseudonym: Uuid::new_v4().to_string(),
            reply_public_key: "rpk".to_string(),
            identity_card: None,
            preferred_role: None,
        };

        let encrypted = encrypt_connection_message(public.as_bytes(), &payload).unwrap();
        let result = decrypt_connection_message(&wrong_secret, &encrypted);

        assert!(result.is_err());
    }

    #[test]
    fn test_generic_encrypt_decrypt_roundtrip() {
        let master_key = [0x42u8; 32];
        let target_pseudonym = Uuid::new_v4();

        let (secret, public) = derive_pseudonym_keypair(&master_key, &target_pseudonym);

        // Use encrypt_message with a ConnectionPayload (same type, generic function)
        let payload = ConnectionPayload {
            message_type: "request".to_string(),
            message: "Generic test".to_string(),
            sender_public_key: "pk".to_string(),
            sender_pseudonym: Uuid::new_v4().to_string(),
            reply_public_key: "rpk".to_string(),
            identity_card: None,
            preferred_role: None,
        };

        let encrypted = encrypt_message(public.as_bytes(), &payload).unwrap();
        let raw = decrypt_message_raw(&secret, &encrypted).unwrap();

        assert_eq!(raw["message_type"].as_str().unwrap(), "request");
        assert_eq!(raw["message"].as_str().unwrap(), "Generic test");
    }

    #[test]
    fn test_generic_encrypt_arbitrary_type() {
        let master_key = [0x42u8; 32];
        let target_pseudonym = Uuid::new_v4();

        let (secret, public) = derive_pseudonym_keypair(&master_key, &target_pseudonym);

        // Encrypt an arbitrary JSON object
        let payload = serde_json::json!({
            "message_type": "query_request",
            "request_id": "test-123",
            "schema_name": "notes",
            "fields": ["title", "body"],
        });

        let encrypted = encrypt_message(public.as_bytes(), &payload).unwrap();
        let raw = decrypt_message_raw(&secret, &encrypted).unwrap();

        assert_eq!(raw["message_type"].as_str().unwrap(), "query_request");
        assert_eq!(raw["request_id"].as_str().unwrap(), "test-123");
        assert_eq!(raw["schema_name"].as_str().unwrap(), "notes");
    }
}
