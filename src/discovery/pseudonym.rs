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
