use fold_db::db_operations::native_index::anonymity::FieldPrivacyClass;
use fold_db::storage::traits::KvStore;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const CONFIG_PREFIX: &str = "discovery:config:";

/// Per-schema discovery opt-in configuration. Persisted in Sled.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryOptIn {
    pub schema_name: String,
    pub category: String,
    pub include_preview: bool,
    pub preview_max_chars: usize,
    pub preview_excluded_fields: Vec<String>,
    #[serde(default)]
    pub field_privacy: HashMap<String, FieldPrivacyClass>,
    pub opted_in_at: chrono::DateTime<chrono::Utc>,
}

impl DiscoveryOptIn {
    pub fn new(schema_name: String, category: String) -> Self {
        Self {
            schema_name,
            category,
            include_preview: false,
            preview_max_chars: 100,
            preview_excluded_fields: Vec::new(),
            field_privacy: HashMap::new(),
            opted_in_at: chrono::Utc::now(),
        }
    }

    pub fn with_preview(mut self, max_chars: usize, excluded_fields: Vec<String>) -> Self {
        self.include_preview = true;
        self.preview_max_chars = max_chars;
        self.preview_excluded_fields = excluded_fields;
        self
    }

    pub fn with_field_privacy(mut self, field_privacy: HashMap<String, FieldPrivacyClass>) -> Self {
        self.field_privacy = field_privacy;
        self
    }
}

fn config_key(schema_name: &str) -> String {
    format!("{}{}", CONFIG_PREFIX, schema_name)
}

/// Save an opt-in config to the store.
pub async fn save_opt_in(store: &dyn KvStore, config: &DiscoveryOptIn) -> Result<(), String> {
    let key = config_key(&config.schema_name);
    let bytes = serde_json::to_vec(config)
        .map_err(|e| format!("Failed to serialize discovery config: {}", e))?;
    store
        .put(key.as_bytes(), bytes)
        .await
        .map_err(|e| format!("Failed to save discovery config: {}", e))
}

/// Remove an opt-in config from the store.
pub async fn remove_opt_in(store: &dyn KvStore, schema_name: &str) -> Result<(), String> {
    let key = config_key(schema_name);
    store
        .delete(key.as_bytes())
        .await
        .map_err(|e| format!("Failed to remove discovery config: {}", e))?;
    Ok(())
}

/// Load an opt-in config from the store.
pub async fn load_opt_in(
    store: &dyn KvStore,
    schema_name: &str,
) -> Result<Option<DiscoveryOptIn>, String> {
    let key = config_key(schema_name);
    let bytes = store
        .get(key.as_bytes())
        .await
        .map_err(|e| format!("Failed to load discovery config: {}", e))?;
    match bytes {
        Some(b) => {
            let config: DiscoveryOptIn = serde_json::from_slice(&b)
                .map_err(|e| format!("Failed to deserialize discovery config: {}", e))?;
            Ok(Some(config))
        }
        None => Ok(None),
    }
}

/// List all opted-in schemas.
pub async fn list_opt_ins(store: &dyn KvStore) -> Result<Vec<DiscoveryOptIn>, String> {
    let results = store
        .scan_prefix(CONFIG_PREFIX.as_bytes())
        .await
        .map_err(|e| format!("Failed to scan discovery configs: {}", e))?;

    let mut configs = Vec::with_capacity(results.len());
    for (_key, value) in results {
        match serde_json::from_slice::<DiscoveryOptIn>(&value) {
            Ok(config) => configs.push(config),
            Err(e) => log::warn!("Failed to deserialize discovery config: {}", e),
        }
    }
    Ok(configs)
}
