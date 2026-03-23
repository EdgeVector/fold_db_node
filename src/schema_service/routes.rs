use actix_web::{web, HttpResponse, Responder};
use serde::Deserialize;
use std::sync::RwLockReadGuard;

use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::schema::types::Schema;

use super::state::{SchemaServiceState, SchemaStorage};
use super::types::*;

// ============== View Route Handlers ==============

/// List all view names
pub(super) async fn list_views(state: web::Data<SchemaServiceState>) -> impl Responder {
    match state.get_view_names() {
        Ok(names) => HttpResponse::Ok().json(ViewsListResponse { views: names }),
        Err(e) => {
            log_feature!(LogFeature::Schema, error, "Failed to list views: {}", e);
            HttpResponse::InternalServerError().json(ErrorResponse {
                error: format!("Failed to list views: {}", e),
            })
        }
    }
}

/// Get all views with definitions
pub(super) async fn get_available_views(
    state: web::Data<SchemaServiceState>,
) -> impl Responder {
    match state.get_all_views() {
        Ok(views) => HttpResponse::Ok().json(AvailableViewsResponse { views }),
        Err(e) => {
            log_feature!(LogFeature::Schema, error, "Failed to get available views: {}", e);
            HttpResponse::InternalServerError().json(ErrorResponse {
                error: format!("Failed to get views: {}", e),
            })
        }
    }
}

/// Get a specific view by name
pub(super) async fn get_view(
    path: web::Path<String>,
    state: web::Data<SchemaServiceState>,
) -> impl Responder {
    let view_name = path.into_inner();
    log_feature!(LogFeature::Schema, info, "Schema service: getting view '{}'", view_name);

    match state.get_view_by_name(&view_name) {
        Ok(Some(view)) => HttpResponse::Ok().json(view),
        Ok(None) => {
            log_feature!(LogFeature::Schema, warn, "View '{}' not found", view_name);
            HttpResponse::NotFound().json(ErrorResponse {
                error: "View not found".to_string(),
            })
        }
        Err(e) => {
            log_feature!(LogFeature::Schema, error, "Failed to get view '{}': {}", view_name, e);
            HttpResponse::InternalServerError().json(ErrorResponse {
                error: format!("Failed to get view: {}", e),
            })
        }
    }
}

/// Register a new view
pub(super) async fn add_view(
    payload: web::Json<AddViewRequest>,
    state: web::Data<SchemaServiceState>,
) -> impl Responder {
    let request = payload.into_inner();
    let view_name = request.name.clone();

    log_feature!(
        LogFeature::Schema, info,
        "Schema service: registering view '{}' with {} input queries and {} output fields",
        view_name, request.input_queries.len(), request.output_fields.len()
    );

    match state.add_view(request).await {
        Ok(ViewAddOutcome::Added(view, schema)) => {
            HttpResponse::Created().json(AddViewResponse {
                view,
                output_schema: schema,
                replaced_schema: None,
            })
        }
        Ok(ViewAddOutcome::AddedWithExistingSchema(view, schema)) => {
            HttpResponse::Ok().json(AddViewResponse {
                view,
                output_schema: schema,
                replaced_schema: None,
            })
        }
        Ok(ViewAddOutcome::Expanded(view, schema, old_name)) => {
            HttpResponse::Created().json(AddViewResponse {
                view,
                output_schema: schema,
                replaced_schema: Some(old_name),
            })
        }
        Err(error) => {
            log_feature!(LogFeature::Schema, error, "Failed to register view '{}': {}", view_name, error);
            HttpResponse::BadRequest().json(ErrorResponse {
                error: format!("Failed to register view: {}", error),
            })
        }
    }
}


/// Acquire a read lock on the schemas map, returning an HTTP 500 on poisoned lock.
fn read_schemas(
    state: &SchemaServiceState,
) -> Result<RwLockReadGuard<'_, std::collections::HashMap<String, Schema>>, HttpResponse> {
    state.schemas.read().map_err(|e| {
        log_feature!(LogFeature::Schema, error, "Failed to acquire schemas read lock: {}", e);
        HttpResponse::InternalServerError().json(ErrorResponse {
            error: "Failed to acquire schemas read lock".to_string(),
        })
    })
}

/// List all available schemas
pub(super) async fn list_schemas(state: web::Data<SchemaServiceState>) -> impl Responder {
    let schemas = match read_schemas(&state) {
        Ok(s) => s,
        Err(r) => return r,
    };

    HttpResponse::Ok().json(SchemasListResponse {
        schemas: schemas.keys().cloned().collect(),
    })
}

/// Get all available schemas with their full definitions
pub(super) async fn get_available_schemas(
    state: web::Data<SchemaServiceState>,
) -> impl Responder {
    let schemas = match read_schemas(&state) {
        Ok(s) => s,
        Err(r) => return r,
    };

    HttpResponse::Ok().json(AvailableSchemasResponse {
        schemas: schemas.values().cloned().collect(),
    })
}

/// Get a specific schema by name
pub(super) async fn get_schema(
    path: web::Path<String>,
    state: web::Data<SchemaServiceState>,
) -> impl Responder {
    let schema_name = path.into_inner();
    log_feature!(LogFeature::Schema, info, "Schema service: getting schema '{}'", schema_name);

    let schemas = match read_schemas(&state) {
        Ok(s) => s,
        Err(r) => return r,
    };

    match schemas.get(&schema_name) {
        Some(schema) => HttpResponse::Ok().json(schema),
        None => {
            log_feature!(LogFeature::Schema, warn, "Schema '{}' not found", schema_name);
            HttpResponse::NotFound().json(ErrorResponse {
                error: "Schema not found".to_string(),
            })
        }
    }
}

/// Query parameters for the find-similar endpoint
#[derive(Debug, Deserialize)]
pub(super) struct SimilarQuery {
    threshold: Option<f64>,
}

/// Find schemas similar to the given schema
pub(super) async fn find_similar(
    path: web::Path<String>,
    query: web::Query<SimilarQuery>,
    state: web::Data<SchemaServiceState>,
) -> impl Responder {
    let schema_name = path.into_inner();
    let threshold = query.threshold.unwrap_or(0.5);

    if !(0.0..=1.0).contains(&threshold) {
        return HttpResponse::BadRequest().json(ErrorResponse {
            error: "Threshold must be between 0.0 and 1.0".to_string(),
        });
    }

    log_feature!(
        LogFeature::Schema, info,
        "Schema service: finding schemas similar to '{}' with threshold {}",
        schema_name, threshold
    );

    match state.find_similar_schemas(&schema_name, threshold) {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => {
            let error_msg = format!("{}", e);
            if error_msg.contains("not found") {
                HttpResponse::NotFound().json(ErrorResponse {
                    error: format!("Schema '{}' not found", schema_name),
                })
            } else {
                HttpResponse::InternalServerError().json(ErrorResponse {
                    error: format!("Failed to find similar schemas: {}", e),
                })
            }
        }
    }
}

/// Reload schemas from the directory
pub(super) async fn reload_schemas(state: web::Data<SchemaServiceState>) -> impl Responder {
    log_feature!(LogFeature::Schema, info, "Schema service: reloading schemas");

    match state.load_schemas().await {
        Ok(_) => {
            let schemas = match read_schemas(&state) {
                Ok(s) => s,
                Err(r) => return r,
            };

            HttpResponse::Ok().json(ReloadResponse {
                success: true,
                schemas_loaded: schemas.len(),
            })
        }
        Err(e) => {
            log_feature!(LogFeature::Schema, error, "Failed to reload schemas: {}", e);
            HttpResponse::InternalServerError().json(ErrorResponse {
                error: format!("Failed to reload schemas: {}", e),
            })
        }
    }
}

pub(super) async fn add_schema(
    payload: web::Json<AddSchemaRequest>,
    state: web::Data<SchemaServiceState>,
) -> impl Responder {
    let request = payload.into_inner();
    let schema_name = request.schema.name.clone();

    log_feature!(
        LogFeature::Schema, info,
        "Schema service: adding schema '{}' with {} mutation mappers",
        schema_name, request.mutation_mappers.len()
    );

    match state
        .add_schema(request.schema, request.mutation_mappers)
        .await
    {
        Ok(SchemaAddOutcome::Added(schema, mutation_mappers)) => {
            HttpResponse::Created().json(AddSchemaResponse {
                schema,
                mutation_mappers,
                replaced_schema: None,
            })
        }
        Ok(SchemaAddOutcome::AlreadyExists(schema, mutation_mappers)) => {
            HttpResponse::Ok().json(AddSchemaResponse {
                schema,
                mutation_mappers,
                replaced_schema: None,
            })
        }
        Ok(SchemaAddOutcome::Expanded(old_name, schema, mutation_mappers)) => {
            HttpResponse::Created().json(AddSchemaResponse {
                schema,
                mutation_mappers,
                replaced_schema: Some(old_name),
            })
        }
        Err(error) => {
            log_feature!(LogFeature::Schema, error, "Failed to add schema '{}': {}", schema_name, error);
            HttpResponse::BadRequest().json(ErrorResponse {
                error: format!("Failed to add schema: {}", error),
            })
        }
    }
}

/// Batch check whether proposed schemas can reuse existing ones
pub(super) async fn batch_check_reuse(
    payload: web::Json<BatchSchemaReuseRequest>,
    state: web::Data<SchemaServiceState>,
) -> impl Responder {
    let request = payload.into_inner();

    match state.batch_check_schema_reuse(&request.schemas) {
        Ok(matches) => HttpResponse::Ok().json(BatchSchemaReuseResponse { matches }),
        Err(e) => {
            log_feature!(LogFeature::Schema, error, "Batch schema reuse check failed: {}", e);
            HttpResponse::InternalServerError().json(ErrorResponse {
                error: format!("Batch schema reuse check failed: {}", e),
            })
        }
    }
}

/// Health check endpoint
pub(super) async fn health_check() -> impl Responder {
    HttpResponse::Ok().json(HealthResponse {
        status: "healthy".to_string(),
    })
}

/// Reset the schema service database
pub(super) async fn reset_database(
    state: web::Data<SchemaServiceState>,
    req: web::Json<ResetRequest>,
) -> impl Responder {
    if !req.confirm {
        return HttpResponse::BadRequest().json(ResetResponse {
            success: false,
            message: "Reset confirmation required. Set 'confirm' to true.".to_string(),
        });
    }

    log_feature!(LogFeature::Schema, info, "Resetting schema service database");

    // Clear the in-memory schemas map
    {
        let mut schemas = match state.schemas.write() {
            Ok(s) => s,
            Err(e) => {
                log_feature!(LogFeature::Schema, error, "Failed to acquire schemas write lock: {}", e);
                return HttpResponse::InternalServerError().json(ResetResponse {
                    success: false,
                    message: "Failed to acquire schemas write lock".to_string(),
                });
            }
        };
        schemas.clear();
    }

    // Clear storage backend
    match &state.storage {
        SchemaStorage::Sled { db, schemas_tree } => {
            if let Err(e) = schemas_tree.clear() {
                log_feature!(LogFeature::Schema, error, "Failed to clear schemas tree: {}", e);
                return HttpResponse::InternalServerError().json(ResetResponse {
                    success: false,
                    message: format!("Failed to reset sled database: {}", e),
                });
            }

            if let Err(e) = db.flush() {
                log_feature!(LogFeature::Schema, warn, "Failed to flush database after reset: {}", e);
            }
        }
        #[cfg(feature = "aws-backend")]
        SchemaStorage::Cloud { store } => {
            if let Err(e) = store.clear_all_schemas().await {
                log_feature!(LogFeature::Schema, error, "Failed to clear DynamoDB schemas: {}", e);
                return HttpResponse::InternalServerError().json(ResetResponse {
                    success: false,
                    message: format!("Failed to reset DynamoDB: {}", e),
                });
            }
        }
    }

    log_feature!(LogFeature::Schema, info, "Schema service database reset successfully");

    HttpResponse::Ok().json(ResetResponse {
        success: true,
        message: "Schema service database reset successfully. All schemas have been cleared."
            .to_string(),
    })
}

// ============== Transform Route Handlers ==============

/// List all transform hashes + names
pub(super) async fn list_transforms(state: web::Data<SchemaServiceState>) -> impl Responder {
    match state.get_transform_list() {
        Ok(transforms) => HttpResponse::Ok().json(TransformsListResponse { transforms }),
        Err(e) => {
            log_feature!(LogFeature::Schema, error, "Failed to list transforms: {}", e);
            HttpResponse::InternalServerError().json(ErrorResponse {
                error: format!("Failed to list transforms: {}", e),
            })
        }
    }
}

/// Register a new transform
pub(super) async fn register_transform(
    payload: web::Json<RegisterTransformRequest>,
    state: web::Data<SchemaServiceState>,
) -> impl Responder {
    let request = payload.into_inner();
    let transform_name = request.name.clone();

    log_feature!(
        LogFeature::Schema, info,
        "Schema service: registering transform '{}' v{} ({} bytes WASM)",
        transform_name, request.version, request.wasm_bytes.len()
    );

    match state.register_transform(request).await {
        Ok((record, TransformAddOutcome::Added)) => {
            HttpResponse::Created().json(RegisterTransformResponse {
                hash: record.hash.clone(),
                record,
                outcome: TransformAddOutcome::Added,
            })
        }
        Ok((record, TransformAddOutcome::AlreadyExists)) => {
            HttpResponse::Ok().json(RegisterTransformResponse {
                hash: record.hash.clone(),
                record,
                outcome: TransformAddOutcome::AlreadyExists,
            })
        }
        Err(error) => {
            log_feature!(LogFeature::Schema, error, "Failed to register transform '{}': {}", transform_name, error);
            HttpResponse::BadRequest().json(ErrorResponse {
                error: format!("Failed to register transform: {}", error),
            })
        }
    }
}

/// Get all transforms with full metadata (no wasm_bytes)
pub(super) async fn get_available_transforms(
    state: web::Data<SchemaServiceState>,
) -> impl Responder {
    match state.get_all_transforms() {
        Ok(transforms) => HttpResponse::Ok().json(AvailableTransformsResponse { transforms }),
        Err(e) => {
            log_feature!(LogFeature::Schema, error, "Failed to get available transforms: {}", e);
            HttpResponse::InternalServerError().json(ErrorResponse {
                error: format!("Failed to get transforms: {}", e),
            })
        }
    }
}

/// Get a specific transform by hash
pub(super) async fn get_transform(
    path: web::Path<String>,
    state: web::Data<SchemaServiceState>,
) -> impl Responder {
    let hash = path.into_inner();
    log_feature!(LogFeature::Schema, info, "Schema service: getting transform '{}'", hash);

    match state.get_transform_by_hash(&hash) {
        Ok(Some(record)) => HttpResponse::Ok().json(record),
        Ok(None) => {
            log_feature!(LogFeature::Schema, warn, "Transform '{}' not found", hash);
            HttpResponse::NotFound().json(ErrorResponse {
                error: "Transform not found".to_string(),
            })
        }
        Err(e) => {
            log_feature!(LogFeature::Schema, error, "Failed to get transform '{}': {}", hash, e);
            HttpResponse::InternalServerError().json(ErrorResponse {
                error: format!("Failed to get transform: {}", e),
            })
        }
    }
}

/// Download WASM bytes for a transform
pub(super) async fn get_transform_wasm(
    path: web::Path<String>,
    state: web::Data<SchemaServiceState>,
) -> impl Responder {
    let hash = path.into_inner();
    log_feature!(LogFeature::Schema, info, "Schema service: getting WASM for transform '{}'", hash);

    match state.get_transform_wasm(&hash) {
        Ok(Some(bytes)) => HttpResponse::Ok()
            .content_type("application/wasm")
            .body(bytes),
        Ok(None) => {
            log_feature!(LogFeature::Schema, warn, "Transform WASM '{}' not found", hash);
            HttpResponse::NotFound().json(ErrorResponse {
                error: "Transform WASM not found".to_string(),
            })
        }
        Err(e) => {
            log_feature!(LogFeature::Schema, error, "Failed to get transform WASM '{}': {}", hash, e);
            HttpResponse::InternalServerError().json(ErrorResponse {
                error: format!("Failed to get transform WASM: {}", e),
            })
        }
    }
}

/// Verify a WASM blob matches a hash
pub(super) async fn verify_transform(
    payload: web::Json<VerifyTransformRequest>,
) -> impl Responder {
    let request = payload.into_inner();
    let (matches, computed_hash) =
        SchemaServiceState::verify_transform(&request.hash, &request.wasm_bytes);

    HttpResponse::Ok().json(VerifyTransformResponse {
        hash: request.hash,
        matches,
        computed_hash,
    })
}

/// Query parameters for the find-similar-transforms endpoint
#[derive(Debug, Deserialize)]
pub(super) struct SimilarTransformQuery {
    threshold: Option<f64>,
}

/// Find transforms with similar names
pub(super) async fn find_similar_transforms(
    path: web::Path<String>,
    query: web::Query<SimilarTransformQuery>,
    state: web::Data<SchemaServiceState>,
) -> impl Responder {
    let name = path.into_inner();
    let threshold = query.threshold.unwrap_or(0.5);

    if !(0.0..=1.0).contains(&threshold) {
        return HttpResponse::BadRequest().json(ErrorResponse {
            error: "Threshold must be between 0.0 and 1.0".to_string(),
        });
    }

    log_feature!(
        LogFeature::Schema, info,
        "Schema service: finding transforms similar to '{}' with threshold {}",
        name, threshold
    );

    match state.find_similar_transforms(&name, threshold) {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => {
            HttpResponse::InternalServerError().json(ErrorResponse {
                error: format!("Failed to find similar transforms: {}", e),
            })
        }
    }
}
