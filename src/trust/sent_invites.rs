//! Sent invites — tracks invites this node has created and shared.
//!
//! Stored under `user_profile/invites/sent/<nonce>` via [`UserProfileStore`].
//! User-level: propagates to every device restored from the same mnemonic
//! so Alice's invite history is the same whether she checks it from her
//! laptop or her phone.

use chrono::{DateTime, Utc};
use fold_db::fold_db_core::FoldDB;
use fold_db::schema::SchemaError;
use serde::{Deserialize, Serialize};

use crate::user_profile::UserProfileStore;

/// Sub-prefix under `UserProfileStore`. One record per nonce.
const SENT_INVITES_PREFIX: &str = "invites/sent/";

fn sent_key(nonce: &str) -> String {
    format!("{SENT_INVITES_PREFIX}{nonce}")
}

/// Status of a sent invite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SentInviteStatus {
    Pending,
    Accepted,
    Expired,
}

/// A record of a sent trust invite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentInvite {
    /// The invite's nonce (unique identifier).
    pub nonce: String,
    /// Who we sent it to (if known — may be "unknown" for link shares).
    pub recipient_hint: String,
    /// Proposed role name.
    pub proposed_role: String,
    /// When the invite was created.
    pub created_at: DateTime<Utc>,
    /// Current status.
    pub status: SentInviteStatus,
}

/// All sent invites for this user. In-memory view — load with
/// [`SentInviteStore::load`], mutate via [`Self::record`] /
/// [`Self::mark_accepted`], persist with [`Self::save`].
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SentInviteStore {
    pub invites: Vec<SentInvite>,
}

impl SentInviteStore {
    /// Load every sent-invite record from the synced user-profile store.
    pub async fn load(db: &FoldDB) -> Result<Self, SchemaError> {
        let store = UserProfileStore::from_db(db);
        let rows: Vec<(String, SentInvite)> = store.scan(SENT_INVITES_PREFIX).await?;
        Ok(Self {
            invites: rows.into_iter().map(|(_, v)| v).collect(),
        })
    }

    /// Persist every invite in this store. Each invite is its own record
    /// keyed by nonce; writes propagate per-record via the sync log.
    pub async fn save(&self, db: &FoldDB) -> Result<(), SchemaError> {
        let store = UserProfileStore::from_db(db);
        for invite in &self.invites {
            store.put(&sent_key(&invite.nonce), invite).await?;
        }
        Ok(())
    }

    /// Record a sent invite.
    pub fn record(&mut self, invite: SentInvite) {
        if !self.invites.iter().any(|i| i.nonce == invite.nonce) {
            self.invites.push(invite);
        }
    }

    /// Mark an invite as accepted (when we see the sender in our contacts).
    pub fn mark_accepted(&mut self, nonce: &str) {
        if let Some(inv) = self.invites.iter_mut().find(|i| i.nonce == nonce) {
            inv.status = SentInviteStatus::Accepted;
        }
    }

    /// Get pending invites.
    pub fn pending(&self) -> Vec<&SentInvite> {
        self.invites
            .iter()
            .filter(|i| i.status == SentInviteStatus::Pending)
            .collect()
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
            .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
        let node = crate::fold_node::FoldNode::new(config).await.unwrap();
        let db = node.get_fold_db().unwrap();
        (db, tmp)
    }

    fn mk_invite(nonce: &str) -> SentInvite {
        SentInvite {
            nonce: nonce.into(),
            recipient_hint: "unknown".into(),
            proposed_role: "friend".into(),
            created_at: Utc::now(),
            status: SentInviteStatus::Pending,
        }
    }

    #[tokio::test]
    async fn empty_load_returns_empty() {
        let (db, _tmp) = setup_db().await;
        let store = SentInviteStore::load(&db).await.unwrap();
        assert!(store.invites.is_empty());
    }

    #[tokio::test]
    async fn record_then_save_roundtrips() {
        let (db, _tmp) = setup_db().await;
        let mut store = SentInviteStore::default();
        store.record(mk_invite("n1"));
        store.record(mk_invite("n2"));
        store.save(&db).await.unwrap();

        let loaded = SentInviteStore::load(&db).await.unwrap();
        assert_eq!(loaded.invites.len(), 2);
        let nonces: std::collections::HashSet<String> =
            loaded.invites.iter().map(|i| i.nonce.clone()).collect();
        assert!(nonces.contains("n1"));
        assert!(nonces.contains("n2"));
    }

    #[tokio::test]
    async fn record_is_idempotent_on_duplicate_nonce() {
        let mut store = SentInviteStore::default();
        store.record(mk_invite("n1"));
        store.record(mk_invite("n1"));
        assert_eq!(store.invites.len(), 1);
    }

    #[tokio::test]
    async fn mark_accepted_updates_status() {
        let mut store = SentInviteStore::default();
        store.record(mk_invite("n1"));
        store.mark_accepted("n1");
        assert_eq!(store.invites[0].status, SentInviteStatus::Accepted);
        assert!(store.pending().is_empty());
    }

    #[tokio::test]
    async fn pending_filters_non_pending() {
        let mut store = SentInviteStore::default();
        store.record(mk_invite("n1"));
        store.record(mk_invite("n2"));
        store.mark_accepted("n1");
        let pending: Vec<&str> = store.pending().iter().map(|i| i.nonce.as_str()).collect();
        assert_eq!(pending, vec!["n2"]);
    }
}
