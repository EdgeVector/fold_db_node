//! Integration test: AI-generated schema names must be semantic, not hashes.
//!
//! Ingests a subset of sample_data/ files (text, JSON, CSV) and asserts that:
//! 1. Schema names are human-readable (not 64-char hex hashes)
//! 2. Different content types get distinct schema names
//! 3. Text files (recipes, journal entries, medical notes) are NOT lumped into
//!    a single generic "document_content" schema
//!
//! Requires:
//! - `ANTHROPIC_API_KEY` environment variable set
//!
//! Run with: `cargo test --test semantic_schema_naming_test -- --ignored --nocapture`

use fold_db::logging::core::run_with_user;
use fold_db::schema_service::state::SchemaServiceState;
use fold_db::schema_service::types::{
    AddSchemaResponse, ErrorResponse, SchemaAddOutcome, SchemasListResponse,
};
use fold_db_node::fold_node::node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use fold_db_node::ingestion::ingestion_service::IngestionService;
use fold_db_node::ingestion::smart_folder::read_file_with_hash;
use fold_db_node::ingestion::{create_progress_tracker, IngestionRequest, ProgressService};
mod common;

use actix_web::{web, App, HttpResponse, HttpServer};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::TcpListener;
use std::path::Path;
use tempfile::TempDir;

// -- Inline schema service handlers (same as smart_folder_ingestion_test) -----

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
        Err(error) => HttpResponse::BadRequest().json(ErrorResponse {
            error: format!("Failed to add schema: {}", error),
        }),
    }
}

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

// -- Helpers ------------------------------------------------------------------

fn is_hash_name(name: &str) -> bool {
    name.len() == 64 && name.chars().all(|c| c.is_ascii_hexdigit())
}

/// Ingest a single file and return (schema_name, success).
async fn ingest_file(
    file_path: &Path,
    source_name: &str,
    user_id: &str,
    ingestion_service: &IngestionService,
    progress_service: &ProgressService,
    node: &FoldNode,
    idx: usize,
) -> Option<String> {
    let (json_data, file_hash, _) = read_file_with_hash(file_path).ok()?;
    let progress_id = format!("test-naming-{}", idx);

    let request = IngestionRequest {
        data: json_data,
        auto_execute: true,
        pub_key: user_id.to_string(),
        source_file_name: Some(source_name.to_string()),
        progress_id: Some(progress_id.clone()),
        file_hash: Some(file_hash),
        source_folder: Some(file_path.parent()?.to_string_lossy().to_string()),
        image_descriptive_name: None,
        org_hash: None,
        image_bytes: None,
    };

    let pid = progress_id.clone();
    let result = run_with_user(user_id, async {
        ingestion_service
            .process_json_with_node_and_progress(request, node, progress_service, pid)
            .await
    })
    .await;

    match result {
        Ok(resp) if resp.success => resp.schema_used,
        Ok(resp) => {
            eprintln!("  Ingestion failed for {}: {:?}", source_name, resp.errors);
            None
        }
        Err(e) => {
            eprintln!("  Ingestion error for {}: {}", source_name, e);
            None
        }
    }
}

// -- Tests --------------------------------------------------------------------

/// Core test: diverse sample files should produce semantic schema names.
#[actix_web::test]
#[ignore] // Requires ANTHROPIC_API_KEY
async fn test_schema_names_are_semantic_not_hashes() {
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("Skipping: ANTHROPIC_API_KEY not set");
        return;
    }

    let (schema_url, schema_handle, _schema_temp_dir) = spawn_local_schema_service().await;

    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_identity(&user_id, &keypair.secret_key_base64())
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    let ingestion_service =
        IngestionService::from_env().expect("Failed to create ingestion service");
    let progress_tracker = create_progress_tracker().await;
    let progress_service = ProgressService::new(progress_tracker);

    let sample_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("sample_data");

    // Test files spanning different content types
    let test_files: Vec<(&str, &str)> = vec![
        (
            "recipes/grandmas_cookies.txt",
            "recipes/grandmas_cookies.txt",
        ),
        ("journal/2025-01-15.txt", "journal/2025-01-15.txt"),
        ("health/doctor_visits.txt", "health/doctor_visits.txt"),
        ("health/medications.json", "health/medications.json"),
        ("contacts/address_book.json", "contacts/address_book.json"),
        ("blog_posts.json", "blog_posts.json"),
        ("meeting_notes.txt", "meeting_notes.txt"),
    ];

    let mut schema_names: Vec<(String, String)> = Vec::new(); // (file, schema_name)

    for (idx, (rel_path, source_name)) in test_files.iter().enumerate() {
        let full_path = sample_dir.join(rel_path);
        if !full_path.exists() {
            eprintln!("Skipping missing file: {}", rel_path);
            continue;
        }

        eprintln!("\nIngesting: {}", rel_path);
        if let Some(name) = ingest_file(
            &full_path,
            source_name,
            &user_id,
            &ingestion_service,
            &progress_service,
            &node,
            idx,
        )
        .await
        {
            eprintln!("  Schema name: {}", name);
            schema_names.push((rel_path.to_string(), name));
        }
    }

    // ── Assertions ──────────────────────────────────────────────────────

    assert!(
        schema_names.len() >= 4,
        "At least 4 files should ingest successfully, got {}",
        schema_names.len()
    );

    // 1. No schema name should be a 64-char hex hash
    let hash_names: Vec<_> = schema_names
        .iter()
        .filter(|(_, name)| is_hash_name(name))
        .collect();
    assert!(
        hash_names.is_empty(),
        "Schema names should be semantic, not hashes. Hash names found: {:?}",
        hash_names
    );

    // 2. No schema should be named after a file extension
    let extension_names: Vec<_> = schema_names
        .iter()
        .filter(|(_, name)| {
            let lower = name.to_lowercase();
            lower == "txt" || lower == "json" || lower == "csv" || lower == "md"
        })
        .collect();
    assert!(
        extension_names.is_empty(),
        "Schema names should not be file extensions: {:?}",
        extension_names
    );

    // 3. No generic "document" catch-all names
    let generic_names: Vec<_> = schema_names
        .iter()
        .filter(|(_, name)| {
            let lower = name.to_lowercase();
            lower.contains("document_content")
                || lower.contains("text_content")
                || lower.contains("file_content")
        })
        .collect();
    assert!(
        generic_names.is_empty(),
        "Schema names should be domain-specific, not generic 'document_content': {:?}",
        generic_names
    );

    // 4. Different content domains should produce different schema names
    //    (recipes vs journal vs health should not all share one schema)
    let unique_names: std::collections::HashSet<&str> =
        schema_names.iter().map(|(_, name)| name.as_str()).collect();
    assert!(
        unique_names.len() >= 3,
        "Different content types should produce at least 3 distinct schemas, got {}: {:?}",
        unique_names.len(),
        unique_names
    );

    // Print final summary
    eprintln!("\n=== Schema Naming Results ===");
    for (file, name) in &schema_names {
        eprintln!("  {} -> {}", file, name);
    }
    eprintln!(
        "  Unique schemas: {} from {} files",
        unique_names.len(),
        schema_names.len()
    );

    // List all schemas in the node
    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    let all_schemas = processor
        .list_schemas()
        .await
        .expect("Failed to list schemas");
    eprintln!("\n=== All Schemas in Node ===");
    for s in &all_schemas {
        eprintln!(
            "  {} (state={:?}, descriptive={:?})",
            s.name(),
            s.state,
            s.schema.descriptive_name
        );
    }

    schema_handle.stop(true).await;
    eprintln!("\nTest complete.");
}

/// Text files from different domains must NOT share a single schema.
#[actix_web::test]
#[ignore] // Requires ANTHROPIC_API_KEY
async fn test_text_files_get_distinct_schemas() {
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("Skipping: ANTHROPIC_API_KEY not set");
        return;
    }

    let (schema_url, schema_handle, _schema_temp_dir) = spawn_local_schema_service().await;

    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_identity(&user_id, &keypair.secret_key_base64())
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    let ingestion_service =
        IngestionService::from_env().expect("Failed to create ingestion service");
    let progress_tracker = create_progress_tracker().await;
    let progress_service = ProgressService::new(progress_tracker);

    let sample_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("sample_data");

    // Three text files from completely different domains
    let text_files = [
        (
            "recipes/grandmas_cookies.txt",
            "recipes/grandmas_cookies.txt",
        ),
        ("journal/2025-01-15.txt", "journal/2025-01-15.txt"),
        ("health/doctor_visits.txt", "health/doctor_visits.txt"),
    ];

    let mut results: Vec<(String, String)> = Vec::new();

    for (idx, (rel_path, source_name)) in text_files.iter().enumerate() {
        let full_path = sample_dir.join(rel_path);
        if !full_path.exists() {
            continue;
        }

        eprintln!("Ingesting: {}", rel_path);
        if let Some(name) = ingest_file(
            &full_path,
            source_name,
            &user_id,
            &ingestion_service,
            &progress_service,
            &node,
            idx,
        )
        .await
        {
            eprintln!("  -> schema: {}", name);
            results.push((rel_path.to_string(), name));
        }
    }

    assert!(
        results.len() >= 2,
        "At least 2 text files should ingest, got {}",
        results.len()
    );

    // All three should have DIFFERENT schema names
    let unique: std::collections::HashSet<&str> = results.iter().map(|(_, n)| n.as_str()).collect();

    eprintln!("\nResults:");
    for (file, name) in &results {
        eprintln!("  {} -> {}", file, name);
    }
    eprintln!("Unique schemas: {}", unique.len());

    assert_eq!(
        unique.len(),
        results.len(),
        "Each text file from a different domain should get its own schema. \
         Got {} unique schemas for {} files: {:?}",
        unique.len(),
        results.len(),
        results
    );

    schema_handle.stop(true).await;
}
