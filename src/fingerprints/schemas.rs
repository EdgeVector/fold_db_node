//! Re-export of the Phase 1 built-in schema descriptive_name
//! constants.
//!
//! The constants themselves live in
//! `schema_service_core::builtin_schemas` because that module is
//! the canonical home for the built-in schema definitions. This
//! re-export keeps the existing `use crate::fingerprints::schemas::*`
//! import sites working without requiring every consumer to reach
//! across crate submodules.
//!
//! **IMPORTANT:** these are descriptive_name values, not runtime
//! schema names. fold_db's schema service canonicalizes every
//! schema to its identity_hash on insert, so the runtime `name`
//! field on every built-in schema is a hash, not "Fingerprint".
//! Consumers must always go through
//! `canonical_names::lookup(DESCRIPTIVE_NAME)` to get the runtime
//! name before issuing mutations or queries.

pub use schema_service_core::builtin_schemas::{
    EDGE, EDGE_BY_FINGERPRINT, EXTRACTION_STATUS, FINGERPRINT, IDENTITY, IDENTITY_RECEIPT,
    INGESTION_ERROR, MENTION, MENTION_BY_FINGERPRINT, MENTION_BY_SOURCE, PERSONA,
    PHASE_1_DESCRIPTIVE_NAMES, RECEIVED_SHARE,
};

/// True when `descriptive_name` belongs to the fingerprints subsystem
/// itself. Case-sensitive — descriptive names are canonical in the
/// fingerprints schema set.
///
/// Authoritative source is `PHASE_1_DESCRIPTIVE_NAMES` re-exported
/// above, which lives in `schema_service_core::builtin_schemas`.
/// We used to maintain a local `SYSTEM_DESCRIPTIVE_NAMES` mirror of
/// this list "to avoid depending on schema_service_core" but the
/// crate is already a dep here (the re-export above wouldn't compile
/// otherwise), so the mirror was pure accidental duplication. Every
/// time fold_db added a new system schema (TriggerFiring, etc.),
/// the bump-cascade bot would open a PR that failed CI on the
/// "lists drifted" test until a human edited the local mirror by
/// hand. Pointing directly at the upstream list eliminates the
/// drift class entirely — new schemas just work.
pub fn is_system_descriptive_schema(descriptive_name: &str) -> bool {
    PHASE_1_DESCRIPTIVE_NAMES.contains(&descriptive_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn all_schema_names_are_unique() {
        let unique: HashSet<_> = PHASE_1_DESCRIPTIVE_NAMES.iter().collect();
        assert_eq!(unique.len(), PHASE_1_DESCRIPTIVE_NAMES.len());
    }

    #[test]
    fn is_system_descriptive_schema_rejects_user_schemas() {
        assert!(!is_system_descriptive_schema("Contacts"));
        assert!(!is_system_descriptive_schema("Notes"));
        assert!(!is_system_descriptive_schema("Photos"));
    }
}
