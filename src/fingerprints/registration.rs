//! Phase 1 schema lookup — fetch the twelve built-in fingerprint
//! schemas from the schema service at subsystem startup, load the
//! canonical versions locally, approve them, and populate the
//! process-wide `canonical_names` registry.
//!
//! ## The architectural invariant (from user direction 2026-04-14)
//!
//! **The twelve Phase 1 fingerprint schemas are system primitives,
//! not user data. They are built into the schema service itself
//! (see `crate::schema_service::builtin_schemas`), not proposed by
//! fold_db_node at startup.** fold_db_node's job is to fetch them
//! by descriptive name, load the canonical version locally, and
//! populate the `descriptive_name → canonical_name` lookup.
//!
//! This differs from the earlier propose-and-verify flow in one
//! important way: fold_db_node no longer carries the schema
//! definitions. The schema service owns them; the node is a
//! consumer. A fold_db_node that talks to a schema service missing
//! these built-ins will fail loudly at startup — that's correct
//! behavior, because fold_db_node cannot install system schemas on
//! behalf of a service that isn't configured for them.
//!
//! ## Flow
//!
//! ```text
//!   lookup_phase_1_schemas(node)
//!       │
//!       │  fetch every schema via
//!       │  /schemas/available → filter by descriptive_name
//!       │
//!       ▼
//!   for each built-in descriptive name:
//!       find canonical schema in the response
//!       if missing → LOUD FAILURE (service is misconfigured)
//!       serialize canonical schema to JSON
//!       schema_manager.load_schema_from_json()
//!       schema_manager.set_schema_state(Approved)
//!       record (descriptive_name → canonical.name) in outcome
//!
//!   install canonical_names map from outcome
//! ```
//!
//! ## Failure posture
//!
//! Fails loudly on the first error. Every error means one of three
//! things is wrong:
//!
//!   1. The schema service is unreachable → startup can't proceed
//!   2. The schema service is reachable but missing a built-in →
//!      the service is misconfigured and must be fixed (or seeded)
//!      before the node can start
//!   3. The local schema manager rejected the canonical JSON →
//!      schema-shape incompatibility between fold_db_node and the
//!      service, which is a deployment mismatch we must not paper
//!      over
//!
//! Partial installation is never acceptable. The resolver can't
//! traverse if half the junction schemas are missing, and the
//! writer can't emit errors if IngestionError isn't registered.

use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::schema::types::Schema;
use fold_db::schema::SchemaState;

use crate::fold_node::FoldNode;
use crate::schema_service::builtin_schemas::PHASE_1_DESCRIPTIVE_NAMES;

use super::canonical_names::{self, CanonicalNames};

/// Outcome of registering a single built-in schema.
#[derive(Debug, Clone)]
pub struct RegisteredSchema {
    /// The descriptive_name we looked up (e.g. "Fingerprint").
    pub descriptive_name: String,
    /// The canonical runtime name assigned by the schema service.
    /// This is what every subsequent mutation and query must use.
    pub canonical_name: String,
    /// The canonical Schema returned by the service. Kept for
    /// diagnostics and for the local load step.
    pub canonical: Schema,
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
    pub fn build_canonical_names(&self) -> FoldDbResult<CanonicalNames> {
        let mut names = CanonicalNames::new();
        for entry in &self.registered {
            names.insert(&entry.descriptive_name, &entry.canonical_name)?;
        }
        Ok(names)
    }
}

/// Backwards-compatible name. Previous callers used
/// `register_phase_1_schemas` when the flow was propose-based. The
/// semantics are now fetch-and-load, but the name stays so downstream
/// imports don't churn.
pub async fn register_phase_1_schemas(node: &FoldNode) -> FoldDbResult<RegistrationOutcome> {
    lookup_phase_1_schemas(node).await
}

/// Fetch every built-in Phase 1 schema from the schema service by
/// descriptive name, load the canonical returned by the service
/// locally, approve it, and populate the process-wide
/// canonical_names registry.
pub async fn lookup_phase_1_schemas(node: &FoldNode) -> FoldDbResult<RegistrationOutcome> {
    let started = std::time::Instant::now();
    log::info!(
        "fingerprints.registration: fetching {} Phase 1 built-in schemas from schema service",
        PHASE_1_DESCRIPTIVE_NAMES.len()
    );
    let mut outcome = RegistrationOutcome::default();

    // 1. Fetch the full list of available schemas from the service.
    // We filter on the client side by descriptive_name rather than
    // adding a new "get by descriptive_name" endpoint — the service
    // side API stays minimal.
    let available = node.fetch_available_schemas().await?;

    // Build a quick lookup: descriptive_name → canonical Schema.
    let mut by_descriptive: std::collections::HashMap<String, Schema> =
        std::collections::HashMap::new();
    for schema in available {
        if let Some(descriptive) = schema.descriptive_name.clone() {
            by_descriptive.insert(descriptive, schema);
        }
    }

    let fold_db = node.get_fold_db()?;
    let schema_manager = fold_db.schema_manager();

    for descriptive in PHASE_1_DESCRIPTIVE_NAMES {
        let canonical = by_descriptive.remove(*descriptive).ok_or_else(|| {
            FoldDbError::Config(format!(
                "fingerprints: schema service is missing built-in schema '{}'. \
                     The service must be running a build that seeds
                     phase-1 built-in schemas (see
                     `src/schema_service/builtin_schemas.rs`). \
                     fold_db_node refuses to start without all twelve.",
                descriptive
            ))
        })?;

        let canonical_name = canonical.name.clone();

        // 2. Load the canonical schema locally. We serialize what the
        // service returned and pass it to the schema manager exactly
        // as-is — the canonical version is the source of truth.
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
            descriptive_name: descriptive.to_string(),
            canonical_name,
            canonical,
        });
    }

    // 4. Install the canonical-name mapping as the process-wide
    // lookup so extractors, the resolver, and the writer can resolve
    // semantic labels to runtime names.
    let mapping = outcome.build_canonical_names()?;
    canonical_names::install(mapping)?;

    log::info!(
        "fingerprints.registration: Phase 1 registration complete in {:?} ({} schemas loaded and approved)",
        started.elapsed(),
        outcome.total()
    );

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
        let outcome = RegistrationOutcome {
            registered: vec![
                RegisteredSchema {
                    descriptive_name: "Fingerprint".to_string(),
                    canonical_name: "sh_abc".to_string(),
                    canonical: dummy_schema("sh_abc"),
                },
                RegisteredSchema {
                    descriptive_name: "Fingerprint".to_string(),
                    canonical_name: "sh_def".to_string(),
                    canonical: dummy_schema("sh_def"),
                },
            ],
        };
        let err = outcome.build_canonical_names().unwrap_err();
        assert!(format!("{}", err).contains("conflicting"));
    }

    #[test]
    fn build_canonical_names_accepts_consistent_entries() {
        let outcome = RegistrationOutcome {
            registered: vec![
                RegisteredSchema {
                    descriptive_name: "Fingerprint".to_string(),
                    canonical_name: "sh_abc".to_string(),
                    canonical: dummy_schema("sh_abc"),
                },
                RegisteredSchema {
                    descriptive_name: "Mention".to_string(),
                    canonical_name: "sh_def".to_string(),
                    canonical: dummy_schema("sh_def"),
                },
            ],
        };
        let names = outcome.build_canonical_names().unwrap();
        assert_eq!(names.get("Fingerprint").unwrap(), "sh_abc");
        assert_eq!(names.get("Mention").unwrap(), "sh_def");
    }

    fn dummy_schema(name: &str) -> Schema {
        use fold_db::schema::types::key_config::KeyConfig;
        use fold_db::schema::types::schema::DeclarativeSchemaType;
        Schema::new(
            name.to_string(),
            DeclarativeSchemaType::Hash,
            Some(KeyConfig::new(Some("id".to_string()), None)),
            Some(vec!["id".to_string()]),
            None,
            None,
        )
    }
}
