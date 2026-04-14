//! Fingerprints, Personas, and Identities.
//!
//! This module implements the three-tier identity substrate described in
//! `docs/designs/fingerprints.md` (workspace root). The layering is:
//!
//!   Fingerprint  → raw observation (email, face embedding, name). Unverified.
//!   Persona      → assumed identity. A saved-query lens over a cluster of
//!                  fingerprints. Inferred, mutable, possibly wrong.
//!   Identity     → verified identity. Signed Identity Card anchored to a
//!                  public key. Immutable. Required for trust-gated operations.
//!
//! ## Privacy principle (non-negotiable)
//!
//! Personas and Identities NEVER publish to discovery. They are strictly
//! local, with Phase 3 direct peer sharing going through the existing E2E
//! messaging layer.
//!
//! This module MUST NOT import from `crate::handlers::discovery::*` or
//! `crate::server::routes::discovery::*`. See TODO-3 in
//! `exemem-workspace/TODOS.md`. A CI grep check in Phase 3 will enforce this
//! rule once the sharing module lands.
//!
//! ## Storage model
//!
//! Twelve schemas, split into three groups:
//!
//! Primary (6):     Fingerprint, Mention, Edge, Identity, IdentityReceipt,
//!                  Persona
//! Junctions (3):   EdgeByFingerprint, MentionByFingerprint, MentionBySource
//!                  (HashRange schemas for reverse-lookup, since fold_db
//!                  does not support array-contains queries)
//! Support (3):     IngestionError, ExtractionStatus, ReceivedShare
//!
//! See `docs/designs/fingerprints_phase1_audit.md` for the audit that led
//! to the junction-schema pattern.

pub mod extractors;
pub mod keys;
pub mod registration;
pub mod schema_definitions;
pub mod schemas;

// Ingest-path components (Phase 1)
// pub mod extractors;
// pub mod edge_builder;
// pub mod hnsw_cache;
// pub mod resolver;

// Identity lifecycle (Phase 3)
// pub mod identity;

// Direct peer sharing (Phase 3)
// pub mod persona_share;

// Commented-out modules are scaffolded but not yet implemented. They will
// be wired in as each Phase 1 / Phase 3 milestone ships.
