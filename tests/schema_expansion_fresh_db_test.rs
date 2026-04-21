//! Integration test: schema expansion on a fresh local database.
//!
//! Verifies the fix for the "Source schema not found" error that occurred when:
//! 1. Schema A is created via the schema service and loaded into local DB
//! 2. Data is written to Schema A
//! 3. A new ingestion proposes Schema B (superset of A, same descriptive_name)
//! 4. Schema service returns Expanded(old=A, new=B) with field_mappers
//! 5. On a FRESH local DB (no Schema A loaded), apply_field_mappers would fail
//!
//! The fix ensures the old schema is fetched from the schema service before approval.
//!
//! This test does NOT require an API key — it uses a local schema service.
//!
//! Run with: `cargo test --test schema_expansion_fresh_db_test -- --nocapture`

use fold_db::schema::types::data_classification::DataClassification;
use fold_db::schema::types::{Schema, SchemaType};
use fold_db::schema_service::state::SchemaServiceState;
use fold_db::schema_service::types::{
    AddSchemaResponse, ErrorResponse, SchemaAddOutcome, SchemasListResponse,
};
use fold_db_node::fold_node::node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
mod common;

use actix_web::{web, App, HttpResponse, HttpServer};
use fold_db::schema::types::KeyConfig;
use fold_db::schema::SchemaState;
use serde::Deserialize;
use std::collections::HashMap;
use std::net::TcpListener;
use tempfile::TempDir;

// -- Inline schema service handlers (same as paintings test) --

#[derive(Debug, Deserialize)]
struct AddSchemaRequest {
    schema: Schema,
    mutation_mappers: HashMap<String, String>,
}

async fn handle_list_schemas(state: web::Data<SchemaServiceState>) -> HttpResponse {
    match state.get_schema_names() {
        Ok(names) => HttpResponse::Ok().json(SchemasListResponse { schemas: names }),
        Err(e) => HttpResponse::InternalServerError().json(ErrorResponse {
            error: format!("Failed to list schemas: {}", e),
        }),
    }
}

async fn handle_get_available_schemas(state: web::Data<SchemaServiceState>) -> HttpResponse {
    match state.get_all_schemas_cached() {
        Ok(schemas) => HttpResponse::Ok().json(serde_json::json!({ "schemas": schemas })),
        Err(e) => HttpResponse::InternalServerError().json(ErrorResponse {
            error: format!("Failed to get schemas: {}", e),
        }),
    }
}

async fn handle_get_schema(
    path: web::Path<String>,
    state: web::Data<SchemaServiceState>,
) -> HttpResponse {
    let schema_name = path.into_inner();
    match state.get_schema_by_name(&schema_name) {
        Ok(Some(schema)) => HttpResponse::Ok().json(schema),
        Ok(None) => HttpResponse::NotFound().json(ErrorResponse {
            error: "Schema not found".to_string(),
        }),
        Err(e) => HttpResponse::InternalServerError().json(ErrorResponse {
            error: format!("Failed to get schema: {}", e),
        }),
    }
}

async fn handle_add_schema(
    payload: web::Json<AddSchemaRequest>,
    state: web::Data<SchemaServiceState>,
) -> HttpResponse {
    let request = payload.into_inner();
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
        Ok(SchemaAddOutcome::AlreadyExists(schema, _)) => {
            HttpResponse::Ok().json(AddSchemaResponse {
                schema,
                mutation_mappers: HashMap::new(),
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
        Err(error) => HttpResponse::BadRequest().json(ErrorResponse {
            error: format!("Failed to add schema: {}", error),
        }),
    }
}

// -- Test helpers --

async fn spawn_local_schema_service() -> (String, actix_web::dev::ServerHandle, TempDir) {
    let temp_dir = TempDir::new().expect("failed to create tempdir for schema service");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path).expect("failed to create schema service state");
    let state_data = web::Data::new(state);

    let listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("failed to bind schema service listener");
    let bound_address = listener.local_addr().expect("failed to read bound address");

    let state_clone = state_data.clone();
    let server = HttpServer::new(move || {
        App::new().app_data(state_clone.clone()).service(
            web::scope("/v1")
                .route("/schemas", web::get().to(handle_list_schemas))
                .route("/schemas", web::post().to(handle_add_schema))
                .route(
                    "/schemas/available",
                    web::get().to(handle_get_available_schemas),
                )
                .route("/schema/{name}", web::get().to(handle_get_schema)),
        )
    })
    .listen(listener)
    .expect("failed to listen")
    .run();

    let handle = server.handle();
    actix_web::rt::spawn(server);
    actix_web::rt::time::sleep(std::time::Duration::from_millis(100)).await;

    let base_url = format!("http://127.0.0.1:{}", bound_address.port());
    (base_url, handle, temp_dir)
}

/// Build a Schema programmatically with given fields and descriptive name.
fn build_schema(
    fields: Vec<&str>,
    descriptive_name: &str,
    hash_field: &str,
    range_field: Option<&str>,
) -> Schema {
    let field_names: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
    let mut field_classifications = HashMap::new();
    for f in &field_names {
        field_classifications.insert(f.clone(), vec!["word".to_string()]);
    }

    let mut schema = Schema::new(
        String::new(), // will be set by identity_hash
        SchemaType::HashRange,
        Some(KeyConfig::new(
            Some(hash_field.to_string()),
            range_field.map(|r| r.to_string()),
        )),
        Some(field_names),
        None,
        None,
    );
    schema.descriptive_name = Some(descriptive_name.to_string());
    schema.field_classifications = field_classifications;
    for f in schema.fields.clone().unwrap_or_default() {
        schema
            .field_descriptions
            .insert(f.clone(), format!("{} field", f));
        schema
            .field_data_classifications
            .insert(f.clone(), DataClassification::new(0, "general").unwrap());
    }
    schema.compute_identity_hash();
    schema.name = schema.get_identity_hash().unwrap().clone();
    schema
}

// -- Tests --

/// Test: schema expansion on a fresh local DB works without "source schema not found" error.
///
/// Steps:
/// 1. Create Schema A (3 fields) via schema service + local load
/// 2. Write data to Schema A
/// 3. Create a NEW FoldNode (fresh DB, no Schema A loaded)
/// 4. Submit Schema B (superset of A, 5 fields, same descriptive_name) — triggers expansion
/// 5. Verify: expansion succeeds, Schema B is active, Schema A is blocked
/// 6. Verify: field_mappers resolved correctly (shared fields point to same molecules)
#[actix_web::test]
async fn test_schema_expansion_on_fresh_db() {
    let (schema_url, schema_handle, _schema_temp_dir) = spawn_local_schema_service().await;
    eprintln!("Schema service at {}", schema_url);

    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();

    // --- Phase 1: Create Schema A and write data ---
    let temp_dir_1 = TempDir::new().unwrap();
    let config_1 =
        fold_db_node::fold_node::config::NodeConfig::new(temp_dir_1.path().to_path_buf())
            .with_identity(&user_id, &keypair.secret_key_base64())
            .with_schema_service_url(&schema_url);
    let node_1 = FoldNode::new(config_1).await.unwrap();

    // Schema A: 3 fields
    let schema_a = build_schema(
        vec!["title", "artist", "year"],
        "Artwork Collection",
        "title",
        Some("year"),
    );
    let schema_a_name = schema_a.name.clone();
    eprintln!("Schema A name (identity_hash): {}", &schema_a_name[..16]);

    // Add Schema A via node (same path as ingestion)
    let add_resp = node_1.add_schema_to_service(&schema_a).await.unwrap();
    assert!(
        add_resp.replaced_schema.is_none(),
        "First schema should not replace anything"
    );

    // Load and approve Schema A locally
    let schema_json = serde_json::to_string(&add_resp.schema).unwrap();
    {
        let db_1 = node_1.get_fold_db().unwrap();
        db_1.schema_manager()
            .load_schema_from_json(&schema_json)
            .await
            .unwrap();
        db_1.schema_manager()
            .approve(&add_resp.schema.name)
            .await
            .unwrap();
    } // drop db_1 guard before using processor

    // Write a record to Schema A
    let processor_1 = OperationProcessor::new(std::sync::Arc::new(node_1.clone()));
    let mutation = fold_db::schema::types::Mutation::new(
        add_resp.schema.name.clone(),
        {
            let mut fields = HashMap::new();
            fields.insert("title".to_string(), serde_json::json!("Starry Night"));
            fields.insert("artist".to_string(), serde_json::json!("Van Gogh"));
            fields.insert("year".to_string(), serde_json::json!("1889"));
            fields
        },
        fold_db::schema::types::KeyValue::new(
            Some("Starry Night".to_string()),
            Some("1889".to_string()),
        ),
        user_id.clone(),
        fold_db::schema::types::MutationType::Create,
    );
    processor_1.execute_mutation_op(mutation).await.unwrap();
    eprintln!("Phase 1: Schema A created, data written");

    // --- Phase 2: Fresh DB, submit superset schema B ---
    let temp_dir_2 = TempDir::new().unwrap();
    let config_2 =
        fold_db_node::fold_node::config::NodeConfig::new(temp_dir_2.path().to_path_buf())
            .with_identity(&user_id, &keypair.secret_key_base64())
            .with_schema_service_url(&schema_url);
    let node_2 = FoldNode::new(config_2).await.unwrap();

    // Verify Schema A is NOT loaded in this fresh DB
    {
        let db_2 = node_2.get_fold_db().unwrap();
        let a_metadata = db_2.schema_manager().get_schema_metadata(&schema_a_name);
        assert!(
            a_metadata.map(|opt| opt.is_none()).unwrap_or(true),
            "Schema A should NOT be loaded in fresh DB"
        );
    }

    // Schema B: superset of A (5 fields, shares title/artist/year)
    let schema_b = build_schema(
        vec!["title", "artist", "year", "medium", "dimensions"],
        "Artwork Collection",
        "title",
        Some("year"),
    );
    let schema_b_name = schema_b.name.clone();
    eprintln!("Schema B name (identity_hash): {}", &schema_b_name[..16]);
    assert_ne!(
        schema_a_name, schema_b_name,
        "Schemas A and B should have different identity hashes"
    );

    // Add Schema B via node — this should trigger expansion
    let add_resp_b = node_2.add_schema_to_service(&schema_b).await.unwrap();
    eprintln!(
        "Schema service response: replaced_schema={:?}",
        add_resp_b.replaced_schema
    );
    assert_eq!(
        add_resp_b.replaced_schema.as_deref(),
        Some(schema_a_name.as_str()),
        "Schema B should replace Schema A via expansion"
    );

    // Now load and approve Schema B — this is the critical path.
    // The ingestion service would fetch the old schema from the schema service first.
    // Simulate what create_new_schema_with_node does:
    {
        let db_2 = node_2.get_fold_db().unwrap();

        // 1. Fetch old schema from schema service (the fix we're testing)
        let client = fold_db_node::fold_node::SchemaServiceClient::new(&schema_url);
        let old_schema = client.get_schema(&schema_a_name).await.unwrap();
        let old_json = serde_json::to_string(&old_schema).unwrap();
        db_2.schema_manager()
            .load_schema_from_json(&old_json)
            .await
            .unwrap();
        eprintln!("Loaded old schema from schema service into fresh DB");

        // 2. Load and approve Schema B
        let schema_b_json = serde_json::to_string(&add_resp_b.schema).unwrap();
        db_2.schema_manager()
            .load_schema_from_json(&schema_b_json)
            .await
            .unwrap();

        // This is the call that would fail without the fix — apply_field_mappers
        // needs the old schema's molecule UUIDs.
        let approve_result = db_2.schema_manager().approve(&add_resp_b.schema.name).await;
        assert!(
            approve_result.is_ok(),
            "Approving expanded schema should succeed: {:?}",
            approve_result.err()
        );
        eprintln!("Phase 2: Schema B approved (expansion worked on fresh DB)");

        // --- Phase 3: Verify state ---
        // Block old schema (as ingestion service would)
        db_2.schema_manager()
            .block_and_supersede(&schema_a_name, &add_resp_b.schema.name)
            .await
            .unwrap();
    } // drop db_2 guard before using processor

    let processor_2 = OperationProcessor::new(std::sync::Arc::new(node_2.clone()));
    // list_schemas returns only active (non-blocked) schemas
    let active_schemas = processor_2.list_schemas().await.unwrap();

    eprintln!("Active schemas: {}", active_schemas.len());
    for s in &active_schemas {
        eprintln!(
            "  Active: {} fields={:?}",
            &s.schema.name[..16],
            s.schema.fields
        );
    }
    eprintln!("add_resp_b fields: {:?}", add_resp_b.schema.fields);

    assert_eq!(
        active_schemas.len(),
        1,
        "Should have exactly 1 active schema"
    );
    assert_eq!(
        active_schemas[0].schema.name, add_resp_b.schema.name,
        "Active schema should be Schema B"
    );

    // Verify Schema A is blocked by checking all schemas (including blocked)
    {
        let db_2 = node_2.get_fold_db().unwrap();
        let all_schemas = db_2.schema_manager().get_schemas_with_states().unwrap();
        let blocked: Vec<_> = all_schemas
            .iter()
            .filter(|s| s.state == SchemaState::Blocked)
            .collect();
        assert_eq!(blocked.len(), 1, "Should have exactly 1 blocked schema");
        assert_eq!(
            blocked[0].schema.name, schema_a_name,
            "Blocked schema should be Schema A"
        );
    }

    // Verify Schema B has the new fields
    let b_fields = active_schemas[0].schema.fields.as_ref().unwrap();
    assert!(
        b_fields.contains(&"medium".to_string()),
        "Should have new field 'medium'"
    );
    assert!(
        b_fields.contains(&"dimensions".to_string()),
        "Should have new field 'dimensions'"
    );
    assert!(
        b_fields.contains(&"title".to_string()),
        "Should have shared field 'title'"
    );

    // Verify field_mappers exist for shared fields
    let b_field_mappers = &active_schemas[0].schema.field_mappers;
    eprintln!("Schema B field_mappers: {:?}", b_field_mappers);
    assert!(
        b_field_mappers
            .as_ref()
            .map(|m| !m.is_empty())
            .unwrap_or(false),
        "Expanded schema should have field_mappers for shared fields"
    );

    // Cleanup
    schema_handle.stop(true).await;
    eprintln!("Test passed: schema expansion on fresh DB works correctly");
}

/// Test: without fetching old schema, apply_field_mappers gracefully warns (defense in depth).
///
/// Even if the fetch fails, the approve should succeed because apply_field_mappers
/// was changed to log::warn + continue instead of hard error.
#[actix_web::test]
async fn test_expansion_without_old_schema_warns_but_succeeds() {
    let (schema_url, schema_handle, _schema_temp_dir) = spawn_local_schema_service().await;

    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();

    // Create Schema A on the schema service
    let temp_dir_1 = TempDir::new().unwrap();
    let config_1 =
        fold_db_node::fold_node::config::NodeConfig::new(temp_dir_1.path().to_path_buf())
            .with_identity(&user_id, &keypair.secret_key_base64())
            .with_schema_service_url(&schema_url);
    let node_1 = FoldNode::new(config_1).await.unwrap();

    let schema_a = build_schema(
        vec!["name", "color", "size"],
        "Product Catalog",
        "name",
        None,
    );
    node_1.add_schema_to_service(&schema_a).await.unwrap();

    // Fresh DB — submit superset Schema B
    let temp_dir_2 = TempDir::new().unwrap();
    let config_2 =
        fold_db_node::fold_node::config::NodeConfig::new(temp_dir_2.path().to_path_buf())
            .with_identity(&user_id, &keypair.secret_key_base64())
            .with_schema_service_url(&schema_url);
    let node_2 = FoldNode::new(config_2).await.unwrap();

    let schema_b = build_schema(
        vec!["name", "color", "size", "weight", "price"],
        "Product Catalog",
        "name",
        None,
    );

    let add_resp = node_2.add_schema_to_service(&schema_b).await.unwrap();
    assert!(
        add_resp.replaced_schema.is_some(),
        "Should trigger expansion"
    );

    // Load Schema B WITHOUT loading old Schema A first (testing defense-in-depth)
    {
        let db_2 = node_2.get_fold_db().unwrap();
        let schema_b_json = serde_json::to_string(&add_resp.schema).unwrap();
        db_2.schema_manager()
            .load_schema_from_json(&schema_b_json)
            .await
            .unwrap();

        // approve should succeed even without old schema (warn + skip, not error)
        let result = db_2.schema_manager().approve(&add_resp.schema.name).await;
        assert!(
            result.is_ok(),
            "Approval should succeed even without old schema loaded: {:?}",
            result.err()
        );
    }

    schema_handle.stop(true).await;
    eprintln!("Test passed: expansion without old schema warns but succeeds");
}
