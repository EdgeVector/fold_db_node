use crate::fold_node::OperationProcessor;
use crate::handlers::{ApiResponse, IntoTypedHandlerError};
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
                .typed_handler_err()?;

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
            let db = op.get_db_public().await.typed_handler_err()?;
            let node_id = db
                .get_node_id()
                .await
                .map_err(|e| crate::handlers::HandlerError::Internal(e.to_string()))?;

            // List schemas with descriptive names
            let all_schemas = db.schema_manager.get_schemas().typed_handler_err()?;
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

// ===== Async query endpoints (via messaging service bulletin board) =====

/// Request to submit an async query to a contact.
#[derive(Debug, Clone, Deserialize)]
pub struct AsyncQueryRequest {
    /// Ed25519 public key of the contact to query
    pub contact_public_key: String,
    /// Schema name to query
    pub schema_name: String,
    /// Fields to return (empty = all authorized)
    #[serde(default)]
    pub fields: Vec<String>,
}

/// Request to browse a contact's schemas asynchronously.
#[derive(Debug, Clone, Deserialize)]
pub struct AsyncBrowseRequest {
    /// Ed25519 public key of the contact
    pub contact_public_key: String,
}

/// POST /api/remote/async-query — submit an async query to a contact via messaging
pub async fn async_query(
    body: web::Json<AsyncQueryRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let req = body.into_inner();

    handler_result_to_response(
        async {
            use crate::discovery::async_query::{LocalAsyncQuery, QueryRequestPayload};
            use crate::discovery::connection;
            use crate::trust::contact_book::ContactBook;

            // Load contact and verify messaging info
            let book = ContactBook::load().map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Failed to load contacts: {e}"))
            })?;
            let contact = book.get(&req.contact_public_key).ok_or_else(|| {
                crate::handlers::HandlerError::BadRequest("Contact not found".into())
            })?;
            if contact.revoked {
                return Err(crate::handlers::HandlerError::BadRequest(
                    "Contact has been revoked".into(),
                ));
            }
            let messaging_pseudonym = contact.messaging_pseudonym.as_ref().ok_or_else(|| {
                crate::handlers::HandlerError::BadRequest(
                    "Contact does not have messaging enabled. Connect via discovery first.".into(),
                )
            })?;
            let messaging_public_key = contact.messaging_public_key.as_ref().ok_or_else(|| {
                crate::handlers::HandlerError::BadRequest(
                    "Contact does not have a messaging public key".into(),
                )
            })?;

            // Get discovery config for publisher
            let (discovery_url, master_key, auth_token) =
                crate::server::routes::discovery::resolve_discovery_config(&node, None).await?;

            // Derive our sender pseudonym + X25519 key
            let hash = crate::discovery::pseudonym::content_hash("connection-sender");
            let our_pseudonym = crate::discovery::pseudonym::derive_pseudonym(&master_key, &hash);
            let our_reply_pk =
                connection::get_pseudonym_public_key_b64(&master_key, &our_pseudonym);

            let request_id = uuid::Uuid::new_v4().to_string();
            let public_key = node.get_node_public_key();

            let payload = QueryRequestPayload {
                message_type: "query_request".to_string(),
                request_id: request_id.clone(),
                schema_name: req.schema_name.clone(),
                fields: req.fields.clone(),
                sender_public_key: public_key.to_string(),
                sender_pseudonym: our_pseudonym.to_string(),
                reply_public_key: our_reply_pk,
            };

            // Encrypt with contact's X25519 messaging key
            let pk_bytes = base64::engine::general_purpose::STANDARD
                .decode(messaging_public_key)
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!(
                        "Invalid messaging public key: {e}"
                    ))
                })?;
            if pk_bytes.len() != 32 {
                return Err(crate::handlers::HandlerError::Internal(
                    "Messaging public key must be 32 bytes".into(),
                ));
            }
            let mut pk_arr = [0u8; 32];
            pk_arr.copy_from_slice(&pk_bytes);

            let encrypted = connection::encrypt_message(&pk_arr, &payload).map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Encryption failed: {e}"))
            })?;
            let encrypted_b64 = base64::engine::general_purpose::STANDARD.encode(&encrypted);

            // Send via messaging service
            let target: uuid::Uuid = messaging_pseudonym.parse().map_err(|_| {
                crate::handlers::HandlerError::Internal("Invalid messaging pseudonym UUID".into())
            })?;
            let publisher = crate::discovery::publisher::DiscoveryPublisher::new(
                master_key,
                discovery_url,
                auth_token,
            );
            publisher
                .connect(target, encrypted_b64, Some(our_pseudonym))
                .await
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!("Failed to send query: {e}"))
                })?;

            // Save locally
            let db = node.get_fold_db().await.map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Failed to access database: {e}"))
            })?;
            let store = db.get_db_ops().metadata_store().inner().clone();
            let local_query = LocalAsyncQuery {
                request_id: request_id.clone(),
                contact_public_key: req.contact_public_key,
                contact_display_name: contact.display_name.clone(),
                schema_name: Some(req.schema_name),
                fields: req.fields,
                query_type: "query".to_string(),
                status: "pending".to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                completed_at: None,
                results: None,
                error: None,
            };
            crate::discovery::async_query::save_async_query(&*store, &local_query)
                .await
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!("Failed to save query: {e}"))
                })?;

            Ok(ApiResponse::success_with_user(
                serde_json::json!({"request_id": request_id}),
                user_hash,
            ))
        }
        .await,
    )
}

/// POST /api/remote/async-browse — request schema list from a contact via messaging
pub async fn async_browse(
    body: web::Json<AsyncBrowseRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let req = body.into_inner();

    handler_result_to_response(
        async {
            use crate::discovery::async_query::{LocalAsyncQuery, SchemaListRequestPayload};
            use crate::discovery::connection;
            use crate::trust::contact_book::ContactBook;

            let book = ContactBook::load().map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Failed to load contacts: {e}"))
            })?;
            let contact = book.get(&req.contact_public_key).ok_or_else(|| {
                crate::handlers::HandlerError::BadRequest("Contact not found".into())
            })?;
            if contact.revoked {
                return Err(crate::handlers::HandlerError::BadRequest(
                    "Contact has been revoked".into(),
                ));
            }
            let messaging_pseudonym = contact.messaging_pseudonym.as_ref().ok_or_else(|| {
                crate::handlers::HandlerError::BadRequest(
                    "Contact does not have messaging enabled".into(),
                )
            })?;
            let messaging_public_key = contact.messaging_public_key.as_ref().ok_or_else(|| {
                crate::handlers::HandlerError::BadRequest(
                    "Contact does not have a messaging public key".into(),
                )
            })?;

            let (discovery_url, master_key, auth_token) =
                crate::server::routes::discovery::resolve_discovery_config(&node, None).await?;

            let hash = crate::discovery::pseudonym::content_hash("connection-sender");
            let our_pseudonym = crate::discovery::pseudonym::derive_pseudonym(&master_key, &hash);
            let our_reply_pk =
                connection::get_pseudonym_public_key_b64(&master_key, &our_pseudonym);

            let request_id = uuid::Uuid::new_v4().to_string();
            let public_key = node.get_node_public_key();

            let payload = SchemaListRequestPayload {
                message_type: "schema_list_request".to_string(),
                request_id: request_id.clone(),
                sender_public_key: public_key.to_string(),
                sender_pseudonym: our_pseudonym.to_string(),
                reply_public_key: our_reply_pk,
            };

            let pk_bytes = base64::engine::general_purpose::STANDARD
                .decode(messaging_public_key)
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!(
                        "Invalid messaging public key: {e}"
                    ))
                })?;
            if pk_bytes.len() != 32 {
                return Err(crate::handlers::HandlerError::Internal(
                    "Messaging public key must be 32 bytes".into(),
                ));
            }
            let mut pk_arr = [0u8; 32];
            pk_arr.copy_from_slice(&pk_bytes);

            let encrypted = connection::encrypt_message(&pk_arr, &payload).map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Encryption failed: {e}"))
            })?;
            let encrypted_b64 = base64::engine::general_purpose::STANDARD.encode(&encrypted);

            let target: uuid::Uuid = messaging_pseudonym.parse().map_err(|_| {
                crate::handlers::HandlerError::Internal("Invalid messaging pseudonym UUID".into())
            })?;
            let publisher = crate::discovery::publisher::DiscoveryPublisher::new(
                master_key,
                discovery_url,
                auth_token,
            );
            publisher
                .connect(target, encrypted_b64, Some(our_pseudonym))
                .await
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!(
                        "Failed to send schema list request: {e}"
                    ))
                })?;

            let db = node.get_fold_db().await.map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Failed to access database: {e}"))
            })?;
            let store = db.get_db_ops().metadata_store().inner().clone();
            let local_query = LocalAsyncQuery {
                request_id: request_id.clone(),
                contact_public_key: req.contact_public_key,
                contact_display_name: contact.display_name.clone(),
                schema_name: None,
                fields: vec![],
                query_type: "schema_list".to_string(),
                status: "pending".to_string(),
                created_at: chrono::Utc::now().to_rfc3339(),
                completed_at: None,
                results: None,
                error: None,
            };
            crate::discovery::async_query::save_async_query(&*store, &local_query)
                .await
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!("Failed to save query: {e}"))
                })?;

            Ok(ApiResponse::success_with_user(
                serde_json::json!({"request_id": request_id}),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/remote/async-queries — list all async queries
pub async fn list_async_queries(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(
        async {
            let db = node.get_fold_db().await.map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Failed to access database: {e}"))
            })?;
            let store = db.get_db_ops().metadata_store().inner().clone();

            let queries = crate::discovery::async_query::list_async_queries(&*store)
                .await
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!("Failed to list queries: {e}"))
                })?;

            Ok(ApiResponse::success_with_user(
                serde_json::json!({"queries": queries}),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/remote/async-query/{id} — get a specific async query
pub async fn get_async_query(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let request_id = path.into_inner();

    handler_result_to_response(
        async {
            let db = node.get_fold_db().await.map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Failed to access database: {e}"))
            })?;
            let store = db.get_db_ops().metadata_store().inner().clone();

            let query = crate::discovery::async_query::get_async_query(&*store, &request_id)
                .await
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!("Failed to get query: {e}"))
                })?
                .ok_or_else(|| crate::handlers::HandlerError::NotFound("Query not found".into()))?;

            Ok(ApiResponse::success_with_user(query, user_hash))
        }
        .await,
    )
}

/// DELETE /api/remote/async-query/{id} — delete an async query
pub async fn delete_async_query(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let request_id = path.into_inner();

    handler_result_to_response(
        async {
            let db = node.get_fold_db().await.map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Failed to access database: {e}"))
            })?;
            let store = db.get_db_ops().metadata_store().inner().clone();

            crate::discovery::async_query::delete_async_query(&*store, &request_id)
                .await
                .map_err(|e| {
                    crate::handlers::HandlerError::Internal(format!("Failed to delete query: {e}"))
                })?;

            Ok(ApiResponse::success_with_user(
                serde_json::json!({"ok": true}),
                user_hash,
            ))
        }
        .await,
    )
}
