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

/// POST /api/discovery/connect — Send a connection request.
pub async fn connect(
    req: HttpRequest,
    body: web::Json<discovery_handlers::ConnectRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (_user_hash, _node) = match require_node_read(&state).await {
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

    match discovery_handlers::connect(&body, &url, &auth_token, &key).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => handler_error_to_response(e),
    }
}

/// GET /api/discovery/requests — Poll for incoming connection requests.
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
