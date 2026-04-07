//! Trust invite — signed, self-contained payload for establishing trust.
//!
//! A trust invite contains the sender's Ed25519 pub key, identity card, and
//! proposed trust distance. The payload is signed so the recipient can verify
//! the sender controls the claimed key.
//!
//! Two delivery modes:
//! 1. **Discovery**: Encrypted with peer's X25519 key, sent via bulletin board
//! 2. **Direct link**: Base64url-encoded token for sharing via text/QR

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use fold_db::security::{Ed25519KeyPair, Ed25519PublicKey, KeyUtils};

use super::identity_card::IdentityCard;

/// A trust invite payload — signed by the sender's Ed25519 key.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustInvite {
    /// The sender's Ed25519 public key (base64).
    pub sender_pub_key: String,
    /// The sender's identity card (display name + optional contact hint).
    pub sender_identity: IdentityCard,
    /// Proposed trust distance for the recipient to grant.
    pub proposed_distance: u64,
    /// Random nonce for replay protection.
    pub nonce: String,
    /// When the invite was created.
    pub created_at: DateTime<Utc>,
    /// Ed25519 signature over the canonical payload (base64).
    pub signature: String,
}

/// The canonical payload that gets signed (everything except the signature).
#[derive(Debug, Serialize)]
struct TrustInvitePayload<'a> {
    sender_pub_key: &'a str,
    sender_identity: &'a IdentityCard,
    proposed_distance: u64,
    nonce: &'a str,
    created_at: &'a DateTime<Utc>,
}

impl TrustInvite {
    /// Create and sign a new trust invite.
    pub fn create(
        private_key_base64: &str,
        public_key_base64: &str,
        identity: &IdentityCard,
        proposed_distance: u64,
    ) -> Result<Self, String> {
        let secret_bytes = base64::engine::general_purpose::STANDARD
            .decode(private_key_base64)
            .map_err(|e| format!("Invalid private key: {e}"))?;
        let keypair = Ed25519KeyPair::from_secret_key(&secret_bytes)
            .map_err(|e| format!("Failed to load keypair: {e}"))?;

        let nonce = KeyUtils::generate_nonce();
        let created_at = Utc::now();

        let payload = TrustInvitePayload {
            sender_pub_key: public_key_base64,
            sender_identity: identity,
            proposed_distance,
            nonce: &nonce,
            created_at: &created_at,
        };
        let payload_bytes = serde_json::to_vec(&payload)
            .map_err(|e| format!("Failed to serialize payload: {e}"))?;

        let signature = keypair.sign(&payload_bytes);
        let signature_base64 = KeyUtils::signature_to_base64(&signature);

        Ok(Self {
            sender_pub_key: public_key_base64.to_string(),
            sender_identity: identity.clone(),
            proposed_distance,
            nonce,
            created_at,
            signature: signature_base64,
        })
    }

    /// Maximum invite age before it's considered expired (30 days).
    const MAX_AGE_SECS: i64 = 30 * 24 * 60 * 60;

    /// Verify the invite's signature and check it hasn't expired.
    pub fn verify(&self) -> Result<bool, String> {
        // Check expiry: reject invites older than MAX_AGE_SECS
        let age = Utc::now()
            .signed_duration_since(self.created_at)
            .num_seconds();
        if age > Self::MAX_AGE_SECS {
            return Err(format!(
                "Trust invite expired ({} days old, max {})",
                age / 86400,
                Self::MAX_AGE_SECS / 86400
            ));
        }
        if age < -300 {
            // Clock skew tolerance: reject invites from >5min in the future
            return Err("Trust invite timestamp is in the future".to_string());
        }

        let pub_key = Ed25519PublicKey::from_base64(&self.sender_pub_key)
            .map_err(|e| format!("Invalid sender public key: {e}"))?;

        let payload = TrustInvitePayload {
            sender_pub_key: &self.sender_pub_key,
            sender_identity: &self.sender_identity,
            proposed_distance: self.proposed_distance,
            nonce: &self.nonce,
            created_at: &self.created_at,
        };
        let payload_bytes = serde_json::to_vec(&payload)
            .map_err(|e| format!("Failed to serialize payload: {e}"))?;

        let signature = KeyUtils::signature_from_base64(&self.signature)
            .map_err(|e| format!("Invalid signature: {e}"))?;

        Ok(pub_key.verify(&payload_bytes, &signature))
    }

    /// Encode the invite as a URL-safe base64 token for direct sharing.
    pub fn to_token(&self) -> Result<String, String> {
        let json =
            serde_json::to_vec(self).map_err(|e| format!("Failed to serialize invite: {e}"))?;
        Ok(URL_SAFE_NO_PAD.encode(&json))
    }

    /// Decode an invite from a URL-safe base64 token.
    pub fn from_token(token: &str) -> Result<Self, String> {
        let bytes = URL_SAFE_NO_PAD
            .decode(token)
            .map_err(|e| format!("Invalid token encoding: {e}"))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("Invalid invite format: {e}"))
    }

    /// Compute a short fingerprint of the sender's public key (first 8 hex chars of SHA256).
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let pub_bytes = base64::engine::general_purpose::STANDARD
            .decode(&self.sender_pub_key)
            .unwrap_or_default();
        let hash = Sha256::digest(&pub_bytes);
        format!(
            "{:x}",
            &hash[..4].iter().fold(0u32, |acc, &b| acc << 8 | b as u32)
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_keypair() -> (String, String) {
        let kp = Ed25519KeyPair::generate().unwrap();
        (kp.secret_key_base64(), kp.public_key_base64())
    }

    #[test]
    fn test_create_and_verify() {
        let (priv_key, pub_key) = test_keypair();
        let identity = IdentityCard::new("Alice".to_string(), Some("alice@test.com".to_string()));

        let invite = TrustInvite::create(&priv_key, &pub_key, &identity, 1).unwrap();
        assert!(invite.verify().unwrap());
        assert_eq!(invite.sender_identity.display_name, "Alice");
        assert_eq!(invite.proposed_distance, 1);
    }

    #[test]
    fn test_token_roundtrip() {
        let (priv_key, pub_key) = test_keypair();
        let identity = IdentityCard::new("Bob".to_string(), None);

        let invite = TrustInvite::create(&priv_key, &pub_key, &identity, 2).unwrap();
        let token = invite.to_token().unwrap();
        let decoded = TrustInvite::from_token(&token).unwrap();

        assert!(decoded.verify().unwrap());
        assert_eq!(decoded.sender_identity.display_name, "Bob");
        assert_eq!(decoded.proposed_distance, 2);
    }

    #[test]
    fn test_tampered_invite_fails_verification() {
        let (priv_key, pub_key) = test_keypair();
        let identity = IdentityCard::new("Alice".to_string(), None);

        let mut invite = TrustInvite::create(&priv_key, &pub_key, &identity, 1).unwrap();
        invite.proposed_distance = 999; // tamper

        assert!(!invite.verify().unwrap());
    }

    #[test]
    fn test_wrong_key_fails_verification() {
        let (priv_key, pub_key) = test_keypair();
        let (_, other_pub_key) = test_keypair();
        let identity = IdentityCard::new("Alice".to_string(), None);

        let mut invite = TrustInvite::create(&priv_key, &pub_key, &identity, 1).unwrap();
        invite.sender_pub_key = other_pub_key; // claim different key

        assert!(!invite.verify().unwrap());
    }
}
