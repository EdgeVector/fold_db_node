use crate::fold_node::OperationProcessor;
use crate::handlers::{ApiResponse, IntoHandlerError};
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};
use fold_db::schema::types::operations::Query;
use serde::{Deserialize, Serialize};

use base64::Engine as _;

/// Request for a remote (node-to-node) query.
/// The caller signs the payload with their Ed25519 private key.
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// Schema info for remote browsing.
#[derive(Debug, Clone, Serialize)]
pub struct SharedSchemaInfo {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptive_name: Option<String>,
}

/// Node info response (unauthenticated).
#[derive(Debug, Clone, Serialize)]
pub struct NodeInfoResponse {
    pub public_key: String,
    pub node_id: String,
    /// Schema names (for backwards compat)
    pub shared_schemas: Vec<String>,
    /// Schemas with descriptive names
    pub schemas: Vec<SharedSchemaInfo>,
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

            // 3. Execute query with access control (domain-aware)
            let query = Query::new(req.schema_name.clone(), req.fields);
            let results = op
                .execute_query_json_with_access(query, &req.public_key)
                .await
                .handler_err("execute remote query")?;

            Ok(ApiResponse::success_with_user(
                serde_json::json!({"results": results}),
                user_hash,
            ))
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

            // List schemas with descriptive names
            let all_schemas = db.schema_manager.get_schemas().handler_err("get schemas")?;
            let shared_schemas: Vec<String> = all_schemas.keys().cloned().collect();
            let schemas: Vec<SharedSchemaInfo> = all_schemas
                .values()
                .map(|s| SharedSchemaInfo {
                    name: s.name.clone(),
                    descriptive_name: s.descriptive_name.clone(),
                })
                .collect();

            Ok(ApiResponse::success_with_user(
                NodeInfoResponse {
                    public_key,
                    node_id,
                    shared_schemas,
                    schemas,
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

// ===== Proxy endpoints (local node signs and forwards to remote) =====

/// Request to query a remote node through the local node as proxy.
#[derive(Debug, Clone, Deserialize)]
pub struct ProxyQueryRequest {
    /// URL of the remote node (e.g., "http://192.168.1.10:9001")
    pub remote_url: String,
    /// Schema name to query on the remote node
    pub schema_name: String,
    /// Fields to return
    pub fields: Vec<String>,
}

/// Request to browse a remote node's available schemas.
#[derive(Debug, Clone, Deserialize)]
pub struct BrowseRemoteRequest {
    /// URL of the remote node
    pub remote_url: String,
}

/// POST /api/remote/proxy-query — query a remote node (local node signs the request)
pub async fn proxy_query(
    body: web::Json<ProxyQueryRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let req = body.into_inner();

    handler_result_to_response(
        async {
            let private_key = node.get_node_private_key();
            let public_key = node.get_node_public_key();

            // Build the payload to sign
            let timestamp = chrono::Utc::now().timestamp();
            let payload = format!("{}:{}:{}", req.schema_name, req.fields.join(","), timestamp);

            // Sign with node's Ed25519 key
            let secret_bytes = base64::engine::general_purpose::STANDARD
                .decode(private_key)
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!("Invalid private key: {e}"))
                })?;
            let keypair = fold_db::security::Ed25519KeyPair::from_secret_key(&secret_bytes)
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!("Failed to load keypair: {e}"))
                })?;
            let signature = keypair.sign(payload.as_bytes());
            let sig_b64 = fold_db::security::KeyUtils::signature_to_base64(&signature);

            // Build the remote request
            let remote_req = RemoteQueryRequest {
                schema_name: req.schema_name,
                fields: req.fields,
                signature: sig_b64,
                public_key: public_key.to_string(),
                timestamp,
            };

            // First, get the remote node's user_hash so we access the right data
            let client = reqwest::Client::new();
            let base = req.remote_url.trim_end_matches('/');
            let identity_resp = client
                .get(format!("{}/api/system/auto-identity", base))
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!(
                        "Failed to get remote identity: {e}"
                    ))
                })?;
            let identity: serde_json::Value = identity_resp.json().await.map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Invalid identity response: {e}"))
            })?;
            let remote_hash = identity
                .get("user_hash")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            // Forward to remote node with the correct user hash
            let url = format!("{}/api/remote/query", base);
            let resp = client
                .post(&url)
                .header("X-User-Hash", &remote_hash)
                .json(&remote_req)
                .timeout(std::time::Duration::from_secs(30))
                .send()
                .await
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!(
                        "Failed to reach remote node: {e}"
                    ))
                })?;

            let status = resp.status();
            let body: serde_json::Value = resp.json().await.map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Invalid response: {e}"))
            })?;

            if !status.is_success() {
                let err = body
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Unknown error");
                return Err(crate::handlers::HandlerError::Internal(format!(
                    "Remote node returned {}: {}",
                    status, err
                )));
            }

            Ok(ApiResponse::success_with_user(body, user_hash))
        }
        .await,
    )
}

/// POST /api/remote/browse — browse a remote node's available schemas
pub async fn browse_remote(
    body: web::Json<BrowseRemoteRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, _node) = node_or_return!(state);
    let req = body.into_inner();

    handler_result_to_response(
        async {
            let client = reqwest::Client::new();
            let url = format!(
                "{}/api/remote/node-info",
                req.remote_url.trim_end_matches('/')
            );
            // Get the remote node's user_hash first
            let identity_resp = client
                .get(format!(
                    "{}/api/system/auto-identity",
                    req.remote_url.trim_end_matches('/')
                ))
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!(
                        "Failed to reach remote node: {e}"
                    ))
                })?;
            let identity: serde_json::Value = identity_resp.json().await.unwrap_or_default();
            let remote_hash = identity
                .get("user_hash")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            let resp = client
                .get(&url)
                .header("X-User-Hash", remote_hash)
                .timeout(std::time::Duration::from_secs(10))
                .send()
                .await
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!(
                        "Failed to reach remote node: {e}"
                    ))
                })?;

            let body: serde_json::Value = resp.json().await.map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Invalid response: {e}"))
            })?;

            Ok(ApiResponse::success_with_user(body, user_hash))
        }
        .await,
    )
}
