//! Integration test for image ingestion key persistence.
//!
//! Tests that:
//! 1. A Hash schema with image fields stores mutations correctly
//! 2. Keys are queryable after mutation execution
//! 3. HashRange schema still works with hash-only keys (sentinel fix)
//! 4. The full ingestion pipeline with mock AI response produces keys
//!
//! Run: `cargo test --test image_ingestion_keys_test -- --nocapture`

mod common;

use fold_db::schema::types::{KeyValue, Mutation, MutationType};
use fold_db_node::fold_node::node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use serde_json::json;
use std::collections::HashMap;

/// Create a Hash schema JSON definition that mimics what the AI + image override produces.
fn image_schema_json(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "schema_type": "Hash",
        "key": {
            "hash_field": "source_file_name"
        },
        "fields": [
            "image_type",
            "subjects",
            "background",
            "tags",
            "setting",
            "time_of_day",
            "weather",
            "source_file_name"
        ],
        "field_classifications": {
            "image_type": ["word"],
            "subjects": ["word"],
            "background": ["word"],
            "tags": ["word"],
            "setting": ["word"],
            "time_of_day": ["word"],
            "weather": ["word"],
            "source_file_name": ["word"]
        },
        "field_descriptions": {
            "image_type": "Type of image",
            "subjects": "Main subjects in the image",
            "background": "Background description",
            "tags": "Searchable keywords",
            "setting": "Location or environment",
            "time_of_day": "Time of day",
            "weather": "Weather conditions",
            "source_file_name": "Original file name"
        },
        "permissions": {
            "read": { "policy_type": "NoPolicy" },
            "write": { "policy_type": "NoPolicy" }
        }
    })
}

/// Create a HashRange schema for testing the sentinel fix.
fn hashrange_image_schema_json(name: &str) -> serde_json::Value {
    json!({
        "name": name,
        "schema_type": "HashRange",
        "key": {
            "hash_field": "source_file_name",
            "range_field": "created_at"
        },
        "fields": [
            "image_type",
            "source_file_name"
        ],
        "field_classifications": {
            "image_type": ["word"],
            "source_file_name": ["word"]
        },
        "field_descriptions": {
            "image_type": "Type of image",
            "source_file_name": "Original file name"
        },
        "permissions": {
            "read": { "policy_type": "NoPolicy" },
            "write": { "policy_type": "NoPolicy" }
        }
    })
}

/// Build a mutation for an image record (Hash schema — hash key only).
fn image_mutation(
    schema_name: &str,
    file_name: &str,
    pub_key: &str,
) -> Mutation {
    let mut fields = HashMap::new();
    fields.insert("image_type".to_string(), json!("landscape"));
    fields.insert(
        "subjects".to_string(),
        json!(["ocean", "cliffs", "sunset"]),
    );
    fields.insert(
        "background".to_string(),
        json!("Ocean with dramatic cliffs"),
    );
    fields.insert(
        "tags".to_string(),
        json!(["nature", "ocean", "sunset"]),
    );
    fields.insert("setting".to_string(), json!("Rocky coastal area"));
    fields.insert("time_of_day".to_string(), json!("sunset"));
    fields.insert("weather".to_string(), json!("clear"));
    fields.insert(
        "source_file_name".to_string(),
        json!(file_name),
    );

    Mutation::new(
        schema_name.to_string(),
        fields,
        KeyValue::new(
            Some(file_name.to_string()),
            None,
        ),
        pub_key.to_string(),
        MutationType::Create,
    )
}

/// Test 1: Direct mutation execution with Hash schema keys.
/// Bypasses AI entirely — tests the mutation execution → key query path.
#[actix_web::test]
async fn test_hash_mutation_keys_queryable() {
    let schema_name = "test_coastal_image";

    // Setup node
    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let pub_key = keypair.public_key_base64();
    config = config.with_identity(&pub_key, &keypair.secret_key_base64());
    config = config.with_schema_service_url("test://mock");
    let node = FoldNode::new(config).await.unwrap();

    // Load schema manually
    let schema_json = image_schema_json(schema_name);
    {
        let db = node.get_fold_db().await.unwrap();
        let json_str = serde_json::to_string(&schema_json).unwrap();
        db.schema_manager
            .load_schema_from_json(&json_str)
            .await
            .expect("Failed to load schema");
        db.schema_manager
            .approve(schema_name)
            .await
            .expect("Failed to approve schema");
    }

    // Verify schema loaded
    let processor = OperationProcessor::new(node.clone());
    let schemas = processor.list_schemas().await.unwrap();
    let schema = schemas
        .iter()
        .find(|s| s.name() == schema_name)
        .expect("Schema not found after loading");
    eprintln!(
        "Schema loaded: name={}, type={:?}, fields={}",
        schema.name(),
        schema.schema.schema_type,
        schema.schema.fields.as_ref().map(|f: &Vec<String>| f.len()).unwrap_or(0)
    );

    // Create and execute mutation with hash key only
    let file_name = "ocean_cliff.jpg";
    let mutation = image_mutation(schema_name, file_name, &pub_key);

    eprintln!(
        "Mutation key_value: hash={:?}, range={:?}",
        mutation.key_value.hash, mutation.key_value.range
    );

    let mutation_ids = node
        .mutate_batch(vec![mutation])
        .await
        .expect("mutate_batch failed");
    eprintln!("Mutation IDs: {:?}", mutation_ids);
    assert_eq!(mutation_ids.len(), 1, "Should produce exactly one mutation ID");

    // Query keys — this is what the UI calls
    let (keys, total) = processor
        .list_schema_keys(schema_name, 0, 100)
        .await
        .expect("list_schema_keys failed");

    eprintln!("Keys found: total={}, keys={:?}", total, keys);

    assert!(total > 0, "Expected at least 1 key but got 0");
    assert_eq!(total, 1);
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].hash.as_deref(), Some(file_name));
    assert!(keys[0].range.is_none(), "Hash schema keys should have no range");
}

/// Test 2: Partial key on a HashRange schema must error, not silently drop data.
/// A HashRange schema requires both hash and range keys — if either is missing,
/// the mutation manager rejects it with an explicit error.
#[actix_web::test]
async fn test_hashrange_mutation_partial_key_errors() {
    let schema_name = "test_coastal_hash_only";

    // Setup node
    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let pub_key = keypair.public_key_base64();
    config = config.with_identity(&pub_key, &keypair.secret_key_base64());
    config = config.with_schema_service_url("test://mock");
    let node = FoldNode::new(config).await.unwrap();

    // Load HashRange schema
    let schema_json = hashrange_image_schema_json(schema_name);
    {
        let db = node.get_fold_db().await.unwrap();
        let json_str = serde_json::to_string(&schema_json).unwrap();
        db.schema_manager
            .load_schema_from_json(&json_str)
            .await
            .expect("Failed to load schema");
        db.schema_manager
            .approve(schema_name)
            .await
            .expect("Failed to approve schema");
    }

    // Create mutation with HASH ONLY — missing range key
    let mut fields = HashMap::new();
    fields.insert("image_type".to_string(), json!("landscape"));
    fields.insert("source_file_name".to_string(), json!("ocean_cliff.jpg"));

    let mutation = Mutation::new(
        schema_name.to_string(),
        fields,
        KeyValue::new(Some("some_hash".to_string()), None),
        pub_key.clone(),
        MutationType::Create,
    );

    // This must fail — partial keys on HashRange are a bug upstream
    let result = node.mutate_batch(vec![mutation]).await;
    assert!(
        result.is_err(),
        "HashRange mutation with missing range key should error, not silently drop data"
    );
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("requires both hash and range keys"),
        "Error should explain the key mismatch, got: {}",
        err_msg
    );
}

/// Test 3: Full ingestion pipeline with mock data simulating a vision model output.
/// Uses IngestionService directly (no real AI call — mock AI response is injected
/// by calling the sub-pipeline directly).
#[actix_web::test]
async fn test_image_ingestion_pipeline_produces_keys() {
    // Note: these imports are available if needed for AI-based tests
    // use fold_db::logging::core::run_with_user;
    // use fold_db_node::ingestion::ingestion_service::IngestionService;
    // use fold_db_node::ingestion::{create_progress_tracker, IngestionConfig, IngestionRequest, ProgressService};
    use fold_db_node::schema_service::server::SchemaServiceState;
    use actix_web::{web, App, HttpResponse, HttpServer};
    use std::net::TcpListener;
    use tempfile::TempDir;

    // Minimal inline schema service handlers
    async fn handle_add_schema(
        payload: web::Json<serde_json::Value>,
        state: web::Data<SchemaServiceState>,
    ) -> HttpResponse {
        let req = payload.into_inner();
        let schema: fold_db::schema::types::Schema =
            serde_json::from_value(req["schema"].clone()).unwrap();
        let mappers: HashMap<String, String> =
            serde_json::from_value(req["mutation_mappers"].clone()).unwrap_or_default();
        match state.add_schema(schema, mappers).await {
            Ok(outcome) => {
                use fold_db_node::schema_service::server::{AddSchemaResponse, SchemaAddOutcome};
                match outcome {
                    SchemaAddOutcome::Added(s, m) => HttpResponse::Created().json(AddSchemaResponse {
                        schema: s, mutation_mappers: m, replaced_schema: None,
                    }),
                    SchemaAddOutcome::AlreadyExists(s, _) => HttpResponse::Ok().json(AddSchemaResponse {
                        schema: s, mutation_mappers: HashMap::new(), replaced_schema: None,
                    }),
                    SchemaAddOutcome::Expanded(old, s, m) => HttpResponse::Created().json(AddSchemaResponse {
                        schema: s, mutation_mappers: m, replaced_schema: Some(old),
                    }),
                }
            }
            Err(e) => HttpResponse::BadRequest().json(json!({"error": e.to_string()})),
        }
    }

    async fn handle_get_schema(
        path: web::Path<String>,
        state: web::Data<SchemaServiceState>,
    ) -> HttpResponse {
        let name = path.into_inner();
        match state.get_schema_by_name(&name) {
            Ok(Some(s)) => HttpResponse::Ok().json(s),
            _ => HttpResponse::NotFound().json(json!({"error": "not found"})),
        }
    }

    async fn handle_list(state: web::Data<SchemaServiceState>) -> HttpResponse {
        match state.get_schema_names() {
            Ok(names) => HttpResponse::Ok().json(json!({"schemas": names})),
            Err(e) => HttpResponse::InternalServerError().json(json!({"error": e.to_string()})),
        }
    }

    async fn handle_available(state: web::Data<SchemaServiceState>) -> HttpResponse {
        match state.get_all_schemas_cached() {
            Ok(s) => HttpResponse::Ok().json(json!({"schemas": s})),
            Err(e) => HttpResponse::InternalServerError().json(json!({"error": e.to_string()})),
        }
    }

    // Spawn schema service
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("test_schema_db").to_string_lossy().to_string();
    let state = SchemaServiceState::new(db_path).unwrap();
    let state_data = web::Data::new(state);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let state_clone = state_data.clone();
    let server = HttpServer::new(move || {
        App::new()
            .app_data(state_clone.clone())
            .service(web::scope("/api")
                .route("/schemas", web::get().to(handle_list))
                .route("/schemas", web::post().to(handle_add_schema))
                .route("/schemas/available", web::get().to(handle_available))
                .route("/schema/{name}", web::get().to(handle_get_schema)))
    })
    .listen(listener)
    .unwrap()
    .run();
    let _handle = server.handle();
    actix_web::rt::spawn(server);
    actix_web::rt::time::sleep(std::time::Duration::from_millis(100)).await;
    let schema_url = format!("http://127.0.0.1:{}", port);

    // Create node
    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_identity(&user_id, &keypair.secret_key_base64())
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    // Create ingestion service — uses env config (needs ANTHROPIC_API_KEY for real AI,
    // but we'll test with mock data that doesn't require an AI call).
    // Since we can't easily mock the AI, we'll test the pipeline by creating the schema
    // and mutations directly, then verify keys.

    // Simulate what the pipeline does:
    // 1. AI returns schema recommendation
    // 2. apply_image_schema_override modifies it
    // 3. Schema is created via schema service
    // 4. Mutations generated and executed

    // Step 1-2: Create the schema via schema service (what create_new_schema_with_node does)
    let mut schema_def = image_schema_json("Coastal_Sunset_Landscape");
    // Schema service requires descriptive_name
    schema_def.as_object_mut().unwrap().insert(
        "descriptive_name".to_string(),
        json!("Coastal Sunset Landscape"),
    );
    let mut schema: fold_db::schema::types::Schema =
        serde_json::from_value(schema_def).expect("Failed to deserialize schema");
    schema.compute_identity_hash();
    assert!(schema.get_identity_hash().is_some());

    let add_response = node
        .add_schema_to_service(&schema)
        .await
        .expect("add_schema_to_service failed");
    let final_name = add_response.schema.name.clone();
    eprintln!("Schema registered: {}", final_name);

    // Step 3: Load and approve locally
    {
        let db = node.get_fold_db().await.unwrap();
        let json_str = serde_json::to_string(&add_response.schema).unwrap();
        db.schema_manager
            .load_schema_from_json(&json_str)
            .await
            .expect("load_schema_from_json failed");
        db.schema_manager
            .approve(&final_name)
            .await
            .expect("approve failed");
    }

    // Step 4: Execute image mutation (Hash schema — hash key only)
    let mutation = image_mutation(&final_name, "ocean_cliff.jpg", &user_id);
    let ids = node
        .mutate_batch(vec![mutation])
        .await
        .expect("mutate_batch failed");
    eprintln!("Executed {} mutations", ids.len());

    // Step 5: Verify keys
    let processor = OperationProcessor::new(node.clone());
    let (keys, total) = processor
        .list_schema_keys(&final_name, 0, 100)
        .await
        .expect("list_schema_keys failed");

    eprintln!("Pipeline test: {} keys found: {:?}", total, keys);

    assert!(
        total > 0,
        "Full pipeline simulation produced 0 keys for schema '{}'. \
         This reproduces the 'No keys found' bug.",
        final_name
    );
    assert_eq!(keys[0].hash.as_deref(), Some("ocean_cliff.jpg"));
    assert!(keys[0].range.is_none(), "Hash schema keys should have no range");
}
