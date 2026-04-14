//! Extractor implementations that turn source records into Fingerprint /
//! Mention / Edge / ExtractionStatus records.
//!
//! Each extractor has two layers:
//!
//! 1. **Pure planning layer** — takes raw inputs (e.g. a list of detected
//!    faces from a single photo) and returns a structured
//!    `ExtractionPlan` describing every record that should be written.
//!    No I/O, fully unit-testable.
//!
//! 2. **Writer layer** — takes an `ExtractionPlan` and an
//!    `OperationProcessor`, actually writes the records via the
//!    standard fold_db mutation path, surfacing `IngestionError` on
//!    any write failure per the loud-failure invariant.
//!
//! Separating these lets us test extraction logic exhaustively without
//! a running node and test the write layer separately with a real
//! schema-service-backed node.

pub mod face;

// Phase 2 extractors
// pub mod ner;
// pub mod email_header;
// pub mod calendar_attendee;
// pub mod contact;
