use crate::fold_node::OperationProcessor;
use crate::handlers::{ApiResponse, IntoHandlerError};
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};
use fold_db::schema::types::operations::Query;
use serde::{Deserialize, Serialize};

/// Request for a remote (node-to-node) query.
/// The caller signs the payload with their Ed25519 private key.
#[derive(Debug, Clone, Deserialize)]
pub struct RemoteQueryRequest {
    /// Schema or view name to query
    pub schema_name: String,
    /// Fields to return (empty = all authorized fields)
    pub fields: Vec<String>,
    /// Ed25519 signature over the payload (base64)
    pub signature: String,
    /// Caller's base64-encoded Ed25519 public key
    pub public_key: String,
    /// Unix timestamp (for replay protection)
    pub timestamp: i64,
}

/// Node info response (unauthenticated).
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfoResponse {
    pub public_key: String,
    pub node_id: String,
    pub shared_schemas: Vec<String>,
}

/// POST /api/remote/query — execute a query from a remote node
///
/// Flow:
/// 1. Verify Ed25519 signature (replay protection via timestamp)
/// 2. Execute query through standard path
/// 3. Return results
pub async fn remote_query(
    body: web::Json<RemoteQueryRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    let req = body.into_inner();

    handler_result_to_response(
        async {
            // 1. Replay protection: reject timestamps > 60 seconds old
            let now = chrono::Utc::now().timestamp();
            let drift = (now - req.timestamp).abs();
            if drift > 60 {
                return Err(crate::handlers::HandlerError::BadRequest(
                    "Request timestamp too far from current time (max 60s drift)".into(),
                ));
            }

            // 2. Verify signature over the canonical payload
            let payload = format!(
                "{}:{}:{}",
                req.schema_name,
                req.fields.join(","),
                req.timestamp
            );
            verify_ed25519_signature(&payload, &req.signature, &req.public_key)
                .map_err(crate::handlers::HandlerError::Unauthorized)?;

            // 3. Execute query
            let db = op.get_db_public().await.handler_err("get database")?;
            let query = Query::new(req.schema_name.clone(), req.fields);
            let results = db
                .query_executor
                .query(query)
                .await
                .handler_err("execute remote query")?;

            // Convert results to JSON
            let results_json = serde_json::to_value(&results).handler_err("serialize results")?;
            Ok(ApiResponse::success_with_user(results_json, user_hash))
        }
        .await,
    )
}

/// GET /api/remote/node-info — unauthenticated node info endpoint
pub async fn node_info(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());

    handler_result_to_response(
        async {
            let public_key = op.get_node_public_key();
            let db = op.get_db_public().await.handler_err("get database")?;
            let node_id = db
                .get_node_id()
                .await
                .map_err(|e| crate::handlers::HandlerError::Internal(e.to_string()))?;

            // List all schema names (access control removed from fold_db)
            let schemas = db.schema_manager.get_schemas().handler_err("get schemas")?;
            let shared_schemas: Vec<String> = schemas.into_keys().collect();

            Ok(ApiResponse::success_with_user(
                NodeInfoResponse {
                    public_key,
                    node_id,
                    shared_schemas,
                },
                user_hash,
            ))
        }
        .await,
    )
}

/// Verify an Ed25519 signature using fold_db's security module.
fn verify_ed25519_signature(
    payload: &str,
    signature_b64: &str,
    public_key_b64: &str,
) -> Result<(), String> {
    let public_key = fold_db::security::Ed25519PublicKey::from_base64(public_key_b64)
        .map_err(|e| format!("Invalid public key: {}", e))?;

    let signature = fold_db::security::KeyUtils::signature_from_base64(signature_b64)
        .map_err(|e| format!("Invalid signature: {}", e))?;

    if public_key.verify(payload.as_bytes(), &signature) {
        Ok(())
    } else {
        Err("Signature verification failed".to_string())
    }
}
