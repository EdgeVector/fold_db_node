use base64::Engine;
use sha2::{Digest, Sha256};

/// Derive a 32-character hex user hash from a base64-encoded public key.
///
/// Algorithm: base64-decode the key -> SHA-256(raw 32 bytes) -> take first 16 bytes -> hex-encode.
///
/// This MUST match the auth_service derivation in
/// `exemem-infra/lambdas/auth_service/src/cli/types.rs::derive_user_hash_from_pubkey`,
/// which hex-decodes and then hashes. Both paths hash the same raw 32-byte Ed25519
/// public key -- the only difference is the input encoding (base64 here, hex there).
pub fn user_hash_from_pubkey(pubkey_b64: &str) -> String {
    let raw_bytes = base64::engine::general_purpose::STANDARD
        .decode(pubkey_b64)
        .expect("user_hash_from_pubkey: public key is not valid base64");
    let digest = Sha256::digest(&raw_bytes);
    digest[..16].iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: encode raw bytes as base64 for test inputs.
    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn test_user_hash_length_and_determinism() {
        let key = b64(&[0x42u8; 32]);
        let hash = user_hash_from_pubkey(&key);
        assert_eq!(hash.len(), 32, "User hash must be 32 hex characters");
        assert_eq!(hash, user_hash_from_pubkey(&key));
    }

    #[test]
    fn test_user_hash_different_keys() {
        let h1 = user_hash_from_pubkey(&b64(&[0x01u8; 32]));
        let h2 = user_hash_from_pubkey(&b64(&[0x02u8; 32]));
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_user_hash_is_lowercase_hex() {
        let hash = user_hash_from_pubkey(&b64(&[0xFFu8; 32]));
        assert!(hash
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn test_matches_auth_service_derivation() {
        // Simulate what auth_service does: hex-decode -> SHA256 -> first 16 bytes -> hex
        let raw_key = [0x42u8; 32];
        let hex_key: String = raw_key.iter().map(|b| format!("{:02x}", b)).collect();

        // auth_service path: hex_decode -> SHA256
        let hex_bytes: Vec<u8> = (0..hex_key.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex_key[i..i + 2], 16).unwrap())
            .collect();
        let auth_digest = Sha256::digest(&hex_bytes);
        let auth_hash: String = auth_digest[..16]
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect();

        // fold_db_node path: base64-decode -> SHA256
        let node_hash = user_hash_from_pubkey(&b64(&raw_key));

        assert_eq!(
            node_hash, auth_hash,
            "fold_db_node and auth_service must derive identical user_hash from the same key"
        );
    }
}
