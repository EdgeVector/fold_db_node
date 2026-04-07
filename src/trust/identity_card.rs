//! Identity card — local-only personal info for trust invites.
//!
//! Stored at `$FOLDDB_HOME/config/identity_card.json`. Never synced to Exemem.
//! Only shared inside E2E-encrypted trust invites with specific peers.

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
}

impl IdentityCard {
    pub fn new(display_name: String, contact_hint: Option<String>) -> Self {
        Self {
            display_name,
            contact_hint,
        }
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

        let card = IdentityCard::new("Alice".to_string(), Some("alice@example.com".to_string()));
        card.save_to(&path).unwrap();

        let loaded = IdentityCard::load_from(&path).unwrap().unwrap();
        assert_eq!(loaded.display_name, "Alice");
        assert_eq!(loaded.contact_hint.as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn test_load_nonexistent_returns_none() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.json");
        assert!(IdentityCard::load_from(&path).unwrap().is_none());
    }

    #[test]
    fn test_identity_card_without_contact_hint() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("identity_card.json");

        let card = IdentityCard::new("Bob".to_string(), None);
        card.save_to(&path).unwrap();

        let loaded = IdentityCard::load_from(&path).unwrap().unwrap();
        assert_eq!(loaded.display_name, "Bob");
        assert!(loaded.contact_hint.is_none());
    }
}
