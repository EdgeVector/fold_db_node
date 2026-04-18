//! HTTP routes for the fingerprint subsystem. Thin actix adapters
//! over `crate::handlers::fingerprints`; no business logic here.

#[cfg(feature = "face-detection")]
pub mod detect_faces;
pub mod identities;
pub mod import_contacts;
pub mod import_identity_card;
pub mod ingest;
pub mod ingest_text;
pub mod ingestion_errors;
pub mod my_identity_card;
pub mod personas;
pub mod received_cards;
pub mod reissue_identity_card;
pub mod suggestions;

#[cfg(feature = "face-detection")]
pub use detect_faces::detect_faces;
pub use identities::list_identities;
pub use import_contacts::import_contacts;
pub use import_identity_card::import_identity_card;
pub use ingest::ingest_photo_faces;
pub use ingest_text::ingest_text_signals;
pub use ingestion_errors::{list_ingestion_errors, resolve_ingestion_error};
pub use my_identity_card::get_my_identity_card;
pub use personas::{delete_persona, get_persona, list_personas, merge_personas, update_persona};
pub use received_cards::{accept_received_card, dismiss_received_card, list_received_cards};
pub use reissue_identity_card::reissue_identity_card;
pub use suggestions::{accept_suggested_persona, list_suggested_personas};
