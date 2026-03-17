use crate::fold_node::OperationProcessor;
use crate::handlers::{ApiResponse, HandlerError, IntoHandlerError};
use crate::server::http_server::AppState;
use crate::server::routes::{handler_result_to_response, node_or_return};
use actix_web::{web, Responder};
use base64::Engine;
use fold_db::schema::types::key_config::KeyConfig;
use fold_db::schema::types::schema::DeclarativeSchemaType as SchemaType;
use fold_db::view::types::{FieldRef, TransformFieldDef, TransformView};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewListResponse {
    pub views: serde_json::Value,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewResponse {
    pub view: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewApproveResponse {
    pub approved: bool,
}

/// JSON format for creating a view via HTTP.
#[derive(Debug, Clone, Deserialize)]
pub struct CreateViewRequest {
    pub name: String,
    /// Schema type: "Single", "Hash", "Range", or "HashRange"
    pub schema_type: SchemaType,
    /// Key configuration — which fields serve as hash/range keys
    #[serde(default)]
    pub key: Option<KeyConfig>,
    pub fields: HashMap<String, CreateViewFieldRequest>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateViewFieldRequest {
    /// Source in "Schema.field" format
    pub source: String,
    /// Base64-encoded forward WASM bytes
    pub wasm_forward: Option<String>,
    /// Base64-encoded inverse WASM bytes
    pub wasm_inverse: Option<String>,
}

impl CreateViewRequest {
    fn into_transform_view(self) -> Result<TransformView, String> {
        let b64_engine = base64::engine::general_purpose::STANDARD;
        let mut fields = HashMap::new();
        for (field_name, field_req) in self.fields {
            let source = FieldRef::try_from(field_req.source)?;
            let wasm_forward = field_req
                .wasm_forward
                .map(|b64| b64_engine.decode(&b64))
                .transpose()
                .map_err(|e| format!("Invalid base64 for wasm_forward: {}", e))?;
            let wasm_inverse = field_req
                .wasm_inverse
                .map(|b64| b64_engine.decode(&b64))
                .transpose()
                .map_err(|e| format!("Invalid base64 for wasm_inverse: {}", e))?;
            fields.insert(
                field_name,
                TransformFieldDef {
                    source,
                    wasm_forward,
                    wasm_inverse,
                },
            );
        }
        Ok(TransformView::new(self.name, self.schema_type, self.key, fields))
    }
}

/// List all views with states.
pub async fn list_views(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let views = op.list_views().await.handler_err("list views")?;
            let count = views.len();
            let views_json =
                serde_json::to_value(&views).handler_err("serialize views")?;
            Ok(ApiResponse::success_with_user(
                ViewListResponse {
                    views: views_json,
                    count,
                },
                user_hash,
            ))
        }
        .await,
    )
}

/// Get a view by name.
pub async fn get_view(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let name = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let view = op
                .get_view(&name)
                .await
                .handler_err("get view")?
                .ok_or_else(|| HandlerError::NotFound(format!("View not found: {}", name)))?;
            let view_json =
                serde_json::to_value(&view).handler_err("serialize view")?;
            Ok(ApiResponse::success_with_user(
                ViewResponse { view: view_json },
                user_hash,
            ))
        }
        .await,
    )
}

/// Create (register) a new view.
pub async fn create_view(
    body: web::Json<CreateViewRequest>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            let view = body
                .into_inner()
                .into_transform_view()
                .map_err(HandlerError::BadRequest)?;
            op.create_view(view).await.handler_err("create view")?;
            Ok(ApiResponse::success_with_user(
                crate::handlers::response::SuccessResponse {
                    success: true,
                    message: None,
                },
                user_hash,
            ))
        }
        .await,
    )
}

/// Approve a view.
pub async fn approve_view(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let name = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.approve_view(&name).await.handler_err("approve view")?;
            Ok(ApiResponse::success_with_user(
                ViewApproveResponse { approved: true },
                user_hash,
            ))
        }
        .await,
    )
}

/// Block a view.
pub async fn block_view(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let name = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(
        async {
            op.block_view(&name).await.handler_err("block view")?;
            Ok(ApiResponse::success_with_user(
                crate::handlers::response::SuccessResponse {
                    success: true,
                    message: None,
                },
                user_hash,
            ))
        }
        .await,
    )
}
