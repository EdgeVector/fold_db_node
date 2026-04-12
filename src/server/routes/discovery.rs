use crate::handlers::discovery as discovery_handlers;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, node_or_return};
use actix_web::{web, HttpRequest, HttpResponse, Responder};

/// Helper to get discovery config.
/// Checks env vars first, then falls back to deriving from Sled config store.
/// Returns (discovery_url, master_key) or an error response.
fn get_discovery_config() -> Result<(String, Vec<u8>), HttpResponse> {
    let not_configured = || {
        HttpResponse::ServiceUnavailable().json(serde_json::json!({
            "ok": false,
            "error": "Discovery not available. Register with Exemem to enable.",
            "code": "DISCOVERY_NOT_CONFIGURED"
        }))
    };

    // Try env vars first (explicit override)
    if let (Ok(url), Ok(key_hex)) = (
        std::env::var("DISCOVERY_SERVICE_URL"),
        std::env::var("DISCOVERY_MASTER_KEY"),
    ) {
        let key = hex::decode(&key_hex).map_err(|_| {
            HttpResponse::InternalServerError().json(serde_json::json!({
                "ok": false,
                "error": "Invalid DISCOVERY_MASTER_KEY (expected hex-encoded bytes).",
                "code": "INVALID_CONFIG"
            }))
        })?;
        return Ok((url, key));
    }

    // Fall back: derive from Sled config store
    // Discovery URL = cloud api_url + "/api" (same API gateway)
    // Master key = SHA256(node private key)
    let data_path = crate::utils::paths::folddb_home()
        .ok()
        .map(|h| h.join("data"))
        .or_else(|| {
            std::env::var("FOLD_STORAGE_PATH")
                .ok()
                .map(std::path::PathBuf::from)
        });

    if let Some(path) = data_path {
        {
            let pool = std::sync::Arc::new(fold_db::storage::SledPool::new(path));
            if let Ok(store) = fold_db::NodeConfigStore::new(pool) {
                if let (Some(cloud), Some(identity)) =
                    (store.get_cloud_config(), store.get_identity())
                {
                    use sha2::{Digest, Sha256};
                    let url = format!("{}/api", cloud.api_url);
                    let key = Sha256::digest(identity.private_key.as_bytes()).to_vec();
                    return Ok((url, key));
                }
            }
        }
    }

    Err(not_configured())
}

/// Extract the auth token from env var, local credential store, or the incoming request's
/// Authorization header. Env var is checked first to avoid unnecessary file reads
/// in dev/CLI mode.
fn get_auth_token(req: &HttpRequest) -> Result<String, HttpResponse> {
    // 1. Env var (no credential file read needed — preferred for dev/CLI)
    if let Ok(token) = std::env::var("DISCOVERY_AUTH_TOKEN") {
        return Ok(token);
    }

    // 2. Local credentials (where register/refresh store the token in desktop app)
    if let Ok(Some(creds)) = crate::keychain::load_credentials() {
        if !creds.session_token.is_empty() {
            return Ok(creds.session_token);
        }
    }

    // 3. Fall back to incoming request's Authorization header
    let auth = req
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    auth.ok_or_else(|| {
        HttpResponse::Unauthorized().json(serde_json::json!({
            "ok": false,
            "error": "No auth token available. Register with Exemem first (POST /api/auth/register).",
            "code": "AUTH_REQUIRED"
        }))
    })
}

/// Check if a discovery handler error is an auth failure.
/// Matches the HandlerError variant directly instead of fragile string matching.
fn is_auth_error(err: &crate::handlers::HandlerError) -> bool {
    matches!(err, crate::handlers::HandlerError::Unauthorized(_)) || err.to_string().contains("401")
}

/// Try to refresh the session token via signed register, then return the new token.
/// Returns None if refresh is not possible (no node, no exemem API, etc.).
async fn try_refresh_token(state: &web::Data<AppState>) -> Option<String> {
    match crate::server::routes::auth::refresh_session_token(state).await {
        Ok(token) => {
            log::info!("Discovery auth: refreshed session token after 401");
            Some(token)
        }
        Err(e) => {
            log::warn!("Discovery auth: token refresh failed: {}", e);
            None
        }
    }
}

/// Resolve discovery config for use in other route modules.
/// Returns (discovery_url, master_key, auth_token) or a HandlerError.
pub async fn resolve_discovery_config(
    node: &crate::fold_node::node::FoldNode,
    req: Option<&HttpRequest>,
) -> Result<(String, Vec<u8>, String), crate::handlers::HandlerError> {
    let (url, key) = get_discovery_config().map_err(|resp| {
        // Extract the error message from the HttpResponse body if possible
        let msg = "Discovery not configured. Register with Exemem to enable (folddb cloud enable).";
        if resp.status() == actix_web::http::StatusCode::INTERNAL_SERVER_ERROR {
            crate::handlers::HandlerError::Internal(msg.to_string())
        } else {
            crate::handlers::HandlerError::ServiceUnavailable(msg.to_string())
        }
    })?;

    // Try to get auth token from env, keychain, or dummy for the request
    let token = if let Some(r) = req {
        get_auth_token(r).map_err(|_| {
            crate::handlers::HandlerError::Unauthorized("No auth token available".to_string())
        })?
    } else {
        // Without a request, try env var and keychain
        std::env::var("DISCOVERY_AUTH_TOKEN")
            .ok()
            .or_else(|| {
                crate::keychain::load_credentials()
                    .ok()
                    .flatten()
                    .filter(|c| !c.session_token.is_empty())
                    .map(|c| c.session_token)
            })
            .ok_or_else(|| {
                crate::handlers::HandlerError::Unauthorized("No auth token available".to_string())
            })?
    };

    let _ = node; // may be used in future for node-specific config
    Ok((url, key, token))
}

/// GET /api/discovery/opt-ins — List all discovery opt-in configs.
pub async fn list_opt_ins(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

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
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::opt_in(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/my-pseudonyms — List all pseudonyms this node publishes.
/// Used by the E2E test framework for cleanup.
pub async fn my_pseudonyms(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (_url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    match discovery_handlers::my_pseudonyms(&node, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/opt-out-all — Clear all discovery opt-ins (test cleanup).
pub async fn opt_out_all(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::opt_out_all(&node).await {
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
    let (_user_hash, node) = node_or_return!(state);

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
    let (_user_hash, node) = node_or_return!(state);

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
        Err(e) if is_auth_error(&e) => {
            if let Some(new_token) = try_refresh_token(&state).await {
                match discovery_handlers::publish(&node, &url, &new_token, &key).await {
                    Ok(response) => return HttpResponse::Ok().json(response),
                    Err(e) => return handler_error_to_response(e),
                }
            }
            handler_error_to_response(e)
        }
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/search — Search the discovery network.
pub async fn search(
    req: HttpRequest,
    body: web::Json<discovery_handlers::SearchRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

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
        Err(e) if is_auth_error(&e) => {
            if let Some(new_token) = try_refresh_token(&state).await {
                match discovery_handlers::search(&body, &node, &url, &new_token, &key).await {
                    Ok(response) => return HttpResponse::Ok().json(response),
                    Err(e) => return handler_error_to_response(e),
                }
            }
            handler_error_to_response(e)
        }
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/connect — Send an E2E encrypted connection request.
pub async fn connect(
    req: HttpRequest,
    body: web::Json<discovery_handlers::ConnectRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

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
        Err(e) if is_auth_error(&e) => {
            if let Some(new_token) = try_refresh_token(&state).await {
                match discovery_handlers::connect(&body, &node, &url, &new_token, &key).await {
                    Ok(response) => return HttpResponse::Ok().json(response),
                    Err(e) => return handler_error_to_response(e),
                }
            }
            handler_error_to_response(e)
        }
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/connection-requests — Poll, decrypt, and list received connection requests.
pub async fn connection_requests(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

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
    let (_user_hash, node) = node_or_return!(state);

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

/// POST /api/discovery/connection-requests/check-network — Ask contacts if they know the requester.
pub async fn check_network(
    req: HttpRequest,
    body: web::Json<discovery_handlers::CheckNetworkRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::initiate_referral_query(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) if is_auth_error(&e) => {
            if let Some(new_token) = try_refresh_token(&state).await {
                match discovery_handlers::initiate_referral_query(
                    &body, &node, &url, &new_token, &key,
                )
                .await
                {
                    Ok(response) => return HttpResponse::Ok().json(response),
                    Err(e) => return handler_error_to_response(e),
                }
            }
            handler_error_to_response(e)
        }
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/sent-requests — List sent connection requests with status.
pub async fn sent_requests(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::list_sent_requests(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/requests — Legacy: Poll for incoming connection requests.
pub async fn poll_requests(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);

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
/// Retries once with a refreshed token on 401.
pub async fn browse_categories(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, _node) = node_or_return!(state);

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
        Err(e) if is_auth_error(&e) => {
            // Try refreshing the token and retrying once
            if let Some(new_token) = try_refresh_token(&state).await {
                match discovery_handlers::browse_categories(&url, &new_token, &key).await {
                    Ok(response) => return HttpResponse::Ok().json(response),
                    Err(e) => return handler_error_to_response(e),
                }
            }
            handler_error_to_response(e)
        }
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/interests — Get detected interest categories.
pub async fn get_interests(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

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
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::toggle_interest(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/interests/detect — Manually trigger interest detection.
pub async fn detect_interests(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::detect_interests(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/similar-profiles — Find users with similar interest fingerprints.
pub async fn similar_profiles(req: HttpRequest, state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

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
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::calendar_sharing_status(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/calendar-sharing/opt-in — Enable calendar sharing.
pub async fn calendar_sharing_opt_in(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::calendar_sharing_opt_in(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/calendar-sharing/opt-out — Disable calendar sharing.
pub async fn calendar_sharing_opt_out(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

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
    let (_user_hash, node) = node_or_return!(state);

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
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::store_peer_events(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/shared-events — Detect and return shared events with connections.
pub async fn get_shared_events(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::get_shared_events(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

// === Photo Moment Detection Routes ===

/// GET /api/discovery/moments/opt-ins — List all moment sharing opt-ins.
pub async fn moment_opt_in_list(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

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
    let (_user_hash, node) = node_or_return!(state);

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
    let (_user_hash, node) = node_or_return!(state);

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
    let (_user_hash, node) = node_or_return!(state);

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
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::moment_receive_hashes(&body, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/moments/detect — Detect shared moments from exchanged hashes.
pub async fn moment_detect(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::moment_detect(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/moments — List all detected shared moments.
pub async fn moment_list(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::moment_list(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

// === Face Discovery Routes ===

/// GET /api/discovery/faces/{schema}/{key} — List face embeddings for a record.
pub async fn list_faces(
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (schema, key) = path.into_inner();

    match discovery_handlers::list_faces(&node, &schema, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// POST /api/discovery/face-search — Search discovery network by face embedding.
pub async fn face_search(
    req: HttpRequest,
    body: web::Json<discovery_handlers::FaceSearchRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::face_search(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) if is_auth_error(&e) => {
            if let Some(new_token) = try_refresh_token(&state).await {
                match discovery_handlers::face_search(&body, &node, &url, &new_token, &key).await {
                    Ok(response) => return HttpResponse::Ok().json(response),
                    Err(e) => return handler_error_to_response(e),
                }
            }
            handler_error_to_response(e)
        }
        Err(e) => handler_error_to_response(e),
    }
}

// === Data Sharing Routes ===

/// POST /api/discovery/share — Send records to a contact via the encrypted bulletin board.
pub async fn share_data(
    req: HttpRequest,
    body: web::Json<discovery_handlers::DataShareRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    let (url, key) = match get_discovery_config() {
        Ok(c) => c,
        Err(response) => return response,
    };

    let auth_token = match get_auth_token(&req) {
        Ok(t) => t,
        Err(response) => return response,
    };

    match discovery_handlers::send_data_share(&body, &node, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) if is_auth_error(&e) => {
            if let Some(new_token) = try_refresh_token(&state).await {
                match discovery_handlers::send_data_share(&body, &node, &url, &new_token, &key)
                    .await
                {
                    Ok(response) => return HttpResponse::Ok().json(response),
                    Err(e) => return handler_error_to_response(e),
                }
            }
            handler_error_to_response(e)
        }
        Err(e) => handler_error_to_response(e),
    }
}

// === Notification Routes ===

/// GET /api/notifications — List all notifications.
pub async fn list_notifications(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::list_notifications(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/notifications/count — Lightweight notification count for polling.
pub async fn notification_count(state: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);

    match discovery_handlers::notification_count(&node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// DELETE /api/notifications/{id} — Dismiss a notification.
pub async fn dismiss_notification(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, node) = node_or_return!(state);
    let notification_id = path.into_inner();

    match discovery_handlers::dismiss_notification(&node, &notification_id).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::HandlerError;

    #[test]
    fn is_auth_error_matches_unauthorized_variant() {
        let err = HandlerError::Unauthorized("token expired".to_string());
        assert!(is_auth_error(&err));
    }

    #[test]
    fn is_auth_error_matches_401_in_internal_error() {
        let err = HandlerError::Internal(
            "Failed to search discovery network: Discovery search failed with status 401: Unauthorized".to_string(),
        );
        assert!(is_auth_error(&err));
    }

    #[test]
    fn is_auth_error_does_not_match_other_errors() {
        let err = HandlerError::Internal("network timeout".to_string());
        assert!(!is_auth_error(&err));
    }

    #[test]
    fn is_auth_error_does_not_match_not_found() {
        let err = HandlerError::NotFound("schema not found".to_string());
        assert!(!is_auth_error(&err));
    }
}
