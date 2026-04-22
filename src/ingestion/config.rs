//! Configuration for the ingestion module

use crate::ingestion::roles::Role;
use fold_db::llm_registry::models;
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;

/// Specifies the AI provider to use for ingestion.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default, utoipa::ToSchema)]
pub enum AIProvider {
    #[default]
    Anthropic,
    Ollama,
}

/// Specifies the backend used to convert images → markdown (vision / OCR).
/// Separate from `AIProvider` (text backend): vision historically only
/// supported Ollama via `file_to_markdown`, with Anthropic vision added for
/// environments without a local Ollama daemon (CI, fresh installs).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default, utoipa::ToSchema)]
pub enum VisionBackend {
    /// Local Ollama vision models (qwen3-vl, glm-ocr). Requires a reachable
    /// Ollama daemon at `ollama.base_url`.
    #[default]
    Ollama,
    /// Anthropic Claude vision. Requires `anthropic.api_key`. Used in CI and
    /// on machines that don't run Ollama locally.
    Anthropic,
}

/// Generation parameters for Ollama models.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
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
    /// Vision model for image captioning/classification (used by file_to_markdown).
    #[serde(default = "default_vision_model")]
    pub vision_model: String,
    /// OCR model for text extraction from scanned documents (used by file_to_markdown).
    #[serde(default = "default_ocr_model")]
    pub ocr_model: String,
    #[serde(default)]
    pub generation_params: OllamaGenerationParams,
}

fn default_vision_model() -> String {
    models::OLLAMA_VISION.to_string()
}

fn default_ocr_model() -> String {
    models::OLLAMA_OCR.to_string()
}

/// Pick a text model default based on available system RAM.
fn default_text_model() -> String {
    let ram_gb = system_ram_gb();
    if ram_gb >= 64 {
        models::OLLAMA_DEFAULT.to_string() // llama3.3 (70B)
    } else if ram_gb >= 32 {
        "llama3.1:8b".to_string()
    } else {
        "llama3.2:3b".to_string()
    }
}

/// Detect total system RAM in GB. Returns 16 if detection fails.
fn system_ram_gb() -> u64 {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("sysctl")
            .arg("-n")
            .arg("hw.memsize")
            .output()
            .ok()
            .and_then(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .trim()
                    .parse::<u64>()
                    .ok()
            })
            .map(|bytes| bytes / (1024 * 1024 * 1024))
            .unwrap_or(16)
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("MemTotal:"))
                    .and_then(|l| {
                        l.split_whitespace()
                            .nth(1)
                            .and_then(|kb| kb.parse::<u64>().ok())
                    })
            })
            .map(|kb| kb / (1024 * 1024))
            .unwrap_or(16)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        16
    }
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            model: default_text_model(),
            base_url: models::OLLAMA_DEFAULT_URL.to_string(),
            vision_model: default_vision_model(),
            ocr_model: default_ocr_model(),
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
            // Haiku 4.5 matches Sonnet 4 quality on the 8-case ingestion eval
            // (see autoresearch-ingestion/evaluate_anthropic.py — both score 0.900)
            // while running ~2.5x faster and ~67% cheaper. Query stays on Sonnet
            // via the explicit `query` override in `IngestionConfig::default()`.
            model: models::ANTHROPIC_HAIKU.to_string(),
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

/// Per-role provider/model/sampling override.
///
/// When a field is `None`, the role inherits from role defaults or the parent
/// config. When set, it wins field-by-field. Keyed by [`Role`] in
/// [`IngestionConfig::overrides`] and [`SavedConfig::overrides`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub struct UseCaseOverride {
    /// Provider for this role. `None` = inherit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<AIProvider>,
    /// Ollama model override. `None` = inherit from role default or
    /// parent's `ollama.model` / `vision_model` / `ocr_model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ollama_model: Option<String>,
    /// Anthropic model override. `None` = inherit from role default or
    /// parent's `anthropic.model`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anthropic_model: Option<String>,
    /// Sampling parameters override. `None` = inherit from role default merged
    /// with parent's hardware-scoped params (`num_ctx`, `num_predict`). When
    /// `Some`, replaces sampling wholesale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_params: Option<OllamaGenerationParams>,
}

impl UseCaseOverride {
    /// Returns true if any field is set (not fully inheriting).
    pub fn is_set(&self) -> bool {
        self.provider.is_some()
            || self.ollama_model.is_some()
            || self.anthropic_model.is_some()
            || self.generation_params.is_some()
    }

    /// Inverse of [`Self::is_set`]. Plumbing for
    /// `#[serde(skip_serializing_if = ...)]` which takes a function path.
    pub fn is_not_set(&self) -> bool {
        !self.is_set()
    }
}

/// Fully-resolved model info for a role. Produced by
/// [`IngestionConfig::resolve`]. Combines role defaults, global Ollama
/// hardware-scoped params, and per-role overrides into the concrete values
/// needed to construct an [`AiBackend`](crate::ingestion::ai::client::AiBackend).
///
/// `api_key`, `anthropic_base_url`, and `ollama_base_url` are provider-global
/// and are NOT overridable per role (by design — prevents config sprawl). See
/// TODOS if per-role base_url is ever needed.
#[derive(Debug, Clone)]
pub struct ResolvedModel {
    /// The role this was resolved for. Echo-back for logging and metrics.
    pub role: Role,
    /// Resolved provider.
    pub provider: AIProvider,
    /// Resolved model ID (e.g. `claude-haiku-4-5-20251001` or `qwen3-vl:2b`).
    pub model: String,
    /// Global Anthropic API key. Empty string when `provider = Ollama`.
    pub api_key: String,
    /// Global Anthropic base URL.
    pub anthropic_base_url: String,
    /// Global Ollama base URL.
    pub ollama_base_url: String,
    /// Sampling params. Only consulted when `provider = Ollama`.
    pub generation_params: OllamaGenerationParams,
    /// Request timeout (seconds).
    pub timeout_seconds: u64,
    /// HTTP retry budget.
    pub max_retries: u32,
}

/// Configuration for the ingestion module.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct IngestionConfig {
    /// Primary AI provider (used for ingestion schema analysis).
    pub provider: AIProvider,
    pub ollama: OllamaConfig,
    #[serde(default)]
    pub anthropic: AnthropicConfig,
    pub enabled: bool,
    pub max_retries: u32,
    pub timeout_seconds: u64,
    pub auto_execute_mutations: bool,

    /// Backend used for image → markdown conversion. Defaults to `Ollama`
    /// for backwards compatibility with existing installs; set to
    /// `Anthropic` (or `INGESTION_VISION_BACKEND=anthropic`) to route
    /// vision through Claude instead.
    #[serde(default)]
    pub vision_backend: VisionBackend,

    /// Per-role provider / model / sampling overrides. Keyed by [`Role`].
    /// When a role has no entry, [`IngestionConfig::resolve`] falls through
    /// to role defaults + global config.
    #[serde(default)]
    pub overrides: HashMap<Role, UseCaseOverride>,
}

impl Default for IngestionConfig {
    fn default() -> Self {
        // Ingestion defaults to Haiku (via AnthropicConfig::default), but
        // natural-language query / agent reasoning is more demanding, so the
        // Role::QueryChat path now gets Sonnet via Role::default_anthropic_model().
        // No explicit override needed — it falls out of role defaults. Kept
        // empty so "no overrides configured" is the honest default state.
        Self {
            provider: AIProvider::default(),
            ollama: OllamaConfig::default(),
            anthropic: AnthropicConfig::default(),
            enabled: false,
            max_retries: 3,
            timeout_seconds: 300,
            auto_execute_mutations: true,
            vision_backend: VisionBackend::default(),
            overrides: HashMap::new(),
        }
    }
}

impl IngestionConfig {
    /// Build an effective config for the LLM query use case.
    ///
    /// Kept for backwards compatibility with callers still reading
    /// `IngestionConfig` fields directly. New code should call
    /// [`IngestionConfig::resolve`] instead and read `ResolvedModel` fields.
    /// PR 3 migrates the last remaining caller (`llm_query::service`) and
    /// removes this method.
    pub fn query_config(&self) -> Self {
        let resolved = self.resolve(Role::QueryChat);
        let mut config = self.clone();
        config.provider = resolved.provider.clone();
        match resolved.provider {
            AIProvider::Ollama => {
                config.ollama.model = resolved.model;
                config.ollama.generation_params = resolved.generation_params;
            }
            AIProvider::Anthropic => {
                config.anthropic.model = resolved.model;
            }
        }
        config
    }

    /// Resolve a role → concrete [`ResolvedModel`]. Pure data; does not build
    /// a backend. Used by the UI/stats endpoints and by [`Self::build_backend`]
    /// (landing in PR 2).
    ///
    /// Precedence, field-by-field:
    /// 1. Per-role override (`self.overrides.get(role)`)
    /// 2. Role default (`Role::default_*`)
    /// 3. Global config (`self.provider`, `self.ollama.*`, `self.anthropic.*`)
    pub fn resolve(&self, role: Role) -> ResolvedModel {
        let override_opt = self.overrides.get(&role);

        // 1. Provider: override > role-aware default (vision_backend, OCR=Ollama) > global
        let provider = override_opt
            .and_then(|o| o.provider.clone())
            .unwrap_or_else(|| match role {
                Role::Vision => match self.vision_backend {
                    VisionBackend::Anthropic => AIProvider::Anthropic,
                    VisionBackend::Ollama => AIProvider::Ollama,
                },
                // OCR has no Anthropic path today — always Ollama.
                Role::Ocr => AIProvider::Ollama,
                _ => self.provider.clone(),
            });

        // 2. Model: override > role default > global ollama.{model,vision_model,ocr_model}
        //    or global anthropic.model.
        let model = match provider {
            AIProvider::Anthropic => override_opt
                .and_then(|o| o.anthropic_model.clone())
                .unwrap_or_else(|| {
                    // Only fall through to the global `anthropic.model` when it
                    // has been explicitly customised; otherwise prefer the
                    // role-aware default so QueryChat keeps Sonnet.
                    let role_default = role.default_anthropic_model();
                    if self.anthropic.model == models::ANTHROPIC_HAIKU {
                        role_default.to_string()
                    } else {
                        self.anthropic.model.clone()
                    }
                }),
            AIProvider::Ollama => override_opt
                .and_then(|o| o.ollama_model.clone())
                .or_else(|| role.default_ollama_model().map(String::from))
                .unwrap_or_else(|| match role {
                    Role::Vision => self.ollama.vision_model.clone(),
                    Role::Ocr => self.ollama.ocr_model.clone(),
                    _ => self.ollama.model.clone(),
                }),
        };

        // 3. Sampling: if override has explicit generation_params, use wholesale.
        //    Otherwise start from role defaults, overlay hardware-scoped fields
        //    from the global config (num_ctx and num_predict respect the user's
        //    RAM / Ollama setup).
        let generation_params =
            if let Some(gp) = override_opt.and_then(|o| o.generation_params.clone()) {
                gp
            } else {
                let mut gp = role.default_generation_params();
                gp.num_ctx = self.ollama.generation_params.num_ctx;
                gp.num_predict = self.ollama.generation_params.num_predict;
                gp
            };

        ResolvedModel {
            role,
            provider,
            model,
            api_key: self.anthropic.api_key.clone(),
            anthropic_base_url: self.anthropic.base_url.clone(),
            ollama_base_url: self.ollama.base_url.clone(),
            generation_params,
            timeout_seconds: self.timeout_seconds,
            max_retries: self.max_retries,
        }
    }

    /// Whether a role has a user-set override. Drives the `[*]` badge in the
    /// UI's Active Models table.
    pub fn is_override_active(&self, role: Role) -> bool {
        self.overrides
            .get(&role)
            .map(|o| o.is_set())
            .unwrap_or(false)
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
                log_feature!(
                    LogFeature::Ingestion,
                    info,
                    "FOLD_CONFIG_DIR not set; using env vars/defaults"
                );
                false
            }
            Some(path) if !path.exists() => {
                log_feature!(
                    LogFeature::Ingestion,
                    info,
                    "No saved ingestion config at {}; using env vars/defaults",
                    path.display()
                );
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
                config.vision_backend = saved.vision_backend;
                config.overrides = saved.overrides;
                true
            }
        };

        // API keys: env vars always win — secrets shouldn't live in config files
        if let Ok(key) = env::var("ANTHROPIC_API_KEY") {
            config.anthropic.api_key = key;
        }

        // Vision backend: env var always wins even over saved config. This lets
        // CI / ephemeral environments route images through Anthropic without
        // mutating the user's on-disk ingestion_config.json.
        if let Ok(v) = env::var("INGESTION_VISION_BACKEND") {
            config.vision_backend = match v.to_lowercase().as_str() {
                "anthropic" => VisionBackend::Anthropic,
                "ollama" => VisionBackend::Ollama,
                other => {
                    log_feature!(
                        LogFeature::Ingestion,
                        warn,
                        "Unrecognized INGESTION_VISION_BACKEND={other:?}; keeping previous value"
                    );
                    config.vision_backend
                }
            };
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
            if let Ok(v) = env::var("OLLAMA_MODEL") {
                config.ollama.model = v;
            }
            if let Ok(v) = env::var("OLLAMA_BASE_URL") {
                config.ollama.base_url = v;
            }
            if let Ok(v) = env::var("OLLAMA_VISION_MODEL") {
                config.ollama.vision_model = v;
            }
            if let Ok(v) = env::var("OLLAMA_OCR_MODEL") {
                config.ollama.ocr_model = v;
            }
            if let Ok(v) = env::var("ANTHROPIC_MODEL") {
                config.anthropic.model = v;
            }
            if let Ok(v) = env::var("ANTHROPIC_BASE_URL") {
                config.anthropic.base_url = v;
            }
        }

        // Runtime settings: env vars override defaults; ingestion is enabled by default
        // when INGESTION_ENABLED is unset (matches original behavior).
        config.enabled = env_bool("INGESTION_ENABLED", true);
        config.max_retries = env_parse("INGESTION_MAX_RETRIES", config.max_retries);
        config.timeout_seconds = env_parse("INGESTION_TIMEOUT_SECONDS", config.timeout_seconds);
        config.auto_execute_mutations =
            env_bool("INGESTION_AUTO_EXECUTE", config.auto_execute_mutations);

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
        let config_path = Self::config_file_path()
            .ok_or("FOLD_CONFIG_DIR is not set; cannot save ingestion config")?;

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
            to_save.anthropic.api_key = existing
                .as_ref()
                .map(|e| e.anthropic.api_key.clone())
                .unwrap_or_default();
        }

        // Dual-write: project overrides[QueryChat] into the legacy `query`
        // field so an older binary (feature flag off) can still read the user's
        // QueryChat override during a rollback.
        to_save.prepare_for_save();

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&to_save)?;
        std::fs::write(&config_path, content)?;
        Ok(())
    }

    // AI config is per-device — saved to ingestion_config.json only.
    // Not synced to Sled because a laptop might run Ollama locally
    // while a phone uses Anthropic's API.

    fn config_file_path() -> Option<std::path::PathBuf> {
        env::var("FOLD_CONFIG_DIR")
            .ok()
            .map(|dir| std::path::Path::new(&dir).join("ingestion_config.json"))
            .or_else(|| {
                crate::utils::paths::folddb_home()
                    .ok()
                    .map(|h| h.join("config").join("ingestion_config.json"))
            })
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
        // Forward-migrate legacy `query` field into `overrides`.
        saved.normalize();
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
///
/// # Schema migration
///
/// Older configs have a top-level `query: UseCaseOverride` field. Newer
/// configs use `overrides: HashMap<Role, UseCaseOverride>`. Both fields are
/// read on load — [`Self::normalize`] seeds `overrides[QueryChat]` from the
/// legacy `query` field when present. During the rollout window, saves emit
/// BOTH fields (dual-write) so an older binary can still read the user's
/// QueryChat override if the feature flag gets rolled back. The legacy
/// `query` field will be dropped two releases after PR 4 ships.
#[derive(Debug, Clone, Serialize, Deserialize, Default, utoipa::ToSchema)]
pub struct SavedConfig {
    pub provider: AIProvider,
    pub ollama: OllamaConfig,
    #[serde(default)]
    pub anthropic: AnthropicConfig,
    /// Backend for image → markdown (Ollama default; Anthropic when set).
    /// Persisted so the UI can surface the chosen backend.
    #[serde(default)]
    pub vision_backend: VisionBackend,
    /// Legacy — pre-overrides schema. Read on load, migrated into `overrides`.
    /// Also written on save (dual-write) until two releases after PR 4 ships.
    #[serde(default, skip_serializing_if = "UseCaseOverride::is_not_set")]
    pub query: UseCaseOverride,
    /// Per-role overrides. The new canonical shape (2026-04-22+).
    #[serde(default)]
    pub overrides: HashMap<Role, UseCaseOverride>,
}

impl SavedConfig {
    /// Forward-migrate a freshly-deserialized config. If the legacy `query`
    /// field is populated and `overrides.QueryChat` is absent, move `query`
    /// into `overrides[QueryChat]`. Idempotent: no-op once the new shape is
    /// present.
    fn normalize(&mut self) {
        if self.query.is_set() && !self.overrides.contains_key(&Role::QueryChat) {
            let legacy = std::mem::take(&mut self.query);
            self.overrides.insert(Role::QueryChat, legacy);
        }
    }

    /// Project `overrides[QueryChat]` down to the legacy `query` field for
    /// dual-write. Called by [`IngestionConfig::save_to_file`] just before
    /// serialization. An older binary (feature flag OFF) reading the saved
    /// file will see the legacy field and preserve the user's QueryChat
    /// override during a rollback.
    fn prepare_for_save(&mut self) {
        self.query = self
            .overrides
            .get(&Role::QueryChat)
            .cloned()
            .unwrap_or_default();
    }
}

// ---- env var helpers ----

fn env_bool(name: &str, default: bool) -> bool {
    env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

fn env_parse<T: std::str::FromStr>(name: &str, default: T) -> T {
    env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = IngestionConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.provider, AIProvider::Anthropic);
        // Ingestion default is Haiku — see AnthropicConfig::default for rationale.
        assert_eq!(config.anthropic.model, models::ANTHROPIC_HAIKU);
        assert_eq!(config.anthropic.base_url, models::ANTHROPIC_API_URL);
        // No explicit overrides by default — role defaults do the work.
        assert!(config.overrides.is_empty());
        // The resolved QueryChat role still picks Sonnet via role defaults.
        assert_eq!(
            config.resolve(Role::QueryChat).model,
            models::ANTHROPIC_SONNET
        );
        // Text model depends on system RAM — just verify it's non-empty
        assert!(!config.ollama.model.is_empty());
        assert_eq!(config.ollama.vision_model, models::OLLAMA_VISION);
        assert_eq!(config.ollama.ocr_model, models::OLLAMA_OCR);
        assert_eq!(config.ollama.base_url, models::OLLAMA_DEFAULT_URL);
        assert_eq!(
            config.ollama.generation_params.num_ctx,
            models::OLLAMA_NUM_CTX
        );
        assert!(
            (config.ollama.generation_params.temperature - models::TEMPERATURE_CREATIVE).abs()
                < f32::EPSILON
        );
        assert!(
            (config.ollama.generation_params.top_p - models::OLLAMA_TOP_P).abs() < f32::EPSILON
        );
        assert_eq!(config.ollama.generation_params.top_k, models::OLLAMA_TOP_K);
        assert_eq!(
            config.ollama.generation_params.num_predict,
            models::OLLAMA_NUM_PREDICT
        );
        assert!(
            (config.ollama.generation_params.repeat_penalty - models::OLLAMA_REPEAT_PENALTY).abs()
                < f32::EPSILON
        );
        assert!(
            (config.ollama.generation_params.presence_penalty - models::OLLAMA_PRESENCE_PENALTY)
                .abs()
                < f32::EPSILON
        );
        assert!(
            (config.ollama.generation_params.min_p - models::OLLAMA_MIN_P).abs() < f32::EPSILON
        );
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.timeout_seconds, 300);
        assert!(config.auto_execute_mutations);
        assert_eq!(config.vision_backend, VisionBackend::Ollama);
    }

    #[test]
    fn vision_backend_defaults_to_ollama() {
        assert_eq!(VisionBackend::default(), VisionBackend::Ollama);
    }

    #[test]
    fn saved_config_without_vision_backend_deserializes_to_ollama_default() {
        // Existing ingestion_config.json files written before this change won't
        // have the `vision_backend` field. They must still load cleanly and
        // preserve Ollama behavior.
        let json = r#"{
            "provider": "Anthropic",
            "ollama": {
                "model": "llama3.3",
                "base_url": "http://localhost:11434",
                "vision_model": "qwen3-vl:2b",
                "ocr_model": "glm-ocr:latest"
            }
        }"#;
        let saved: SavedConfig = serde_json::from_str(json).expect("should parse legacy config");
        assert_eq!(saved.vision_backend, VisionBackend::Ollama);
    }

    #[test]
    fn saved_config_round_trip_preserves_vision_backend() {
        let original = SavedConfig {
            vision_backend: VisionBackend::Anthropic,
            ..Default::default()
        };
        let json = serde_json::to_string(&original).unwrap();
        let round_tripped: SavedConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped.vision_backend, VisionBackend::Anthropic);
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

    // ---- resolve() — basic role resolution, no overrides ----

    #[test]
    fn resolve_ingestion_text_defaults_to_anthropic_haiku() {
        let config = IngestionConfig::default();
        let r = config.resolve(Role::IngestionText);
        assert_eq!(r.role, Role::IngestionText);
        assert_eq!(r.provider, AIProvider::Anthropic);
        assert_eq!(r.model, models::ANTHROPIC_HAIKU);
    }

    #[test]
    fn resolve_query_chat_defaults_to_anthropic_sonnet() {
        // Regression protection: the legacy `query` override used to pin
        // Sonnet explicitly. Now Sonnet comes from `Role::default_anthropic_model`.
        let config = IngestionConfig::default();
        let r = config.resolve(Role::QueryChat);
        assert_eq!(r.provider, AIProvider::Anthropic);
        assert_eq!(r.model, models::ANTHROPIC_SONNET);
    }

    #[test]
    fn resolve_smart_folder_defaults_to_haiku_and_zero_temperature() {
        let config = IngestionConfig::default();
        let r = config.resolve(Role::SmartFolder);
        assert_eq!(r.provider, AIProvider::Anthropic);
        assert_eq!(r.model, models::ANTHROPIC_HAIKU);
        // Classifier: deterministic.
        assert!(
            (r.generation_params.temperature - models::TEMPERATURE_DETERMINISTIC).abs()
                < f32::EPSILON
        );
    }

    #[test]
    fn resolve_vision_picks_ollama_when_vision_backend_is_ollama() {
        let config = IngestionConfig {
            vision_backend: VisionBackend::Ollama,
            ..Default::default()
        };
        let r = config.resolve(Role::Vision);
        assert_eq!(r.provider, AIProvider::Ollama);
        assert_eq!(r.model, config.ollama.vision_model);
    }

    #[test]
    fn resolve_vision_picks_anthropic_when_vision_backend_is_anthropic() {
        let config = IngestionConfig {
            vision_backend: VisionBackend::Anthropic,
            ..Default::default()
        };
        let r = config.resolve(Role::Vision);
        assert_eq!(r.provider, AIProvider::Anthropic);
        assert_eq!(r.model, models::ANTHROPIC_HAIKU);
    }

    #[test]
    fn resolve_ocr_always_uses_ollama_regardless_of_global_provider() {
        let config = IngestionConfig {
            provider: AIProvider::Anthropic,
            ..Default::default()
        };
        let r = config.resolve(Role::Ocr);
        assert_eq!(r.provider, AIProvider::Ollama);
        assert_eq!(r.model, config.ollama.ocr_model);
    }

    #[test]
    fn resolve_propagates_global_hardware_scoped_generation_params() {
        // num_ctx and num_predict are hardware-scoped — the role default must
        // respect the user's system setting rather than hardcoding a value.
        let mut config = IngestionConfig::default();
        config.ollama.generation_params.num_ctx = 99_999;
        config.ollama.generation_params.num_predict = 88_888;
        let r = config.resolve(Role::SmartFolder);
        assert_eq!(r.generation_params.num_ctx, 99_999);
        assert_eq!(r.generation_params.num_predict, 88_888);
    }

    // ---- resolve() with overrides ----

    #[test]
    fn resolve_override_provider_flips_to_ollama() {
        let mut config = IngestionConfig::default();
        config.overrides.insert(
            Role::IngestionText,
            UseCaseOverride {
                provider: Some(AIProvider::Ollama),
                ..Default::default()
            },
        );
        let r = config.resolve(Role::IngestionText);
        assert_eq!(r.provider, AIProvider::Ollama);
        assert_eq!(r.model, config.ollama.model);
    }

    #[test]
    fn resolve_override_anthropic_model_wins() {
        let mut config = IngestionConfig::default();
        config.overrides.insert(
            Role::QueryChat,
            UseCaseOverride {
                anthropic_model: Some("claude-opus-custom".to_string()),
                ..Default::default()
            },
        );
        let r = config.resolve(Role::QueryChat);
        assert_eq!(r.provider, AIProvider::Anthropic);
        assert_eq!(r.model, "claude-opus-custom");
    }

    #[test]
    fn resolve_override_ollama_model_wins_when_provider_is_ollama() {
        let mut config = IngestionConfig {
            provider: AIProvider::Ollama,
            ..Default::default()
        };
        config.overrides.insert(
            Role::IngestionText,
            UseCaseOverride {
                provider: Some(AIProvider::Ollama),
                ollama_model: Some("codellama:70b".to_string()),
                ..Default::default()
            },
        );
        let r = config.resolve(Role::IngestionText);
        assert_eq!(r.provider, AIProvider::Ollama);
        assert_eq!(r.model, "codellama:70b");
    }

    #[test]
    fn resolve_override_generation_params_replaces_wholesale() {
        let mut config = IngestionConfig::default();
        let custom_gp = OllamaGenerationParams {
            temperature: 1.9,
            num_ctx: 123,
            num_predict: 456,
            top_p: 0.5,
            top_k: 42,
            repeat_penalty: 1.5,
            presence_penalty: 0.3,
            min_p: 0.1,
        };
        config.overrides.insert(
            Role::SmartFolder,
            UseCaseOverride {
                generation_params: Some(custom_gp.clone()),
                ..Default::default()
            },
        );
        let r = config.resolve(Role::SmartFolder);
        assert!((r.generation_params.temperature - 1.9).abs() < f32::EPSILON);
        assert_eq!(r.generation_params.num_ctx, 123);
        assert_eq!(r.generation_params.num_predict, 456);
        assert_eq!(r.generation_params.top_k, 42);
    }

    // ---- is_override_active() ----

    #[test]
    fn is_override_active_returns_false_when_no_entry() {
        let config = IngestionConfig::default();
        for role in Role::ALL {
            assert!(!config.is_override_active(*role));
        }
    }

    #[test]
    fn is_override_active_returns_false_when_entry_has_all_none_fields() {
        let mut config = IngestionConfig::default();
        config
            .overrides
            .insert(Role::IngestionText, UseCaseOverride::default());
        assert!(!config.is_override_active(Role::IngestionText));
    }

    #[test]
    fn is_override_active_returns_true_when_entry_has_any_field_set() {
        let mut config = IngestionConfig::default();
        config.overrides.insert(
            Role::Vision,
            UseCaseOverride {
                provider: Some(AIProvider::Anthropic),
                ..Default::default()
            },
        );
        assert!(config.is_override_active(Role::Vision));
        assert!(!config.is_override_active(Role::Ocr));
    }

    // ---- Serde migration — legacy `query` → overrides[QueryChat] ----

    /// CRITICAL: every user's on-disk config today has the legacy `query`
    /// field. Failing to migrate it into `overrides[QueryChat]` silently
    /// loses their Sonnet override. This test protects that contract.
    #[test]
    fn legacy_query_field_migrates_into_overrides_querychat() {
        let legacy_json = r#"{
            "provider": "Anthropic",
            "ollama": {
                "model": "llama3.3",
                "base_url": "http://localhost:11434",
                "vision_model": "qwen3-vl:2b",
                "ocr_model": "glm-ocr:latest"
            },
            "anthropic": {
                "api_key": "",
                "model": "claude-haiku-4-5-20251001",
                "base_url": "https://api.anthropic.com"
            },
            "query": {
                "anthropic_model": "claude-sonnet-4-20250514"
            }
        }"#;
        let mut saved: SavedConfig = serde_json::from_str(legacy_json).unwrap();
        saved.normalize();
        let migrated = saved
            .overrides
            .get(&Role::QueryChat)
            .expect("legacy `query` should migrate into overrides[QueryChat]");
        assert_eq!(
            migrated.anthropic_model.as_deref(),
            Some("claude-sonnet-4-20250514")
        );
        // Legacy field cleared after migration.
        assert!(!saved.query.is_set());
    }

    #[test]
    fn new_overrides_format_deserializes_directly() {
        let new_json = r#"{
            "provider": "Anthropic",
            "ollama": {
                "model": "llama3.3",
                "base_url": "http://localhost:11434",
                "vision_model": "qwen3-vl:2b",
                "ocr_model": "glm-ocr:latest"
            },
            "anthropic": {
                "api_key": "",
                "model": "claude-haiku-4-5-20251001",
                "base_url": "https://api.anthropic.com"
            },
            "overrides": {
                "Vision": { "provider": "Anthropic" }
            }
        }"#;
        let mut saved: SavedConfig = serde_json::from_str(new_json).unwrap();
        saved.normalize();
        let vision_override = saved.overrides.get(&Role::Vision).unwrap();
        assert_eq!(vision_override.provider, Some(AIProvider::Anthropic));
    }

    #[test]
    fn both_legacy_and_new_fields_present_overrides_wins() {
        let both_json = r#"{
            "provider": "Anthropic",
            "ollama": {
                "model": "llama3.3",
                "base_url": "http://localhost:11434",
                "vision_model": "qwen3-vl:2b",
                "ocr_model": "glm-ocr:latest"
            },
            "anthropic": {
                "api_key": "",
                "model": "claude-haiku-4-5-20251001",
                "base_url": "https://api.anthropic.com"
            },
            "query": { "anthropic_model": "legacy-sonnet" },
            "overrides": {
                "QueryChat": { "anthropic_model": "new-sonnet" }
            }
        }"#;
        let mut saved: SavedConfig = serde_json::from_str(both_json).unwrap();
        saved.normalize();
        assert_eq!(
            saved
                .overrides
                .get(&Role::QueryChat)
                .unwrap()
                .anthropic_model
                .as_deref(),
            Some("new-sonnet"),
            "when both present, `overrides` wins; legacy is dropped"
        );
    }

    #[test]
    fn normalize_is_idempotent() {
        let legacy_json = r#"{
            "provider": "Anthropic",
            "ollama": { "model": "m", "base_url": "u", "vision_model": "v", "ocr_model": "o" },
            "query": { "anthropic_model": "sonnet" }
        }"#;
        let mut saved: SavedConfig = serde_json::from_str(legacy_json).unwrap();
        saved.normalize();
        let first = saved.overrides.clone();
        saved.normalize();
        assert_eq!(saved.overrides, first);
    }

    #[test]
    fn prepare_for_save_projects_overrides_querychat_into_legacy_query() {
        let mut saved = SavedConfig::default();
        saved.overrides.insert(
            Role::QueryChat,
            UseCaseOverride {
                anthropic_model: Some("sonnet-via-overrides".to_string()),
                ..Default::default()
            },
        );
        saved.prepare_for_save();
        assert_eq!(
            saved.query.anthropic_model.as_deref(),
            Some("sonnet-via-overrides")
        );
    }

    #[test]
    fn prepare_for_save_clears_legacy_query_when_no_querychat_override() {
        let mut saved = SavedConfig::default();
        saved.query.anthropic_model = Some("stale".to_string());
        saved.prepare_for_save();
        assert!(!saved.query.is_set());
    }

    /// CRITICAL: full roundtrip — legacy JSON on disk loads, migrates, saves
    /// in the new shape (with dual-write), reloads cleanly.
    #[test]
    fn legacy_roundtrip_preserves_querychat_override_end_to_end() {
        let legacy_json = r#"{
            "provider": "Anthropic",
            "ollama": {
                "model": "llama3.3",
                "base_url": "http://localhost:11434",
                "vision_model": "qwen3-vl:2b",
                "ocr_model": "glm-ocr:latest"
            },
            "anthropic": {
                "api_key": "",
                "model": "claude-haiku-4-5-20251001",
                "base_url": "https://api.anthropic.com"
            },
            "query": { "anthropic_model": "claude-sonnet-4-20250514" }
        }"#;

        // 1. Load legacy.
        let mut saved: SavedConfig = serde_json::from_str(legacy_json).unwrap();
        saved.normalize();

        // 2. Prepare for save (dual-write).
        saved.prepare_for_save();

        // 3. Re-serialize.
        let reserialized = serde_json::to_string(&saved).unwrap();

        // 4. Reload.
        let mut reloaded: SavedConfig = serde_json::from_str(&reserialized).unwrap();
        reloaded.normalize();

        // The QueryChat override survived end-to-end.
        assert_eq!(
            reloaded
                .overrides
                .get(&Role::QueryChat)
                .unwrap()
                .anthropic_model
                .as_deref(),
            Some("claude-sonnet-4-20250514")
        );

        // And the legacy `query` field is also populated for old-binary compat.
        assert_eq!(
            reloaded.query.anthropic_model.as_deref(),
            Some("claude-sonnet-4-20250514"),
            "dual-write preserves backwards compat during the rollout window"
        );
    }

    #[test]
    fn config_without_any_overrides_round_trips_cleanly() {
        let saved = SavedConfig::default();
        let json = serde_json::to_string(&saved).unwrap();
        let back: SavedConfig = serde_json::from_str(&json).unwrap();
        assert!(back.overrides.is_empty());
        assert!(!back.query.is_set());
    }
}
