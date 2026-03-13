use sha2::{Digest, Sha256};

/// Derive a 32-character hex user hash from a public key.
///
/// Algorithm: SHA-256(public_key_bytes) → take first 16 bytes → hex-encode.
/// This is the canonical user identity derivation used throughout the system
/// (HTTP routes, CLI, frontend). Keeping it in one place prevents divergence.
pub fn user_hash_from_pubkey(pubkey: &str) -> String {
    let digest = Sha256::digest(pubkey.as_bytes());
    digest[..16]
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_hash_length_and_determinism() {
        let hash = user_hash_from_pubkey("test-public-key-abc123");
        assert_eq!(hash.len(), 32, "User hash must be 32 hex characters");
        // Deterministic
        assert_eq!(hash, user_hash_from_pubkey("test-public-key-abc123"));
    }

    #[test]
    fn test_user_hash_different_keys() {
        let h1 = user_hash_from_pubkey("key-a");
        let h2 = user_hash_from_pubkey("key-b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_user_hash_is_lowercase_hex() {
        let hash = user_hash_from_pubkey("any-key");
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }
}
