//! Configuration for the ingestion module

use fold_db::llm_registry::models;
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde::{Deserialize, Serialize};
use std::env;

/// Specifies the AI provider to use for ingestion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default, utoipa::ToSchema)]
pub enum AIProvider {
    #[default]
    Anthropic,
    Ollama,
}

/// Generation parameters for Ollama models.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OllamaGenerationParams {
    /// Context window size in tokens (2048..=250000).
    pub num_ctx: u32,
    /// Sampling temperature (0.0..=2.0).
    pub temperature: f32,
    /// Top-p (nucleus) sampling (0.0..=1.0).
    pub top_p: f32,
    /// Top-k sampling (0 = disabled).
    pub top_k: u32,
    /// Maximum tokens to generate (2048..=32000).
    pub num_predict: u32,
    /// Repeat penalty (0.0..=2.0).
    pub repeat_penalty: f32,
    /// Presence penalty (0.0..=2.0).
    pub presence_penalty: f32,
    /// Min-p sampling threshold (0.0..=1.0).
    pub min_p: f32,
}

impl Default for OllamaGenerationParams {
    fn default() -> Self {
        Self {
            num_ctx: models::OLLAMA_NUM_CTX,
            temperature: models::TEMPERATURE_CREATIVE,
            top_p: models::OLLAMA_TOP_P,
            top_k: models::OLLAMA_TOP_K,
            num_predict: models::OLLAMA_NUM_PREDICT,
            repeat_penalty: models::OLLAMA_REPEAT_PENALTY,
            presence_penalty: models::OLLAMA_PRESENCE_PENALTY,
            min_p: models::OLLAMA_MIN_P,
        }
    }
}

/// Configuration for the Ollama AI provider.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OllamaConfig {
    pub model: String,
    pub base_url: String,
    #[serde(default)]
    pub generation_params: OllamaGenerationParams,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            model: models::OLLAMA_DEFAULT.to_string(),
            base_url: models::OLLAMA_DEFAULT_URL.to_string(),
            generation_params: OllamaGenerationParams::default(),
        }
    }
}

impl OllamaConfig {
    pub fn validate(&self) -> Result<(), crate::ingestion::IngestionError> {
        if self.model.is_empty() {
            return Err(crate::ingestion::IngestionError::configuration_error(
                "Ollama model is required",
            ));
        }
        if self.base_url.is_empty() {
            return Err(crate::ingestion::IngestionError::configuration_error(
                "Ollama base URL is required",
            ));
        }
        Ok(())
    }
}

/// Configuration for the Anthropic AI provider.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AnthropicConfig {
    pub api_key: String,
    pub model: String,
    pub base_url: String,
}

impl Default for AnthropicConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: models::ANTHROPIC_SONNET.to_string(),
            base_url: models::ANTHROPIC_API_URL.to_string(),
        }
    }
}

impl AnthropicConfig {
    pub fn validate(&self) -> Result<(), crate::ingestion::IngestionError> {
        if self.api_key.is_empty() {
            return Err(crate::ingestion::IngestionError::configuration_error(
                "Anthropic API key is required",
            ));
        }
        if self.model.is_empty() {
            return Err(crate::ingestion::IngestionError::configuration_error(
                "Anthropic model is required",
            ));
        }
        if self.base_url.is_empty() {
            return Err(crate::ingestion::IngestionError::configuration_error(
                "Anthropic base URL is required",
            ));
        }
        Ok(())
    }
}

/// Configuration for the ingestion module.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct IngestionConfig {
    pub provider: AIProvider,
    pub ollama: OllamaConfig,
    #[serde(default)]
    pub anthropic: AnthropicConfig,
    pub enabled: bool,
    pub max_retries: u32,
    pub timeout_seconds: u64,
    pub auto_execute_mutations: bool,
}

impl Default for IngestionConfig {
    fn default() -> Self {
        Self {
            provider: AIProvider::default(),
            ollama: OllamaConfig::default(),
            anthropic: AnthropicConfig::default(),
            enabled: false,
            max_retries: 3,
            timeout_seconds: 300,
            auto_execute_mutations: true,
        }
    }
}

impl IngestionConfig {
    /// Return a copy with sensitive values (API keys) masked for safe display.
    pub fn redacted(&self) -> Self {
        let mut copy = self.clone();
        if !copy.anthropic.api_key.is_empty() {
            copy.anthropic.api_key = "***configured***".to_string();
        }
        copy
    }

    /// Load and fully validate config. Returns an error if any provider field
    /// is invalid (missing API key, empty model, etc.).
    pub fn from_env() -> Result<Self, crate::ingestion::IngestionError> {
        let config = Self::load()?;
        config.validate()?;
        Ok(config)
    }

    /// Load config from the saved file and environment variables.
    ///
    /// Precedence (highest to lowest):
    /// - `ANTHROPIC_API_KEY` env var (secrets never live in files)
    /// - Saved config file (UI choices)
    /// - Other env vars (only when no saved config)
    /// - Compiled-in defaults
    ///
    /// Returns an error if the config file exists but cannot be read or parsed.
    pub fn load() -> Result<Self, crate::ingestion::IngestionError> {
        let mut config = IngestionConfig::default();

        // Apply saved config (UI choices override defaults).
        // No FOLD_CONFIG_DIR or file not found → silent fallback to defaults.
        // File exists but unreadable/unparseable → fail fast.
        let has_saved = match Self::config_file_path() {
            None => {
                log_feature!(LogFeature::Ingestion, info, "FOLD_CONFIG_DIR not set; using env vars/defaults");
                false
            }
            Some(path) if !path.exists() => {
                log_feature!(LogFeature::Ingestion, info, "No saved ingestion config at {}; using env vars/defaults", path.display());
                false
            }
            Some(path) => {
                let saved = Self::load_from_file(&path)?;
                log_feature!(
                    LogFeature::Ingestion,
                    info,
                    "Loaded saved ingestion config: provider={:?}, model={}",
                    saved.provider,
                    match saved.provider {
                        AIProvider::Ollama => &saved.ollama.model,
                        AIProvider::Anthropic => &saved.anthropic.model,
                    }
                );
                config.provider = saved.provider;
                config.ollama = saved.ollama;
                config.anthropic = saved.anthropic;
                true
            }
        };

        // API keys: env vars always win — secrets shouldn't live in config files
        if let Ok(key) = env::var("ANTHROPIC_API_KEY") {
            config.anthropic.api_key = key;
        }

        // Provider selection and non-secret model settings only apply when
        // there's no saved config (saved config already has these)
        if !has_saved {
            if let Ok(p) = env::var("AI_PROVIDER") {
                config.provider = match p.to_lowercase().as_str() {
                    "ollama" => AIProvider::Ollama,
                    _ => AIProvider::Anthropic,
                };
            }
            if let Ok(v) = env::var("OLLAMA_MODEL") { config.ollama.model = v; }
            if let Ok(v) = env::var("OLLAMA_BASE_URL") { config.ollama.base_url = v; }
            if let Ok(v) = env::var("ANTHROPIC_MODEL") { config.anthropic.model = v; }
            if let Ok(v) = env::var("ANTHROPIC_BASE_URL") { config.anthropic.base_url = v; }
        }

        // Runtime settings: env vars override defaults; ingestion is enabled by default
        // when INGESTION_ENABLED is unset (matches original behavior).
        config.enabled = env_bool("INGESTION_ENABLED", true);
        config.max_retries = env_parse("INGESTION_MAX_RETRIES", config.max_retries);
        config.timeout_seconds = env_parse("INGESTION_TIMEOUT_SECONDS", config.timeout_seconds);
        config.auto_execute_mutations = env_bool("INGESTION_AUTO_EXECUTE", config.auto_execute_mutations);

        Ok(config)
    }

    /// Validate the configuration based on the selected provider.
    pub fn validate(&self) -> Result<(), crate::ingestion::IngestionError> {
        match self.provider {
            AIProvider::Ollama => self.ollama.validate(),
            AIProvider::Anthropic => self.anthropic.validate(),
        }
    }

    /// Check if ingestion is enabled and properly configured.
    pub fn is_ready(&self) -> bool {
        self.enabled && self.validate().is_ok()
    }

    /// Save provider/model settings to the config file.
    ///
    /// If the incoming api_key is empty or redacted, the existing saved key is
    /// preserved. If the file exists but cannot be read, returns an error rather
    /// than silently clearing the key.
    pub fn save_to_file(config: &SavedConfig) -> Result<(), Box<dyn std::error::Error>> {
        let config_path = Self::config_file_path().ok_or("FOLD_CONFIG_DIR is not set; cannot save ingestion config")?;

        let mut to_save = config.clone();
        // Preserve API keys if not explicitly set (redacted or empty)
        let existing = if config_path.exists() {
            Self::load_from_file(&config_path)
                .map_err(|e| format!("Failed to read existing config to preserve API key: {e}"))
                .ok()
        } else {
            None
        };
        if to_save.anthropic.api_key.is_empty() || to_save.anthropic.api_key == "***configured***" {
            to_save.anthropic.api_key = existing.as_ref().map(|e| e.anthropic.api_key.clone()).unwrap_or_default();
        }

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&to_save)?;
        std::fs::write(&config_path, content)?;
        Ok(())
    }

    fn config_file_path() -> Option<std::path::PathBuf> {
        env::var("FOLD_CONFIG_DIR")
            .ok()
            .map(|dir| std::path::Path::new(&dir).join("ingestion_config.json"))
    }

    fn load_from_file(
        path: &std::path::Path,
    ) -> Result<SavedConfig, crate::ingestion::IngestionError> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            crate::ingestion::IngestionError::configuration_error(format!(
                "Failed to read config file {}: {e}",
                path.display()
            ))
        })?;
        let mut saved: SavedConfig = serde_json::from_str(&content).map_err(|e| {
            crate::ingestion::IngestionError::configuration_error(format!(
                "Failed to parse config file {}: {e}",
                path.display()
            ))
        })?;
        // Strip redaction placeholder — treat as "no key configured"
        if saved.anthropic.api_key == "***configured***" {
            saved.anthropic.api_key = String::new();
        }
        Ok(saved)
    }

    /// Load config best-effort: returns a valid config or falls back to defaults.
    /// Errors are logged but never propagated — use `load()` directly if you need
    /// to handle failures.
    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_else(|e| {
            log::warn!("Failed to load ingestion config: {e}. Using defaults.");
            IngestionConfig::default()
        })
    }
}

/// Provider/model settings persisted to disk by the UI.
/// Runtime fields (enabled, retries, timeout) are controlled via env vars only.
#[derive(Debug, Clone, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub struct SavedConfig {
    pub provider: AIProvider,
    pub ollama: OllamaConfig,
    #[serde(default)]
    pub anthropic: AnthropicConfig,
}

// ---- env var helpers ----

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn env_parse<T: std::str::FromStr>(name: &str, default: T) -> T {
    env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = IngestionConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.provider, AIProvider::Anthropic);
        assert_eq!(config.anthropic.model, models::ANTHROPIC_SONNET);
        assert_eq!(config.anthropic.base_url, models::ANTHROPIC_API_URL);
        assert_eq!(config.ollama.model, models::OLLAMA_DEFAULT);
        assert_eq!(config.ollama.base_url, models::OLLAMA_DEFAULT_URL);
        assert_eq!(config.ollama.generation_params.num_ctx, models::OLLAMA_NUM_CTX);
        assert!((config.ollama.generation_params.temperature - models::TEMPERATURE_CREATIVE).abs() < f32::EPSILON);
        assert!((config.ollama.generation_params.top_p - models::OLLAMA_TOP_P).abs() < f32::EPSILON);
        assert_eq!(config.ollama.generation_params.top_k, models::OLLAMA_TOP_K);
        assert_eq!(config.ollama.generation_params.num_predict, models::OLLAMA_NUM_PREDICT);
        assert!((config.ollama.generation_params.repeat_penalty - models::OLLAMA_REPEAT_PENALTY).abs() < f32::EPSILON);
        assert!((config.ollama.generation_params.presence_penalty - models::OLLAMA_PRESENCE_PENALTY).abs() < f32::EPSILON);
        assert!((config.ollama.generation_params.min_p - models::OLLAMA_MIN_P).abs() < f32::EPSILON);
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.timeout_seconds, 300);
        assert!(config.auto_execute_mutations);
    }

    #[test]
    fn test_validation_anthropic_fails_without_api_key() {
        let config = IngestionConfig {
            provider: AIProvider::Anthropic,
            ..Default::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_validation_anthropic_succeeds_with_api_key() {
        let mut config = IngestionConfig {
            provider: AIProvider::Anthropic,
            ..Default::default()
        };
        config.anthropic.api_key = "test-key".to_string();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validation_ollama_succeeds_by_default() {
        let config = IngestionConfig {
            provider: AIProvider::Ollama,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_is_ready() {
        let mut config = IngestionConfig {
            provider: AIProvider::Anthropic,
            ..Default::default()
        };
        assert!(!config.is_ready());

        config.enabled = true;
        config.anthropic.api_key = "test-key".to_string();
        assert!(config.is_ready());

        config.provider = AIProvider::Ollama;
        assert!(config.is_ready());
    }
}
