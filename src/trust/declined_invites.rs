//! Declined invites — tracks invites the user chose to decline.
//!
//! Stored under `user_profile/invites/declined/<nonce>` via
//! [`UserProfileStore`]. User-level: a decision to decline an invite
//! propagates to every device the user owns, so they don't see the same
//! invite pop up again on another device.

use chrono::{DateTime, Utc};
use fold_db::fold_db_core::FoldDB;
use fold_db::schema::SchemaError;
use serde::{Deserialize, Serialize};

use crate::user_profile::UserProfileStore;

/// Sub-prefix under `UserProfileStore`. One record per nonce.
const DECLINED_INVITES_PREFIX: &str = "invites/declined/";

fn declined_key(nonce: &str) -> String {
    format!("{DECLINED_INVITES_PREFIX}{nonce}")
}

/// A record of a declined trust invite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclinedInvite {
    /// The sender's Ed25519 public key.
    pub sender_pub_key: String,
    /// The sender's display name from the invite.
    pub sender_display_name: String,
    /// The sender's contact hint.
    pub sender_contact_hint: Option<String>,
    /// The proposed role name.
    pub proposed_role: String,
    /// When the invite was declined.
    pub declined_at: DateTime<Utc>,
    /// The invite's nonce (to identify duplicates).
    pub nonce: String,
}

/// All declined invites for this user. Load with [`DeclinedInviteStore::load`]
/// and persist with [`Self::save`]. Each nonce gets its own record in the
/// synced store.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeclinedInviteStore {
    pub invites: Vec<DeclinedInvite>,
}

impl DeclinedInviteStore {
    /// Load every declined-invite record from the synced user-profile store.
    pub async fn load(db: &FoldDB) -> Result<Self, SchemaError> {
        let store = UserProfileStore::from_db(db);
        let rows: Vec<(String, DeclinedInvite)> = store.scan(DECLINED_INVITES_PREFIX).await?;
        Ok(Self {
            invites: rows.into_iter().map(|(_, v)| v).collect(),
        })
    }

    /// Persist every declined invite. Each invite is keyed by its nonce.
    pub async fn save(&self, db: &FoldDB) -> Result<(), SchemaError> {
        let store = UserProfileStore::from_db(db);
        for invite in &self.invites {
            store.put(&declined_key(&invite.nonce), invite).await?;
        }
        Ok(())
    }

    /// Record a declined invite.
    pub fn decline(&mut self, invite: DeclinedInvite) {
        // Don't duplicate — check by nonce
        if !self.invites.iter().any(|i| i.nonce == invite.nonce) {
            self.invites.push(invite);
        }
    }

    /// Check if an invite (by nonce) was previously declined.
    pub fn is_declined(&self, nonce: &str) -> bool {
        self.invites.iter().any(|i| i.nonce == nonce)
    }

    /// Remove a decline record (user changed their mind). Also deletes the
    /// synced record so peer devices stop seeing it as declined.
    pub async fn undecline(&mut self, db: &FoldDB, nonce: &str) -> Result<bool, SchemaError> {
        let len = self.invites.len();
        self.invites.retain(|i| i.nonce != nonce);
        if self.invites.len() < len {
            let store = UserProfileStore::from_db(db);
            store.delete(&declined_key(nonce)).await?;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn setup_db() -> (std::sync::Arc<FoldDB>, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let config = crate::fold_node::NodeConfig::new(tmp.path().to_path_buf())
            .with_schema_service_url("test://mock")
            .with_seed_identity(crate::identity::identity_from_keypair(&keypair));
        let node = crate::fold_node::FoldNode::new(config).await.unwrap();
        let db = node.get_fold_db().unwrap();
        (db, tmp)
    }

    fn mk_declined(nonce: &str) -> DeclinedInvite {
        DeclinedInvite {
            sender_pub_key: "pk".into(),
            sender_display_name: "Sender".into(),
            sender_contact_hint: None,
            proposed_role: "friend".into(),
            declined_at: Utc::now(),
            nonce: nonce.into(),
        }
    }

    #[tokio::test]
    async fn decline_then_load_roundtrips() {
        let (db, _tmp) = setup_db().await;
        let mut store = DeclinedInviteStore::default();
        store.decline(mk_declined("n1"));
        store.decline(mk_declined("n2"));
        store.save(&db).await.unwrap();

        let loaded = DeclinedInviteStore::load(&db).await.unwrap();
        assert_eq!(loaded.invites.len(), 2);
        assert!(loaded.is_declined("n1"));
        assert!(loaded.is_declined("n2"));
        assert!(!loaded.is_declined("n3"));
    }

    #[tokio::test]
    async fn decline_is_idempotent_on_duplicate_nonce() {
        let mut store = DeclinedInviteStore::default();
        store.decline(mk_declined("n1"));
        store.decline(mk_declined("n1"));
        assert_eq!(store.invites.len(), 1);
    }

    #[tokio::test]
    async fn undecline_removes_from_store_and_sled() {
        let (db, _tmp) = setup_db().await;
        let mut store = DeclinedInviteStore::default();
        store.decline(mk_declined("n1"));
        store.save(&db).await.unwrap();

        assert!(store.undecline(&db, "n1").await.unwrap());
        assert!(!store.undecline(&db, "n1").await.unwrap());

        let loaded = DeclinedInviteStore::load(&db).await.unwrap();
        assert!(!loaded.is_declined("n1"));
    }
}
