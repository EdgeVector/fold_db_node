//! HTTP routes for the fingerprint subsystem. Thin actix adapters
//! over `crate::handlers::fingerprints`; no business logic here.

pub mod import_contacts;
pub mod ingest;
pub mod ingest_text;
pub mod ingestion_errors;
pub mod my_identity_card;
pub mod personas;
pub mod suggestions;

pub use import_contacts::import_contacts;
pub use ingest::ingest_photo_faces;
pub use ingest_text::ingest_text_signals;
pub use ingestion_errors::{list_ingestion_errors, resolve_ingestion_error};
pub use my_identity_card::get_my_identity_card;
pub use personas::{delete_persona, get_persona, list_personas, update_persona};
pub use suggestions::{accept_suggested_persona, list_suggested_personas};
