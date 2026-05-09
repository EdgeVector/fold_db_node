//! Regression test: Apple Notes ingestion routes every record into a single
//! canonical schema regardless of LLM non-determinism.
//!
//! Reproduces the dogfood symptom from 2026-05-09 where 132 imported notes
//! fragmented across `Project Notes` (1), `Content Documents` (40), and
//! `Documents` (62) because each batch independently went through the LLM
//! schema classifier and got a different `descriptive_name` back.
//!
//! With `IngestionRequest::forced_schema_descriptive_name = Some("Apple Notes")`
//! the LLM is bypassed and a deterministic schema definition is synthesized
//! from the data sample. Multi-batch ingestion must consolidate into one
//! active schema with the expected record count.
//!
//! Hermetic — no `ANTHROPIC_API_KEY` required. The test runs in CI.
//!
//! Run with: `cargo test --test apple_notes_canonical_schema_test`

use fold_db::user_context::run_with_user;
use fold_db_node::fold_node::node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use fold_db_node::ingestion::ingestion_service::IngestionService;
use fold_db_node::ingestion::{create_progress_tracker, IngestionRequest, ProgressService};

use serde_json::json;
use std::path::Path;
use std::sync::Arc;

mod common;

use common::schema_service::spawn_schema_service;

/// Build five mock Apple Notes records — same field shape as the
/// production extractor (`apple_import::notes::to_json_records`) so the
/// synthesized canonical schema matches what real Apple Notes ingestion
/// would land on.
fn mock_note_records() -> Vec<serde_json::Value> {
    let bodies = [
        ("Meeting prep", "Quarterly review agenda and action items"),
        ("Recipe ideas", "Sourdough variations to try this weekend"),
        ("Travel notes", "Things to check before the Lisbon trip"),
        (
            "Work shower thoughts",
            "Random hallway-test ideas to stress-test next sprint",
        ),
        ("Reading list", "Articles and papers to read this week"),
    ];
    bodies
        .iter()
        .enumerate()
        .map(|(i, (title, body))| {
            json!({
                "title": title,
                "body": body,
                "created_at": format!("2026-05-0{}", i + 1),
                "modified_at": format!("2026-05-0{}", i + 2),
                // Matches the content_hash format produced by
                // `apple_import::content_hash` (16-hex-char SHA256 prefix).
                // The exact value is not load-bearing for the test — only
                // uniqueness and presence are.
                "content_hash": format!("{:016x}", 0xc0ffee_u64.wrapping_add(i as u64)),
                "source": "apple_notes",
            })
        })
        .collect()
}

#[actix_web::test]
async fn apple_notes_consolidate_into_one_canonical_schema() {
    // 1. Local schema service.
    let svc = spawn_schema_service().await;
    let schema_url = svc.url.clone();

    // 2. FoldNode pointing at the in-process schema service.
    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair))
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    // 3. Ingestion service with default config — no AI provider configured.
    //    The forced-schema path bypasses the readiness gate and the LLM
    //    call entirely, so this construction is sufficient.
    let ingestion_service = Arc::new(
        IngestionService::from_config_dir(Path::new("/nonexistent/folddb-test-config"))
            .unwrap_or_else(|_| {
                IngestionService::new(
                    Path::new("/nonexistent/folddb-test-config").to_path_buf(),
                    fold_db_node::ingestion::IngestionConfig::default(),
                )
                .expect("construct IngestionService with default config")
            }),
    );
    let progress_tracker = create_progress_tracker().await;
    let progress_service = ProgressService::new(progress_tracker);

    // 4. Send each of the 5 records as its own ingestion call.
    //    The single-record-per-call shape mirrors the multi-batch fragmentation
    //    that bit production: each call is an independent classifier opportunity,
    //    and without the forced schema, each one could pick a different name.
    let records = mock_note_records();
    for (i, record) in records.iter().enumerate() {
        let progress_id = format!("apple-notes-mock-{}", i);
        let request = IngestionRequest {
            data: serde_json::Value::Array(vec![record.clone()]),
            auto_execute: true,
            pub_key: user_id.clone(),
            source_file_name: None,
            progress_id: Some(progress_id.clone()),
            file_hash: None,
            source_folder: None,
            image_descriptive_name: None,
            org_hash: None,
            image_bytes: None,
            forced_schema_descriptive_name: Some("Apple Notes".to_string()),
        };

        let svc = ingestion_service.clone();
        let pid = progress_id.clone();
        let result = run_with_user(&user_id, async {
            svc.process_json_with_node_and_progress(request, &node, &progress_service, pid)
                .await
        })
        .await;

        let response = result.expect("ingestion call returned Err");
        assert!(
            response.success,
            "record {} ingestion failed: {:?}",
            i, response.errors
        );
        assert!(
            response.mutations_executed >= 1,
            "record {} produced no executed mutations",
            i
        );
    }

    // 5. Assert: one active schema with descriptive_name="Apple Notes" and
    //    exactly 5 records.
    let processor = OperationProcessor::new(Arc::new(node.clone()));
    let all_schemas = processor.list_schemas().await.expect("list_schemas failed");

    let apple_notes_schemas: Vec<_> = all_schemas
        .iter()
        .filter(|s| s.state != fold_db::schema::SchemaState::Blocked)
        .filter(|s| {
            s.schema
                .descriptive_name
                .as_deref()
                .map(|d| d == "Apple Notes")
                .unwrap_or(false)
        })
        .collect();

    assert_eq!(
        apple_notes_schemas.len(),
        1,
        "expected exactly one active schema with descriptive_name='Apple Notes', \
         got {}. All active descriptive_names: {:?}",
        apple_notes_schemas.len(),
        all_schemas
            .iter()
            .filter(|s| s.state != fold_db::schema::SchemaState::Blocked)
            .map(|s| s.schema.descriptive_name.as_deref().unwrap_or("(none)"))
            .collect::<Vec<_>>()
    );

    let schema_name = apple_notes_schemas[0].schema.name.clone();
    let count = processor
        .count_schema_records(&schema_name)
        .await
        .expect("count_schema_records failed");

    assert_eq!(
        count,
        records.len(),
        "expected {} records in canonical schema '{}', got {}",
        records.len(),
        schema_name,
        count,
    );
}
