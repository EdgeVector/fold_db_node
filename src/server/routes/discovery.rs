use crate::handlers::discovery as discovery_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, require_node_read};
use actix_web::{web, HttpRequest, HttpResponse, Responder};

/// Helper to get discovery config from environment.
/// Returns (discovery_url, master_key) or an error response.
fn get_discovery_config() -> Result<(String, Vec<u8>), HttpResponse> {
    let url = std::env::var("DISCOVERY_SERVICE_URL").map_err(|_| {
        HttpResponse::ServiceUnavailable().json(serde_json::json!({
            "ok": false,
            "error": "Discovery service not configured. Set DISCOVERY_SERVICE_URL.",
            "code": "DISCOVERY_NOT_CONFIGURED"
        }))
    })?;

    let key_hex = std::env::var("DISCOVERY_MASTER_KEY").map_err(|_| {
        HttpResponse::ServiceUnavailable().json(serde_json::json!({
            "ok": false,
            "error": "Discovery master key not configured. Set DISCOVERY_MASTER_KEY.",
            "code": "DISCOVERY_NOT_CONFIGURED"
        }))
    })?;

    let key = hex::decode(&key_hex).map_err(|_| {
        HttpResponse::InternalServerError().json(serde_json::json!({
            "ok": false,
            "error": "Invalid DISCOVERY_MASTER_KEY (expected hex-encoded bytes).",
            "code": "INVALID_CONFIG"
        }))
    })?;

    Ok((url, key))
}

/// Extract the auth token from the DISCOVERY_AUTH_TOKEN env var or the
/// incoming request's Authorization header.
fn get_auth_token(req: &HttpRequest) -> Result<String, HttpResponse> {
    // First check env var (for server-side automated publishing)
    if let Ok(token) = std::env::var("DISCOVERY_AUTH_TOKEN") {
        return Ok(token);
    }

    // Fall back to incoming request's Authorization header
    let auth = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    auth.ok_or_else(|| {
        HttpResponse::Unauthorized().json(serde_json::json!({
            "ok": false,
            "error": "Missing auth token. Set DISCOVERY_AUTH_TOKEN or pass Authorization: Bearer <token>.",
            "code": "AUTH_REQUIRED"
        }))
    })
}

/// GET /api/discovery/opt-ins — List all discovery opt-in configs.
pub async fn list_opt_ins(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::list_opt_ins(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/opt-in — Opt-in a schema for discovery.
pub async fn opt_in(
    body: web::Json<discovery_handlers::OptInRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::opt_in(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/opt-out — Opt-out a schema from discovery.
pub async fn opt_out(
    req: HttpRequest,
    body: web::Json<discovery_handlers::OptOutRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    let (url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::opt_out(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/publish — Publish embeddings for all opted-in schemas.
pub async fn publish(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    let (url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::publish(&node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/search — Search the discovery network.
pub async fn search(
    req: HttpRequest,
    body: web::Json<discovery_handlers::SearchRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    let (url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::search(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/connect — Send an E2E encrypted connection request.
pub async fn connect(
    req: HttpRequest,
    body: web::Json<discovery_handlers::ConnectRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    let (url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::connect(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/connection-requests — Poll, decrypt, and list received connection requests.
pub async fn connection_requests(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    let (url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::poll_and_decrypt_requests(&node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/connection-requests/respond — Accept or decline a connection request.
pub async fn respond_to_request(
    req: HttpRequest,
    body: web::Json<discovery_handlers::RespondToRequestPayload>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    let (url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::respond_to_request(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/sent-requests — List sent connection requests with status.
pub async fn sent_requests(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::list_sent_requests(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/requests — Legacy: Poll for incoming connection requests.
pub async fn poll_requests(req: HttpRequest, _state: web::Data<AppState>) -> impl Responder {
    let (url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::poll_requests(&url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/browse/categories — Browse available categories on the network.
pub async fn browse_categories(req: HttpRequest, _state: web::Data<AppState>) -> impl Responder {
    let (url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::browse_categories(&url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/interests — Get detected interest categories.
pub async fn get_interests(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::get_interests(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/interests/toggle — Toggle an interest category.
pub async fn toggle_interest(
    body: web::Json<discovery_handlers::ToggleInterestRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::toggle_interest(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/interests/detect — Manually trigger interest detection.
pub async fn detect_interests(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::detect_interests(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/similar-profiles — Find users with similar interest fingerprints.
pub async fn similar_profiles(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    let (url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::similar_profiles(&node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

// === Calendar Sharing Routes ===

/// GET /api/discovery/calendar-sharing/status — Get calendar sharing opt-in status.
pub async fn calendar_sharing_status(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::calendar_sharing_status(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/calendar-sharing/opt-in — Enable calendar sharing.
pub async fn calendar_sharing_opt_in(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::calendar_sharing_opt_in(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/calendar-sharing/opt-out — Disable calendar sharing.
pub async fn calendar_sharing_opt_out(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::calendar_sharing_opt_out(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/calendar-sharing/sync — Sync calendar events for comparison.
pub async fn sync_calendar_events(
    body: web::Json<discovery_handlers::SyncCalendarEventsRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::sync_calendar_events(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/calendar-sharing/peer-events — Store peer event fingerprints.
pub async fn store_peer_events(
    body: web::Json<discovery_handlers::StorePeerEventsRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::store_peer_events(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/shared-events — Detect and return shared events with connections.
pub async fn get_shared_events(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::get_shared_events(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

// === Photo Moment Detection Routes ===

/// GET /api/discovery/moments/opt-ins — List all moment sharing opt-ins.
pub async fn moment_opt_in_list(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::moment_opt_in_list(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/moments/opt-in — Opt-in to photo moment sharing with a peer.
pub async fn moment_opt_in(
    body: web::Json<discovery_handlers::MomentOptInRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::moment_opt_in(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/moments/opt-out — Opt-out of photo moment sharing with a peer.
pub async fn moment_opt_out(
    body: web::Json<discovery_handlers::MomentOptOutRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::moment_opt_out(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/moments/scan — Scan local photos and generate moment hashes.
pub async fn moment_scan(
    body: web::Json<Vec<discovery_handlers::PhotoMetadata>>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    let (_url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    match discovery_handlers::moment_scan(&node, &key, &body).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/moments/receive — Receive moment hashes from a peer.
pub async fn moment_receive_hashes(
    body: web::Json<discovery_handlers::MomentHashReceiveRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::moment_receive_hashes(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/moments/detect — Detect shared moments from exchanged hashes.
pub async fn moment_detect(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::moment_detect(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/moments — List all detected shared moments.
pub async fn moment_list(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = match require_node_read(&state).await {
        Ok(res) => res,
        Err(response) => return response,
    };

    match discovery_handlers::moment_list(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}
