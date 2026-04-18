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
pub use detect_faces::{detect_faces, DetectFacesRequest, DetectFacesResponse};
pub use identities::{list_identities, IdentityAuditRow, ListIdentitiesResponse};
pub use import_contacts::import_contacts;
pub use import_identity_card::{
    import_identity_card, ImportIdentityCardRequest, ImportIdentityCardResponse,
    IncomingIdentityCard,
};
pub use ingest::{
    ingest_photo_faces_batch, DetectedFaceDto, IngestPhotoFacesRequest, IngestPhotoFacesResponse,
    PhotoFacesDto, PhotoIngestResult,
};
pub use ingest_text::{
    ingest_text_signals_batch, IngestTextSignalsRequest, IngestTextSignalsResponse,
};
pub use ingestion_errors::{
    list_ingestion_errors, resolve_ingestion_error, IngestionErrorView, ListIngestionErrorsResponse,
};
pub use my_identity_card::{get_my_identity_card, MyIdentityCardResponse};
pub use personas::{
    apply_persona_patch, delete_persona, get_persona, list_personas, merge_personas,
    update_persona_threshold, ListPersonasResponse, MergePersonasRequest, PersonaDetailResponse,
    PersonaPatch, PersonaSummary,
};
pub use received_cards::{
    accept_received_card, dismiss_received_card, list_received_cards, AcceptReceivedCardRequest,
    AcceptReceivedCardResponse, ListReceivedCardsResponse, ReceivedCardView,
};
pub use reissue_identity_card::{reissue_identity_card, ReissueIdentityCardRequest};
pub use suggestions::{
    accept_suggested_persona, list_suggested_personas, AcceptSuggestedRequest,
    ListSuggestedResponse, SuggestedPersonaView,
};
