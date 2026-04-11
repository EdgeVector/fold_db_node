//! Security-related HTTP routes for key management and authentication

use crate::server::http_server::AppState;
use crate::server::routes::node_or_return;
use actix_web::{web, HttpResponse, Responder};
use serde_json::json;
// OpenAPI annotations are attached via #[utoipa::path(...)] on handlers

#[utoipa::path(
    get,
    path = "/api/security/system-key",
    tag = "security",
    responses((status = 200, description = "System key"), (status = 404, description = "Not found"))
)]
pub async fn get_system_public_key(data: web::Data<AppState>) -> impl Responder {
    let (_user_hash, node) = node_or_return!(data);
    let security_manager = node.get_security_manager().clone();

    match security_manager.get_system_public_key() {
        Ok(Some(key_info)) => HttpResponse::Ok().json(json!({
            "success": true,
            "key": key_info
        })),
        Ok(None) => HttpResponse::NotFound().json(json!({
            "success": false,
            "error": "System key not found"
        })),
        Err(e) => HttpResponse::InternalServerError().json(json!({
            "success": false,
            "error": e.to_string()
        })),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn minimal() {
        // Intentionally empty: compile-time smoke test for this module
    }
}
