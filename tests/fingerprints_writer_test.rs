//! Integration test: face extractor plan → schema service registration
//! → writer → query-back, end to end.
//!
//! Closes the loop that tests #15 (round-trip contract) and #17 (no-
//! bypass registration verification) only verified in pieces. This
//! test:
//!
//! 1. Spins up an in-process schema service
//! 2. Creates a FoldNode pointing at it
//! 3. Registers all twelve fingerprint schemas via
//!    `register_phase_1_schemas()`
//! 4. Runs the face extractor planner against a synthetic two-face
//!    photo to produce a FaceExtractionPlan
//! 5. Writes every planned record via `writer::write_records()`
//! 6. Queries back each canonical schema and verifies the expected
//!    records are present
//!
//! The schema service in-process setup is lifted from the pattern
//! established in tests/image_ingestion_keys_test.rs and tests/
//! fingerprints_registration_test.rs.

use actix_web::{web, App, HttpResponse, HttpServer};
use fold_db::schema::types::operations::Query;
use fold_db_node::fingerprints::canonical_names;
use fold_db_node::fingerprints::extractors::face::{plan_face_extraction, DetectedFace};
use fold_db_node::fingerprints::registration::register_phase_1_schemas;
use fold_db_node::fingerprints::schemas;
use fold_db_node::fingerprints::writer::write_records;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::{FoldNode, OperationProcessor};
use fold_db_node::schema_service::server::{
    AddSchemaResponse, SchemaAddOutcome, SchemaServiceState,
};
use serde_json::json;
use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Arc;
use tempfile::TempDir;

async fn handle_add_schema(
    payload: web::Json<serde_json::Value>,
    state: web::Data<SchemaServiceState>,
) -> HttpResponse {
    let req = payload.into_inner();
    let schema: fold_db::schema::types::Schema = match serde_json::from_value(req["schema"].clone())
    {
        Ok(s) => s,
        Err(e) => {
            return HttpResponse::BadRequest().json(json!({
                "error": format!("deserialize schema: {}", e)
            }))
        }
    };
    let mappers: HashMap<String, String> =
        serde_json::from_value(req["mutation_mappers"].clone()).unwrap_or_default();
    match state.add_schema(schema, mappers).await {
        Ok(outcome) => match outcome {
            SchemaAddOutcome::Added(s, m) => HttpResponse::Created().json(AddSchemaResponse {
                schema: s,
                mutation_mappers: m,
                replaced_schema: None,
            }),
            SchemaAddOutcome::AlreadyExists(s, _) => HttpResponse::Ok().json(AddSchemaResponse {
                schema: s,
                mutation_mappers: HashMap::new(),
                replaced_schema: None,
            }),
            SchemaAddOutcome::Expanded(old, s, m) => {
                HttpResponse::Created().json(AddSchemaResponse {
                    schema: s,
                    mutation_mappers: m,
                    replaced_schema: Some(old),
                })
            }
        },
        Err(e) => HttpResponse::BadRequest().json(json!({ "error": e.to_string() })),
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

async fn handle_list_schemas(state: web::Data<SchemaServiceState>) -> HttpResponse {
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

struct SpawnedService {
    url: String,
    _temp_dir: TempDir,
}

async fn spawn_schema_service() -> SpawnedService {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir
        .path()
        .join("writer_test_schema_registry")
        .to_string_lossy()
        .to_string();
    let state = SchemaServiceState::new(db_path).unwrap();
    fold_db_node::schema_service::builtin_schemas::seed(&state)
        .await
        .expect("seed built-in schemas");
    let state_data = web::Data::new(state);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let state_clone = state_data.clone();

    let server = HttpServer::new(move || {
        App::new().app_data(state_clone.clone()).service(
            web::scope("/api")
                .route("/schemas", web::get().to(handle_list_schemas))
                .route("/schemas", web::post().to(handle_add_schema))
                .route("/schemas/available", web::get().to(handle_available))
                .route("/schema/{name}", web::get().to(handle_get_schema)),
        )
    })
    .listen(listener)
    .unwrap()
    .run();

    actix_web::rt::spawn(server);
    actix_web::rt::time::sleep(std::time::Duration::from_millis(200)).await;

    SpawnedService {
        url: format!("http://127.0.0.1:{}", port),
        _temp_dir: temp_dir,
    }
}

async fn create_node(schema_service_url: &str) -> (Arc<FoldNode>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_string_lossy().to_string();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(path.into())
        .with_schema_service_url(schema_service_url)
        .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
    let node = FoldNode::new(config).await.expect("create FoldNode");
    (Arc::new(node), tmp)
}

fn synthetic_face(seed: f32) -> DetectedFace {
    DetectedFace {
        embedding: vec![seed; 512],
        bbox: [0.1, 0.2, 0.3, 0.4],
        confidence: 0.95,
    }
}

/// End-to-end: register schemas → plan → write → query back.
#[actix_web::test]
async fn face_extraction_plan_writes_all_records_through_schema_service() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;

    // 1. Schema registration — the real thing against the service.
    let reg = register_phase_1_schemas(&node)
        .await
        .expect("register_phase_1_schemas must succeed");
    assert_eq!(reg.total(), 12);

    // 2. Build a plan for a synthetic two-face photo.
    let faces = vec![synthetic_face(0.1), synthetic_face(0.2)];
    let plan = plan_face_extraction(
        "Photos",
        "IMG_writer_test_0001",
        &faces,
        "mn_writer_test_0001",
        "es_Photos:IMG_writer_test_0001:face_detect",
        "2026-04-14T18:00:00Z",
    );

    // Sanity check the plan shape before writing.
    assert!(!plan.ran_empty);
    assert_eq!(plan.count_for_schema(schemas::FINGERPRINT), 2);
    assert_eq!(plan.count_for_schema(schemas::MENTION), 1);
    assert_eq!(plan.count_for_schema(schemas::MENTION_BY_SOURCE), 1);
    assert_eq!(plan.count_for_schema(schemas::MENTION_BY_FINGERPRINT), 2);
    assert_eq!(plan.count_for_schema(schemas::EDGE), 1);
    assert_eq!(plan.count_for_schema(schemas::EDGE_BY_FINGERPRINT), 2);
    assert_eq!(plan.count_for_schema(schemas::EXTRACTION_STATUS), 1);

    // 3. Write every record via the writer, which resolves every
    //    descriptive_schema to a canonical name via canonical_names::lookup().
    let write_outcome = write_records(node.clone(), &plan.records)
        .await
        .expect("writer must succeed");

    // Writer outcome mirrors the planned counts by descriptive schema.
    assert_eq!(write_outcome.total(), plan.records.len());
    assert_eq!(write_outcome.count_for_descriptive(schemas::FINGERPRINT), 2);
    assert_eq!(write_outcome.count_for_descriptive(schemas::MENTION), 1);
    assert_eq!(write_outcome.count_for_descriptive(schemas::EDGE), 1);
    assert_eq!(
        write_outcome.count_for_descriptive(schemas::EDGE_BY_FINGERPRINT),
        2
    );
    assert_eq!(
        write_outcome.count_for_descriptive(schemas::MENTION_BY_FINGERPRINT),
        2
    );
    assert_eq!(
        write_outcome.count_for_descriptive(schemas::MENTION_BY_SOURCE),
        1
    );
    assert_eq!(
        write_outcome.count_for_descriptive(schemas::EXTRACTION_STATUS),
        1
    );

    // Every written record references a canonical schema that differs
    // from the descriptive name — this is the bypass-detector applied
    // to the writer (as distinct from the registration bypass-detector).
    for rec in &write_outcome.written {
        assert_ne!(
            rec.canonical_schema, rec.descriptive_schema,
            "writer must always write under canonical names, not descriptive labels"
        );
    }

    // 4. Query-back verification. For the Fingerprint schema, we
    //    expect two records after writing two faces.
    let processor = OperationProcessor::new(node.clone());
    let fingerprint_canonical = canonical_names::lookup(schemas::FINGERPRINT).unwrap();
    let query = Query {
        schema_name: fingerprint_canonical,
        fields: vec!["id".to_string(), "kind".to_string(), "value".to_string()],
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let results = processor
        .execute_query_json(query)
        .await
        .expect("fingerprint query must succeed");

    assert_eq!(
        results.len(),
        2,
        "expected 2 Fingerprint records after writing 2 faces, got {}",
        results.len()
    );
    // Every returned record should have kind = face_embedding.
    for record in &results {
        let fields = record.get("fields").expect("fields envelope");
        assert_eq!(fields["kind"], json!("face_embedding"));
    }

    // Query the Mention schema — exactly one record (one photo).
    let mention_canonical = canonical_names::lookup(schemas::MENTION).unwrap();
    let mention_query = Query {
        schema_name: mention_canonical,
        fields: vec![
            "id".to_string(),
            "source_schema".to_string(),
            "source_key".to_string(),
            "extractor".to_string(),
        ],
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let mention_results = processor
        .execute_query_json(mention_query)
        .await
        .expect("mention query must succeed");
    assert_eq!(mention_results.len(), 1);
    let mention = &mention_results[0];
    let fields = mention.get("fields").expect("fields envelope");
    assert_eq!(fields["source_schema"], json!("Photos"));
    assert_eq!(fields["source_key"], json!("IMG_writer_test_0001"));
    assert_eq!(fields["extractor"], json!("face_detect"));
}

/// Writer fails loudly when canonical_names registry is uninitialized.
#[actix_web::test]
async fn writer_fails_loudly_without_canonical_names_registered() {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;

    // Do NOT call register_phase_1_schemas. The canonical_names
    // registry is empty and no schemas exist on the node.
    let faces = vec![synthetic_face(0.1)];
    let plan = plan_face_extraction(
        "Photos",
        "IMG_unregistered_test",
        &faces,
        "mn_unregistered",
        "es_Photos:IMG_unregistered_test:face_detect",
        "2026-04-14T18:00:00Z",
    );

    let result = write_records(node.clone(), &plan.records).await;
    let err = result.expect_err("writer must fail without registered schemas");
    let msg = format!("{}", err);
    assert!(
        msg.contains("cannot resolve descriptive_schema")
            || msg.contains("registry not initialized")
            || msg.contains("no canonical name"),
        "expected loud lookup-failure error, got: {}",
        msg
    );
}
