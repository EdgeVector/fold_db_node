//! `UserProfileStore` — typed accessor for user-scoped state in the synced
//! `metadata` namespace.
//!
//! User-scoped state is anything that should converge across every device
//! restored from the same mnemonic: identity card, contact book, trust map,
//! sharing roles, invite history. All keys live under the `user_profile/`
//! prefix in `metadata` so it's easy to scan and clearly separated from
//! engine-internal metadata (node_id, idempotency, process_results).
//!
//! Why the `metadata` namespace specifically: it's wrapped in
//! [`SyncingKvStore`] when cloud sync is active. Writes go through the
//! sync log, get replayed on peer devices, converge. When cloud sync is
//! off this degrades to a plain local Sled tree — same API, no sync.
//!
//! This is the ONLY place user-level state should live. If you find
//! yourself adding a new JSON file under `$FOLDDB_HOME/config/*.json`,
//! stop and use this instead.

use fold_db::fold_db_core::FoldDB;
use fold_db::schema::SchemaError;
use fold_db::storage::traits::{KvStore, TypedStore};
use fold_db::storage::TypedKvStore;
use serde::{de::DeserializeOwned, Serialize};
use std::sync::Arc;

/// Prefix for every key written by this store. Prevents collisions with
/// other `metadata` users (node_id, idempotency markers, process_results)
/// and lets us scan the whole user profile with one prefix query.
const USER_PROFILE_PREFIX: &str = "user_profile/";

/// Typed accessor for the synced user-profile slice of the `metadata`
/// namespace.
///
/// Construct via [`UserProfileStore::from_db`]. Cheap to construct — the
/// underlying `Arc<dyn KvStore>` is shared with the rest of the fold_db
/// runtime. Holds the raw KV handle (rather than a `TypedKvStore`) so
/// it's `Clone`.
#[derive(Clone)]
pub struct UserProfileStore {
    raw: Arc<dyn KvStore>,
}

impl UserProfileStore {
    /// Build a store over the metadata namespace of `db`.
    pub fn from_db(db: &FoldDB) -> Self {
        Self {
            raw: db.db_ops().metadata().raw_metadata_kv(),
        }
    }

    fn typed(&self) -> TypedKvStore<dyn KvStore> {
        TypedKvStore::new(self.raw.clone())
    }

    /// Build the full synced key for a caller-supplied sub-key.
    fn key(suffix: &str) -> String {
        format!("{USER_PROFILE_PREFIX}{suffix}")
    }

    /// Read a JSON-serialized value by sub-key. Returns `None` if absent.
    pub async fn get<T>(&self, suffix: &str) -> Result<Option<T>, SchemaError>
    where
        T: DeserializeOwned + Send + Sync,
    {
        self.typed()
            .get_item::<T>(&Self::key(suffix))
            .await
            .map_err(|e| {
                SchemaError::InvalidData(format!("user_profile read '{suffix}' failed: {e}"))
            })
    }

    /// Write a JSON-serialized value by sub-key. Write propagates via the
    /// sync log to every peer device sharing the user's prefix.
    pub async fn put<T>(&self, suffix: &str, value: &T) -> Result<(), SchemaError>
    where
        T: Serialize + Send + Sync,
    {
        self.typed()
            .put_item(&Self::key(suffix), value)
            .await
            .map_err(|e| {
                SchemaError::InvalidData(format!("user_profile write '{suffix}' failed: {e}"))
            })
    }

    /// Delete a key. Returns whether it existed before deletion.
    pub async fn delete(&self, suffix: &str) -> Result<bool, SchemaError> {
        self.typed()
            .delete_item(&Self::key(suffix))
            .await
            .map_err(|e| {
                SchemaError::InvalidData(format!("user_profile delete '{suffix}' failed: {e}"))
            })
    }

    /// Scan all keys under a given sub-prefix and deserialize each value.
    /// Returned keys are stripped of the global `user_profile/` prefix but
    /// retain the caller-supplied sub-prefix, so `scan("contacts/")` yields
    /// `("contacts/<pubkey>", Contact)` pairs.
    pub async fn scan<T>(&self, sub_prefix: &str) -> Result<Vec<(String, T)>, SchemaError>
    where
        T: DeserializeOwned + Send + Sync,
    {
        let full = Self::key(sub_prefix);
        let raw = self
            .typed()
            .scan_items_with_prefix::<T>(&full)
            .await
            .map_err(|e| {
                SchemaError::InvalidData(format!("user_profile scan '{sub_prefix}' failed: {e}"))
            })?;
        Ok(raw
            .into_iter()
            .map(|(k, v)| {
                let stripped = k
                    .strip_prefix(USER_PROFILE_PREFIX)
                    .unwrap_or(&k)
                    .to_string();
                (stripped, v)
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
    struct TestBlob {
        name: String,
        count: u32,
    }

    async fn setup_store() -> (UserProfileStore, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let config = crate::fold_node::NodeConfig::new(tmp.path().to_path_buf())
            .with_schema_service_url("test://mock")
            .with_seed_identity(crate::identity::identity_from_keypair(&keypair));
        let node = crate::fold_node::FoldNode::new(config).await.unwrap();
        let db = node.get_fold_db().unwrap();
        let store = UserProfileStore::from_db(&db);
        (store, tmp)
    }

    #[tokio::test]
    async fn put_then_get_roundtrips() {
        let (store, _tmp) = setup_store().await;
        let blob = TestBlob {
            name: "alice".into(),
            count: 3,
        };
        store.put("test/blob", &blob).await.unwrap();
        let got: Option<TestBlob> = store.get("test/blob").await.unwrap();
        assert_eq!(got, Some(blob));
    }

    #[tokio::test]
    async fn get_missing_returns_none() {
        let (store, _tmp) = setup_store().await;
        let got: Option<TestBlob> = store.get("absent").await.unwrap();
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn delete_removes_key() {
        let (store, _tmp) = setup_store().await;
        store
            .put(
                "x",
                &TestBlob {
                    name: "a".into(),
                    count: 1,
                },
            )
            .await
            .unwrap();
        assert!(store.delete("x").await.unwrap());
        let got: Option<TestBlob> = store.get("x").await.unwrap();
        assert_eq!(got, None);
    }

    #[tokio::test]
    async fn scan_with_prefix_returns_stripped_keys() {
        let (store, _tmp) = setup_store().await;
        store
            .put(
                "contacts/alice",
                &TestBlob {
                    name: "alice".into(),
                    count: 1,
                },
            )
            .await
            .unwrap();
        store
            .put(
                "contacts/bob",
                &TestBlob {
                    name: "bob".into(),
                    count: 2,
                },
            )
            .await
            .unwrap();
        store
            .put(
                "other/ignored",
                &TestBlob {
                    name: "ignored".into(),
                    count: 0,
                },
            )
            .await
            .unwrap();

        let mut pairs: Vec<(String, TestBlob)> = store.scan("contacts/").await.unwrap();
        pairs.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].0, "contacts/alice");
        assert_eq!(pairs[1].0, "contacts/bob");
        assert_eq!(pairs[0].1.name, "alice");
        assert_eq!(pairs[1].1.name, "bob");
    }
}
