//! HTTP route handlers for LLM query workflow.
//!
//! These are Actix-web route handlers that delegate to the shared handler layer.
//! They handle HTTP-specific concerns (request/response extraction) and OpenAPI documentation.

use super::service::LlmQueryService;
use super::session::SessionManager;
use super::types::*;
use crate::handlers::llm as shared_handlers;
use crate::handlers::llm::AgentQueryHandlerRequest;
use crate::ingestion::IngestionConfig;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, require_node};
use actix_web::{web, HttpResponse, Responder};
use serde_json::json;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Shared state for LLM query routes
pub struct LlmQueryState {
    pub service: RwLock<Option<Arc<LlmQueryService>>>,
    pub session_manager: Arc<SessionManager>,
}

impl LlmQueryState {
    pub fn new() -> Self {
        let config = IngestionConfig::load_or_default();
        let service = match LlmQueryService::new(config) {
            Ok(svc) => Some(Arc::new(svc)),
            Err(e) => {
                log::warn!("LLM Query service not available: {}. LLM query endpoints will return errors until configured.", e);
                None
            }
        };
        let session_manager = Arc::new(SessionManager::new());
        Self {
            service: RwLock::new(service),
            session_manager,
        }
    }

    /// Reload the LLM query service with fresh config
    pub async fn reload(&self) {
        let config = IngestionConfig::load_or_default();
        match LlmQueryService::new(config) {
            Ok(svc) => {
                let mut guard = self.service.write().await;
                *guard = Some(Arc::new(svc));
                log::info!("LlmQueryService reloaded with new configuration");
            }
            Err(e) => {
                log::warn!("Failed to reload LlmQueryService: {}", e);
            }
        }
    }
}

impl Default for LlmQueryState {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper to require LLM service or return error response
async fn require_service(llm_state: &LlmQueryState) -> Result<Arc<LlmQueryService>, HttpResponse> {
    let guard = llm_state.service.read().await;
    guard.clone().ok_or_else(|| {
        HttpResponse::ServiceUnavailable().json(json!({
            "error": "LLM Query service not configured",
            "message": "Please configure AI_PROVIDER and ANTHROPIC_API_KEY or OLLAMA_BASE_URL environment variables to use this feature"
        }))
    })
}

/// Common setup: require LLM service + authenticated node read guard.
async fn require_llm_context(
    app_state: &web::Data<AppState>,
    llm_state: &LlmQueryState,
) -> Result<
    (
        Arc<LlmQueryService>,
        String,
        Arc<crate::fold_node::FoldNode>,
    ),
    HttpResponse,
> {
    let service = require_service(llm_state).await?;
    let (user_hash, node_arc) = require_node(app_state).await?;
    Ok((service, user_hash, node_arc))
}

/// Convert a HandlerResult with data into an HttpResponse, or 500 if data is missing.
fn data_or_500<T: serde::Serialize>(
    result: Result<crate::handlers::response::ApiResponse<T>, crate::handlers::HandlerError>,
) -> HttpResponse {
    match result {
        Ok(response) => match response.data {
            Some(data) => HttpResponse::Ok().json(data),
            None => {
                HttpResponse::InternalServerError().json(json!({"error": "Missing response data"}))
            }
        },
        Err(e) => handler_error_to_response(e),
    }
}

/// Analyze if a follow-up question can be answered from existing context
#[utoipa::path(
    post,
    path = "/api/llm-query/analyze-followup",
    tag = "llm-query",
    request_body = ChatRequest,
    responses(
        (status = 200, description = "Follow-up analysis result", body = FollowupAnalysis),
        (status = 404, description = "Session not found"),
        (status = 500, description = "Server error")
    )
)]
pub async fn analyze_followup(
    request: web::Json<ChatRequest>,
    app_state: web::Data<AppState>,
    llm_state: web::Data<LlmQueryState>,
) -> impl Responder {
    let (service, user_hash, node) = match require_llm_context(&app_state, &llm_state).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    data_or_500(
        shared_handlers::analyze_followup(
            request.into_inner(),
            &user_hash,
            service.as_ref(),
            llm_state.session_manager.as_ref(),
            &node,
        )
        .await,
    )
}

/// Ask a follow-up question about query results
#[utoipa::path(
    post,
    path = "/api/llm-query/chat",
    tag = "llm-query",
    request_body = ChatRequest,
    responses(
        (status = 200, description = "Answer to question", body = ChatResponse),
        (status = 404, description = "Session not found"),
        (status = 500, description = "Server error")
    )
)]
pub async fn chat(
    request: web::Json<ChatRequest>,
    app_state: web::Data<AppState>,
    llm_state: web::Data<LlmQueryState>,
) -> impl Responder {
    let (service, user_hash, node) = match require_llm_context(&app_state, &llm_state).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    data_or_500(
        shared_handlers::chat(
            request.into_inner(),
            &user_hash,
            service.as_ref(),
            llm_state.session_manager.as_ref(),
            &node,
        )
        .await,
    )
}

/// Execute an AI-native index query workflow
#[utoipa::path(
    post,
    path = "/api/llm-query/native-index",
    tag = "llm-query",
    request_body = RunQueryRequest,
    responses(
        (status = 200, description = "AI-native index query result", body = String),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Server error")
    )
)]
pub async fn ai_native_index_query(
    request: web::Json<RunQueryRequest>,
    app_state: web::Data<AppState>,
    llm_state: web::Data<LlmQueryState>,
) -> impl Responder {
    let (service, user_hash, node) = match require_llm_context(&app_state, &llm_state).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    data_or_500(
        shared_handlers::ai_native_index_query(
            request.into_inner(),
            &user_hash,
            service.as_ref(),
            llm_state.session_manager.as_ref(),
            &node,
        )
        .await,
    )
}

/// Execute an agent query - an autonomous LLM agent that can use tools
#[utoipa::path(
    post,
    path = "/api/llm-query/agent",
    tag = "llm-query",
    request_body = AgentQueryRequest,
    responses(
        (status = 200, description = "Agent query result", body = AgentQueryResponse),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Server error")
    )
)]
pub async fn agent_query(
    request: web::Json<AgentQueryRequest>,
    app_state: web::Data<AppState>,
    llm_state: web::Data<LlmQueryState>,
    progress_tracker: web::Data<crate::ingestion::ProgressTracker>,
) -> impl Responder {
    let (service, user_hash, node) = match require_llm_context(&app_state, &llm_state).await {
        Ok(ctx) => ctx,
        Err(response) => return response,
    };

    let handler_request = AgentQueryHandlerRequest {
        query: request.query.clone(),
        session_id: request.session_id.clone(),
        max_iterations: request.max_iterations,
        context: request.context.clone(),
    };

    data_or_500(
        shared_handlers::agent_query(
            handler_request,
            &user_hash,
            service.as_ref(),
            llm_state.session_manager.as_ref(),
            &node,
            Some(progress_tracker.get_ref()),
        )
        .await,
    )
}
