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
//! This module — along with the sibling `src/handlers/fingerprints/` and
//! `src/server/routes/fingerprints/` trees — MUST NOT import from
//! `crate::handlers::discovery::*` or `crate::server::routes::discovery::*`,
//! and MUST NOT hardcode any `/api/discovery/` URL. Direct peer sharing and
//! Identity Card exchange go through the existing E2E messaging layer only.
//!
//! Enforced structurally by `tests/identity_sharing_fence_test.rs` — it
//! greps every `.rs` file under those trees on every `cargo test` run and
//! fails the build if any forbidden import or URL appears. See TODO-3 in
//! `exemem-workspace/TODOS.md`. The fix when the fence fires is almost
//! never to loosen it; route the new capability through messaging instead.
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

pub mod auto_propose;
pub mod canonical_names;
pub mod extractors;
pub mod face_ann_cache;
pub mod ingest_photo;
pub mod ingest_text;
pub mod ingestion_error_writer;
pub mod keys;
pub mod planned_record;
pub mod registration;
pub mod resolver;
pub mod schema_policy;
pub mod schemas;
pub mod self_identity;
pub mod writer;

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
