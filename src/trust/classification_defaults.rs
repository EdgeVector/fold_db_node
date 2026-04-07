//! Classification defaults — maps data classification to default access policies.
//!
//! When a schema is approved, its fields get default access policies based on
//! the schema's data classification (sensitivity + domain). Users can override.
//! Stored at `$FOLDDB_HOME/config/classification_defaults.json`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::utils::paths::folddb_home;

const CLASSIFICATION_DEFAULTS_FILE: &str = "config/classification_defaults.json";

/// Default access policy derived from data classification.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationDefault {
    pub trust_domain: String,
    pub read_max: u64,
    pub write_max: u64,
}

/// Maps (sensitivity_level, data_domain) to default access policy.
/// Key format: "{sensitivity_level}:{data_domain}" (e.g., "3:medical").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassificationDefaultsConfig {
    pub defaults: HashMap<String, ClassificationDefault>,
}

impl Default for ClassificationDefaultsConfig {
    fn default() -> Self {
        let mut defaults = HashMap::new();

        let add = |defaults: &mut HashMap<String, ClassificationDefault>,
                   key: &str,
                   domain: &str,
                   read_max: u64| {
            defaults.insert(
                key.to_string(),
                ClassificationDefault {
                    trust_domain: domain.to_string(),
                    read_max,
                    write_max: 0,
                },
            );
        };

        // Medical: always restrictive
        add(&mut defaults, "4:medical", "medical", 1);
        add(&mut defaults, "3:medical", "medical", 1);
        add(&mut defaults, "2:medical", "medical", 2);

        // Financial: restrictive
        add(&mut defaults, "4:financial", "financial", 1);
        add(&mut defaults, "3:financial", "financial", 1);
        add(&mut defaults, "2:financial", "financial", 2);

        // Health: moderate
        add(&mut defaults, "3:health", "health", 1);
        add(&mut defaults, "2:health", "health", 2);
        add(&mut defaults, "1:health", "health", 3);

        // Identity: restrictive in personal domain
        add(&mut defaults, "4:identity", "personal", 0);
        add(&mut defaults, "3:identity", "personal", 1);

        // General/personal: graduated
        add(&mut defaults, "0:general", "personal", u64::MAX); // public
        add(&mut defaults, "1:general", "personal", 5);
        add(&mut defaults, "2:general", "personal", 3);
        add(&mut defaults, "3:general", "personal", 1);
        add(&mut defaults, "4:general", "personal", 0); // owner only

        Self { defaults }
    }
}

impl ClassificationDefaultsConfig {
    fn file_path() -> Result<PathBuf, String> {
        Ok(folddb_home()?.join(CLASSIFICATION_DEFAULTS_FILE))
    }

    pub fn load() -> Result<Self, String> {
        Self::load_from(&Self::file_path()?)
    }

    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read classification defaults: {e}"))?;
        serde_json::from_str(&data)
            .map_err(|e| format!("Failed to parse classification defaults: {e}"))
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
            .map_err(|e| format!("Failed to serialize classification defaults: {e}"))?;
        std::fs::write(path, data)
            .map_err(|e| format!("Failed to write classification defaults: {e}"))
    }

    /// Look up the default policy for a classification.
    /// Tries exact match first, then fallback to "general" domain at same sensitivity.
    pub fn lookup(&self, sensitivity_level: u8, data_domain: &str) -> ClassificationDefault {
        // Exact match
        let key = format!("{}:{}", sensitivity_level, data_domain);
        if let Some(d) = self.defaults.get(&key) {
            return d.clone();
        }
        // Fallback: same sensitivity, "general" domain
        let fallback = format!("{}:general", sensitivity_level);
        if let Some(d) = self.defaults.get(&fallback) {
            return d.clone();
        }
        // Ultimate fallback: owner-only in personal domain
        ClassificationDefault {
            trust_domain: "personal".to_string(),
            read_max: 0,
            write_max: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = ClassificationDefaultsConfig::default();
        assert!(!config.defaults.is_empty());

        // Medical high sensitivity → medical domain, read_max 1
        let medical = config.lookup(4, "medical");
        assert_eq!(medical.trust_domain, "medical");
        assert_eq!(medical.read_max, 1);

        // General public → personal domain, public
        let public = config.lookup(0, "general");
        assert_eq!(public.trust_domain, "personal");
        assert_eq!(public.read_max, u64::MAX);
    }

    #[test]
    fn test_lookup_fallback() {
        let config = ClassificationDefaultsConfig::default();

        // Unknown domain at sensitivity 2 → falls back to "2:general"
        let unknown = config.lookup(2, "unknown_domain");
        assert_eq!(unknown.trust_domain, "personal");
        assert_eq!(unknown.read_max, 3);
    }

    #[test]
    fn test_lookup_ultimate_fallback() {
        let config = ClassificationDefaultsConfig {
            defaults: HashMap::new(),
        };
        // Empty config → owner-only
        let result = config.lookup(3, "anything");
        assert_eq!(result.read_max, 0);
    }

    #[test]
    fn test_roundtrip() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("defaults.json");
        let config = ClassificationDefaultsConfig::default();
        config.save_to(&path).unwrap();
        let loaded = ClassificationDefaultsConfig::load_from(&path).unwrap();
        assert_eq!(loaded.defaults.len(), config.defaults.len());
    }
}
