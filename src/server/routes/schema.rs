use crate::fold_node::OperationProcessor;
use crate::handlers::{ApiResponse, HandlerError, IntoHandlerError};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use crate::server::http_server::AppState;
use crate::server::routes::{handler_error_to_response, handler_result_to_response, node_or_return};
use actix_web::{web, HttpResponse, Responder};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaListResponse {
    pub schemas: serde_json::Value,
    pub count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaResponse {
    pub schema: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaLoadResponse {
    pub available_schemas_loaded: usize,
    pub schemas_loaded_to_db: usize,
    pub failed_schemas: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaApproveResponse {
    pub approved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemaKeysResponse {
    pub keys: Vec<fold_db::schema::types::KeyValue>,
    pub total_count: usize,
}

#[utoipa::path(
    get,
    path = "/api/schemas",
    tag = "schemas",
    responses(
        (status = 200, description = "Array of schemas with states"),
        (status = 500, description = "Server error")
    )
)]
pub async fn list_schemas(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(async {
        let schemas = op.list_schemas().await.handler_err("list schemas")?;
        let count = schemas.len();
        let mut schemas_json = serde_json::to_value(&schemas).handler_err("serialize schemas")?;
        add_display_names(&mut schemas_json);
        Ok(ApiResponse::success_with_user(SchemaListResponse { schemas: schemas_json, count }, user_hash))
    }.await)
}

/// Get a schema by name.
#[utoipa::path(
    get,
    path = "/api/schema/{name}",
    tag = "schemas",
    params(
        ("name" = String, Path, description = "Schema name")
    ),
    responses(
        (status = 200, description = "Schema", body = Schema),
        (status = 404, description = "Schema not found"),
        (status = 500, description = "Server error")
    )
)]
pub async fn get_schema(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let name = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(async {
        let schema_with_state = op.get_schema(&name).await
            .handler_err("get schema")?
            .ok_or_else(|| HandlerError::NotFound(format!("Schema not found: {}", name)))?;
        let mut schema_json = serde_json::to_value(&schema_with_state).handler_err("serialize schema")?;
        add_display_name_to_object(&mut schema_json);
        Ok(ApiResponse::success_with_user(SchemaResponse { schema: schema_json }, user_hash))
    }.await)
}

/// Approve a schema for queries and mutations
#[utoipa::path(
    post,
    path = "/api/schema/{name}/approve",
    tag = "schemas",
    params(
        ("name" = String, Path, description = "Schema name")
    ),
    responses(
        (status = 200, description = "Schema approved successfully"),
        (status = 500, description = "Server error")
    )
)]
pub async fn approve_schema(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let schema_name = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(async {
        op.approve_schema(&schema_name).await.handler_err("approve schema")?;
        Ok(ApiResponse::success_with_user(SchemaApproveResponse { approved: true }, user_hash))
    }.await)
}

/// Block a schema from queries and mutations
#[utoipa::path(
    post,
    path = "/api/schema/{name}/block",
    tag = "schemas",
    params(
        ("name" = String, Path, description = "Schema name")
    ),
    responses(
        (status = 200, description = "Success status"),
        (status = 500, description = "Server error")
    )
)]
pub async fn block_schema(path: web::Path<String>, state: web::Data<AppState>) -> impl Responder {
    let schema_name = path.into_inner();
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(async {
        op.block_schema(&schema_name).await.handler_err("block schema")?;
        Ok(ApiResponse::success_with_user(
            crate::handlers::response::SuccessResponse { success: true, message: None },
            user_hash,
        ))
    }.await)
}

/// Query parameters for schema keys pagination
#[derive(Debug, Deserialize)]
pub struct SchemaKeysQuery {
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// List keys for a schema with pagination
#[utoipa::path(
    get,
    path = "/api/schema/{name}/keys",
    tag = "schemas",
    params(
        ("name" = String, Path, description = "Schema name"),
        ("offset" = Option<usize>, Query, description = "Pagination offset (default 0)"),
        ("limit" = Option<usize>, Query, description = "Page size (default 50)")
    ),
    responses(
        (status = 200, description = "Paginated list of keys"),
        (status = 500, description = "Server error")
    )
)]
pub async fn list_schema_keys(
    path: web::Path<String>,
    query: web::Query<SchemaKeysQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let name = path.into_inner();
    let offset = query.offset.unwrap_or(0);
    let limit = query.limit.unwrap_or(50);
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    handler_result_to_response(async {
        let (keys, total_count) = op.list_schema_keys(&name, offset, limit).await.handler_err("list keys")?;
        Ok(ApiResponse::success_with_user(SchemaKeysResponse { keys, total_count }, user_hash))
    }.await)
}

/// Load schemas from standard directories into memory as Available
#[utoipa::path(
    post,
    path = "/api/schemas/load",
    tag = "schemas",
    responses(
        (status = 200, description = "Load counts for available and data schemas"),
        (status = 500, description = "Server error")
    )
)]
pub async fn load_schemas(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);
    let op = OperationProcessor::new(node.clone());
    match op.load_schemas().await {
        Ok((available_schemas_loaded, schemas_loaded_to_db, failed_schemas)) => {
            log_feature!(
                LogFeature::Schema,
                info,
                "Loaded {} of {} schemas from schema service",
                schemas_loaded_to_db,
                available_schemas_loaded
            );
            HttpResponse::Ok().json(ApiResponse::success_with_user(
                SchemaLoadResponse { available_schemas_loaded, schemas_loaded_to_db, failed_schemas },
                user_hash,
            ))
        }
        Err(e) => {
            log_feature!(LogFeature::Schema, error, "Failed to load schemas: {}", e);
            handler_error_to_response(HandlerError::from(e))
        }
    }
}

/// Add `display_name` to a single schema JSON object.
/// Uses `descriptive_name` if present, otherwise falls back to `name` (the hash).
fn add_display_name_to_object(obj: &mut Value) {
    if let Value::Object(map) = obj {
        let display = map
            .get("descriptive_name")
            .and_then(|v| v.as_str())
            .or_else(|| map.get("name").and_then(|v| v.as_str()))
            .unwrap_or("")
            .to_string();
        map.insert("display_name".to_string(), Value::String(display));
    }
}

/// Add `display_name` to each schema in a JSON array.
fn add_display_names(arr: &mut Value) {
    if let Value::Array(items) = arr {
        for item in items.iter_mut() {
            add_display_name_to_object(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn display_name_uses_descriptive_name_when_present() {
        let mut schema = json!({"name": "abc123", "descriptive_name": "User Profile"});
        add_display_name_to_object(&mut schema);
        assert_eq!(schema["display_name"], "User Profile");
    }

    #[test]
    fn display_name_falls_back_to_hash() {
        let mut schema = json!({"name": "abc123"});
        add_display_name_to_object(&mut schema);
        assert_eq!(schema["display_name"], "abc123");
    }

    #[test]
    fn display_name_ignores_null_descriptive_name() {
        let mut schema = json!({"name": "abc123", "descriptive_name": null});
        add_display_name_to_object(&mut schema);
        assert_eq!(schema["display_name"], "abc123");
    }

    #[test]
    fn add_display_names_processes_array() {
        let mut schemas = json!([
            {"name": "hash1", "descriptive_name": "Friendly Name"},
            {"name": "hash2"}
        ]);
        add_display_names(&mut schemas);
        assert_eq!(schemas[0]["display_name"], "Friendly Name");
        assert_eq!(schemas[1]["display_name"], "hash2");
    }
}
