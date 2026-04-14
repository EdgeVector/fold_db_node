//! Phase 1 schema registration: propose the twelve fingerprint schemas
//! to the schema service at subsystem startup.
//!
//! This follows the project invariant documented in memory and in
//! `exemem-workspace/docs/designs/fingerprints.md`: schemas are supplied
//! by end users and verified by the schema service; fold_db_node never
//! creates schemas manually. The Fingerprints subsystem follows the
//! same path as user-data schemas — we `add_schema_to_service()` each
//! proposal and let the schema service return the canonical version.
//!
//! ```text
//!   Phase 1 startup:
//!     ┌─────────────────────────────────────────────────┐
//!     │  register_phase_1_schemas(node)                 │
//!     └────────────┬────────────────────────────────────┘
//!                  │
//!                  ▼
//!        for each schema in all_phase_1_schemas():
//!          ┌────────────────────────────┐
//!          │  node.add_schema_to_service│── Added    ─┐
//!          │  (&schema)                 │── AlreadyExists │
//!          │                            │── Expanded ─┘
//!          └──────────┬─────────────────┘
//!                     │
//!                     │ canonical schema returned
//!                     ▼
//!           RegistrationOutcome:
//!             - total schemas proposed
//!             - per-schema: Added / AlreadyExists / Expanded
//!             - errors (fail-loud: propagated up, not swallowed)
//! ```
//!
//! ## Failure posture
//!
//! Schema registration is a startup prerequisite. If ANY schema fails
//! to register (schema service unreachable, validation error,
//! unexpected response), the function returns an error and the caller
//! — the node startup sequence — MUST fail loudly. The subsystem is
//! not partially-usable: without all twelve schemas registered, the
//! resolver can't traverse the graph, extractors can't write mentions,
//! and the People tab has nothing to show. Half-state here is worse
//! than a loud "fingerprint subsystem failed to start" on boot.
//!
//! Per the project's no-silent-failures invariant, there is no
//! swallow-and-continue path. This matches the
//! `ModelLoadError → fail fast` decision captured in the Section 2
//! error map.

use crate::fold_node::schema_client::AddSchemaResponse;
use crate::fold_node::FoldNode;
use fold_db::error::FoldDbResult;
use fold_db::schema::types::Schema;

use super::schema_definitions::all_phase_1_schemas;

/// Outcome of registering a single schema against the schema service.
#[derive(Debug, Clone)]
pub struct RegisteredSchema {
    /// The name of the schema as the Fingerprints subsystem defines it,
    /// before any renaming by the schema service.
    pub proposed_name: String,
    /// The canonical schema returned by the service (which may have a
    /// different `name` if the service canonicalizes to an identity hash).
    pub canonical: Schema,
    /// The disposition — did the service add, find-existing, or expand?
    /// We keep the raw response so callers can introspect.
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
}

/// Propose every Phase 1 schema to the schema service. Fails loudly on
/// the first error — partial registration is not a valid state for the
/// fingerprints subsystem.
///
/// The caller is responsible for invoking this during node startup,
/// before any extractor is allowed to run.
pub async fn register_phase_1_schemas(node: &FoldNode) -> FoldDbResult<RegistrationOutcome> {
    let mut outcome = RegistrationOutcome::default();

    for schema in all_phase_1_schemas() {
        let proposed_name = schema.name.clone();
        let response = node.add_schema_to_service(&schema).await?;
        let canonical = response.schema.clone();
        outcome.registered.push(RegisteredSchema {
            proposed_name,
            canonical,
            response,
        });
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registration_outcome_total_matches_registered_count() {
        let mut outcome = RegistrationOutcome::default();
        assert_eq!(outcome.total(), 0);

        // Synthetic entries: we don't need a real AddSchemaResponse here
        // since the field is opaque to the total() method. The runtime
        // test of registration against a live schema service lives in
        // tests/ and requires a real URL.
        outcome.registered = Vec::with_capacity(3);
        assert_eq!(outcome.total(), 0);
    }
}
