//! Phase 1 schema registration — propose every fingerprint schema to
//! the schema service at subsystem startup, load the canonical
//! versions the service returns, approve them, and populate the
//! process-wide `canonical_names` lookup.
//!
//! ## The architectural invariant (from user correction 2026-04-14)
//!
//! **All schemas must come from the schema service. No local
//! overrides, ever.** The fingerprints subsystem never creates a
//! schema locally without first proposing it to the service. The
//! service canonicalizes the schema (renames it to its identity_hash,
//! possibly expands it to match an existing schema, or returns an
//! already-existing match) and we trust whatever it returns.
//!
//! This mirrors the existing flow in
//! `src/ingestion/ingestion_service/schema_creation.rs` that the AI
//! ingestion pipeline uses — the fingerprints subsystem is not
//! special, it's just another proposer.
//!
//! ## Flow
//!
//! ```text
//!   register_phase_1_schemas(node)
//!       │
//!       │  for each of the twelve schemas:
//!       │
//!       ▼
//!   ┌─────────────────────────────┐
//!   │ 1. compute_identity_hash    │  (schema_definitions already does
//!   │    (already done in         │   this in build())
//!   │     schema_definitions.rs)  │
//!   └─────────────┬───────────────┘
//!                 │
//!                 ▼
//!   ┌─────────────────────────────┐
//!   │ 2. node.add_schema_to_      │  NETWORK CALL — trust what
//!   │    service(&schema)         │  the service returns
//!   └─────────────┬───────────────┘
//!                 │
//!                 ▼
//!   ┌─────────────────────────────┐
//!   │ 3. AddSchemaResponse {      │
//!   │      schema: canonical,     │  canonical.name is the
//!   │      mutation_mappers,      │  identity_hash, NOT our
//!   │      replaced_schema?       │  proposed semantic name
//!   │    }                        │
//!   └─────────────┬───────────────┘
//!                 │
//!                 ▼
//!   ┌─────────────────────────────┐
//!   │ 4. load_schema_from_json(   │  load the CANONICAL schema
//!   │      canonical_json)        │  locally — not our proposal
//!   └─────────────┬───────────────┘
//!                 │
//!                 ▼
//!   ┌─────────────────────────────┐
//!   │ 5. schema_manager           │  mark usable for mutations
//!   │    .approve(&canonical_name)│
//!   └─────────────┬───────────────┘
//!                 │
//!                 ▼
//!   ┌─────────────────────────────┐
//!   │ 6. canonical_names          │  register descriptive_name
//!   │    .insert(                 │  → canonical_name mapping so
//!   │       descriptive_name,     │  extractors/resolver can
//!   │       canonical.name)       │  look up runtime names
//!   └─────────────────────────────┘
//! ```
//!
//! ## Failure posture
//!
//! Schema registration is a startup prerequisite for the entire
//! fingerprints subsystem. If ANY schema fails to register, the
//! function returns an error and the caller MUST fail loudly — the
//! subsystem cannot operate with only some schemas available. This
//! matches the "no silent failures" invariant and the Section 2
//! error-map decision on `ModelLoadError → fail fast`.
//!
//! ## What this module does NOT do
//!
//! - It does not handle the schema-service `replaced_schema`
//!   (expansion) case. None of the fingerprint schemas have ever
//!   been submitted before this subsystem exists, so there is
//!   nothing to expand. If the service ever returns a non-None
//!   `replaced_schema` during registration, we fail loudly — that
//!   means something about the fingerprints-schema ecosystem is in
//!   an unexpected state and partial registration is unsafe.
//! - It does not acquire a schema_creation_lock. Registration runs
//!   exactly once at subsystem startup; there is no concurrency.
//! - It does not accept manual overrides. The canonical name comes
//!   from the service and the service alone.

use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::schema::SchemaState;

use crate::fold_node::schema_client::AddSchemaResponse;
use crate::fold_node::FoldNode;

use super::canonical_names::{self, CanonicalNames};
use super::schema_definitions::all_phase_1_schemas;

/// Outcome of registering a single schema.
#[derive(Debug, Clone)]
pub struct RegisteredSchema {
    /// The descriptive_name we proposed (e.g. "Fingerprint").
    pub descriptive_name: String,
    /// The canonical runtime name assigned by the schema service.
    /// This is what every subsequent mutation and query must use.
    pub canonical_name: String,
    /// Raw response from the schema service — kept for diagnostics.
    pub response: AddSchemaResponse,
}

/// Outcome of registering the whole Phase 1 schema set.
#[derive(Debug, Clone, Default)]
pub struct RegistrationOutcome {
    pub registered: Vec<RegisteredSchema>,
}

impl RegistrationOutcome {
    pub fn total(&self) -> usize {
        self.registered.len()
    }

    /// Convenience: build the canonical-names mapping from the outcome.
    /// Used by the top-level registration step.
    pub fn build_canonical_names(&self) -> FoldDbResult<CanonicalNames> {
        let mut names = CanonicalNames::new();
        for entry in &self.registered {
            names.insert(&entry.descriptive_name, &entry.canonical_name)?;
        }
        Ok(names)
    }
}

/// Register every Phase 1 fingerprint schema with the schema service.
///
/// For each of the twelve schemas:
///   1. Propose to the schema service via `add_schema_to_service`
///   2. Load the canonical returned schema locally
///   3. Approve it
///   4. Record the descriptive_name → canonical_name mapping
///
/// After this returns successfully, the global `canonical_names`
/// registry is populated and the rest of the fingerprints subsystem
/// can look up runtime names via `canonical_names::lookup(&str)`.
///
/// Fails loudly on the first error. Registration is all-or-nothing
/// because partial state is worse than no state — the resolver can't
/// traverse if half the junction schemas are missing, and the
/// writer can't emit errors if IngestionError isn't registered.
pub async fn register_phase_1_schemas(node: &FoldNode) -> FoldDbResult<RegistrationOutcome> {
    let mut outcome = RegistrationOutcome::default();

    let fold_db = node.get_fold_db()?;
    let schema_manager = fold_db.schema_manager();

    for schema in all_phase_1_schemas() {
        let descriptive_name = schema
            .descriptive_name
            .clone()
            .unwrap_or_else(|| schema.name.clone());

        // 1. Propose to the schema service. trust whatever it returns.
        let response = node.add_schema_to_service(&schema).await.map_err(|e| {
            FoldDbError::Config(format!(
                "fingerprints: schema service rejected '{}': {}",
                descriptive_name, e
            ))
        })?;

        // We only accept clean Added / AlreadyExists. Expansion of a
        // fingerprints schema into an existing schema on first
        // registration would mean the schema-service ecosystem is in
        // an unexpected state — fail loudly rather than half-install.
        if response.replaced_schema.is_some() {
            return Err(FoldDbError::Config(format!(
                "fingerprints: schema '{}' unexpectedly expanded an existing schema '{:?}'. \
                 Refusing to install partial fingerprint graph.",
                descriptive_name, response.replaced_schema
            )));
        }

        let canonical = response.schema.clone();
        let canonical_name = canonical.name.clone();

        // 2. Serialize the canonical schema to JSON and load it
        // locally. We load the canonical version returned by the
        // service — NOT the version we proposed, which would
        // bypass canonicalization.
        let canonical_json = serde_json::to_string(&canonical).map_err(|e| {
            FoldDbError::Config(format!(
                "fingerprints: failed to serialize canonical '{}': {}",
                canonical_name, e
            ))
        })?;

        schema_manager
            .load_schema_from_json(&canonical_json)
            .await
            .map_err(|e| {
                FoldDbError::Config(format!(
                    "fingerprints: failed to load canonical '{}' locally: {}",
                    canonical_name, e
                ))
            })?;

        // 3. Approve the loaded schema so mutations can write to it.
        schema_manager
            .set_schema_state(&canonical_name, SchemaState::Approved)
            .await
            .map_err(|e| {
                FoldDbError::Config(format!(
                    "fingerprints: failed to approve canonical '{}': {}",
                    canonical_name, e
                ))
            })?;

        outcome.registered.push(RegisteredSchema {
            descriptive_name,
            canonical_name,
            response,
        });
    }

    // 4. Install the canonical-name mapping as the process-wide
    // lookup so extractors, the resolver, and the writer can resolve
    // semantic labels to runtime names.
    let mapping = outcome.build_canonical_names()?;
    canonical_names::install(mapping)?;

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_outcome_total_matches_registered_count() {
        let outcome = RegistrationOutcome::default();
        assert_eq!(outcome.total(), 0);
    }

    #[test]
    fn build_canonical_names_rejects_conflicts() {
        // If the same descriptive_name ends up with two different
        // canonical names across the registered set, we must surface
        // the conflict rather than silently overwrite.
        let response = dummy_response("sh_xyz");
        let outcome = RegistrationOutcome {
            registered: vec![
                RegisteredSchema {
                    descriptive_name: "Fingerprint".to_string(),
                    canonical_name: "sh_abc".to_string(),
                    response: response.clone(),
                },
                RegisteredSchema {
                    descriptive_name: "Fingerprint".to_string(),
                    canonical_name: "sh_def".to_string(),
                    response,
                },
            ],
        };
        let err = outcome.build_canonical_names().unwrap_err();
        assert!(format!("{}", err).contains("conflicting"));
    }

    #[test]
    fn build_canonical_names_accepts_consistent_entries() {
        let response = dummy_response("sh_xyz");
        let outcome = RegistrationOutcome {
            registered: vec![
                RegisteredSchema {
                    descriptive_name: "Fingerprint".to_string(),
                    canonical_name: "sh_abc".to_string(),
                    response: response.clone(),
                },
                RegisteredSchema {
                    descriptive_name: "Mention".to_string(),
                    canonical_name: "sh_def".to_string(),
                    response,
                },
            ],
        };
        let names = outcome.build_canonical_names().unwrap();
        assert_eq!(names.get("Fingerprint").unwrap(), "sh_abc");
        assert_eq!(names.get("Mention").unwrap(), "sh_def");
    }

    fn dummy_response(name: &str) -> AddSchemaResponse {
        use fold_db::schema::types::key_config::KeyConfig;
        use fold_db::schema::types::schema::DeclarativeSchemaType;
        use fold_db::schema::types::Schema;
        AddSchemaResponse {
            schema: Schema::new(
                name.to_string(),
                DeclarativeSchemaType::Hash,
                Some(KeyConfig::new(Some("id".to_string()), None)),
                Some(vec!["id".to_string()]),
                None,
                None,
            ),
            mutation_mappers: Default::default(),
            replaced_schema: None,
        }
    }
}
