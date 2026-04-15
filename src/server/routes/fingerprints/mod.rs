//! HTTP routes for the fingerprint subsystem. Thin actix adapters
//! over `crate::handlers::fingerprints`; no business logic here.

pub mod ingest;
pub mod ingestion_errors;
pub mod personas;

pub use ingest::ingest_photo_faces;
pub use ingestion_errors::{list_ingestion_errors, resolve_ingestion_error};
pub use personas::{get_persona, list_personas, update_persona};
