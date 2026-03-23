use crate::fold_node::OperationProcessor;
use crate::handlers::{ApiResponse, IntoHandlerError};
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};
use fold_db::access::{CapabilityConstraint, CapabilityKind, FieldAccessPolicy, PaymentGate};
use serde::{Deserialize, Serialize};

// ===== Request/Response types =====

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustGrantRequest {
    pub public_key: String,
    pub distance: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustOverrideRequest {
    pub public_key: String,
    pub distance: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustGrantsResponse {
    pub grants: Vec<TrustGrantEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustGrantEntry {
    pub public_key: String,
    pub distance: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrustResolveResponse {
    pub public_key: String,
    pub distance: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetFieldPolicyRequest {
    pub policy: FieldAccessPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldPolicyResponse {
    pub schema_name: String,
    pub field_name: String,
    pub policy: Option<FieldAccessPolicy>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLogResponse {
    pub events: serde_json::Value,
    pub count: usize,
}

// ===== Trust management endpoints =====

/// POST /api/trust/grant — assign trust to a public key at a distance
pub async fn grant_trust(
    body: web::Json<TrustGrantRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.grant_trust(&body.public_key, body.distance)
                .await
                .handler_err("grant trust")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"granted": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// DELETE /api/trust/revoke/{key} — revoke trust for a public key
pub async fn revoke_trust(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let public_key = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.revoke_trust(&public_key)
                .await
                .handler_err("revoke trust")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"revoked": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/trust/grants — list all trust assignments
pub async fn list_trust_grants(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let grants = op.list_trust_grants().await.handler_err("list grants")?;
            let entries: Vec<TrustGrantEntry> = grants
                .into_iter()
                .map(|(public_key, distance)| TrustGrantEntry {
                    public_key,
                    distance,
                })
                .collect();
            Ok(ApiResponse::success_with_user(
                TrustGrantsResponse { grants: entries },
                user_hash,
            ))
        }
        .await,
    )
}

/// PUT /api/trust/override — set explicit distance override
pub async fn set_trust_override(
    body: web::Json<TrustOverrideRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.set_trust_override(&body.public_key, body.distance)
                .await
                .handler_err("set override")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"override_set": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/trust/resolve/{key} — check resolved distance for a key
pub async fn resolve_trust(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let public_key = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let distance = op
                .resolve_trust_distance(&public_key)
                .await
                .handler_err("resolve trust")?;
            Ok(ApiResponse::success_with_user(
                TrustResolveResponse {
                    public_key,
                    distance,
                },
                user_hash,
            ))
        }
        .await,
    )
}

// ===== Schema policy endpoints =====

/// PUT /api/schema/{name}/field/{field}/policy — set field access policy
pub async fn set_field_policy(
    path: web::Path<(String, String)>,
    body: web::Json<SetFieldPolicyRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (schema_name, field_name) = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.set_field_access_policy(&schema_name, &field_name, body.into_inner().policy)
                .await
                .handler_err("set field policy")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"policy_set": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/schema/{name}/field/{field}/policy — get field access policy
pub async fn get_field_policy(
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (schema_name, field_name) = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let policy = op
                .get_field_access_policy(&schema_name, &field_name)
                .await
                .handler_err("get field policy")?;
            Ok(ApiResponse::success_with_user(
                FieldPolicyResponse {
                    schema_name,
                    field_name,
                    policy,
                },
                user_hash,
            ))
        }
        .await,
    )
}

// ===== Audit log endpoint =====

/// GET /api/trust/audit?limit=100 — get recent audit events
pub async fn get_audit_log(
    query: web::Query<AuditLogQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    let limit = query.limit.unwrap_or(100);
    handler_result_to_response(
        async {
            let log = op.get_audit_log(limit).await.handler_err("get audit log")?;
            let count = log.total_events();
            let events_json =
                serde_json::to_value(log.events()).handler_err("serialize audit log")?;
            Ok(ApiResponse::success_with_user(
                AuditLogResponse {
                    events: events_json,
                    count,
                },
                user_hash,
            ))
        }
        .await,
    )
}

#[derive(Debug, Deserialize)]
pub struct AuditLogQuery {
    pub limit: Option<usize>,
}

// ===== Capability token endpoints =====

#[derive(Debug, Clone, Deserialize)]
pub struct IssueCapabilityRequest {
    pub schema_name: String,
    pub field_name: String,
    pub public_key: String,
    pub kind: CapabilityKind,
    pub quota: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RevokeCapabilityRequest {
    pub schema_name: String,
    pub field_name: String,
    pub public_key: String,
    pub kind: CapabilityKind,
}

/// POST /api/capabilities/issue — issue a capability token
pub async fn issue_capability(
    body: web::Json<IssueCapabilityRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    let req = body.into_inner();
    handler_result_to_response(
        async {
            let constraint =
                CapabilityConstraint::new(req.public_key, req.kind, req.quota);
            op.issue_capability(&req.schema_name, &req.field_name, constraint)
                .await
                .handler_err("issue capability")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"issued": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// DELETE /api/capabilities/revoke — revoke a capability token
pub async fn revoke_capability(
    body: web::Json<RevokeCapabilityRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    let req = body.into_inner();
    handler_result_to_response(
        async {
            op.revoke_capability(&req.schema_name, &req.field_name, &req.public_key, req.kind)
                .await
                .handler_err("revoke capability")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"revoked": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/capabilities/list/{schema}/{field} — list capabilities for a field
pub async fn list_capabilities(
    path: web::Path<(String, String)>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (schema_name, field_name) = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let caps = op
                .list_capabilities(&schema_name, &field_name)
                .await
                .handler_err("list capabilities")?;
            let caps_json =
                serde_json::to_value(&caps).handler_err("serialize capabilities")?;
            Ok(ApiResponse::success_with_user(caps_json, user_hash))
        }
        .await,
    )
}

// ===== Payment gate endpoints =====

#[derive(Debug, Clone, Deserialize)]
pub struct SetPaymentGateRequest {
    pub gate: PaymentGate,
}

/// PUT /api/schema/{name}/payment-gate — set payment gate
pub async fn set_payment_gate(
    path: web::Path<String>,
    body: web::Json<SetPaymentGateRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let schema_name = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.set_payment_gate(&schema_name, body.into_inner().gate)
                .await
                .handler_err("set payment gate")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"payment_gate_set": true}),
                user_hash,
            ))
        }
        .await,
    )
}

/// GET /api/schema/{name}/payment-gate — get payment gate
pub async fn get_payment_gate(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let schema_name = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let gate = op
                .get_payment_gate(&schema_name)
                .await
                .handler_err("get payment gate")?;
            Ok(ApiResponse::success_with_user(
                serde_json::json!({"schema_name": schema_name, "payment_gate": gate}),
                user_hash,
            ))
        }
        .await,
    )
}
