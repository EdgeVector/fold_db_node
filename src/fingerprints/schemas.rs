//! Phase 1 schema definitions for the fingerprint substrate.
//!
//! Twelve schemas split into three groups, following the audit findings
//! in `docs/designs/fingerprints_phase1_audit.md`:
//!
//! Primary:    Fingerprint, Mention, Edge, Identity, IdentityReceipt, Persona
//! Junctions:  EdgeByFingerprint, MentionByFingerprint, MentionBySource
//! Support:    IngestionError, ExtractionStatus, ReceivedShare
//!
//! Each schema uses `DeclarativeSchemaDefinition` with:
//! - `KeyConfig { hash_field, range_field }` for primary-key derivation
//! - `FieldValueType` declarations on every field (no `Any` escape hatch)
//!
//! Key derivation happens at mutation time via `KeyValue::from_mutation`,
//! which reads the value of the declared `hash_field` from the mutation
//! payload. Content-derived keys (e.g. `fp_<sha256(kind, value)>`) are
//! computed by the caller before the mutation and passed as a field
//! value; fold_db then uses that value as the primary key and its
//! upsert semantics handle dedup.
//!
//! ## Junction reverse-lookup patterns
//!
//!    ┌──────────────────────────────────────────────────────┐
//!    │  Fingerprint --- (one) ----- EdgeByFingerprint       │
//!    │       fp_X            rows:                          │
//!    │                         (fp_X, eg_1)                 │
//!    │                         (fp_X, eg_2)                 │
//!    │                         (fp_X, eg_3) ...             │
//!    │                                                       │
//!    │  HashKey("fp_X") on EdgeByFingerprint                │
//!    │    → edge_ids [eg_1, eg_2, eg_3]                     │
//!    │    → batch fetch from Edge schema                    │
//!    │                                                       │
//!    │  Each Edge insert writes TWO junction rows:          │
//!    │    (edge.a, edge.id) and (edge.b, edge.id)           │
//!    │  so reverse lookup resolves either endpoint via      │
//!    │  a single HashKey query.                             │
//!    └──────────────────────────────────────────────────────┘
//!
//! The same pattern is used for `MentionByFingerprint` (one row per
//! (Mention, Fingerprint) reference) and `MentionBySource` (one row
//! per Mention, hashed by "<source_schema>:<source_key>").

// TODO: Populate this module with schema definition constants once the
// round-trip integration test (task 15) confirms how schemas should be
// registered. The current plan is to emit each schema as a JSON payload
// that mirrors what the schema service accepts, then call
// `DbOperations::store_schema` at node startup before any extractor
// writes. If the schema service gatekeeps schema creation for
// similarity/deduplication purposes, the call path routes through it
// instead.
//
// Schema names (for reference by other modules):

pub const FINGERPRINT: &str = "Fingerprint";
pub const MENTION: &str = "Mention";
pub const EDGE: &str = "Edge";
pub const IDENTITY: &str = "Identity";
pub const IDENTITY_RECEIPT: &str = "IdentityReceipt";
pub const PERSONA: &str = "Persona";

pub const EDGE_BY_FINGERPRINT: &str = "EdgeByFingerprint";
pub const MENTION_BY_FINGERPRINT: &str = "MentionByFingerprint";
pub const MENTION_BY_SOURCE: &str = "MentionBySource";

pub const INGESTION_ERROR: &str = "IngestionError";
pub const EXTRACTION_STATUS: &str = "ExtractionStatus";
pub const RECEIVED_SHARE: &str = "ReceivedShare";

/// Every Phase 1 schema name, in registration order. Junctions come after
/// primaries because the resolver writes primaries first.
pub const PHASE_1_SCHEMAS: &[&str] = &[
    FINGERPRINT,
    MENTION,
    EDGE,
    IDENTITY,
    IDENTITY_RECEIPT,
    PERSONA,
    EDGE_BY_FINGERPRINT,
    MENTION_BY_FINGERPRINT,
    MENTION_BY_SOURCE,
    INGESTION_ERROR,
    EXTRACTION_STATUS,
    RECEIVED_SHARE,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_1_schema_set_has_twelve_entries() {
        assert_eq!(PHASE_1_SCHEMAS.len(), 12);
    }

    #[test]
    fn all_schema_names_are_unique() {
        use std::collections::HashSet;
        let unique: HashSet<_> = PHASE_1_SCHEMAS.iter().collect();
        assert_eq!(unique.len(), PHASE_1_SCHEMAS.len());
    }
}
