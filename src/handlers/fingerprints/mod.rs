//! Framework-agnostic handlers for the fingerprint subsystem.
//!
//! Exposes the Persona view layer (list + detail-with-resolved-cluster)
//! that the People tab UI consumes. Callers pass descriptive schema
//! names through `canonical_names::lookup()` transparently — HTTP
//! clients don't need to know about the canonical-name indirection.
//!
//! Each handler returns `HandlerResult<T>` in the standard
//! framework-agnostic shape; the HTTP routes layer
//! (`crate::server::routes::fingerprints`) wraps them in actix
//! responses.

pub mod personas;

pub use personas::{
    get_persona, list_personas, update_persona_threshold, ListPersonasResponse,
    PersonaDetailResponse, PersonaSummary,
};
