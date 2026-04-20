//! HTTP routes for the memory subsystem.
//!
//! Currently exposes one endpoint: `POST /api/memory/register`. Calls
//! `fold_db_node::memory::register_memory_schema` so external tools (the
//! dogfood harness, CLI scripts, MCP servers, etc.) can bring the memory
//! schema up without embedding Rust.
//!
//! The endpoint is idempotent — repeated calls return the same canonical
//! name because the schema service dedupes by identity hash.
//!
//! Additional memory routes (consolidation view registration, etc.) will
//! land here as later phases ship.

use crate::memory;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};
use serde::{Deserialize, Serialize};

use crate::handlers::response::{ApiResponse, IntoTypedHandlerError};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterMemorySchemaResponse {
    /// The canonical schema name assigned by the schema service after
    /// identity-hash canonicalization. Callers must use this name for
    /// subsequent mutations and queries.
    pub canonical_name: String,
    /// The descriptive name consumers can look the schema up by.
    pub descriptive_name: String,
}

/// `POST /api/memory/register` — register the Memory schema against the
/// configured schema service. Idempotent.
pub async fn register_memory_schema(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    handler_result_to_response(
        async {
            let canonical_name = memory::register_memory_schema(&node)
                .await
                .typed_handler_err()?;

            Ok(ApiResponse::success_with_user(
                RegisterMemorySchemaResponse {
                    canonical_name,
                    descriptive_name: memory::MEMORY_DESCRIPTIVE_NAME.to_string(),
                },
                user_hash,
            ))
        }
        .await,
    )
}
