//! Shared state for the ingestion service.
//!
//! The service is wrapped in a `tokio::sync::RwLock` so config saves can
//! swap it out at runtime. This type lives in the pure ingestion module
//! (no actix/web dependencies) so downstream callers can pass it in by
//! reference.

use crate::ingestion::ingestion_service::IngestionService;
use std::sync::Arc;

/// Shared ingestion service state — wrapped in `RwLock` so config saves can reload it.
pub type IngestionServiceState = tokio::sync::RwLock<Option<Arc<IngestionService>>>;

/// Return a cloned `Arc<IngestionService>` if the service is currently available.
pub async fn get_ingestion_service(state: &IngestionServiceState) -> Option<Arc<IngestionService>> {
    state.read().await.clone()
}
