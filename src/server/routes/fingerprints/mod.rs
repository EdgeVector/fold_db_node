//! HTTP routes for the fingerprint subsystem. Thin actix adapters
//! over `crate::handlers::fingerprints`; no business logic here.

pub mod personas;

pub use personas::{get_persona, list_personas, update_persona};
