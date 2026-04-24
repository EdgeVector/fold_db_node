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
use fold_db_node::fold_node::node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use fold_db_node::ingestion::ingestion_service::IngestionService;
use fold_db_node::ingestion::smart_folder::read_file_with_hash;
use fold_db_node::ingestion::{create_progress_tracker, IngestionRequest, ProgressService};
mod common;

use std::path::Path;

use common::schema_service::{spawn_schema_service, SpawnedSchemaService};

async fn spawn_local_schema_service() -> SpawnedSchemaService {
    spawn_schema_service().await
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

    let svc = spawn_local_schema_service().await;
    let schema_url = svc.url.clone();

    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair))
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

    svc.handle.stop(true).await;
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

    let svc = spawn_local_schema_service().await;
    let schema_url = svc.url.clone();

    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair))
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

    svc.handle.stop(true).await;
}
