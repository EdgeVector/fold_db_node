//! Master-key resolution and OS keychain-backed encryption for sensitive files.
//!
//! When the `os-keychain` feature is enabled, a random 32-byte master key is
//! stored in the OS keychain (macOS Keychain / Windows Credential Manager /
//! Linux Secret Service). All sensitive files on disk (node_identity, e2e key,
//! credentials, LLM API keys) are encrypted with this master key using
//! AES-256-GCM.
//!
//! When the feature is disabled (dev mode), the keychain-touching helpers are
//! not compiled and files remain plaintext — matching the SSH private-key
//! security model. The `FOLDDB_MASTER_KEY` env var is still honored either
//! way, so headless / sandboxed callers have a consistent escape hatch.
//!
//! # Silent-mint hazard
//!
//! Historically [`encrypt_and_write`] and [`read_and_decrypt`] called a
//! `get_or_create_master_key` helper that minted a fresh key whenever the
//! keychain reported `NoEntry`. If a sensitive file was already on disk (a
//! credential blob, an Anthropic API key, the node identity), the freshly
//! minted key would silently orphan the previously sealed data. The identity
//! defense (`refuse-to-rotate` in `identity.rs`) generalized to every caller
//! is the split between [`get_master_key`] (no minting) and
//! [`initialize_master_key`] (idempotent mint, used only by the identity
//! bootstrap path).

#[cfg(feature = "os-keychain")]
use fold_db::crypto::envelope::{decrypt_envelope, encrypt_envelope};
#[cfg(feature = "os-keychain")]
use rand::RngCore;

#[cfg(feature = "os-keychain")]
const KEYCHAIN_SERVICE: &str = "com.folddb.node";
#[cfg(feature = "os-keychain")]
const KEYCHAIN_MASTER_KEY: &str = "master-key";

/// Read a 32-byte master key from the `FOLDDB_MASTER_KEY` env var (64 hex
/// chars). The documented escape hatch for headless / sandboxed contexts
/// where the OS keychain isn't reachable but the user knows the right key.
///
/// Always available — both `os-keychain` and dev-mode builds consult it.
pub(crate) fn master_key_from_env() -> Result<Option<[u8; 32]>, String> {
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

/// Look up the master key WITHOUT minting one if absent.
///
/// Resolution order:
/// 1. `FOLDDB_MASTER_KEY` env var (always consulted).
/// 2. OS keychain (only when `os-keychain` is enabled).
///
/// Returns `Ok(Some(key))` when present, `Ok(None)` when neither source has
/// it, and `Err` for malformed env values or any keychain access failure
/// other than `NoEntry` (permission denied, keychain locked, etc.) — callers
/// see the underlying problem instead of papering over it.
#[cfg(feature = "os-keychain")]
pub fn try_get_master_key() -> Result<Option<[u8; 32]>, String> {
    if let Some(k) = master_key_from_env()? {
        return Ok(Some(k));
    }
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_MASTER_KEY)
        .map_err(|e| format!("Failed to access OS keychain: {}", e))?;
    match entry.get_secret() {
        Ok(bytes) => {
            if bytes.len() != 32 {
                return Err(format!(
                    "Master key in OS keychain has invalid length: {} (expected 32)",
                    bytes.len()
                ));
            }
            let mut key = [0u8; 32];
            key.copy_from_slice(&bytes);
            Ok(Some(key))
        }
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("Failed to read master key from OS keychain: {}", e)),
    }
}

/// Dev-mode shape: no keychain to consult, only the env-var escape hatch.
#[cfg(not(feature = "os-keychain"))]
pub fn try_get_master_key() -> Result<Option<[u8; 32]>, String> {
    master_key_from_env()
}

/// Retrieve the master key, erroring with a structured message if neither
/// source produced one. Used by every read/write site that operates on
/// already-sealed data — silent minting here would orphan whatever was
/// previously sealed. The identity-rotation defense generalized to every
/// sensitive blob.
#[cfg(feature = "os-keychain")]
pub fn get_master_key() -> Result<[u8; 32], String> {
    try_get_master_key()?.ok_or_else(|| {
        "Sensitive store on disk requires a master key, but none was found in \
         the OS keychain or FOLDDB_MASTER_KEY. Refusing to mint a fresh one — \
         that would orphan all previously-sealed data (identity, credentials, \
         LLM API keys). Run from a context with keychain access (the Tauri \
         app), or set FOLDDB_MASTER_KEY=<64-hex-bytes> to provide the master \
         key explicitly."
            .to_string()
    })
}

/// Mint and store a fresh master key. Intended for the identity-bootstrap
/// path only — every other caller must use [`get_master_key`] so a wiped
/// keychain surfaces an error instead of silently rotating the master key.
///
/// Idempotent: if a key already exists (in the keychain or via env var), the
/// existing key is returned unchanged. Safe to call repeatedly during boot.
#[cfg(feature = "os-keychain")]
pub fn initialize_master_key() -> Result<[u8; 32], String> {
    if let Some(k) = try_get_master_key()? {
        return Ok(k);
    }
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_MASTER_KEY)
        .map_err(|e| format!("Failed to access OS keychain: {}", e))?;
    let mut key = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut key);
    entry
        .set_secret(&key)
        .map_err(|e| format!("Failed to store master key in OS keychain: {}", e))?;
    tracing::info!("Generated and stored new master key in OS keychain");
    Ok(key)
}

/// Encrypt data with the master key and write to disk.
///
/// Errors if no master key can be resolved — refuses to silently mint a
/// fresh one that would orphan previously-sealed sibling files.
#[cfg(feature = "os-keychain")]
pub fn encrypt_and_write(path: &std::path::Path, plaintext: &[u8]) -> Result<(), String> {
    let master_key = get_master_key()?;
    let envelope = encrypt_envelope(&master_key, plaintext)
        .map_err(|e| format!("Failed to encrypt: {}", e))?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create directory: {}", e))?;
    }

    crate::sensitive_io::write_atomic_0600(path, &envelope)
}

/// Read an encrypted file from disk and decrypt with the master key.
#[cfg(feature = "os-keychain")]
pub fn read_and_decrypt(path: &std::path::Path) -> Result<Vec<u8>, String> {
    let master_key = get_master_key()?;
    let envelope =
        std::fs::read(path).map_err(|e| format!("Failed to read encrypted file: {}", e))?;
    decrypt_envelope(&master_key, &envelope).map_err(|e| format!("Failed to decrypt: {}", e))
}

/// Delete the master key from the OS keychain (e.g. on node reset).
#[cfg(feature = "os-keychain")]
pub fn delete_master_key() -> Result<(), String> {
    let entry = keyring::Entry::new(KEYCHAIN_SERVICE, KEYCHAIN_MASTER_KEY)
        .map_err(|e| format!("Failed to access OS keychain: {}", e))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // Already gone
        Err(e) => Err(format!(
            "Failed to delete master key from OS keychain: {}",
            e
        )),
    }
}

/// Test-only helpers for managing the process-wide `FOLDDB_MASTER_KEY`
/// env var across all tests in the crate.
///
/// `FOLDDB_MASTER_KEY` is the canonical "no-mint" escape hatch consulted
/// by every sensitive read/write site (see [`master_key_from_env`]).
/// Multiple test modules (identity, credentials, anthropic key store, web
/// search key store, ingestion config, auth handlers, ...) exercise paths
/// that go through [`get_master_key`], so they all need a master key
/// available — and a few identity tests need to observe the "no master
/// key" failure shape, which means clearing the env var.
///
/// Without coordination, parallel tests can flip each other's view of the
/// env var mid-run and produce wildly confusing failures. This module
/// provides the single shared mutex every such test acquires before
/// touching `FOLDDB_MASTER_KEY`.
#[cfg(test)]
pub(crate) mod test_master_key {
    use std::sync::{Mutex, MutexGuard};

    /// Process-global mutex guarding `FOLDDB_MASTER_KEY` env-var access.
    /// Acquire this in any test that depends on the env var's value
    /// being either set or unset.
    pub static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// 32 bytes of 0xee — deterministic, obviously-fake test value.
    pub const TEST_KEY_HEX: &str =
        "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

    /// Acquire [`ENV_LOCK`] without changing the env var. Use in tests
    /// that need to observe the env var as unset (or to set it to a
    /// non-default value themselves) — drop the guard at end of test.
    pub fn lock() -> MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// RAII guard that holds [`ENV_LOCK`], sets `FOLDDB_MASTER_KEY` to
    /// [`TEST_KEY_HEX`], and clears the env var on drop. The standard
    /// preamble for any test that exercises the encrypted read/write path.
    pub struct WithMasterKey {
        _guard: MutexGuard<'static, ()>,
    }

    impl Drop for WithMasterKey {
        fn drop(&mut self) {
            std::env::remove_var("FOLDDB_MASTER_KEY");
        }
    }

    pub fn with_set() -> WithMasterKey {
        let guard = lock();
        std::env::set_var("FOLDDB_MASTER_KEY", TEST_KEY_HEX);
        WithMasterKey { _guard: guard }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> std::sync::MutexGuard<'static, ()> {
        test_master_key::lock()
    }

    #[cfg(feature = "os-keychain")]
    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        // Test the envelope encrypt/decrypt directly (doesn't need OS keychain)
        let key = [0x42u8; 32];
        let plaintext = b"test credentials json";
        let envelope = encrypt_envelope(&key, plaintext).unwrap();
        let decrypted = decrypt_envelope(&key, &envelope).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    /// FOLDDB_MASTER_KEY unblocks the read/write path even when the keychain
    /// is unreachable. Identity.rs already had this regression test for its
    /// own resolve_master_key path; this is the same property at the
    /// secure_store layer, so credentials / Anthropic key / future sensitive
    /// blobs all share the escape hatch.
    #[cfg(feature = "os-keychain")]
    #[test]
    fn folddb_master_key_env_var_unblocks_encrypt_and_write() {
        let _guard = env_lock();
        let key_hex = "1".repeat(64); // 32 bytes of 0x11
        std::env::set_var("FOLDDB_MASTER_KEY", &key_hex);

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("envvar.enc");
        let plaintext = b"sealed via env var";

        let write_result = encrypt_and_write(&path, plaintext);
        let read_result = if write_result.is_ok() {
            Some(read_and_decrypt(&path))
        } else {
            None
        };
        std::env::remove_var("FOLDDB_MASTER_KEY");

        write_result.expect("encrypt_and_write should succeed under FOLDDB_MASTER_KEY");
        let decrypted = read_result
            .unwrap()
            .expect("read_and_decrypt should succeed");
        assert_eq!(decrypted, plaintext);
    }

    /// Regression for the silent-mint hazard. With no env var set and (we
    /// assume) no keychain entry, `encrypt_and_write` MUST surface an error
    /// rather than silently minting a key — that's how the identity defense
    /// generalizes to credentials / API keys / etc.
    ///
    /// Skipped in CI and on machines that happen to have a leftover keychain
    /// entry from prior local runs (not our concern here; the identity
    /// defense already covers the same failure shape end-to-end).
    #[cfg(feature = "os-keychain")]
    #[test]
    fn encrypt_and_write_errors_when_no_master_key_available() {
        if std::env::var("CI").is_ok() {
            return;
        }
        let _guard = env_lock();
        std::env::remove_var("FOLDDB_MASTER_KEY");

        if try_get_master_key().ok().flatten().is_some() {
            // Local keychain has a leftover entry from prior dev runs.
            // The behavior we want to assert is "no key -> error", which
            // can't be observed when a key is present. Skip rather than
            // wipe a key the developer may need.
            return;
        }

        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nokey.enc");
        let err = match encrypt_and_write(&path, b"should not reach disk") {
            Ok(()) => panic!(
                "encrypt_and_write must error when no master key is available; \
                 silent minting orphans previously-sealed sibling blobs"
            ),
            Err(e) => e,
        };
        assert!(
            err.contains("FOLDDB_MASTER_KEY"),
            "error must mention the env-var escape hatch; got: {err}"
        );
        assert!(
            !path.exists(),
            "encrypt_and_write must not create the file when it errors"
        );
    }

    /// `initialize_master_key` is the only place that's allowed to mint.
    /// It must be safe to call repeatedly (e.g. across daemon restarts) —
    /// the second call must return the SAME key, not a fresh one, so we
    /// can call it unconditionally on the bootstrap path.
    ///
    /// Uses FOLDDB_MASTER_KEY so we can prove idempotency without touching
    /// the live OS keychain (gates the test behind CI-friendly env var,
    /// matching the env-var unblock test above).
    #[cfg(feature = "os-keychain")]
    #[test]
    fn initialize_master_key_is_idempotent() {
        let _guard = env_lock();
        let key_hex = "2".repeat(64); // 32 bytes of 0x22
        std::env::set_var("FOLDDB_MASTER_KEY", &key_hex);

        let first = initialize_master_key();
        let second = initialize_master_key();
        std::env::remove_var("FOLDDB_MASTER_KEY");

        let first = first.expect("first initialize_master_key call should succeed");
        let second = second.expect("second initialize_master_key call should succeed");
        assert_eq!(first, second, "initialize_master_key must be idempotent");
        assert_eq!(first, [0x22u8; 32]);
    }

    /// `master_key_from_env` is the cross-feature primitive — exercise it
    /// directly so dev-mode builds still cover the env-var path even though
    /// they don't compile any of the keychain helpers.
    #[test]
    fn master_key_from_env_decodes_64_hex_chars() {
        let _guard = env_lock();
        let key_hex = "3".repeat(64); // 32 bytes of 0x33
        std::env::set_var("FOLDDB_MASTER_KEY", &key_hex);
        let result = master_key_from_env();
        std::env::remove_var("FOLDDB_MASTER_KEY");
        let key = result.unwrap().expect("env var should be honored");
        assert_eq!(key, [0x33u8; 32]);
    }

    #[test]
    fn master_key_from_env_rejects_wrong_length() {
        let _guard = env_lock();
        std::env::set_var("FOLDDB_MASTER_KEY", "deadbeef"); // 4 bytes, not 32
        let result = master_key_from_env();
        std::env::remove_var("FOLDDB_MASTER_KEY");
        let err = result.expect_err("wrong-length env var should error");
        assert!(err.contains("32 bytes"), "got: {err}");
    }

    #[test]
    fn master_key_from_env_returns_none_when_unset() {
        let _guard = env_lock();
        std::env::remove_var("FOLDDB_MASTER_KEY");
        let result = master_key_from_env().unwrap();
        assert!(result.is_none());
    }
}
