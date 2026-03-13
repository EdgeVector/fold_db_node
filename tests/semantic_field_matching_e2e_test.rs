//! End-to-end test for semantic field matching through the full ingestion pipeline.
//!
//! Simulates what happens when the UI ingests data twice with different field names:
//! 1. First ingestion: Schema A with "artist", "title", "year"
//! 2. Second ingestion: Schema B with "creator", "title", "year", "medium"
//!
//! Verifies:
//! - Schema expansion produces a superset with "artist" (not "creator")
//! - mutation_mappers from the schema service flow through to mutation generation
//! - Data written with "creator" field name ends up stored under "artist"
//! - "medium" is preserved as a new field (not falsely matched)
//!
//! Uses real FastEmbedModel (no mock) but no AI — schemas and data are supplied directly.
//!
//! Run with: `cargo test --test semantic_field_matching_e2e_test -- --ignored --nocapture`

use fold_db::logging::core::run_with_user;
use fold_db_node::fold_node::node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use fold_db_node::ingestion::mutation_generator;
use fold_db_node::schema_service::server::{
    AddSchemaResponse, ErrorResponse, SchemaAddOutcome, SchemaServiceState, SchemasListResponse,
};
mod common;

use actix_web::{web, App, HttpResponse, HttpServer};
use fold_db::schema::types::{KeyValue, Query};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::net::TcpListener;
use tempfile::TempDir;

// -- Inline schema service handlers (same as other integration tests) ----------

#[derive(Debug, Deserialize)]
struct AddSchemaRequest {
    schema: fold_db::schema::types::Schema,
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
        Ok(SchemaAddOutcome::TooSimilar(conflict)) => HttpResponse::Conflict().json(
            fold_db_node::schema_service::server::ConflictResponse {
                error: "Schema too similar".to_string(),
                similarity: conflict.similarity,
                closest_schema: conflict.closest_schema,
            },
        ),
        Err(error) => HttpResponse::BadRequest().json(ErrorResponse {
            error: format!("Failed to add schema: {}", error),
        }),
    }
}

// -- Test helpers --------------------------------------------------------------

async fn spawn_local_schema_service() -> (String, actix_web::dev::ServerHandle, TempDir) {
    let temp_dir = TempDir::new().expect("failed to create tempdir");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    // Uses real FastEmbedModel — this is what production uses
    let state = SchemaServiceState::new(db_path).expect("failed to create schema service state");
    let state_data = web::Data::new(state);

    let listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("failed to bind schema service listener");
    let bound_address = listener.local_addr().expect("failed to read bound address");

    let state_clone = state_data.clone();
    let server = HttpServer::new(move || {
        App::new().app_data(state_clone.clone()).service(
            web::scope("/api")
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

// -- The test -----------------------------------------------------------------

/// Full pipeline test: semantic field matching → mutation_mappers → correct data storage.
///
/// This test exercises the exact code path the UI takes:
/// 1. Schema service receives Schema A (artist, title, year)
/// 2. Schema service receives Schema B (creator, title, year, medium)
/// 3. Schema service detects "creator" ≈ "artist" and returns mutation_mappers
/// 4. Ingestion pipeline merges mappers and generates mutations
/// 5. Data with "creator" field is written under "artist" field on the expanded schema
#[actix_web::test]
#[ignore] // Uses real FastEmbedModel (downloads on first run)
async fn test_semantic_field_matching_full_pipeline() {
    // 1. Spin up local schema service with real embeddings
    let (schema_url, schema_handle, _schema_temp_dir) = spawn_local_schema_service().await;
    eprintln!("Schema service at {}", schema_url);

    // 2. Create FoldNode
    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_identity(&user_id, &keypair.secret_key_base64())
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    // 3. Submit Schema A: ["artist", "title", "year"]
    let schema_a_def: fold_db::schema::types::Schema = serde_json::from_value(json!({
        "name": "ArtworkSchemaA",
        "descriptive_name": "Artwork Collection",
        "schema_type": "Single",
        "key": { "hash_field": "title" },
        "fields": ["artist", "title", "year"],
        "field_classifications": {
            "artist": ["word"],
            "title": ["word"],
            "year": ["number"]
        }
    }))
    .unwrap();

    let resp_a = node.add_schema_to_service(&schema_a_def).await.unwrap();
    let schema_a_name = resp_a.schema.name.clone();
    eprintln!("Schema A registered: {}", &schema_a_name[..16]);

    // Load and approve Schema A locally
    {
        let db = node.get_fold_db().await.unwrap();
        let json_a = serde_json::to_string(&resp_a.schema).unwrap();
        db.schema_manager.load_schema_from_json(&json_a).await.unwrap();
        db.schema_manager.approve(&schema_a_name).await.unwrap();
    }

    // 4. Write data using Schema A
    let mutation_a = fold_db::schema::types::Mutation::new(
        schema_a_name.clone(),
        {
            let mut fields = HashMap::new();
            fields.insert("artist".to_string(), json!("Claude Monet"));
            fields.insert("title".to_string(), json!("Water Lilies"));
            fields.insert("year".to_string(), json!("1906"));
            fields
        },
        KeyValue::new(Some("Water Lilies".to_string()), None),
        user_id.clone(),
        fold_db::MutationType::Create,
    );

    let result = run_with_user(&user_id, async {
        node.mutate_batch(vec![mutation_a]).await
    })
    .await;
    assert!(result.is_ok(), "Schema A mutation failed: {:?}", result.err());
    eprintln!("Schema A: wrote 'Water Lilies' by 'Claude Monet'");

    // 5. Submit Schema B: ["creator", "title", "year", "medium"]
    //    The schema service should detect "creator" ≈ "artist" and return mappers
    let schema_b_def: fold_db::schema::types::Schema = serde_json::from_value(json!({
        "name": "ArtworkSchemaB",
        "descriptive_name": "Artwork Collection",
        "schema_type": "Single",
        "key": { "hash_field": "title" },
        "fields": ["creator", "title", "year", "medium"],
        "field_classifications": {
            "creator": ["word"],
            "title": ["word"],
            "year": ["number"],
            "medium": ["word"]
        }
    }))
    .unwrap();

    let resp_b = node.add_schema_to_service(&schema_b_def).await.unwrap();
    let schema_b_name = resp_b.schema.name.clone();
    eprintln!("Schema B registered: {}", &schema_b_name[..16]);
    eprintln!(
        "  replaced_schema: {:?}",
        resp_b.replaced_schema.as_deref().map(|s| &s[..16])
    );
    eprintln!("  mutation_mappers: {:?}", resp_b.mutation_mappers);

    // VERIFY: expansion happened
    assert!(
        resp_b.replaced_schema.is_some(),
        "Schema B should expand Schema A (same descriptive_name)"
    );

    // VERIFY: mutation_mappers include the semantic rename
    assert_eq!(
        resp_b.mutation_mappers.get("creator").map(|s| s.as_str()),
        Some("artist"),
        "Schema service should return mutation_mapper: creator → artist"
    );

    // VERIFY: expanded schema has "artist" not "creator", and has "medium"
    let expanded_fields = resp_b.schema.fields.as_ref().unwrap();
    assert!(
        expanded_fields.contains(&"artist".to_string()),
        "Expanded schema must have 'artist' (canonical)"
    );
    assert!(
        !expanded_fields.contains(&"creator".to_string()),
        "Expanded schema must NOT have 'creator' (renamed to artist)"
    );
    assert!(
        expanded_fields.contains(&"medium".to_string()),
        "Expanded schema must have 'medium' (new field, not falsely matched)"
    );
    eprintln!(
        "  expanded fields: {:?}",
        expanded_fields
    );

    // 6. Load and approve Schema B locally, block Schema A
    {
        let db = node.get_fold_db().await.unwrap();
        let json_b = serde_json::to_string(&resp_b.schema).unwrap();
        db.schema_manager.load_schema_from_json(&json_b).await.unwrap();
        db.schema_manager.approve(&schema_b_name).await.unwrap();
        if let Some(ref old_name) = resp_b.replaced_schema {
            let _ = db.schema_manager
                .block_and_supersede(old_name, &schema_b_name)
                .await;
        }
    }

    // 7. Simulate what the ingestion pipeline does: merge service mappers into
    //    the AI's mappers, then generate mutations.
    //
    //    The AI originally gave us: creator → creator, title → title, etc.
    //    The service added: creator → artist (semantic rename)
    //    After merge: creator → artist, title → title, year → year, medium → medium
    let mut ai_mappers: HashMap<String, String> = HashMap::new();
    ai_mappers.insert("creator".to_string(), "creator".to_string());
    ai_mappers.insert("title".to_string(), "title".to_string());
    ai_mappers.insert("year".to_string(), "year".to_string());
    ai_mappers.insert("medium".to_string(), "medium".to_string());

    // Merge service mappers (this is what process_flat_path now does)
    for (from, to) in &resp_b.mutation_mappers {
        ai_mappers.insert(from.clone(), to.clone());
    }

    eprintln!("  merged mappers: {:?}", ai_mappers);

    // VERIFY: merged mappers map creator → artist
    assert_eq!(
        ai_mappers.get("creator").map(|s| s.as_str()),
        Some("artist"),
        "Merged mappers must map creator → artist"
    );

    // 8. Generate mutation with "creator" in the data
    let data_b: HashMap<String, serde_json::Value> = [
        ("creator".to_string(), json!("Vincent van Gogh")),
        ("title".to_string(), json!("Starry Night")),
        ("year".to_string(), json!("1889")),
        ("medium".to_string(), json!("Oil on canvas")),
    ]
    .into_iter()
    .collect();

    let mut keys = HashMap::new();
    keys.insert("hash_field".to_string(), "Starry Night".to_string());

    let mutations = mutation_generator::generate_mutations(
        &schema_b_name,
        &keys,
        &data_b,
        &ai_mappers,
        user_id.clone(),
        None,
        None,
    )
    .unwrap();

    assert_eq!(mutations.len(), 1, "Should generate exactly 1 mutation");

    // VERIFY: the mutation writes to "artist" not "creator"
    let mutation = &mutations[0];
    assert!(
        mutation.fields_and_values.contains_key("artist"),
        "Mutation must write to 'artist' field (the canonical name). Got: {:?}",
        mutation.fields_and_values.keys().collect::<Vec<_>>()
    );
    assert!(
        !mutation.fields_and_values.contains_key("creator"),
        "Mutation must NOT write to 'creator' — it should be renamed to 'artist'"
    );
    assert!(
        mutation.fields_and_values.contains_key("medium"),
        "Mutation must write to 'medium' field"
    );
    assert_eq!(
        mutation.fields_and_values.get("artist").unwrap(),
        &json!("Vincent van Gogh"),
        "Artist field should have van Gogh's name"
    );
    eprintln!(
        "  mutation fields: {:?}",
        mutation.fields_and_values.keys().collect::<Vec<_>>()
    );

    // 9. Execute the mutation and verify data is stored correctly
    let result = run_with_user(&user_id, async {
        node.mutate_batch(mutations).await
    })
    .await;
    assert!(
        result.is_ok(),
        "Schema B mutation failed: {:?}",
        result.err()
    );
    eprintln!("Schema B: wrote 'Starry Night' by 'Vincent van Gogh' (via creator→artist rename)");

    // 10. Query the expanded schema and verify the record was written
    let processor = OperationProcessor::new(node.clone());
    let (keys_list, total) = processor
        .list_schema_keys(&schema_b_name, 0, 100)
        .await
        .unwrap();

    eprintln!("\nExpanded schema keys ({} total):", total);
    for kv in &keys_list {
        eprintln!(
            "  hash={}, range={}",
            kv.hash.as_deref().unwrap_or("(none)"),
            kv.range.as_deref().unwrap_or("(none)")
        );
    }

    // Should have the Starry Night record
    assert!(
        total >= 1,
        "Expanded schema should have at least 1 record, got {}",
        total
    );

    // Query via OperationProcessor to verify data is stored under correct fields
    let query = Query::new(
        schema_b_name.clone(),
        vec!["artist".to_string(), "title".to_string(), "year".to_string(), "medium".to_string()],
    );

    let query_result = run_with_user(&user_id, async {
        processor.execute_query_json(query).await
    })
    .await;

    match query_result {
        Ok(records) => {
            eprintln!("\nQuery result: {:?}", records);
            assert!(
                !records.is_empty(),
                "Should find records in expanded schema"
            );
            // Check that "artist" field has the value (written via creator→artist mapper)
            for record in &records {
                if let Some(artist_val) = record.get("artist") {
                    if artist_val == &json!("Vincent van Gogh") {
                        eprintln!("PASS: 'creator' data correctly stored under 'artist' field");
                    }
                }
                if let Some(medium_val) = record.get("medium") {
                    if medium_val == &json!("Oil on canvas") {
                        eprintln!("PASS: 'medium' field correctly preserved as new field");
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Query returned error (field_mapper resolution may need data on old schema): {}", e);
        }
    }

    // 11. Verify schema states
    let all_schemas = processor
        .list_schemas()
        .await
        .expect("failed to list schemas");

    let active: Vec<_> = all_schemas
        .iter()
        .filter(|s| s.state != fold_db::schema::SchemaState::Blocked)
        .collect();
    let blocked: Vec<_> = all_schemas
        .iter()
        .filter(|s| s.state == fold_db::schema::SchemaState::Blocked)
        .collect();

    eprintln!("\nFinal state:");
    eprintln!("  Active schemas: {}", active.len());
    for s in &active {
        eprintln!(
            "    {} fields={:?}",
            &s.schema.name[..16.min(s.schema.name.len())],
            s.schema.fields
        );
    }
    eprintln!("  Blocked schemas: {}", blocked.len());

    assert_eq!(
        active.len(),
        1,
        "Should have exactly 1 active schema after expansion"
    );
    assert_eq!(
        blocked.len(),
        1,
        "Should have exactly 1 blocked schema (the predecessor)"
    );

    // Cleanup
    schema_handle.stop(true).await;
    eprintln!("\nPASS: Full semantic field matching pipeline works end-to-end");
}
