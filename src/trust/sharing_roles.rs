//! Sharing roles — user-facing abstraction over trust tiers.
//!
//! Users assign roles ("friend", "doctor", "trainer") instead of managing
//! raw trust tiers. Each role maps to a (domain, tier) pair. Role
//! definitions are user-level — they propagate across every device restored
//! from the same mnemonic so Alice's custom roles follow her to every
//! device. Stored under `user_profile/sharing_roles/<name>` via
//! [`UserProfileStore`].
//!
//! First-time load on a device with no synced roles seeds the built-in
//! defaults (`friend`, `doctor`, etc.) and persists them.

use fold_db::access::AccessTier;
use fold_db::fold_db_core::FoldDB;
use fold_db::schema::SchemaError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::user_profile::UserProfileStore;

/// Sub-prefix under `UserProfileStore`. One record per role name.
const SHARING_ROLES_PREFIX: &str = "sharing_roles/";

fn role_key(name: &str) -> String {
    format!("{SHARING_ROLES_PREFIX}{name}")
}

/// A sharing role maps a user-friendly name to a (domain, tier) pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharingRole {
    pub name: String,
    pub domain: String,
    pub tier: AccessTier,
    pub description: String,
}

/// All role definitions for this user.
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
                   tier: AccessTier,
                   desc: &str| {
            roles.insert(
                name.to_string(),
                SharingRole {
                    name: name.to_string(),
                    domain: domain.to_string(),
                    tier,
                    description: desc.to_string(),
                },
            );
        };

        // Personal domain
        add(
            &mut roles,
            "close_friend",
            "personal",
            AccessTier::Inner,
            "Can see most personal data",
        );
        add(
            &mut roles,
            "friend",
            "personal",
            AccessTier::Trusted,
            "Can see general personal data",
        );
        add(
            &mut roles,
            "acquaintance",
            "personal",
            AccessTier::Outer,
            "Minimal personal sharing",
        );

        // Family domain
        add(
            &mut roles,
            "family",
            "family",
            AccessTier::Inner,
            "Can see family-related data",
        );

        // Health domain
        add(
            &mut roles,
            "trainer",
            "health",
            AccessTier::Trusted,
            "Can see fitness and wellness data",
        );

        // Medical domain
        add(
            &mut roles,
            "doctor",
            "medical",
            AccessTier::Inner,
            "Can see medical records",
        );

        // Financial domain
        add(
            &mut roles,
            "financial_advisor",
            "financial",
            AccessTier::Inner,
            "Can see financial data",
        );

        Self { roles }
    }
}

impl SharingRoleConfig {
    /// Load all role definitions from the synced user-profile store. If the
    /// store is empty (first-time load on a fresh device or first run after
    /// this migration), seed the built-in defaults and persist them so
    /// peer devices see the same baseline.
    pub async fn load(db: &FoldDB) -> Result<Self, SchemaError> {
        let store = UserProfileStore::from_db(db);
        let rows: Vec<(String, SharingRole)> = store.scan(SHARING_ROLES_PREFIX).await?;
        if rows.is_empty() {
            let defaults = Self::default();
            defaults.save(db).await?;
            return Ok(defaults);
        }
        let roles: HashMap<String, SharingRole> = rows
            .into_iter()
            .map(|(_, role)| (role.name.clone(), role))
            .collect();
        Ok(Self { roles })
    }

    /// Persist every role definition. Each role is keyed by name.
    pub async fn save(&self, db: &FoldDB) -> Result<(), SchemaError> {
        let store = UserProfileStore::from_db(db);
        for (name, role) in &self.roles {
            store.put(&role_key(name), role).await?;
        }
        Ok(())
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
        assert_eq!(friend.tier, AccessTier::Trusted);

        let doctor = config.get_role("doctor").unwrap();
        assert_eq!(doctor.domain, "medical");
        assert_eq!(doctor.tier, AccessTier::Inner);
    }

    #[test]
    fn test_roles_for_domain() {
        let config = SharingRoleConfig::default();
        let personal = config.roles_for_domain("personal");
        assert_eq!(personal.len(), 3); // close_friend, friend, acquaintance
        assert!(personal.iter().all(|r| r.domain == "personal"));
    }

    #[tokio::test]
    async fn first_load_seeds_defaults() {
        let (db, _tmp) = setup_db().await;
        let config = SharingRoleConfig::load(&db).await.unwrap();
        assert!(!config.roles.is_empty());
        assert!(config.get_role("friend").is_some());

        // Reload should return the same persisted roles, not re-seed.
        let again = SharingRoleConfig::load(&db).await.unwrap();
        assert_eq!(again.roles.len(), config.roles.len());
    }

    #[tokio::test]
    async fn save_then_load_roundtrips_custom_role() {
        let (db, _tmp) = setup_db().await;
        let mut config = SharingRoleConfig::load(&db).await.unwrap();
        config.roles.insert(
            "lawyer".to_string(),
            SharingRole {
                name: "lawyer".to_string(),
                domain: "legal".to_string(),
                tier: AccessTier::Inner,
                description: "Legal counsel".to_string(),
            },
        );
        config.save(&db).await.unwrap();

        let loaded = SharingRoleConfig::load(&db).await.unwrap();
        let lawyer = loaded.get_role("lawyer").unwrap();
        assert_eq!(lawyer.domain, "legal");
        assert_eq!(lawyer.tier, AccessTier::Inner);
    }
}
