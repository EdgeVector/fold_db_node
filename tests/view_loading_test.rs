use actix_web::{web, App, HttpResponse, HttpServer};
use fold_db::schema::types::field_value_type::FieldValueType;
use fold_db::schema::types::key_config::KeyConfig;
use fold_db::schema::types::operations::Query;
use fold_db::schema::types::schema::DeclarativeSchemaType as SchemaType;
use fold_db::schema::types::Schema;
use fold_db::storage::config::DatabaseConfig;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use std::collections::HashMap;
use std::net::TcpListener;

use fold_db::schema_service::types::StoredView;

/// Spawn a mock schema service that serves pre-configured schemas and views.
async fn spawn_mock_service(
    schemas: HashMap<String, Schema>,
    views: HashMap<String, StoredView>,
) -> (String, actix_web::dev::ServerHandle) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = listener.local_addr().unwrap();

    let schemas_data = web::Data::new(schemas);
    let views_data = web::Data::new(views);

    let server = HttpServer::new(move || {
        App::new()
            .app_data(schemas_data.clone())
            .app_data(views_data.clone())
            .route(
                "/api/schema/{name}",
                web::get().to(
                    |path: web::Path<String>,
                     schemas: web::Data<HashMap<String, Schema>>| async move {
                        let name = path.into_inner();
                        match schemas.get(&name) {
                            Some(schema) => HttpResponse::Ok().json(schema),
                            None => HttpResponse::NotFound()
                                .json(serde_json::json!({"error": "not found"})),
                        }
                    },
                ),
            )
            .route(
                "/api/view/{name}",
                web::get().to(
                    |path: web::Path<String>,
                     views: web::Data<HashMap<String, StoredView>>| async move {
                        let name = path.into_inner();
                        match views.get(&name) {
                            Some(view) => HttpResponse::Ok().json(view),
                            None => HttpResponse::NotFound()
                                .json(serde_json::json!({"error": "not found"})),
                        }
                    },
                ),
            )
    })
    .listen(listener)
    .unwrap()
    .run();

    let handle = server.handle();
    actix_web::rt::spawn(server);
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    (format!("http://{}", addr), handle)
}

/// Create a minimal schema with typed fields for testing.
/// `output_fields` are the fields that appear in the schema definition.
/// The schema uses Range type with `date` as the range key.
fn make_test_schema(name: &str, fields: &[&str]) -> Schema {
    let mut schema = Schema::new(
        name.to_string(),
        SchemaType::Range,
        Some(KeyConfig::new(None, Some("date".to_string()))),
        Some(fields.iter().map(|f| f.to_string()).collect()),
        None,
        None,
    );
    schema.descriptive_name = Some(name.to_string());
    for field in fields {
        schema
            .field_types
            .insert(field.to_string(), FieldValueType::Any);
    }
    schema
}

/// Create an output schema for a view — only includes the queried fields.
fn make_output_schema(name: &str, fields: &[&str]) -> Schema {
    make_test_schema(name, fields)
}

/// Create a StoredView for testing.
fn make_stored_view(
    name: &str,
    source_schema: &str,
    source_fields: &[&str],
    output_schema_name: &str,
) -> StoredView {
    StoredView {
        name: name.to_string(),
        input_queries: vec![Query::new(
            source_schema.to_string(),
            source_fields.iter().map(|f| f.to_string()).collect(),
        )],
        transform_hash: None,
        wasm_bytes: None,
        output_schema_name: output_schema_name.to_string(),
        schema_type: SchemaType::Range,
    }
}

async fn make_node(schema_service_url: &str) -> FoldNode {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().to_path_buf();
    // Keep tempdir alive by leaking it (test-only)
    std::mem::forget(dir);

    // Test-only keys (valid ed25519 keypair, base64-encoded)
    let config = NodeConfig {
        database: DatabaseConfig::local(db_path),
        schema_service_url: Some(schema_service_url.to_string()),
        private_key: Some("AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=".to_string()),
        public_key: Some("O2onvM62pC1io6jQKm8Nc2UyFXcd4kOmOsBIoYtZ2ik=".to_string()),
        ..Default::default()
    };
    FoldNode::new(config).await.unwrap()
}

#[actix_web::test]
async fn load_view_with_schema_dependency() {
    // Output schema only has the queried field (title), matching the input query
    let output_schema = make_output_schema("output_schema_1", &["title"]);
    let source_schema = make_test_schema("SourceSchema", &["title", "date"]);

    let mut schemas = HashMap::new();
    schemas.insert("output_schema_1".to_string(), output_schema);
    schemas.insert("SourceSchema".to_string(), source_schema);

    let mut views = HashMap::new();
    views.insert(
        "MyView".to_string(),
        make_stored_view("MyView", "SourceSchema", &["title"], "output_schema_1"),
    );

    let (url, handle) = spawn_mock_service(schemas, views).await;
    let node = make_node(&url).await;

    let result = node.load_view_from_service("MyView").await.unwrap();

    assert!(
        result.loaded_views.contains(&"MyView".to_string()),
        "MyView should be in loaded_views"
    );
    assert!(
        result.loaded_schemas.contains(&"SourceSchema".to_string())
            || result
                .loaded_schemas
                .contains(&"output_schema_1".to_string()),
        "Schemas should be loaded"
    );

    // Verify view is locally registered
    let db = node.get_fold_db().unwrap();
    assert!(
        db.schema_manager.get_view("MyView").unwrap().is_some(),
        "MyView should be registered locally"
    );

    handle.stop(true).await;
}

#[actix_web::test]
async fn load_view_chain_resolves_dependencies() {
    // ViewB → ViewA → SourceSchema
    let output_a = make_output_schema("output_a", &["content"]);
    let output_b = make_output_schema("output_b", &["content"]);
    let source = make_test_schema("SourceSchema", &["content", "date"]);

    let mut schemas = HashMap::new();
    schemas.insert("output_a".to_string(), output_a);
    schemas.insert("output_b".to_string(), output_b);
    schemas.insert("SourceSchema".to_string(), source);

    let mut views = HashMap::new();
    views.insert(
        "ViewA".to_string(),
        make_stored_view("ViewA", "SourceSchema", &["content"], "output_a"),
    );
    views.insert(
        "ViewB".to_string(),
        make_stored_view("ViewB", "ViewA", &["content"], "output_b"),
    );

    let (url, handle) = spawn_mock_service(schemas, views).await;
    let node = make_node(&url).await;

    let result = node.load_view_from_service("ViewB").await.unwrap();

    assert!(result.loaded_views.contains(&"ViewA".to_string()));
    assert!(result.loaded_views.contains(&"ViewB".to_string()));

    // Both views should be registered locally
    let db = node.get_fold_db().unwrap();
    assert!(db.schema_manager.get_view("ViewA").unwrap().is_some());
    assert!(db.schema_manager.get_view("ViewB").unwrap().is_some());

    handle.stop(true).await;
}

#[actix_web::test]
async fn load_view_missing_on_service_errors() {
    let (url, handle) = spawn_mock_service(HashMap::new(), HashMap::new()).await;
    let node = make_node(&url).await;

    let result = node.load_view_from_service("NonExistent").await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("NonExistent"),
        "Error should mention the missing view name, got: {}",
        err
    );

    handle.stop(true).await;
}

#[actix_web::test]
async fn load_view_skips_already_loaded() {
    let output_schema = make_output_schema("output_schema_1", &["title"]);
    let source_schema = make_test_schema("SourceSchema", &["title", "date"]);

    let mut schemas = HashMap::new();
    schemas.insert("output_schema_1".to_string(), output_schema);
    schemas.insert("SourceSchema".to_string(), source_schema);

    let mut views = HashMap::new();
    views.insert(
        "MyView".to_string(),
        make_stored_view("MyView", "SourceSchema", &["title"], "output_schema_1"),
    );

    let (url, handle) = spawn_mock_service(schemas, views).await;
    let node = make_node(&url).await;

    // Load once
    node.load_view_from_service("MyView").await.unwrap();

    // Load again — should skip
    let result = node.load_view_from_service("MyView").await.unwrap();
    assert!(
        result.already_loaded.contains(&"MyView".to_string()),
        "Second load should report MyView as already loaded"
    );
    assert!(
        result.loaded_views.is_empty(),
        "No new views should be loaded"
    );

    handle.stop(true).await;
}

#[actix_web::test]
async fn load_view_circular_dependency_errors() {
    // ViewA depends on ViewB, ViewB depends on ViewA
    let output_a = make_test_schema("output_a", &["x", "date"]);
    let output_b = make_test_schema("output_b", &["x", "date"]);

    let mut schemas = HashMap::new();
    schemas.insert("output_a".to_string(), output_a);
    schemas.insert("output_b".to_string(), output_b);

    let mut views = HashMap::new();
    views.insert(
        "ViewA".to_string(),
        make_stored_view("ViewA", "ViewB", &["x"], "output_a"),
    );
    views.insert(
        "ViewB".to_string(),
        make_stored_view("ViewB", "ViewA", &["x"], "output_b"),
    );

    let (url, handle) = spawn_mock_service(schemas, views).await;
    let node = make_node(&url).await;

    let result = node.load_view_from_service("ViewA").await;

    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("Circular") || err.contains("already being loaded"),
        "Should detect circular dependency, got: {}",
        err
    );

    handle.stop(true).await;
}

#[actix_web::test]
async fn load_view_converts_stored_view_fields_correctly() {
    let mut output_schema = make_output_schema("typed_output", &["count", "name"]);
    output_schema
        .field_types
        .insert("count".to_string(), FieldValueType::Integer);
    output_schema
        .field_types
        .insert("name".to_string(), FieldValueType::String);

    let source = make_test_schema("Source", &["count", "name", "date"]);

    let mut schemas = HashMap::new();
    schemas.insert("typed_output".to_string(), output_schema);
    schemas.insert("Source".to_string(), source);

    let mut views = HashMap::new();
    views.insert(
        "TypedView".to_string(),
        make_stored_view("TypedView", "Source", &["count", "name"], "typed_output"),
    );

    let (url, handle) = spawn_mock_service(schemas, views).await;
    let node = make_node(&url).await;

    node.load_view_from_service("TypedView").await.unwrap();

    let db = node.get_fold_db().unwrap();
    let view = db.schema_manager.get_view("TypedView").unwrap().unwrap();

    assert_eq!(
        view.output_fields.get("count"),
        Some(&FieldValueType::Integer),
        "count should be Integer type from output schema"
    );
    assert_eq!(
        view.output_fields.get("name"),
        Some(&FieldValueType::String),
        "name should be String type from output schema"
    );

    handle.stop(true).await;
}
