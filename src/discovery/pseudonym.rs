use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Derive a deterministic, unlinkable pseudonym UUID from a master secret and record content hash.
///
/// Properties:
/// - **Deterministic**: Same master key + same content = same pseudonym (enables dedup)
/// - **Unlinkable**: Two pseudonyms from the same user are computationally indistinguishable from random
/// - **Different users, same content**: Different pseudonyms (different master keys)
///
/// Uses keyed SHA-256: `SHA256(master_key || "discovery-pseudonym" || content_hash)`,
/// then takes the first 16 bytes as a UUID v4-compatible identifier.
pub fn derive_pseudonym(master_key: &[u8], content_hash: &[u8]) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(master_key);
    hasher.update(b"discovery-pseudonym");
    hasher.update(content_hash);
    let hash = hasher.finalize();

    // Take first 16 bytes, set UUID version 4 bits for compatibility
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&hash[..16]);
    // Set version 4 (bits 12-15 of byte 6)
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    // Set variant (bits 6-7 of byte 8)
    bytes[8] = (bytes[8] & 0x3F) | 0x80;

    Uuid::from_bytes(bytes)
}

/// Compute the content hash of a record's concatenated field text.
/// This is used as input to `derive_pseudonym`.
pub fn content_hash(text: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    hasher.finalize().to_vec()
}

/// Compute the content hash from raw bytes (e.g. embedding float bytes).
pub fn content_hash_bytes(data: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}
