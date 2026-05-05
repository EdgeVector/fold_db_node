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
///
/// If the tree already holds an `ENC:` blob, master-key resolution refuses
/// to silently generate a fresh keychain master key (which would orphan the
/// existing identity and silently rotate the node's user_hash across daemon
/// restarts). In that case the caller gets a structured error and can either
/// run from a context with keychain access or set `FOLDDB_MASTER_KEY` to
/// override.
pub fn open(pool: Arc<SledPool>) -> Result<IdentityStore, String> {
    let has_encrypted_identity = peek_encrypted_identity(&pool)?;
    let key = resolve_master_key(has_encrypted_identity)?;
    Ok(IdentityStore {
        pool,
        master_key: key,
    })
}

/// Open the identity tree and check whether `KEY_PRIVATE` already holds an
/// encrypted (`ENC:` prefixed) blob. Used by [`open`] to decide whether
/// silent master-key creation is safe.
fn peek_encrypted_identity(pool: &Arc<SledPool>) -> Result<bool, String> {
    let guard = pool
        .acquire_arc()
        .map_err(|e| format!("Failed to acquire Sled pool: {e}"))?;
    let tree = guard
        .db()
        .open_tree(TREE_NAME)
        .map_err(|e| format!("Failed to open identity tree: {e}"))?;
    let stored = tree
        .get(KEY_PRIVATE)
        .map_err(|e| format!("Failed to read identity tree: {e}"))?;
    Ok(stored
        .as_ref()
        .and_then(|v| std::str::from_utf8(v).ok())
        .map(|s| s.starts_with(ENCRYPTED_PREFIX))
        .unwrap_or(false))
}

/// Read a 32-byte master key from the `FOLDDB_MASTER_KEY` env var (64 hex
/// chars). Provides a manual escape hatch for headless / sandboxed contexts
/// where the OS keychain isn't reachable but the user knows the right key.
fn master_key_from_env() -> Result<Option<[u8; 32]>, String> {
    let raw = match std::env::var("FOLDDB_MASTER_KEY") {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let trimmed = raw.trim();
    let bytes =
        hex::decode(trimmed).map_err(|e| format!("FOLDDB_MASTER_KEY must be 64 hex chars: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "FOLDDB_MASTER_KEY must decode to 32 bytes, got {}",
            bytes.len()
        ));
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(Some(key))
}

#[cfg(feature = "os-keychain")]
fn resolve_master_key(has_encrypted_identity: bool) -> Result<Option<[u8; 32]>, String> {
    if let Some(k) = master_key_from_env()? {
        return Ok(Some(k));
    }
    if has_encrypted_identity {
        // Existing encrypted blob on disk — refuse to silently mint a fresh
        // master key. That used to silently rotate user_hash across daemon
        // restarts whenever the keychain reported NoEntry (sandboxed launchd,
        // unsigned-binary access denial, etc), orphaning every previously
        // ingested record.
        match crate::secure_store::try_get_master_key()? {
            Some(k) => Ok(Some(k)),
            None => Err(
                "Encrypted node identity exists on disk, but no master key was \
                 found in the OS keychain. Refusing to regenerate — that would \
                 silently rotate your user_hash and orphan all previously \
                 ingested data. Run from the Tauri app where the keychain is \
                 reachable, or set FOLDDB_MASTER_KEY=<64-hex-bytes> to \
                 provide the master key explicitly."
                    .to_string(),
            ),
        }
    } else {
        crate::secure_store::get_or_create_master_key().map(Some)
    }
}

#[cfg(not(feature = "os-keychain"))]
fn resolve_master_key(has_encrypted_identity: bool) -> Result<Option<[u8; 32]>, String> {
    if let Some(k) = master_key_from_env()? {
        return Ok(Some(k));
    }
    if has_encrypted_identity {
        // The tree was sealed by a build with `os-keychain` on, but this
        // build can't reach the keychain at all. Returning Ok(None) here
        // would let the next `set` write a plaintext value over the
        // encrypted one and silently rotate the identity.
        return Err(
            "Encrypted node identity exists on disk, but this binary was built \
             without the os-keychain feature. Run from a build that has \
             keychain access (the Tauri app), or set \
             FOLDDB_MASTER_KEY=<64-hex-bytes> to decrypt explicitly."
                .to_string(),
        );
    }
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

    /// Every identity test acquires this lock before touching the store.
    /// `FOLDDB_MASTER_KEY` is process-wide state and a parallel test that
    /// sets/unsets it can flip another test's tree between plaintext and
    /// ENC:-prefixed encoding mid-run, which produces wildly confusing
    /// "encrypted blob exists but no master key" failures. Serializing
    /// the test module avoids that.
    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    #[test]
    fn empty_pool_has_no_identity() {
        let _guard = env_lock();
        std::env::remove_var("FOLDDB_MASTER_KEY");
        let (pool, _tmp) = temp_pool();
        let store = open(pool).unwrap();
        assert!(store.get().unwrap().is_none());
    }

    #[test]
    fn set_then_get_roundtrips() {
        let _guard = env_lock();
        std::env::remove_var("FOLDDB_MASTER_KEY");
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
        let _guard = env_lock();
        std::env::remove_var("FOLDDB_MASTER_KEY");
        let (pool, _tmp) = temp_pool();
        let first = load_or_generate(Arc::clone(&pool)).unwrap();
        let second = load_or_generate(pool).unwrap();
        assert_identity_eq(&first, &second);
    }

    #[test]
    fn save_standalone_roundtrips_via_fresh_pool() {
        let _guard = env_lock();
        std::env::remove_var("FOLDDB_MASTER_KEY");
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
        // No identity-store interaction — no env_lock needed.
        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let id = identity_from_keypair(&keypair);
        assert_eq!(id.private_key, keypair.secret_key_base64());
        assert_eq!(id.public_key, keypair.public_key_base64());
    }

    #[test]
    fn clear_removes_identity() {
        let _guard = env_lock();
        std::env::remove_var("FOLDDB_MASTER_KEY");
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

    fn write_raw_encrypted_blob(pool: &Arc<SledPool>, public_key: &str) {
        let guard = pool.acquire_arc().unwrap();
        let tree = guard.db().open_tree(TREE_NAME).unwrap();
        let fake_blob = format!("{ENCRYPTED_PREFIX}fakebase64payload");
        tree.insert(KEY_PRIVATE, fake_blob.as_bytes()).unwrap();
        tree.insert(KEY_PUBLIC, public_key.as_bytes()).unwrap();
    }

    fn read_raw_pubkey(pool: &Arc<SledPool>) -> Option<Vec<u8>> {
        let guard = pool.acquire_arc().unwrap();
        let tree = guard.db().open_tree(TREE_NAME).unwrap();
        tree.get(KEY_PUBLIC).unwrap().map(|v| v.to_vec())
    }

    /// Regression test for the P0 silent identity-rotation bug. If an
    /// encrypted blob is on disk and the master key is unreachable
    /// (no os-keychain feature, or keychain returns NoEntry under
    /// sandboxed launchd), `open()` MUST fail rather than fall through
    /// to a path that would generate a fresh keypair and stamp it over
    /// the existing one — that's what orphaned every previously-ingested
    /// record across `folddb daemon stop && folddb daemon start`.
    #[test]
    fn refuses_to_open_when_encrypted_blob_present_and_no_master_key() {
        let _guard = env_lock();
        std::env::remove_var("FOLDDB_MASTER_KEY");

        let (pool, _tmp) = temp_pool();
        write_raw_encrypted_blob(&pool, "original-pub");

        let err = match open(Arc::clone(&pool)) {
            Ok(_) => panic!("open must fail when encrypted blob present and no master key"),
            Err(e) => e,
        };
        assert!(
            err.contains("Encrypted node identity exists on disk"),
            "got: {err}"
        );

        // Critical invariant: failure path MUST NOT have rewritten the tree.
        // Any rewrite here is the silent-rotation bug.
        assert_eq!(
            read_raw_pubkey(&pool).as_deref(),
            Some(b"original-pub" as &[u8]),
            "open() failure path must not rotate the persisted public key"
        );
    }

    /// `load_or_generate` is the path the live daemon takes on every boot.
    /// It must surface the same error as `open()` rather than silently
    /// generating a fresh keypair, otherwise the daemon's restart cycle
    /// keeps stamping new identities into the tree.
    #[test]
    fn load_or_generate_does_not_silently_replace_unreadable_identity() {
        let _guard = env_lock();
        std::env::remove_var("FOLDDB_MASTER_KEY");

        let (pool, _tmp) = temp_pool();
        write_raw_encrypted_blob(&pool, "preserved-pub");

        let err = match load_or_generate(Arc::clone(&pool)) {
            Ok(id) => panic!(
                "load_or_generate must fail; instead returned identity with pub {}",
                id.public_key
            ),
            Err(e) => e,
        };
        assert!(
            err.contains("Encrypted node identity exists on disk"),
            "got: {err}"
        );
        assert_eq!(
            read_raw_pubkey(&pool).as_deref(),
            Some(b"preserved-pub" as &[u8]),
            "load_or_generate failure path must not rotate the persisted public key"
        );
    }

    /// FOLDDB_MASTER_KEY is the documented escape hatch for headless
    /// contexts where the OS keychain isn't reachable. With it set,
    /// `open()` succeeds even when an `ENC:` blob is present.
    #[test]
    fn foldb_master_key_env_var_unblocks_open() {
        let _guard = env_lock();
        let key_hex = "0".repeat(64); // 32 zero bytes
        std::env::set_var("FOLDDB_MASTER_KEY", &key_hex);

        let (pool, _tmp) = temp_pool();
        write_raw_encrypted_blob(&pool, "any-pub");

        let result = open(Arc::clone(&pool));
        std::env::remove_var("FOLDDB_MASTER_KEY");
        assert!(
            result.is_ok(),
            "open should succeed when FOLDDB_MASTER_KEY is set; got err: {:?}",
            result.err()
        );
    }

    /// Daemon-restart simulation: write identity, drop the store, reopen
    /// against the same pool, verify the identity round-trips byte-for-byte.
    /// This is the closest unit-level analogue to the prompt's acceptance
    /// criterion ("`folddb daemon stop && folddb daemon start` preserves
    /// the user_hash"). See the tests/cli_integration_test.rs daemon
    /// fixture for the heavier process-spawning variant.
    #[test]
    fn identity_survives_store_drop_and_reopen() {
        // Lock for the env_lock-tests, so a parallel test can't have
        // FOLDDB_MASTER_KEY set across our load_or_generate calls — that
        // would silently switch storage between plaintext and ENC: blobs.
        let _guard = env_lock();
        std::env::remove_var("FOLDDB_MASTER_KEY");

        let (pool, _tmp) = temp_pool();
        let original = load_or_generate(Arc::clone(&pool)).unwrap();
        // Drop the first store handle (simulates daemon shutdown).
        // `pool` survives — same Sled directory, fresh handle.
        let reopened = load_or_generate(Arc::clone(&pool)).unwrap();
        assert_identity_eq(&original, &reopened);

        // Repeated re-opens must remain stable. Catches any bug that
        // rotates identity on the Nth boot rather than the second.
        for i in 0..5 {
            let again = load_or_generate(Arc::clone(&pool))
                .unwrap_or_else(|e| panic!("reopen #{i} failed: {e}"));
            assert_identity_eq(&original, &again);
        }
    }
}
