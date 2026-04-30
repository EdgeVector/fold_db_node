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

/// Descriptive names of schemas owned by the fingerprints subsystem.
/// The generic-ingest hook (`crate::ingestion::fingerprint_hook`)
/// uses this list as a skip-set so the hook does not recurse into its
/// own writes — a Mention insert must not trigger another extraction
/// pass on the Mention record's fields.
///
/// Mirrors `PHASE_1_DESCRIPTIVE_NAMES` re-exported above; we duplicate
/// the list locally so the hook can query without depending on
/// `schema_service_core` directly. The unit test below pins the two
/// lists to the same set so they cannot drift.
pub const SYSTEM_DESCRIPTIVE_NAMES: &[&str] = &[
    EDGE,
    EDGE_BY_FINGERPRINT,
    EXTRACTION_STATUS,
    FINGERPRINT,
    IDENTITY,
    IDENTITY_RECEIPT,
    INGESTION_ERROR,
    MENTION,
    MENTION_BY_FINGERPRINT,
    MENTION_BY_SOURCE,
    PERSONA,
    RECEIVED_SHARE,
];

/// True when `descriptive_name` belongs to the fingerprints subsystem
/// itself. Case-sensitive — descriptive names are canonical in the
/// fingerprints schema set.
pub fn is_system_descriptive_schema(descriptive_name: &str) -> bool {
    SYSTEM_DESCRIPTIVE_NAMES.contains(&descriptive_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn phase_1_schema_set_has_twelve_entries() {
        assert_eq!(PHASE_1_DESCRIPTIVE_NAMES.len(), 12);
    }

    #[test]
    fn all_schema_names_are_unique() {
        let unique: HashSet<_> = PHASE_1_DESCRIPTIVE_NAMES.iter().collect();
        assert_eq!(unique.len(), PHASE_1_DESCRIPTIVE_NAMES.len());
    }

    #[test]
    fn system_descriptive_names_match_phase_1_set() {
        let local: HashSet<_> = SYSTEM_DESCRIPTIVE_NAMES.iter().copied().collect();
        let upstream: HashSet<_> = PHASE_1_DESCRIPTIVE_NAMES.iter().copied().collect();
        assert_eq!(
            local, upstream,
            "SYSTEM_DESCRIPTIVE_NAMES drifted from PHASE_1_DESCRIPTIVE_NAMES — \
             update both lists when a new fingerprint subsystem schema is added"
        );
    }

    #[test]
    fn is_system_descriptive_schema_recognizes_all_subsystem_schemas() {
        for name in PHASE_1_DESCRIPTIVE_NAMES {
            assert!(
                is_system_descriptive_schema(name),
                "{} should be classified as a system schema",
                name
            );
        }
    }

    #[test]
    fn is_system_descriptive_schema_rejects_user_schemas() {
        assert!(!is_system_descriptive_schema("Contacts"));
        assert!(!is_system_descriptive_schema("Notes"));
        assert!(!is_system_descriptive_schema("Photos"));
    }
}
