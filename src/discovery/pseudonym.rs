use uuid::Uuid;

pub fn derive_pseudonym(master_key: &[u8], content_hash: &[u8]) -> Uuid {
    fold_db::db_operations::native_index::pseudonym::derive_pseudonym_uuid(master_key, content_hash)
}

pub fn content_hash(text: &str) -> Vec<u8> {
    fold_db::db_operations::native_index::pseudonym::content_hash(text)
}

pub fn content_hash_bytes(data: &[u8]) -> Vec<u8> {
    fold_db::db_operations::native_index::pseudonym::content_hash_bytes(data)
}

/// Derive a **stable identity pseudonym** for this node.
///
/// Used as the primary match key in referral queries so that Alice and Bob
/// independently derive the *same* pseudonym for a third party Charlie,
/// regardless of which schemas Charlie has opted into discovery at any
/// given moment. Unlike pseudonyms derived from schema names or
/// `"connection-sender"`, this one depends only on the master key and is
/// stable across the lifetime of the node identity.
///
/// This pseudonym is used **only for matching** (identity equality). Replies
/// and encryption continue to use the existing `connection-sender` pseudonym
/// and its X25519 key pair.
pub fn derive_identity_pseudonym(master_key: &[u8]) -> Uuid {
    let hash = content_hash("identity");
    derive_pseudonym(master_key, &hash)
}
