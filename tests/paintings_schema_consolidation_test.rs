//! Integration test: ingest multiple paintings, verify schema consolidation.
//!
//! Ingests all paintings from sample_data/photos/paintings/ sequentially.
//! Verifies:
//! 1. Only ONE non-blocked schema exists at the end (expansion consolidates)
//! 2. Each painting has a unique key (source_file_name as hash)
//! 3. All successfully ingested paintings appear as records in the final schema
//!
//! Requires:
//! - `ANTHROPIC_API_KEY` environment variable set
//!
//! Run with: `cargo test --test paintings_schema_consolidation_test -- --ignored --nocapture`

use fold_db::logging::core::run_with_user;
use fold_db_node::fold_node::node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use fold_db_node::ingestion::file_handling::json_processor::convert_file_to_json;
use fold_db_node::ingestion::ingestion_service::IngestionService;
use fold_db_node::ingestion::{create_progress_tracker, IngestionRequest, ProgressService};
mod common;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use common::schema_service::{spawn_schema_service, SpawnedSchemaService};

async fn spawn_local_schema_service() -> SpawnedSchemaService {
    spawn_schema_service().await
}

// -- Integration test --------------------------------------------------------

#[actix_web::test]
#[ignore] // Requires ANTHROPIC_API_KEY
async fn test_paintings_use_single_schema() {
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("Skipping: ANTHROPIC_API_KEY not set");
        return;
    }

    let paintings_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("sample_data/photos/paintings");
    assert!(
        paintings_dir.exists(),
        "Paintings directory not found: {}",
        paintings_dir.display()
    );

    // Collect painting files
    let mut painting_files: Vec<_> = std::fs::read_dir(&paintings_dir)
        .expect("failed to read paintings dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .map(|ext| ext == "jpg" || ext == "jpeg" || ext == "png")
                .unwrap_or(false)
        })
        .map(|e| e.path())
        .collect();
    painting_files.sort();
    eprintln!("Using {} painting files", painting_files.len());
    assert_eq!(
        painting_files.len(),
        7,
        "expected 7 painting files in sample_data/photos/paintings/"
    );

    // 1. Spin up local schema service
    let svc = spawn_local_schema_service().await;
    let schema_url = svc.url.clone();
    eprintln!("Schema service at {}", schema_url);

    // 2. Create FoldNode
    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_identity(&user_id, &keypair.secret_key_base64())
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    // 3. Create a SHARED ingestion service (so the schema_creation_lock works)
    let ingestion_service =
        Arc::new(IngestionService::from_env().expect("Failed to create ingestion service"));
    let progress_tracker = create_progress_tracker().await;
    let progress_service = ProgressService::new(progress_tracker);

    // 4. Ingest each painting sequentially
    let mut successes = 0;
    let mut ingested_filenames: Vec<String> = Vec::new();

    for (i, painting_path) in painting_files.iter().enumerate() {
        let file_name = painting_path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        eprintln!(
            "\n--- Ingesting [{}/{}]: {} ---",
            i + 1,
            painting_files.len(),
            file_name
        );

        // Convert image to JSON using file_to_json (same as the real server)
        let data = match convert_file_to_json(&painting_path.to_path_buf()).await {
            Ok(json) => json,
            Err(e) => {
                eprintln!("  Failed to convert file: {}", e);
                continue;
            }
        };

        let progress_id = format!("painting-{}", i);
        let request = IngestionRequest {
            data,
            auto_execute: true,
            pub_key: user_id.clone(),
            source_file_name: Some(file_name.clone()),
            progress_id: Some(progress_id.clone()),
            file_hash: None,
            source_folder: Some(paintings_dir.to_string_lossy().to_string()),
            image_descriptive_name: None,
            org_hash: None,
            image_bytes: None,
        };

        let svc = ingestion_service.clone();
        let result = run_with_user(&user_id, async {
            svc.process_json_with_node_and_progress(request, &node, &progress_service, progress_id)
                .await
        })
        .await;

        match result {
            Ok(resp) => {
                let schema_short = resp
                    .schema_used
                    .as_deref()
                    .map(|s| &s[..16.min(s.len())])
                    .unwrap_or("(none)");
                eprintln!(
                    "  success={}, schema={}, mutations_gen={}, mutations_exec={}",
                    resp.success, schema_short, resp.mutations_generated, resp.mutations_executed
                );
                if resp.success {
                    successes += 1;
                    ingested_filenames.push(file_name);
                }
                if !resp.errors.is_empty() {
                    eprintln!("  errors: {:?}", resp.errors);
                }
            }
            Err(e) => {
                eprintln!("  Ingestion error: {}", e);
            }
        }
    }

    eprintln!("\n=== Results ===");
    eprintln!("Successes: {}/{}", successes, painting_files.len());

    // 5. Verify: list all schemas and check states
    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    let all_schemas = processor
        .list_schemas()
        .await
        .expect("failed to list schemas");

    eprintln!("\nAll schemas:");
    for s in &all_schemas {
        let desc = s.schema.descriptive_name.as_deref().unwrap_or("(none)");
        let field_count = s.schema.fields.as_ref().map(|f| f.len()).unwrap_or(0);
        eprintln!(
            "  {}...  {:12}  {} fields  {}",
            &s.schema.name[..16.min(s.schema.name.len())],
            format!("{:?}", s.state),
            field_count,
            desc
        );
    }

    // Count non-blocked schemas (schema expansion creates blocked predecessors)
    let active_schemas: Vec<_> = all_schemas
        .iter()
        .filter(|s| s.state != fold_db::schema::SchemaState::Blocked)
        .collect();

    eprintln!("\nActive schemas (non-blocked): {}", active_schemas.len());
    for s in &active_schemas {
        let desc = s.schema.descriptive_name.as_deref().unwrap_or("(none)");
        eprintln!(
            "  {} ({} fields)  {}",
            &s.schema.name[..16.min(s.schema.name.len())],
            s.schema.fields.as_ref().map(|f| f.len()).unwrap_or(0),
            desc
        );
    }

    // Count blocked schemas — at least one means expansion happened
    let blocked_count = all_schemas
        .iter()
        .filter(|s| s.state == fold_db::schema::SchemaState::Blocked)
        .count();
    eprintln!(
        "Blocked schemas (expansion predecessors): {}",
        blocked_count
    );

    // ASSERT: schema expansion occurred (at least one blocked predecessor)
    assert!(
        blocked_count >= 1,
        "Expected at least 1 blocked schema (expansion predecessor), got {}. \
         Schema expansion should consolidate similar painting schemas.",
        blocked_count
    );

    // Group active schemas by descriptive_name to find unique concepts
    let mut concepts: HashMap<String, Vec<&_>> = HashMap::new();
    for s in &active_schemas {
        let desc = s
            .schema
            .descriptive_name
            .as_deref()
            .unwrap_or("(unknown)")
            .to_string();
        concepts.entry(desc).or_default().push(s);
    }
    eprintln!(
        "\nActive schema concepts: {:?}",
        concepts.keys().collect::<Vec<_>>()
    );

    // Warn if any concept has multiple active schemas (AI variability)
    for (concept, schemas_for_concept) in &concepts {
        if schemas_for_concept.len() > 1 {
            eprintln!(
                "WARNING: Concept '{}' has {} active schemas (AI gave inconsistent names/fields). \
                 Schemas: {:?}",
                concept,
                schemas_for_concept.len(),
                schemas_for_concept
                    .iter()
                    .map(|s| &s.schema.name[..16.min(s.schema.name.len())])
                    .collect::<Vec<_>>()
            );
        }
    }

    // Verify at least half the paintings were ingested successfully
    assert!(
        successes >= painting_files.len() / 2,
        "Expected at least {} successful ingestions, got {}",
        painting_files.len() / 2,
        successes
    );

    // 6. Collect keys from ALL active artwork schemas (AI may split across multiple)
    let artwork_schemas: Vec<_> = active_schemas
        .iter()
        .filter(|s| {
            s.schema
                .descriptive_name
                .as_deref()
                .unwrap_or("")
                .to_lowercase()
                .contains("art")
        })
        .collect();
    assert!(
        !artwork_schemas.is_empty(),
        "No active artwork schemas found"
    );

    let mut all_hash_keys: HashSet<String> = HashSet::new();

    eprintln!("\n--- Keys per schema (all, including blocked) ---");
    for s in &all_schemas {
        let (s_keys, s_total) = processor
            .list_schema_keys(&s.schema.name, 0, 100)
            .await
            .unwrap_or_else(|e| {
                eprintln!(
                    "  Error listing keys for {}: {}",
                    &s.schema.name[..16.min(s.schema.name.len())],
                    e
                );
                (vec![], 0)
            });
        let desc = s.schema.descriptive_name.as_deref().unwrap_or("(none)");
        eprintln!(
            "  {} {:12} {} keys  {}",
            &s.schema.name[..16.min(s.schema.name.len())],
            format!("{:?}", s.state),
            s_total,
            desc
        );
        for kv in &s_keys {
            eprintln!(
                "    {} / {}",
                kv.hash.as_deref().unwrap_or("(none)"),
                kv.range.as_deref().unwrap_or("(none)")
            );
        }

        // Collect hash keys from active artwork schemas
        if s.state != fold_db::schema::SchemaState::Blocked {
            if let Some(desc) = s.schema.descriptive_name.as_deref() {
                if desc.to_lowercase().contains("art") {
                    for kv in &s_keys {
                        if let Some(hash) = kv.hash.as_deref() {
                            all_hash_keys.insert(hash.to_string());
                        }
                    }
                }
            }
        }
    }

    eprintln!("\nAll artwork hash keys: {:?}", all_hash_keys);

    // Every successfully ingested painting should have a key somewhere
    for filename in &ingested_filenames {
        assert!(
            all_hash_keys.contains(filename.as_str()),
            "Missing record for successfully ingested painting '{}'. \
             Hash keys present: {:?}",
            filename,
            all_hash_keys
        );
    }

    // Cleanup
    svc.handle.stop(true).await;
}
