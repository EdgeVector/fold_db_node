//! Identity card — display name + contact info shared across all devices
//! restored from the same mnemonic.
//!
//! Stored in the synced `user_profile/identity_card` key via
//! [`UserProfileStore`]. Writes propagate to peer devices via the sync log.
//! There is no per-device JSON file and no unsynced Sled copy — the card
//! is user-level by nature (same user on every device), so there's exactly
//! one canonical location.

use fold_db::fold_db_core::FoldDB;
use fold_db::schema::SchemaError;
use serde::{Deserialize, Serialize};

use crate::user_profile::UserProfileStore;

/// Storage sub-key under `UserProfileStore`.
const IDENTITY_CARD_KEY: &str = "identity_card";

/// A user's identity card. Propagates across every device that shares the
/// user's Ed25519 identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityCard {
    /// Human-readable display name (required).
    pub display_name: String,
    /// Optional contact hint for verification (email, phone, handle, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contact_hint: Option<String>,
    /// Optional birthday for verification (MM-DD format).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub birthday: Option<String>,
}

impl IdentityCard {
    pub fn new(
        display_name: String,
        contact_hint: Option<String>,
        birthday: Option<String>,
    ) -> Self {
        Self {
            display_name,
            contact_hint,
            birthday,
        }
    }

    /// Validate birthday format (MM-DD). Returns error message if invalid.
    pub fn validate_birthday(input: &str) -> Result<(), String> {
        if input.is_empty() {
            return Ok(());
        }
        let parts: Vec<&str> = input.split('-').collect();
        if parts.len() != 2 {
            return Err("Expected MM-DD format (e.g. 03-15)".to_string());
        }
        let month: u32 = parts[0]
            .parse()
            .map_err(|_| "Month must be a number".to_string())?;
        let day: u32 = parts[1]
            .parse()
            .map_err(|_| "Day must be a number".to_string())?;
        if !(1..=12).contains(&month) {
            return Err(format!("Month {} out of range (01-12)", month));
        }
        if !(1..=31).contains(&day) {
            return Err(format!("Day {} out of range (01-31)", day));
        }
        Ok(())
    }

    /// Load the identity card from the synced user-profile store. Returns
    /// `None` if the user has not set one yet.
    pub async fn load(db: &FoldDB) -> Result<Option<Self>, SchemaError> {
        UserProfileStore::from_db(db).get(IDENTITY_CARD_KEY).await
    }

    /// Save the identity card. Writes propagate to every peer device.
    pub async fn save(&self, db: &FoldDB) -> Result<(), SchemaError> {
        UserProfileStore::from_db(db)
            .put(IDENTITY_CARD_KEY, self)
            .await
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

    #[tokio::test]
    async fn save_then_load_roundtrips() {
        let (db, _tmp) = setup_db().await;
        let card = IdentityCard::new(
            "Alice".to_string(),
            Some("alice@example.com".to_string()),
            Some("03-15".to_string()),
        );
        card.save(&db).await.unwrap();

        let loaded = IdentityCard::load(&db).await.unwrap().unwrap();
        assert_eq!(loaded.display_name, "Alice");
        assert_eq!(loaded.contact_hint.as_deref(), Some("alice@example.com"));
        assert_eq!(loaded.birthday.as_deref(), Some("03-15"));
    }

    #[tokio::test]
    async fn load_absent_returns_none() {
        let (db, _tmp) = setup_db().await;
        let loaded = IdentityCard::load(&db).await.unwrap();
        assert!(loaded.is_none());
    }

    #[tokio::test]
    async fn save_overwrites_existing() {
        let (db, _tmp) = setup_db().await;
        IdentityCard::new("Alice".into(), None, None)
            .save(&db)
            .await
            .unwrap();
        IdentityCard::new("Alice v2".into(), Some("new@example.com".into()), None)
            .save(&db)
            .await
            .unwrap();
        let loaded = IdentityCard::load(&db).await.unwrap().unwrap();
        assert_eq!(loaded.display_name, "Alice v2");
        assert_eq!(loaded.contact_hint.as_deref(), Some("new@example.com"));
    }

    #[test]
    fn test_validate_birthday() {
        assert!(IdentityCard::validate_birthday("").is_ok());
        assert!(IdentityCard::validate_birthday("03-15").is_ok());
        assert!(IdentityCard::validate_birthday("12-31").is_ok());
        assert!(IdentityCard::validate_birthday("01-01").is_ok());
        assert!(IdentityCard::validate_birthday("13-01").is_err());
        assert!(IdentityCard::validate_birthday("00-15").is_err());
        assert!(IdentityCard::validate_birthday("03-32").is_err());
        assert!(IdentityCard::validate_birthday("not-a-date").is_err());
        assert!(IdentityCard::validate_birthday("3-5").is_ok()); // permissive parsing
    }
}
