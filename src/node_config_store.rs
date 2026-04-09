//! Temporary stubs for `fold_db::storage::node_config_store` types.
//!
//! These stubs exist so that fold_db_node code can be written and reviewed
//! against the NodeConfigStore API before the fold_db PR that adds the real
//! types has merged. Once that PR lands and fold_db is updated, this module
//! should be deleted and all imports switched to `fold_db::storage::node_config_store`.
//!
//! **Do not use these stubs for any purpose other than compilation scaffolding.**

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Structs mirroring the fold_db NodeConfigStore API
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudCredentials {
    pub api_url: String,
    pub api_key: String,
    pub session_token: Option<String>,
    pub user_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeIdentity {
    pub private_key: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    pub provider: String,
    pub anthropic_key: Option<String>,
    pub anthropic_model: Option<String>,
    pub anthropic_base_url: Option<String>,
    pub ollama_model: Option<String>,
    pub ollama_url: Option<String>,
    pub ollama_vision_model: Option<String>,
}

/// Stub for the Sled-backed config store that will live in fold_db.
///
/// In production, this is obtained via `FoldDB::config_store()` which returns
/// `Option<&NodeConfigStore>`. The real implementation stores values in a
/// dedicated Sled tree (`node_config`).
pub struct NodeConfigStore {
    tree: sled::Tree,
}

impl NodeConfigStore {
    /// Open (or create) the `node_config` tree on the given Sled database.
    /// This constructor is only used by the stub; the real one lives in fold_db.
    pub fn open(db: &sled::Db) -> Result<Self, sled::Error> {
        let tree = db.open_tree("node_config")?;
        Ok(Self { tree })
    }

    pub fn get_cloud_config(&self) -> Option<CloudCredentials> {
        let api_url = self.get_str("cloud:api_url")?;
        let api_key = self.get_str("cloud:api_key")?;
        let session_token = self.get_str("cloud:session_token");
        let user_hash = self.get_str("cloud:user_hash");
        Some(CloudCredentials {
            api_url,
            api_key,
            session_token,
            user_hash,
        })
    }

    pub fn set_cloud_config(&self, creds: &CloudCredentials) -> Result<(), sled::Error> {
        self.tree
            .insert("cloud:api_url", creds.api_url.as_bytes())?;
        self.tree
            .insert("cloud:api_key", creds.api_key.as_bytes())?;
        if let Some(ref token) = creds.session_token {
            self.tree
                .insert("cloud:session_token", token.as_bytes())?;
        }
        if let Some(ref hash) = creds.user_hash {
            self.tree.insert("cloud:user_hash", hash.as_bytes())?;
        }
        self.tree.flush()?;
        Ok(())
    }

    pub fn get_identity(&self) -> Option<NodeIdentity> {
        let private_key = self.get_str("identity:private_key")?;
        let public_key = self.get_str("identity:public_key")?;
        Some(NodeIdentity {
            private_key,
            public_key,
        })
    }

    pub fn set_identity(&self, id: &NodeIdentity) -> Result<(), sled::Error> {
        self.tree
            .insert("identity:private_key", id.private_key.as_bytes())?;
        self.tree
            .insert("identity:public_key", id.public_key.as_bytes())?;
        self.tree.flush()?;
        Ok(())
    }

    pub fn get_ai_config(&self) -> Option<AiConfig> {
        let provider = self.get_str("ai:provider")?;
        Some(AiConfig {
            provider,
            anthropic_key: self.get_str("ai:anthropic_key"),
            anthropic_model: self.get_str("ai:anthropic_model"),
            anthropic_base_url: self.get_str("ai:anthropic_base_url"),
            ollama_model: self.get_str("ai:ollama_model"),
            ollama_url: self.get_str("ai:ollama_url"),
            ollama_vision_model: self.get_str("ai:ollama_vision_model"),
        })
    }

    pub fn set_ai_config(&self, config: &AiConfig) -> Result<(), sled::Error> {
        self.tree
            .insert("ai:provider", config.provider.as_bytes())?;
        self.set_optional("ai:anthropic_key", &config.anthropic_key)?;
        self.set_optional("ai:anthropic_model", &config.anthropic_model)?;
        self.set_optional("ai:anthropic_base_url", &config.anthropic_base_url)?;
        self.set_optional("ai:ollama_model", &config.ollama_model)?;
        self.set_optional("ai:ollama_url", &config.ollama_url)?;
        self.set_optional("ai:ollama_vision_model", &config.ollama_vision_model)?;
        self.tree.flush()?;
        Ok(())
    }

    pub fn is_cloud_enabled(&self) -> bool {
        self.get_cloud_config().is_some()
    }

    pub fn get_display_name(&self) -> Option<String> {
        self.get_str("identity_card:display_name")
    }

    pub fn set_display_name(&self, name: &str) -> Result<(), sled::Error> {
        self.tree
            .insert("identity_card:display_name", name.as_bytes())?;
        self.tree.flush()?;
        Ok(())
    }

    pub fn get_contact_hint(&self) -> Option<String> {
        self.get_str("identity_card:contact_hint")
    }

    pub fn set_contact_hint(&self, hint: &str) -> Result<(), sled::Error> {
        self.tree
            .insert("identity_card:contact_hint", hint.as_bytes())?;
        self.tree.flush()?;
        Ok(())
    }

    pub fn get_schema_service_url(&self) -> Option<String> {
        self.get_str("schema_service_url")
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    // -- private helpers --

    fn get_str(&self, key: &str) -> Option<String> {
        self.tree
            .get(key)
            .ok()?
            .map(|v| String::from_utf8_lossy(&v).to_string())
    }

    fn set_optional(&self, key: &str, value: &Option<String>) -> Result<(), sled::Error> {
        match value {
            Some(v) => {
                self.tree.insert(key, v.as_bytes())?;
            }
            None => {
                self.tree.remove(key)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> NodeConfigStore {
        let db = sled::Config::new().temporary(true).open().unwrap();
        NodeConfigStore::open(&db).unwrap()
    }

    #[test]
    fn test_empty_store() {
        let store = temp_store();
        assert!(store.is_empty());
        assert!(store.get_cloud_config().is_none());
        assert!(store.get_identity().is_none());
        assert!(store.get_ai_config().is_none());
        assert!(store.get_display_name().is_none());
    }

    #[test]
    fn test_cloud_config_roundtrip() {
        let store = temp_store();
        let creds = CloudCredentials {
            api_url: "https://api.exemem.com".to_string(),
            api_key: "key123".to_string(),
            session_token: Some("tok456".to_string()),
            user_hash: Some("hash789".to_string()),
        };
        store.set_cloud_config(&creds).unwrap();
        assert!(!store.is_empty());
        assert!(store.is_cloud_enabled());

        let loaded = store.get_cloud_config().unwrap();
        assert_eq!(loaded.api_url, "https://api.exemem.com");
        assert_eq!(loaded.api_key, "key123");
        assert_eq!(loaded.session_token.as_deref(), Some("tok456"));
        assert_eq!(loaded.user_hash.as_deref(), Some("hash789"));
    }

    #[test]
    fn test_identity_roundtrip() {
        let store = temp_store();
        let id = NodeIdentity {
            private_key: "priv".to_string(),
            public_key: "pub".to_string(),
        };
        store.set_identity(&id).unwrap();

        let loaded = store.get_identity().unwrap();
        assert_eq!(loaded.private_key, "priv");
        assert_eq!(loaded.public_key, "pub");
    }

    #[test]
    fn test_ai_config_roundtrip() {
        let store = temp_store();
        let config = AiConfig {
            provider: "anthropic".to_string(),
            anthropic_key: Some("sk-ant-xxx".to_string()),
            anthropic_model: Some("claude-sonnet-4-20250514".to_string()),
            anthropic_base_url: None,
            ollama_model: None,
            ollama_url: None,
            ollama_vision_model: None,
        };
        store.set_ai_config(&config).unwrap();

        let loaded = store.get_ai_config().unwrap();
        assert_eq!(loaded.provider, "anthropic");
        assert_eq!(loaded.anthropic_key.as_deref(), Some("sk-ant-xxx"));
        assert!(loaded.ollama_model.is_none());
    }

    #[test]
    fn test_display_name_and_contact_hint() {
        let store = temp_store();
        store.set_display_name("Alice").unwrap();
        store
            .set_contact_hint("alice@example.com")
            .unwrap();
        assert_eq!(store.get_display_name().as_deref(), Some("Alice"));
        assert_eq!(
            store.get_contact_hint().as_deref(),
            Some("alice@example.com")
        );
    }
}
