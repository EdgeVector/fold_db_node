//! Re-export of the Phase 1 built-in schema descriptive_name
//! constants.
//!
//! The constants themselves live in
//! `crate::schema_service::builtin_schemas` because that module is
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

pub use crate::schema_service::builtin_schemas::{
    EDGE, EDGE_BY_FINGERPRINT, EXTRACTION_STATUS, FINGERPRINT, IDENTITY, IDENTITY_RECEIPT,
    INGESTION_ERROR, MENTION, MENTION_BY_FINGERPRINT, MENTION_BY_SOURCE, PERSONA,
    PHASE_1_DESCRIPTIVE_NAMES, RECEIVED_SHARE,
};

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
}
