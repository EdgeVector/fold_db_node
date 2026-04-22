//! AI Role registry — named use cases for LLM dispatch inside fold_db_node.
//!
//! Every callsite that fires an LLM call is one of these [`Role`]s. The Role
//! declares its semantic purpose, its default provider preferences, and its
//! default sampling parameters. `IngestionConfig::resolve(role)` combines a
//! Role with the user's global config and per-role overrides to produce a
//! [`ResolvedModel`](crate::ingestion::config::ResolvedModel) ready for
//! backend construction.
//!
//! Adding a new LLM-using feature is a one-line addition here plus a
//! `resolve(Role::NewVariant)` at the callsite. The HashMap-based override
//! map on `IngestionConfig` accepts the new key automatically.
//!
//! Kept in `fold_db_node` (not `fold_db::llm_registry`) because only the node
//! dispatches to LLM providers. `llm_registry` stays focused on model ID
//! constants that are shared across fold_db, fold_db_node, and schema_service.

use crate::ingestion::config::OllamaGenerationParams;
use fold_db::llm_registry::models;
use serde::{Deserialize, Serialize};

/// Named AI use cases inside fold_db_node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
pub enum Role {
    /// Schema analysis + extraction during ingestion of user data. The primary
    /// ingestion LLM call — pulls structured fields out of arbitrary JSON.
    IngestionText,
    /// Image → markdown conversion. Routed through Ollama vision models or
    /// Anthropic Claude vision depending on `vision_backend`.
    Vision,
    /// Text extraction from scanned documents and image-based PDF pages.
    /// Always uses Ollama today (`glm-ocr`); Anthropic has no dedicated OCR
    /// endpoint.
    Ocr,
    /// Smart-folder classification: given a folder of files, decide which
    /// schema each fits. Low-temperature classifier; determinism wins.
    SmartFolder,
    /// Discovery / interests inference: classify a user action into one of
    /// the predefined interest categories for peer discovery.
    DiscoveryInterests,
    /// Mutation agent: decides when and how to execute mutations proposed by
    /// ingestion. Runs alongside `IngestionText` but is a distinct concern.
    MutationAgent,
    /// Natural-language query and chat over the user's data. Reasoning-heavy;
    /// defaults to a higher-capability model (Sonnet by default).
    QueryChat,
}

impl Role {
    /// Canonical ordering. Used by the UI table, the `/api/ingestion/config/roles`
    /// endpoint response, and the stats endpoint. Declaration order here is
    /// load-bearing for all of them.
    pub const ALL: &'static [Role] = &[
        Role::IngestionText,
        Role::SmartFolder,
        Role::DiscoveryInterests,
        Role::MutationAgent,
        Role::QueryChat,
        Role::Vision,
        Role::Ocr,
    ];

    /// Human-readable name shown in the UI and API responses.
    pub fn display_name(&self) -> &'static str {
        match self {
            Role::IngestionText => "Ingestion Text",
            Role::Vision => "Vision",
            Role::Ocr => "OCR",
            Role::SmartFolder => "Smart Folder",
            Role::DiscoveryInterests => "Discovery Interests",
            Role::MutationAgent => "Mutation Agent",
            Role::QueryChat => "Query & Chat",
        }
    }

    /// One-line description of what the role does. Shown in the UI override row
    /// and in doc comments on the OpenAPI schema.
    pub fn doc(&self) -> &'static str {
        match self {
            Role::IngestionText => "Schema analysis + structured extraction from ingested content.",
            Role::Vision => "Image → markdown conversion (captioning, scene understanding).",
            Role::Ocr => "Text extraction from scanned documents and image PDFs.",
            Role::SmartFolder => "Classify files into schemas for smart-folder batch ingestion.",
            Role::DiscoveryInterests => {
                "Classify user activity into interest categories for peer discovery."
            }
            Role::MutationAgent => {
                "Decide when and how to execute mutations proposed by ingestion."
            }
            Role::QueryChat => "Natural-language search and chat over the user's data.",
        }
    }

    /// Whether a role can be tested with an arbitrary text prompt. Vision and
    /// Ocr return `false` — they require image bytes, not a text prompt.
    pub fn is_text_capable(&self) -> bool {
        !matches!(self, Role::Vision | Role::Ocr)
    }

    /// Default Anthropic model for this role. Resolution precedence: user
    /// override > this default. References `fold_db::llm_registry::models`
    /// constants so model upgrades happen in one place.
    pub fn default_anthropic_model(&self) -> &'static str {
        match self {
            // Reasoning-heavy roles want Sonnet.
            Role::QueryChat => models::ANTHROPIC_SONNET,
            // Fast classification / extraction roles use Haiku.
            Role::IngestionText
            | Role::Vision
            | Role::SmartFolder
            | Role::DiscoveryInterests
            | Role::MutationAgent => models::ANTHROPIC_HAIKU,
            // Ocr has no Anthropic path today; fall through to Haiku if the
            // user somehow selects Anthropic for OCR (tested in config.rs).
            Role::Ocr => models::ANTHROPIC_HAIKU,
        }
    }

    /// Default Ollama model for this role. `None` means the role inherits the
    /// global `IngestionConfig::ollama.model` (or `vision_model`/`ocr_model`
    /// for the vision/ocr roles). Keeps hardware-aware defaults (RAM-based
    /// text model selection) working without duplicating that logic here.
    pub fn default_ollama_model(&self) -> Option<&'static str> {
        match self {
            // Vision / OCR have their own dedicated model slots on OllamaConfig.
            Role::Vision => None,
            Role::Ocr => None,
            // Everyone else inherits the global `ollama.model`.
            _ => None,
        }
    }

    /// Recommended sampling parameters for this role. Classifiers want
    /// Temperature=0 (deterministic); reasoning roles want higher creativity.
    /// Hardware-scoped fields (`num_ctx`, `num_predict`) are overwritten from
    /// the global Ollama config during resolve; the values here are the
    /// role's recommendation when no hardware-aware override applies.
    pub fn default_generation_params(&self) -> OllamaGenerationParams {
        let base = OllamaGenerationParams::default();
        match self {
            // Classifiers: deterministic, small output.
            Role::SmartFolder | Role::DiscoveryInterests => OllamaGenerationParams {
                temperature: models::TEMPERATURE_DETERMINISTIC,
                num_predict: 256,
                ..base
            },
            // OCR: deterministic, large output for long documents.
            Role::Ocr => OllamaGenerationParams {
                temperature: models::TEMPERATURE_DETERMINISTIC,
                ..base
            },
            // Ingestion extraction + mutation agent: focused, not deterministic.
            Role::IngestionText | Role::MutationAgent => OllamaGenerationParams {
                temperature: models::TEMPERATURE_FOCUSED,
                ..base
            },
            // Vision: mild creativity for captioning.
            Role::Vision => OllamaGenerationParams {
                temperature: 0.2,
                ..base
            },
            // Query & Chat: creative, open-ended.
            Role::QueryChat => OllamaGenerationParams {
                temperature: models::TEMPERATURE_CREATIVE,
                ..base
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_has_seven_unique_variants() {
        assert_eq!(Role::ALL.len(), 7);
        let mut seen = std::collections::HashSet::new();
        for role in Role::ALL {
            assert!(seen.insert(*role), "duplicate role in ALL: {:?}", role);
        }
    }

    #[test]
    fn all_covers_every_variant() {
        // Exhaustive match: if a new variant is added, this test forces a
        // corresponding update to Role::ALL.
        for role in Role::ALL {
            match role {
                Role::IngestionText
                | Role::Vision
                | Role::Ocr
                | Role::SmartFolder
                | Role::DiscoveryInterests
                | Role::MutationAgent
                | Role::QueryChat => {}
            }
        }
    }

    #[test]
    fn display_name_and_doc_are_non_empty() {
        for role in Role::ALL {
            assert!(!role.display_name().is_empty(), "{:?}", role);
            assert!(!role.doc().is_empty(), "{:?}", role);
        }
    }

    #[test]
    fn is_text_capable_excludes_vision_and_ocr() {
        for role in Role::ALL {
            match role {
                Role::Vision | Role::Ocr => assert!(!role.is_text_capable(), "{:?}", role),
                _ => assert!(role.is_text_capable(), "{:?}", role),
            }
        }
    }

    #[test]
    fn default_anthropic_model_is_non_empty_for_every_role() {
        for role in Role::ALL {
            assert!(!role.default_anthropic_model().is_empty(), "{:?}", role);
        }
    }

    #[test]
    fn query_chat_defaults_to_sonnet() {
        assert_eq!(
            Role::QueryChat.default_anthropic_model(),
            models::ANTHROPIC_SONNET
        );
    }

    #[test]
    fn classifier_and_fast_roles_default_to_haiku() {
        for role in [
            Role::IngestionText,
            Role::Vision,
            Role::SmartFolder,
            Role::DiscoveryInterests,
            Role::MutationAgent,
        ] {
            assert_eq!(
                role.default_anthropic_model(),
                models::ANTHROPIC_HAIKU,
                "{:?}",
                role
            );
        }
    }

    #[test]
    fn default_ollama_model_is_none_for_all_roles_today() {
        // No role pins a specific Ollama model id — they all inherit from
        // the global `ollama.model` / `ollama.vision_model` / `ollama.ocr_model`.
        for role in Role::ALL {
            assert!(role.default_ollama_model().is_none(), "{:?}", role);
        }
    }

    #[test]
    fn classifier_roles_have_zero_temperature() {
        for role in [Role::SmartFolder, Role::DiscoveryInterests, Role::Ocr] {
            let params = role.default_generation_params();
            assert!(
                (params.temperature - models::TEMPERATURE_DETERMINISTIC).abs() < f32::EPSILON,
                "{:?}",
                role
            );
        }
    }

    #[test]
    fn query_chat_has_creative_temperature() {
        let params = Role::QueryChat.default_generation_params();
        assert!((params.temperature - models::TEMPERATURE_CREATIVE).abs() < f32::EPSILON);
    }

    #[test]
    fn ingestion_and_mutation_roles_are_focused() {
        for role in [Role::IngestionText, Role::MutationAgent] {
            let params = role.default_generation_params();
            assert!(
                (params.temperature - models::TEMPERATURE_FOCUSED).abs() < f32::EPSILON,
                "{:?}",
                role
            );
        }
    }

    #[test]
    fn classifier_num_predict_is_small() {
        // SmartFolder and DiscoveryInterests emit tiny JSON responses; their
        // default num_predict should not waste tokens.
        for role in [Role::SmartFolder, Role::DiscoveryInterests] {
            assert_eq!(role.default_generation_params().num_predict, 256);
        }
    }

    #[test]
    fn role_serializes_to_its_variant_name() {
        // JSON serialization is used by `overrides` HashMap keys on disk and
        // by API responses. Must produce stable variant names.
        assert_eq!(
            serde_json::to_string(&Role::QueryChat).unwrap(),
            "\"QueryChat\""
        );
        assert_eq!(
            serde_json::to_string(&Role::IngestionText).unwrap(),
            "\"IngestionText\""
        );
    }

    #[test]
    fn role_deserializes_from_variant_name() {
        let role: Role = serde_json::from_str("\"QueryChat\"").unwrap();
        assert_eq!(role, Role::QueryChat);
    }

    #[test]
    fn role_is_copy_and_hashable() {
        // Compile-time assertion: these traits are required for HashMap<Role, _>.
        fn assert_copy<T: Copy>() {}
        fn assert_hash<T: std::hash::Hash + Eq>() {}
        assert_copy::<Role>();
        assert_hash::<Role>();
    }
}
