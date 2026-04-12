use crate::fold_node::OperationProcessor;
use crate::handlers::{ApiResponse, IntoTypedHandlerError};
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};
use base64::Engine as _;
use serde::{Deserialize, Serialize};

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

/// GET /api/remote/node-info — unauthenticated node info endpoint
pub async fn node_info(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());

    handler_result_to_response(
        async {
            let public_key = op.get_node_public_key();
            let db = op.get_db_public().typed_handler_err()?;
            let node_id = db
                .get_node_id()
                .await
                .map_err(|e| crate::handlers::HandlerError::Internal(e.to_string()))?;

            // List schemas with descriptive names
            let all_schemas = db.schema_manager().get_schemas().typed_handler_err()?;
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

/// Validated contact info needed to send an async message.
struct ContactMessagingInfo {
    messaging_pseudonym: String,
    messaging_pk_bytes: [u8; 32],
    display_name: String,
}

/// Validate a contact for async messaging: load, check revocation, extract messaging keys.
fn validate_contact_for_messaging(
    contact_public_key: &str,
) -> Result<ContactMessagingInfo, crate::handlers::HandlerError> {
    use crate::trust::contact_book::ContactBook;

    let book = ContactBook::load().map_err(|e| {
        crate::handlers::HandlerError::Internal(format!("Failed to load contacts: {e}"))
    })?;
    let contact = book
        .get(contact_public_key)
        .ok_or_else(|| crate::handlers::HandlerError::BadRequest("Contact not found".into()))?;
    if contact.revoked {
        return Err(crate::handlers::HandlerError::BadRequest(
            "Contact has been revoked".into(),
        ));
    }
    let messaging_pseudonym = contact
        .messaging_pseudonym
        .as_ref()
        .ok_or_else(|| {
            crate::handlers::HandlerError::BadRequest(
                "Contact does not have messaging enabled. Connect via discovery first.".into(),
            )
        })?
        .clone();
    let messaging_public_key = contact.messaging_public_key.as_ref().ok_or_else(|| {
        crate::handlers::HandlerError::BadRequest(
            "Contact does not have a messaging public key".into(),
        )
    })?;

    let pk_bytes = base64::engine::general_purpose::STANDARD
        .decode(messaging_public_key)
        .map_err(|e| {
            crate::handlers::HandlerError::Internal(format!("Invalid messaging public key: {e}"))
        })?;
    if pk_bytes.len() != 32 {
        return Err(crate::handlers::HandlerError::Internal(
            "Messaging public key must be 32 bytes".into(),
        ));
    }
    let mut pk_arr = [0u8; 32];
    pk_arr.copy_from_slice(&pk_bytes);

    Ok(ContactMessagingInfo {
        messaging_pseudonym,
        messaging_pk_bytes: pk_arr,
        display_name: contact.display_name.to_string(),
    })
}

/// Sender identity derived from discovery config.
struct SenderInfo {
    pseudonym: uuid::Uuid,
    reply_public_key: String,
    discovery_url: String,
    master_key: Vec<u8>,
    auth_token: String,
}

impl SenderInfo {
    fn pseudonym_str(&self) -> String {
        self.pseudonym.to_string()
    }
}

/// Resolve discovery config and derive sender identity (pseudonym + reply key).
/// Callers use this to build the message payload, then call `send_encrypted_message`.
async fn resolve_sender_info(
    node: &crate::fold_node::node::FoldNode,
) -> Result<SenderInfo, crate::handlers::HandlerError> {
    use crate::discovery::connection;

    let (discovery_url, master_key, auth_token) =
        crate::server::routes::discovery::resolve_discovery_config(node, None).await?;

    let hash = crate::discovery::pseudonym::content_hash("connection-sender");
    let our_pseudonym = crate::discovery::pseudonym::derive_pseudonym(&master_key, &hash);
    let reply_public_key = connection::get_pseudonym_public_key_b64(&master_key, &our_pseudonym);

    Ok(SenderInfo {
        pseudonym: our_pseudonym,
        reply_public_key,
        discovery_url,
        master_key,
        auth_token,
    })
}

/// Encrypt a payload and send it to a contact via the messaging service bulletin board.
async fn send_encrypted_message<P: serde::Serialize>(
    sender: &SenderInfo,
    contact: &ContactMessagingInfo,
    payload: &P,
) -> Result<(), crate::handlers::HandlerError> {
    use crate::discovery::connection;

    let encrypted = connection::encrypt_message(&contact.messaging_pk_bytes, payload)
        .map_err(|e| crate::handlers::HandlerError::Internal(format!("Encryption failed: {e}")))?;
    let encrypted_b64 = base64::engine::general_purpose::STANDARD.encode(&encrypted);

    let target: uuid::Uuid = contact.messaging_pseudonym.parse().map_err(|_| {
        crate::handlers::HandlerError::Internal("Invalid messaging pseudonym UUID".into())
    })?;
    let publisher = crate::discovery::publisher::DiscoveryPublisher::new(
        sender.master_key.clone(),
        sender.discovery_url.clone(),
        sender.auth_token.clone(),
    );
    publisher
        .connect(target, encrypted_b64, Some(sender.pseudonym))
        .await
        .map_err(|e| {
            crate::handlers::HandlerError::Internal(format!("Failed to send message: {e}"))
        })?;

    Ok(())
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

            let contact = validate_contact_for_messaging(&req.contact_public_key)?;
            let sender = resolve_sender_info(&node).await?;

            let request_id = uuid::Uuid::new_v4().to_string();
            let public_key = node.get_node_public_key();

            let payload = QueryRequestPayload {
                message_type: "query_request".to_string(),
                request_id: request_id.clone(),
                schema_name: req.schema_name.clone(),
                fields: req.fields.clone(),
                sender_public_key: public_key.to_string(),
                sender_pseudonym: sender.pseudonym_str(),
                reply_public_key: sender.reply_public_key.clone(),
            };

            send_encrypted_message(&sender, &contact, &payload).await?;

            let db = node.get_fold_db().map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Failed to access database: {e}"))
            })?;
            let store = db.get_db_ops().raw_metadata_store();
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

            let contact = validate_contact_for_messaging(&req.contact_public_key)?;
            let sender = resolve_sender_info(&node).await?;

            let request_id = uuid::Uuid::new_v4().to_string();
            let public_key = node.get_node_public_key();

            let payload = SchemaListRequestPayload {
                message_type: "schema_list_request".to_string(),
                request_id: request_id.clone(),
                sender_public_key: public_key.to_string(),
                sender_pseudonym: sender.pseudonym_str(),
                reply_public_key: sender.reply_public_key.clone(),
            };

            send_encrypted_message(&sender, &contact, &payload).await?;

            let db = node.get_fold_db().map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Failed to access database: {e}"))
            })?;
            let store = db.get_db_ops().raw_metadata_store();
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
            let db = node.get_fold_db().map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Failed to access database: {e}"))
            })?;
            let store = db.get_db_ops().raw_metadata_store();

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
            let db = node.get_fold_db().map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Failed to access database: {e}"))
            })?;
            let store = db.get_db_ops().raw_metadata_store();

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
            let db = node.get_fold_db().map_err(|e| {
                crate::handlers::HandlerError::Internal(format!("Failed to access database: {e}"))
            })?;
            let store = db.get_db_ops().raw_metadata_store();

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
