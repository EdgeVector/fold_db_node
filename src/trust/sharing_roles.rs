//! Sharing roles — user-facing abstraction over trust distances.
//!
//! Users assign roles ("friend", "doctor", "trainer") instead of managing
//! raw distance numbers. Each role maps to a (domain, distance) pair.
//! Stored at `$FOLDDB_HOME/config/sharing_roles.json`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::utils::paths::folddb_home;

const SHARING_ROLES_FILE: &str = "config/sharing_roles.json";

/// A sharing role maps a user-friendly name to a (domain, distance) pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharingRole {
    pub name: String,
    pub domain: String,
    pub distance: u64,
    pub description: String,
}

/// All role definitions for this node. User-editable.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharingRoleConfig {
    pub roles: HashMap<String, SharingRole>,
}

impl Default for SharingRoleConfig {
    fn default() -> Self {
        let mut roles = HashMap::new();

        let add = |roles: &mut HashMap<String, SharingRole>,
                   name: &str,
                   domain: &str,
                   distance: u64,
                   desc: &str| {
            roles.insert(
                name.to_string(),
                SharingRole {
                    name: name.to_string(),
                    domain: domain.to_string(),
                    distance,
                    description: desc.to_string(),
                },
            );
        };

        // Personal domain
        add(
            &mut roles,
            "close_friend",
            "personal",
            1,
            "Can see most personal data",
        );
        add(
            &mut roles,
            "friend",
            "personal",
            3,
            "Can see general personal data",
        );
        add(
            &mut roles,
            "acquaintance",
            "personal",
            5,
            "Minimal personal sharing",
        );

        // Family domain
        add(
            &mut roles,
            "family",
            "family",
            1,
            "Can see family-related data",
        );

        // Health domain
        add(
            &mut roles,
            "trainer",
            "health",
            2,
            "Can see fitness and wellness data",
        );

        // Medical domain
        add(
            &mut roles,
            "doctor",
            "medical",
            1,
            "Can see medical records",
        );

        // Financial domain
        add(
            &mut roles,
            "financial_advisor",
            "financial",
            1,
            "Can see financial data",
        );

        Self { roles }
    }
}

impl SharingRoleConfig {
    fn file_path() -> Result<PathBuf, String> {
        Ok(folddb_home()?.join(SHARING_ROLES_FILE))
    }

    pub fn load() -> Result<Self, String> {
        Self::load_from(&Self::file_path()?)
    }

    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            let config = Self::default();
            config.save_to(path)?;
            return Ok(config);
        }
        let data =
            std::fs::read_to_string(path).map_err(|e| format!("Failed to read roles: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("Failed to parse roles: {e}"))
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::file_path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config dir: {e}"))?;
        }
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize roles: {e}"))?;
        std::fs::write(path, data).map_err(|e| format!("Failed to write roles: {e}"))
    }

    pub fn get_role(&self, name: &str) -> Option<&SharingRole> {
        self.roles.get(name)
    }

    pub fn roles_for_domain(&self, domain: &str) -> Vec<&SharingRole> {
        self.roles.values().filter(|r| r.domain == domain).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_default_roles() {
        let config = SharingRoleConfig::default();
        assert!(config.get_role("friend").is_some());
        assert!(config.get_role("doctor").is_some());
        assert!(config.get_role("trainer").is_some());
        assert!(config.get_role("financial_advisor").is_some());
        assert!(config.get_role("family").is_some());
        assert!(config.get_role("close_friend").is_some());
        assert!(config.get_role("acquaintance").is_some());

        let friend = config.get_role("friend").unwrap();
        assert_eq!(friend.domain, "personal");
        assert_eq!(friend.distance, 3);

        let doctor = config.get_role("doctor").unwrap();
        assert_eq!(doctor.domain, "medical");
        assert_eq!(doctor.distance, 1);
    }

    #[test]
    fn test_roles_for_domain() {
        let config = SharingRoleConfig::default();
        let personal = config.roles_for_domain("personal");
        assert_eq!(personal.len(), 3); // close_friend, friend, acquaintance
        assert!(personal.iter().all(|r| r.domain == "personal"));
    }

    #[test]
    fn test_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("roles.json");

        let config = SharingRoleConfig::default();
        config.save_to(&path).unwrap();

        let loaded = SharingRoleConfig::load_from(&path).unwrap();
        assert_eq!(loaded.roles.len(), config.roles.len());
    }

    #[test]
    fn test_load_creates_default_if_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent.json");
        assert!(!path.exists());

        let config = SharingRoleConfig::load_from(&path).unwrap();
        assert!(!config.roles.is_empty());
        assert!(path.exists()); // Should have been created
    }
}
