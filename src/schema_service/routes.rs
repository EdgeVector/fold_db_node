use actix_web::{web, HttpResponse, Responder};
use serde::Deserialize;
use std::sync::RwLockReadGuard;

use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::schema::types::Schema;

use super::state::{SchemaServiceState, SchemaStorage};
use super::types::*;

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
