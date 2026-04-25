//! Node identity store — the ONE canonical location for the Ed25519
//! keypair that authenticates this device to Exemem.
//!
//! Stored in its own Sled tree (`node_identity`) under the node's
//! existing SledPool. AES-256-GCM at rest when the `os-keychain` feature
//! is enabled (keychain master key); plaintext otherwise — matching the
//! SSH private-key model where OS disk encryption is the at-rest defense.
//!
//! # Why a dedicated store (not `NodeConfigStore`)
//!
//! `fold_db::NodeConfigStore` encrypts sensitive fields with a key
//! derived from the Ed25519 seed itself — which creates a chicken-and-egg
//! loop if identity lives there: you need the identity to decrypt the
//! store that holds the identity. Historically that worked because the
//! private key was bootstrapped from a disk file (`node_identity.json`).
//! Stage 4 removes that file, so the bootstrap key must come from
//! somewhere else. We use the OS keychain master key here instead,
//! which is independent of the Ed25519 seed.
//!
//! This module owns:
//! - the Sled tree that holds `identity:private_key` + `identity:public_key`
//! - the choice of encryption key (keychain master when available)
//! - all reads/writes — no other module should touch the tree directly.
//!
//! # Dev mode
//!
//! Without the `os-keychain` feature, the private key is stored
//! plaintext. Fine for development; do not ship prod builds that way.

use std::path::Path;
use std::sync::Arc;

use fold_db::storage::SledPool;
// Canonical identity struct lives in fold_db — re-exported here so callers
// only ever see one `NodeIdentity` type. `IdentityStore` is the single
// storage; `fold_db::NodeIdentity` is the single in-memory shape.
pub use fold_db::NodeIdentity;

const TREE_NAME: &str = "node_identity";
const KEY_PRIVATE: &[u8] = b"private_key";
const KEY_PUBLIC: &[u8] = b"public_key";

/// Prefix marker for encrypted string values. Matches the convention
/// used by `fold_db::NodeConfigStore` so raw-reads can distinguish
/// ciphertext from legacy plaintext values.
const ENCRYPTED_PREFIX: &str = "ENC:";

/// Build a [`NodeIdentity`] from an [`fold_db::security::Ed25519KeyPair`].
pub fn identity_from_keypair(keypair: &fold_db::security::Ed25519KeyPair) -> NodeIdentity {
    NodeIdentity {
        private_key: keypair.secret_key_base64(),
        public_key: keypair.public_key_base64(),
    }
}

/// Generate a fresh Ed25519 keypair and wrap it in a [`NodeIdentity`].
pub fn generate_identity() -> Result<NodeIdentity, String> {
    let keypair = fold_db::security::Ed25519KeyPair::generate()
        .map_err(|e| format!("Failed to generate Ed25519 keypair: {e}"))?;
    Ok(identity_from_keypair(&keypair))
}

/// Load the persistent Ed25519 signing keypair from a [`NodeIdentity`].
///
/// The fold_db factory takes this as a required parameter so molecule
/// signatures match the node's public identity. Loading at boot from the
/// persisted private key (instead of generating a fresh keypair inside
/// fold_db) guarantees that property across restarts.
pub fn signer_from_identity(
    identity: &NodeIdentity,
) -> Result<Arc<fold_db::security::Ed25519KeyPair>, String> {
    let keypair = fold_db::security::Ed25519KeyPair::from_secret_key_base64(&identity.private_key)
        .map_err(|e| format!("Failed to load signing keypair from node identity: {e}"))?;
    Ok(Arc::new(keypair))
}

/// Opens the identity tree on `pool` and returns a handle. Clones of
/// the pool share the same file-lock holder, so multiple handles are
/// safe concurrently.
///
/// When `os-keychain` is enabled, the master key is fetched once per
/// call — callers that open the store repeatedly should cache the
/// handle if that's a concern.
pub fn open(pool: Arc<SledPool>) -> Result<IdentityStore, String> {
    // Touch the tree so any IO / migration runs up-front rather than
    // surprising the first reader.
    {
        let guard = pool
            .acquire_arc()
            .map_err(|e| format!("Failed to acquire Sled pool: {e}"))?;
        guard
            .db()
            .open_tree(TREE_NAME)
            .map_err(|e| format!("Failed to open identity tree: {e}"))?;
    }
    let key = resolve_master_key()?;
    Ok(IdentityStore {
        pool,
        master_key: key,
    })
}

#[cfg(feature = "os-keychain")]
fn resolve_master_key() -> Result<Option<[u8; 32]>, String> {
    crate::secure_store::get_or_create_master_key().map(Some)
}

#[cfg(not(feature = "os-keychain"))]
fn resolve_master_key() -> Result<Option<[u8; 32]>, String> {
    // Dev mode: plaintext. Same SSH-like model the rest of the node
    // uses when the keychain feature is off.
    Ok(None)
}

/// Opens the identity store at `$FOLDDB_HOME/data` with its own
/// short-lived SledPool. Used by CLI commands that run pre-daemon
/// (setup, restore, recovery-phrase) where no shared pool exists.
/// Returns the pool alongside the store so the caller holds the
/// flock until it drops both.
pub fn open_standalone(data_path: &Path) -> Result<(Arc<SledPool>, IdentityStore), String> {
    let pool = Arc::new(SledPool::new(data_path.to_path_buf()));
    let store = open(Arc::clone(&pool))?;
    Ok((pool, store))
}

/// A handle to the identity tree on a specific `SledPool`.
pub struct IdentityStore {
    pool: Arc<SledPool>,
    master_key: Option<[u8; 32]>,
}

impl IdentityStore {
    fn with_tree<T>(
        &self,
        f: impl FnOnce(&sled::Tree) -> Result<T, sled::Error>,
    ) -> Result<T, String> {
        let guard = self
            .pool
            .acquire_arc()
            .map_err(|e| format!("Failed to acquire Sled pool: {e}"))?;
        let tree = guard
            .db()
            .open_tree(TREE_NAME)
            .map_err(|e| format!("Failed to open identity tree: {e}"))?;
        f(&tree).map_err(|e| format!("Identity-store IO failed: {e}"))
    }

    /// Read the node's identity. Returns `None` if no identity has
    /// been persisted yet (first-time install, or after a reset).
    pub fn get(&self) -> Result<Option<NodeIdentity>, String> {
        let public_key_bytes = self.with_tree(|tree| tree.get(KEY_PUBLIC))?;
        let private_stored_bytes = self.with_tree(|tree| tree.get(KEY_PRIVATE))?;
        let (Some(public_bytes), Some(private_bytes)) = (public_key_bytes, private_stored_bytes)
        else {
            return Ok(None);
        };
        let public_key = String::from_utf8(public_bytes.to_vec())
            .map_err(|e| format!("Identity public key is not valid UTF-8: {e}"))?;
        let private_stored = String::from_utf8(private_bytes.to_vec())
            .map_err(|e| format!("Identity private key is not valid UTF-8: {e}"))?;
        let private_key = self.decrypt_private(&private_stored)?;
        Ok(Some(NodeIdentity {
            private_key,
            public_key,
        }))
    }

    /// Persist the identity. Overwrites any previous value.
    pub fn set(&self, id: &NodeIdentity) -> Result<(), String> {
        let encrypted = self.encrypt_private(&id.private_key)?;
        self.with_tree(|tree| {
            tree.insert(KEY_PRIVATE, encrypted.as_bytes())?;
            tree.insert(KEY_PUBLIC, id.public_key.as_bytes())?;
            Ok(())
        })
    }

    /// Delete the identity (for node resets). Idempotent.
    pub fn clear(&self) -> Result<(), String> {
        self.with_tree(|tree| {
            tree.remove(KEY_PRIVATE)?;
            tree.remove(KEY_PUBLIC)?;
            Ok(())
        })
    }

    fn encrypt_private(&self, plaintext: &str) -> Result<String, String> {
        let Some(key) = self.master_key else {
            // Dev mode — write plaintext, no prefix so dev readers
            // without the feature flag can still load it.
            return Ok(plaintext.to_string());
        };
        let envelope = fold_db::crypto::encrypt_envelope(&key, plaintext.as_bytes())
            .map_err(|e| format!("Failed to encrypt private key: {e}"))?;
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(envelope);
        Ok(format!("{ENCRYPTED_PREFIX}{b64}"))
    }

    fn decrypt_private(&self, stored: &str) -> Result<String, String> {
        if let Some(b64) = stored.strip_prefix(ENCRYPTED_PREFIX) {
            let Some(key) = self.master_key else {
                return Err(
                    "Identity private key is encrypted but no keychain master key is available"
                        .to_string(),
                );
            };
            use base64::Engine;
            let envelope = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("Failed to base64-decode encrypted identity: {e}"))?;
            let plaintext = fold_db::crypto::decrypt_envelope(&key, &envelope)
                .map_err(|e| format!("Failed to decrypt private key: {e}"))?;
            String::from_utf8(plaintext)
                .map_err(|e| format!("Decrypted private key is not valid UTF-8: {e}"))
        } else {
            // Legacy plaintext — accept (dev mode).
            Ok(stored.to_string())
        }
    }
}

/// Read the identity from the pool. Convenience wrapper.
pub fn load(pool: Arc<SledPool>) -> Result<Option<NodeIdentity>, String> {
    open(pool)?.get()
}

/// Persist the identity on the pool. Convenience wrapper.
pub fn save(pool: Arc<SledPool>, id: &NodeIdentity) -> Result<(), String> {
    open(pool)?.set(id)
}

/// Persist `id` at `data_path` using a short-lived standalone SledPool.
///
/// For CLI / setup / restore / test flows that don't have a live node pool.
/// Returns once the write is flushed; the pool's file-lock is released on
/// drop so the daemon can subsequently boot against the same path.
pub fn save_standalone(data_path: &Path, id: &NodeIdentity) -> Result<(), String> {
    let (_pool, store) = open_standalone(data_path)?;
    store.set(id)
}

/// Read the identity from a Sled database at `data_path`, without
/// requiring a live node or pre-existing pool. Returns `None` if no
/// identity has been persisted yet. Symmetric with [`save_standalone`]
/// — opens a short-lived pool, reads, releases the flock on return.
pub fn load_standalone(data_path: &Path) -> Result<Option<NodeIdentity>, String> {
    let (_pool, store) = open_standalone(data_path)?;
    store.get()
}

/// Load the identity, generating + persisting a new keypair if none
/// exists. Used by the fresh-install startup path.
pub fn load_or_generate(pool: Arc<SledPool>) -> Result<NodeIdentity, String> {
    let store = open(pool)?;
    if let Some(id) = store.get()? {
        return Ok(id);
    }
    let id = generate_identity()?;
    store.set(&id)?;
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_pool() -> (Arc<SledPool>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let pool = Arc::new(SledPool::new(tmp.path().to_path_buf()));
        (pool, tmp)
    }

    fn assert_identity_eq(a: &NodeIdentity, b: &NodeIdentity) {
        assert_eq!(a.private_key, b.private_key, "private key mismatch");
        assert_eq!(a.public_key, b.public_key, "public key mismatch");
    }

    #[test]
    fn empty_pool_has_no_identity() {
        let (pool, _tmp) = temp_pool();
        let store = open(pool).unwrap();
        assert!(store.get().unwrap().is_none());
    }

    #[test]
    fn set_then_get_roundtrips() {
        let (pool, _tmp) = temp_pool();
        let store = open(pool).unwrap();
        let id = NodeIdentity {
            private_key: "priv-b64".to_string(),
            public_key: "pub-b64".to_string(),
        };
        store.set(&id).unwrap();
        let loaded = store.get().unwrap().expect("identity should be persisted");
        assert_identity_eq(&loaded, &id);
    }

    #[test]
    fn load_or_generate_generates_first_then_loads() {
        let (pool, _tmp) = temp_pool();
        let first = load_or_generate(Arc::clone(&pool)).unwrap();
        let second = load_or_generate(pool).unwrap();
        assert_identity_eq(&first, &second);
    }

    #[test]
    fn save_standalone_roundtrips_via_fresh_pool() {
        let tmp = tempfile::tempdir().unwrap();
        let id = NodeIdentity {
            private_key: "priv".to_string(),
            public_key: "pub".to_string(),
        };
        save_standalone(tmp.path(), &id).unwrap();
        let pool = Arc::new(SledPool::new(tmp.path().to_path_buf()));
        let loaded = load(pool)
            .unwrap()
            .expect("identity should survive pool drop");
        assert_identity_eq(&loaded, &id);
    }

    #[test]
    fn identity_from_keypair_matches_direct_base64() {
        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let id = identity_from_keypair(&keypair);
        assert_eq!(id.private_key, keypair.secret_key_base64());
        assert_eq!(id.public_key, keypair.public_key_base64());
    }

    #[test]
    fn clear_removes_identity() {
        let (pool, _tmp) = temp_pool();
        let store = open(pool).unwrap();
        store
            .set(&NodeIdentity {
                private_key: "p".to_string(),
                public_key: "P".to_string(),
            })
            .unwrap();
        assert!(store.get().unwrap().is_some());
        store.clear().unwrap();
        assert!(store.get().unwrap().is_none());
    }
}
