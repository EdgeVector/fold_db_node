//! Identity card — local-only personal info for trust invites.
//!
//! Stored at `$FOLDDB_HOME/config/identity_card.json`. Never synced to Exemem.
//! Only shared inside E2E-encrypted trust invites with specific peers.

use fold_db::NodeConfigStore;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::utils::paths::folddb_home;

const IDENTITY_CARD_FILE: &str = "config/identity_card.json";

/// A user's local identity card. Never leaves the device except inside
/// E2E-encrypted trust invites to specific peers.
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

    /// Resolve the path to the identity card file.
    fn file_path() -> Result<PathBuf, String> {
        Ok(folddb_home()?.join(IDENTITY_CARD_FILE))
    }

    /// Load the identity card from disk. Returns `None` if the file doesn't exist.
    pub fn load() -> Result<Option<Self>, String> {
        Self::load_from(&Self::file_path()?)
    }

    /// Load from a specific path (for testing).
    pub fn load_from(path: &Path) -> Result<Option<Self>, String> {
        if !path.exists() {
            return Ok(None);
        }
        let data = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read identity card: {e}"))?;
        let card: Self = serde_json::from_str(&data)
            .map_err(|e| format!("Failed to parse identity card: {e}"))?;
        Ok(Some(card))
    }

    /// Save the identity card to disk.
    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::file_path()?)
    }

    /// Save to a specific path (for testing).
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config directory: {e}"))?;
        }
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize identity card: {e}"))?;
        std::fs::write(path, data).map_err(|e| format!("Failed to write identity card: {e}"))?;
        Ok(())
    }

    /// Load the identity card from the Sled config store.
    ///
    /// Returns `None` if the store has no display_name (not yet migrated).
    pub fn load_from_sled(store: &NodeConfigStore) -> Option<Self> {
        let display_name = store.get_display_name()?;
        let contact_hint = store.get_contact_hint();
        let birthday = store.get_birthday();
        Some(Self {
            display_name,
            contact_hint,
            birthday,
        })
    }

    /// Save the identity card to the Sled config store.
    pub fn save_to_sled(&self, store: &NodeConfigStore) -> Result<(), String> {
        store
            .set_display_name(&self.display_name)
            .map_err(|e| format!("Failed to save display_name to Sled: {e}"))?;
        if let Some(ref hint) = self.contact_hint {
            store
                .set_contact_hint(hint)
                .map_err(|e| format!("Failed to save contact_hint to Sled: {e}"))?;
        }
        if let Some(ref bday) = self.birthday {
            store
                .set_birthday(bday)
                .map_err(|e| format!("Failed to save birthday to Sled: {e}"))?;
        }
        Ok(())
    }

    /// Delete the identity card from disk.
    pub fn delete() -> Result<(), String> {
        let path = Self::file_path()?;
        if path.exists() {
            std::fs::remove_file(&path)
                .map_err(|e| format!("Failed to delete identity card: {e}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_identity_card_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("identity_card.json");

        let card = IdentityCard::new(
            "Alice".to_string(),
            Some("alice@example.com".to_string()),
            Some("03-15".to_string()),
        );
        card.save_to(&path).unwrap();

        let loaded = IdentityCard::load_from(&path).unwrap().unwrap();
        assert_eq!(loaded.display_name, "Alice");
        assert_eq!(loaded.contact_hint.as_deref(), Some("alice@example.com"));
        assert_eq!(loaded.birthday.as_deref(), Some("03-15"));
    }

    #[test]
    fn test_load_nonexistent_returns_none() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.json");
        assert!(IdentityCard::load_from(&path).unwrap().is_none());
    }

    #[test]
    fn test_identity_card_without_optional_fields() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("identity_card.json");

        let card = IdentityCard::new("Bob".to_string(), None, None);
        card.save_to(&path).unwrap();

        let loaded = IdentityCard::load_from(&path).unwrap().unwrap();
        assert_eq!(loaded.display_name, "Bob");
        assert!(loaded.contact_hint.is_none());
        assert!(loaded.birthday.is_none());
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

    #[test]
    fn test_backward_compat_no_birthday() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("identity_card.json");

        // Old format without birthday field
        let json = r#"{"display_name": "Alice", "contact_hint": "alice@example.com"}"#;
        std::fs::write(&path, json).unwrap();

        let loaded = IdentityCard::load_from(&path).unwrap().unwrap();
        assert_eq!(loaded.display_name, "Alice");
        assert!(loaded.birthday.is_none());
    }
}
