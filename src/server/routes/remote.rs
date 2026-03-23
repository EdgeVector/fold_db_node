use crate::fold_node::OperationProcessor;
use crate::handlers::{ApiResponse, IntoHandlerError};
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};
use fold_db::access::AccessContext;
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
/// 2. Resolve trust distance from owner's trust graph
/// 3. Build AccessContext
/// 4. Execute query through standard path with access checks
/// 5. Return only authorized fields
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

            // 3. Resolve trust distance
            let trust_distance = op
                .resolve_trust_distance(&req.public_key)
                .await
                .handler_err("resolve trust")?;

            // 4. Build AccessContext
            let ctx = AccessContext {
                user_id: req.public_key.clone(),
                trust_distance,
                public_keys: vec![req.public_key.clone()],
                paid_schemas: Default::default(),
                clearance_level: 0,
            };

            // 5. Execute query with access control
            let db = op.get_db_public().await.handler_err("get database")?;
            let query = Query::new(req.schema_name.clone(), req.fields);
            let results = db
                .query_executor
                .query_with_access(query, &ctx, None)
                .await
                .handler_err("execute remote query")?;

            // 6. Log audit event
            let event = fold_db::access::AuditEvent::new(
                &req.public_key,
                fold_db::access::AuditAction::Read {
                    schema_name: req.schema_name,
                    fields: results.keys().cloned().collect(),
                },
                trust_distance,
                &fold_db::access::AccessDecision::Granted,
            );
            let _ = db.db_ops.append_audit_event(event).await;

            // Convert results to JSON
            let results_json =
                serde_json::to_value(&results).handler_err("serialize results")?;
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

            // List schemas that have at least one field with a non-owner access policy
            let schemas = db.schema_manager.get_schemas().handler_err("get schemas")?;
            let shared_schemas: Vec<String> = schemas
                .into_iter()
                .filter(|(_, schema)| {
                    use fold_db::schema::types::field::Field;
                    schema.runtime_fields.values().any(|fv| {
                        fv.common()
                            .access_policy
                            .as_ref()
                            .map(|p| p.trust_distance.read_max > 0)
                            .unwrap_or(false)
                    })
                })
                .map(|(name, _)| name)
                .collect();

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
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD;

    let public_key = fold_db::security::Ed25519PublicKey::from_base64(public_key_b64)
        .map_err(|e| format!("Invalid public key: {}", e))?;

    let signature_bytes = b64
        .decode(signature_b64)
        .map_err(|e| format!("Invalid signature base64: {}", e))?;

    public_key
        .verify_raw(payload.as_bytes(), &signature_bytes)
        .map_err(|e| format!("{}", e))
}
