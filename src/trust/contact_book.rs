//! Contact book — maps public keys to human-readable identity info.
//!
//! Stored under the synced `user_profile/contacts/<pubkey>` keys via
//! [`UserProfileStore`]. Each contact is an independent record; writes
//! propagate to peer devices via the sync log so restored devices see
//! the same trust relationships automatically. There is no per-device
//! JSON file — trust is user-level by nature (Alice trusts Bob, not
//! "Alice's laptop trusts Bob").

use chrono::{DateTime, Utc};
use fold_db::fold_db_core::FoldDB;
use fold_db::schema::SchemaError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::user_profile::UserProfileStore;

/// Sub-prefix under `UserProfileStore` for contact records.
/// Each contact is stored at `contacts/<base64_pubkey>`.
const CONTACTS_PREFIX: &str = "contacts/";

fn contact_key(public_key: &str) -> String {
    format!("{CONTACTS_PREFIX}{public_key}")
}

/// Direction of the trust relationship.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrustDirection {
    /// You trust them, they haven't trusted you back.
    Outgoing,
    /// They trust you, you haven't trusted them back.
    Incoming,
    /// Mutual trust established.
    Mutual,
}

/// A contact entry — identity info received via a trust invite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contact {
    /// The peer's Ed25519 public key (base64).
    pub public_key: String,
    /// Display name from their identity card.
    pub display_name: String,
    /// Optional contact hint from their identity card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_hint: Option<String>,
    /// Direction of trust.
    pub direction: TrustDirection,
    /// When the trust relationship was established.
    pub connected_at: DateTime<Utc>,
    /// Discovery pseudonym, if the connection came from discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pseudonym: Option<String>,
    /// Bulletin board pseudonym for async messaging (UUID string).
    /// Set when the contact exchanges messaging keys (e.g., via discovery connection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messaging_pseudonym: Option<String>,
    /// X25519 public key for encrypting async messages (base64).
    /// Set alongside `messaging_pseudonym`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub messaging_public_key: Option<String>,
    /// **Stable identity pseudonym** of this contact, derived from their
    /// master key alone (not from any schema name). Used as the primary
    /// match key in referral queries so that two different nodes that both
    /// know this contact will match on the same value regardless of
    /// whichever schemas the contact has opted into discovery.
    ///
    /// `None` on legacy rows created before this field was introduced;
    /// match logic falls back to `pseudonym` / `messaging_pseudonym` in
    /// that case.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity_pseudonym: Option<String>,
    /// Whether trust has been revoked (kept for history).
    #[serde(default)]
    pub revoked: bool,
    /// Roles assigned to this contact, keyed by domain.
    /// e.g., {"personal": "friend", "health": "trainer"}
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub roles: HashMap<String, String>,
}

impl Contact {
    /// Create a contact from a discovery connection acceptance.
    #[allow(clippy::too_many_arguments)]
    pub fn from_discovery(
        public_key: String,
        display_name: String,
        contact_hint: Option<String>,
        direction: TrustDirection,
        pseudonym: Option<String>,
        messaging_public_key: Option<String>,
        identity_pseudonym: Option<String>,
        role_domain: String,
        role_name: String,
    ) -> Self {
        let mut roles = HashMap::new();
        roles.insert(role_domain, role_name);
        Self {
            public_key,
            display_name,
            contact_hint,
            direction,
            connected_at: Utc::now(),
            messaging_pseudonym: pseudonym.clone(),
            pseudonym,
            messaging_public_key,
            identity_pseudonym,
            revoked: false,
            roles,
        }
    }
}

/// In-memory view of every contact for this user. Load with
/// [`ContactBook::load`], mutate via [`Self::upsert_contact`] /
/// [`Self::revoke`] / [`Self::mark_mutual`], persist with
/// [`ContactBook::save`]. All persistence flows through the synced
/// user-profile store, so saves propagate to peer devices.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContactBook {
    /// Map from public key to contact info.
    pub contacts: HashMap<String, Contact>,
}

impl ContactBook {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load every contact from the synced user-profile store.
    pub async fn load(db: &FoldDB) -> Result<Self, SchemaError> {
        let store = UserProfileStore::from_db(db);
        let rows: Vec<(String, Contact)> = store.scan(CONTACTS_PREFIX).await?;
        let contacts = rows
            .into_iter()
            .map(|(key, contact)| {
                // Strip the `contacts/` sub-prefix to recover the pubkey.
                let pubkey = key
                    .strip_prefix(CONTACTS_PREFIX)
                    .unwrap_or(&key)
                    .to_string();
                (pubkey, contact)
            })
            .collect();
        Ok(Self { contacts })
    }

    /// Persist every contact in this book. Writes each contact as its own
    /// record at `user_profile/contacts/<pubkey>`. There's no atomic
    /// "write the whole book" operation — each contact converges
    /// independently via its own sync-log entry, which is the right
    /// shape for multi-device: a peer adding a contact doesn't conflict
    /// with us adding a different one.
    pub async fn save(&self, db: &FoldDB) -> Result<(), SchemaError> {
        let store = UserProfileStore::from_db(db);
        for (pubkey, contact) in &self.contacts {
            store.put(&contact_key(pubkey), contact).await?;
        }
        Ok(())
    }

    /// Persist a single contact. Preferred over [`Self::save`] when only
    /// one contact changed — avoids re-writing the whole book.
    pub async fn save_one(db: &FoldDB, contact: &Contact) -> Result<(), SchemaError> {
        let store = UserProfileStore::from_db(db);
        store.put(&contact_key(&contact.public_key), contact).await
    }

    /// Add or update a contact. Preserves existing roles on update.
    pub fn upsert_contact(&mut self, contact: Contact) {
        let key = contact.public_key.clone();
        if let Some(existing) = self.contacts.get_mut(&key) {
            existing.display_name = contact.display_name;
            existing.contact_hint = contact.contact_hint;
            existing.direction = contact.direction;
            existing.connected_at = contact.connected_at;
            if contact.pseudonym.is_some() {
                existing.pseudonym = contact.pseudonym;
            }
            // Preserve messaging fields — only overwrite if new values provided
            if contact.messaging_pseudonym.is_some() {
                existing.messaging_pseudonym = contact.messaging_pseudonym;
            }
            if contact.messaging_public_key.is_some() {
                existing.messaging_public_key = contact.messaging_public_key;
            }
            if contact.identity_pseudonym.is_some() {
                existing.identity_pseudonym = contact.identity_pseudonym;
            }
            // Preserve existing roles — don't overwrite with empty map
            if !contact.roles.is_empty() {
                existing.roles = contact.roles;
            }
            existing.revoked = false;
        } else {
            self.contacts.insert(key, contact);
        }
    }

    /// Mark a contact as revoked (keep for history).
    pub fn revoke(&mut self, public_key: &str) -> bool {
        if let Some(contact) = self.contacts.get_mut(public_key) {
            contact.revoked = true;
            true
        } else {
            false
        }
    }

    /// Update direction to mutual when both sides have trusted.
    pub fn mark_mutual(&mut self, public_key: &str) {
        if let Some(contact) = self.contacts.get_mut(public_key) {
            contact.direction = TrustDirection::Mutual;
        }
    }

    /// Get a contact by public key.
    pub fn get(&self, public_key: &str) -> Option<&Contact> {
        self.contacts.get(public_key)
    }

    /// List all active (non-revoked) contacts.
    pub fn active_contacts(&self) -> Vec<&Contact> {
        self.contacts.values().filter(|c| !c.revoked).collect()
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

    fn make_contact(pubkey: &str, name: &str) -> Contact {
        Contact {
            public_key: pubkey.to_string(),
            display_name: name.to_string(),
            contact_hint: None,
            direction: TrustDirection::Outgoing,
            connected_at: Utc::now(),
            pseudonym: None,
            messaging_pseudonym: None,
            messaging_public_key: None,
            identity_pseudonym: None,
            revoked: false,
            roles: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn empty_book_loads_empty() {
        let (db, _tmp) = setup_db().await;
        let book = ContactBook::load(&db).await.unwrap();
        assert!(book.contacts.is_empty());
    }

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let (db, _tmp) = setup_db().await;
        let mut book = ContactBook::new();
        book.upsert_contact(make_contact("pk_alice", "Alice"));
        book.upsert_contact(make_contact("pk_bob", "Bob"));
        book.save(&db).await.unwrap();

        let loaded = ContactBook::load(&db).await.unwrap();
        assert_eq!(loaded.contacts.len(), 2);
        assert_eq!(loaded.get("pk_alice").unwrap().display_name, "Alice");
        assert_eq!(loaded.get("pk_bob").unwrap().display_name, "Bob");
    }

    #[tokio::test]
    async fn save_one_persists_single_contact() {
        let (db, _tmp) = setup_db().await;
        ContactBook::save_one(&db, &make_contact("pk_carol", "Carol"))
            .await
            .unwrap();
        let loaded = ContactBook::load(&db).await.unwrap();
        assert_eq!(loaded.contacts.len(), 1);
        assert_eq!(loaded.get("pk_carol").unwrap().display_name, "Carol");
    }

    #[tokio::test]
    async fn revoke_sticks_across_reload() {
        let (db, _tmp) = setup_db().await;
        let mut book = ContactBook::new();
        book.upsert_contact(make_contact("pk_alice", "Alice"));
        book.save(&db).await.unwrap();

        book.revoke("pk_alice");
        book.save(&db).await.unwrap();

        let loaded = ContactBook::load(&db).await.unwrap();
        assert!(loaded.get("pk_alice").unwrap().revoked);
        assert!(loaded.active_contacts().is_empty());
    }

    #[tokio::test]
    async fn active_contacts_filters_revoked() {
        let mut book = ContactBook::new();
        book.upsert_contact(make_contact("a", "A"));
        book.upsert_contact(make_contact("b", "B"));
        book.revoke("a");
        let active = book.active_contacts();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].display_name, "B");
    }

    #[tokio::test]
    async fn upsert_preserves_existing_roles_on_empty_update() {
        let mut book = ContactBook::new();
        let mut first = make_contact("pk_x", "X");
        first.roles.insert("personal".into(), "friend".into());
        book.upsert_contact(first);
        book.upsert_contact(make_contact("pk_x", "X-updated"));
        let c = book.get("pk_x").unwrap();
        assert_eq!(c.display_name, "X-updated");
        assert_eq!(c.roles.get("personal").unwrap(), "friend");
    }

    #[tokio::test]
    async fn mark_mutual_updates_direction() {
        let mut book = ContactBook::new();
        book.upsert_contact(make_contact("pk", "name"));
        book.mark_mutual("pk");
        assert_eq!(book.get("pk").unwrap().direction, TrustDirection::Mutual);
    }
}
