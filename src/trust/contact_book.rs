//! Contact book — maps public keys to human-readable identity info.
//!
//! Stored at `$FOLDDB_HOME/config/contact_book.json`. Local-only, never synced.
//! Populated when trust invites are accepted.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::utils::paths::folddb_home;

const CONTACT_BOOK_FILE: &str = "config/contact_book.json";

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
    /// Trust distance in your graph.
    pub trust_distance: u64,
    /// Direction of trust.
    pub direction: TrustDirection,
    /// When the trust relationship was established.
    pub connected_at: DateTime<Utc>,
    /// Discovery pseudonym, if the connection came from discovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pseudonym: Option<String>,
    /// Whether trust has been revoked (kept for history).
    #[serde(default)]
    pub revoked: bool,
    /// Roles assigned to this contact, keyed by domain.
    /// e.g., {"personal": "friend", "health": "trainer"}
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub roles: HashMap<String, String>,
}

/// The contact book — all known trust contacts.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContactBook {
    /// Map from public key to contact info.
    pub contacts: HashMap<String, Contact>,
}

impl ContactBook {
    pub fn new() -> Self {
        Self::default()
    }

    fn file_path() -> Result<PathBuf, String> {
        Ok(folddb_home()?.join(CONTACT_BOOK_FILE))
    }

    pub fn load() -> Result<Self, String> {
        Self::load_from(&Self::file_path()?)
    }

    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::new());
        }
        let data = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read contact book: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("Failed to parse contact book: {e}"))
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::file_path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {e}"))?;
        }
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize contact book: {e}"))?;
        std::fs::write(path, data).map_err(|e| format!("Failed to write contact book: {e}"))?;
        Ok(())
    }

    /// Add or update a contact.
    pub fn upsert_contact(&mut self, contact: Contact) {
        let key = contact.public_key.clone();
        if let Some(existing) = self.contacts.get_mut(&key) {
            existing.display_name = contact.display_name;
            existing.contact_hint = contact.contact_hint;
            existing.trust_distance = contact.trust_distance;
            existing.direction = contact.direction;
            if contact.pseudonym.is_some() {
                existing.pseudonym = contact.pseudonym;
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
    use tempfile::TempDir;

    fn test_contact(key: &str, name: &str, distance: u64) -> Contact {
        Contact {
            public_key: key.to_string(),
            display_name: name.to_string(),
            contact_hint: None,
            trust_distance: distance,
            direction: TrustDirection::Outgoing,
            connected_at: Utc::now(),
            pseudonym: None,
            revoked: false,
            roles: HashMap::new(),
        }
    }

    #[test]
    fn test_contact_book_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("contacts.json");

        let mut book = ContactBook::new();
        book.upsert_contact(test_contact("pk_alice", "Alice", 1));
        book.upsert_contact(test_contact("pk_bob", "Bob", 2));
        book.save_to(&path).unwrap();

        let loaded = ContactBook::load_from(&path).unwrap();
        assert_eq!(loaded.contacts.len(), 2);
        assert_eq!(loaded.get("pk_alice").unwrap().display_name, "Alice");
    }

    #[test]
    fn test_revoke_keeps_history() {
        let mut book = ContactBook::new();
        book.upsert_contact(test_contact("pk_alice", "Alice", 1));
        assert_eq!(book.active_contacts().len(), 1);

        book.revoke("pk_alice");
        assert_eq!(book.active_contacts().len(), 0);
        assert!(book.get("pk_alice").unwrap().revoked);
    }

    #[test]
    fn test_upsert_updates_existing() {
        let mut book = ContactBook::new();
        book.upsert_contact(test_contact("pk_alice", "Alice", 1));
        book.upsert_contact(Contact {
            public_key: "pk_alice".to_string(),
            display_name: "Alice Chen".to_string(),
            contact_hint: Some("alice@example.com".to_string()),
            trust_distance: 2,
            direction: TrustDirection::Mutual,
            connected_at: Utc::now(),
            pseudonym: None,
            revoked: false,
            roles: HashMap::new(),
        });
        let alice = book.get("pk_alice").unwrap();
        assert_eq!(alice.display_name, "Alice Chen");
        assert_eq!(alice.trust_distance, 2);
        assert_eq!(alice.direction, TrustDirection::Mutual);
    }

    #[test]
    fn test_mark_mutual() {
        let mut book = ContactBook::new();
        book.upsert_contact(test_contact("pk_alice", "Alice", 1));
        assert_eq!(
            book.get("pk_alice").unwrap().direction,
            TrustDirection::Outgoing
        );

        book.mark_mutual("pk_alice");
        assert_eq!(
            book.get("pk_alice").unwrap().direction,
            TrustDirection::Mutual
        );
    }
}
