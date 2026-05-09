//! Integration test: pre-PR-#946 fragmented Apple-source records are
//! consolidated into the canonical schemas by the boot-time migration.
//!
//! Setup mirrors the dogfood symptom from 2026-05-09 — a 132-note import
//! that fragmented across "Project Notes", "Content Documents", and
//! "Documents". We simulate fragmentation by ingesting batches with
//! `forced_schema_descriptive_name` set to the **wrong** (non-canonical)
//! names, which exercises the same forced-schema synthesis path PR #946
//! introduced but pins each batch to a different descriptive_name. This
//! produces the same on-disk shape that the buggy classifier produced,
//! without needing an LLM in the test.
//!
//! Contract being verified:
//!   * After migration, the canonical "Apple Notes" schema holds every
//!     apple-sourced record that previously lived in any non-canonical
//!     schema (consolidation).
//!   * A second invocation of `run` is a content-hash-keyed no-op — the
//!     canonical schema record count does not change (idempotency).
//!   * `run_if_needed` writes the marker file on success and short-circuits
//!     on subsequent calls.
//!
//! NOT verified (intentional — see `apple_consolidation.rs` module docstring):
//! the fragmented schemas still hold their original records. fold_db is
//! append-only and `MutationType::Delete` doesn't actually remove molecule
//! entries, so the migration only consolidates forward; the canonical schema
//! becomes the source of truth and the legacy schemas hold a strict subset.
//!
//! Hermetic — no `ANTHROPIC_API_KEY` required.
//!
//! Run with: `cargo test --test apple_consolidation_migration_test`

use fold_db::user_context::run_with_user;
use fold_db_node::fold_node::migrations::apple_consolidation;
use fold_db_node::fold_node::node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use fold_db_node::ingestion::ingestion_service::IngestionService;
use fold_db_node::ingestion::{create_progress_tracker, IngestionRequest, ProgressService};

use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;

mod common;

use common::schema_service::spawn_schema_service;

/// Build N mock Apple Notes records with the same field shape the production
/// extractor (`apple_import::notes::to_json_records`) emits, indexed so each
/// `content_hash` is unique. The `source` field is the migration's detection
/// signal — the canonical key value PR #946's forced schema synthesizes.
fn mock_note_records(start_idx: usize, count: usize) -> Vec<Value> {
    (start_idx..start_idx + count)
        .map(|i| {
            json!({
                "title": format!("Note {}", i),
                "body": format!("Body of note {} — meeting notes / reminders / etc.", i),
                "created_at": format!("2026-05-{:02}", (i % 28) + 1),
                "modified_at": format!("2026-05-{:02}", (i % 28) + 1),
                "content_hash": format!("{:016x}", 0xc0ffee_u64.wrapping_add(i as u64)),
                "source": "apple_notes",
            })
        })
        .collect()
}

async fn ingest_into(
    node: &FoldNode,
    ingestion: &Arc<IngestionService>,
    progress: &ProgressService,
    user_id: &str,
    descriptive_name: &str,
    records: Vec<Value>,
) {
    let progress_id = format!("seed-{}", descriptive_name.replace(' ', "-").to_lowercase());
    let request = IngestionRequest {
        data: Value::Array(records),
        auto_execute: true,
        pub_key: user_id.to_string(),
        source_file_name: None,
        progress_id: Some(progress_id.clone()),
        file_hash: None,
        source_folder: None,
        image_descriptive_name: None,
        org_hash: None,
        image_bytes: None,
        forced_schema_descriptive_name: Some(descriptive_name.to_string()),
    };

    let svc = ingestion.clone();
    let pid = progress_id.clone();
    let response = run_with_user(user_id, async {
        svc.process_json_with_node_and_progress(request, node, progress, pid)
            .await
    })
    .await
    .expect("ingestion call returned Err");

    assert!(
        response.success,
        "seeding into '{}' failed: {:?}",
        descriptive_name, response.errors
    );
}

/// Count user-visible records in every active schema with the given
/// descriptive name. Uses `count_schema_records`, which walks the molecule's
/// declared keys — fine for assertions about consolidation since the
/// canonical schema starts empty in the test fixtures.
async fn count_in(processor: &OperationProcessor, descriptive_name: &str) -> usize {
    let all_schemas = processor.list_schemas().await.expect("list_schemas failed");
    let mut total = 0;
    for sws in &all_schemas {
        if sws.state == fold_db::schema::SchemaState::Blocked {
            continue;
        }
        if sws.schema.descriptive_name.as_deref() != Some(descriptive_name) {
            continue;
        }
        total += processor
            .count_schema_records(&sws.schema.name)
            .await
            .expect("count_schema_records failed");
    }
    total
}

#[actix_web::test]
async fn migration_consolidates_fragmented_apple_notes() {
    let svc = spawn_schema_service().await;
    let schema_url = svc.url.clone();

    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair))
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

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

    // Seed pre-#946 fragmentation: same Apple Notes record shape across three
    // non-canonical descriptive_names. Counts mirror the dogfood ratio
    // (small + medium + large) so this stresses the per-schema scan loop.
    ingest_into(
        &node,
        &ingestion_service,
        &progress_service,
        &user_id,
        "Project Notes",
        mock_note_records(0, 1),
    )
    .await;
    ingest_into(
        &node,
        &ingestion_service,
        &progress_service,
        &user_id,
        "Content Documents",
        mock_note_records(1, 4),
    )
    .await;
    ingest_into(
        &node,
        &ingestion_service,
        &progress_service,
        &user_id,
        "Documents",
        mock_note_records(5, 6),
    )
    .await;

    // Pre-state: each fragmented schema holds its slice; canonical doesn't
    // exist yet.
    let processor = OperationProcessor::new(Arc::new(node.clone()));
    assert_eq!(count_in(&processor, "Project Notes").await, 1);
    assert_eq!(count_in(&processor, "Content Documents").await, 4);
    assert_eq!(count_in(&processor, "Documents").await, 6);
    assert_eq!(count_in(&processor, "Apple Notes").await, 0);

    let outcome = run_with_user(&user_id, async {
        apple_consolidation::run(&node, &ingestion_service).await
    })
    .await
    .expect("migration returned Err");

    assert!(!outcome.skipped, "migration should not have been skipped");
    assert_eq!(outcome.migrated, 11, "migrated count");
    assert!(
        outcome.schemas_scanned >= 3,
        "expected to scan at least the 3 fragmented schemas, got {}",
        outcome.schemas_scanned,
    );

    // Post-state: canonical schema holds the full set. The fragmented
    // schemas still hold their slices (fold_db is append-only — the
    // migration only consolidates forward), but the canonical "Apple Notes"
    // schema is now the queryable source of truth.
    assert_eq!(
        count_in(&processor, "Apple Notes").await,
        11,
        "canonical 'Apple Notes' must hold every record from every fragmented schema"
    );

    // Idempotency: a second run re-extracts and re-ingests the same records.
    // The canonical schema's hash key is `content_hash`, so re-ingestion is
    // content-hash-keyed and the canonical record count stays at 11.
    let second = run_with_user(&user_id, async {
        apple_consolidation::run(&node, &ingestion_service).await
    })
    .await
    .expect("second migration returned Err");

    assert_eq!(
        count_in(&processor, "Apple Notes").await,
        11,
        "second run must not duplicate or drop canonical records"
    );
    // Re-extraction is allowed (originals still exist), but the migration
    // counter should match the consistent steady state — same 11 records
    // re-attempted.
    assert_eq!(second.migrated, 11);
}

#[actix_web::test]
async fn run_if_needed_writes_marker_and_skips_thereafter() {
    let svc = spawn_schema_service().await;
    let schema_url = svc.url.clone();

    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair))
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    let ingestion_service = Arc::new(
        IngestionService::new(
            Path::new("/nonexistent/folddb-test-config").to_path_buf(),
            fold_db_node::ingestion::IngestionConfig::default(),
        )
        .expect("construct IngestionService with default config"),
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let config_dir = tmp.path();
    let marker = config_dir.join(".apple_consolidation_complete");
    assert!(!marker.exists(), "marker should not exist before first run");

    // Empty node — no records — first run still writes the marker so
    // subsequent boots skip without re-scanning.
    let first = run_with_user(&user_id, async {
        apple_consolidation::run_if_needed(&node, &ingestion_service, config_dir).await
    })
    .await
    .expect("first run_if_needed returned Err");
    assert!(
        !first.skipped,
        "first run with absent marker should not be skipped"
    );
    assert_eq!(first.migrated, 0);
    assert!(
        marker.exists(),
        "marker file should be present after first run"
    );

    // Second invocation: marker present → short-circuits.
    let second = run_with_user(&user_id, async {
        apple_consolidation::run_if_needed(&node, &ingestion_service, config_dir).await
    })
    .await
    .expect("second run_if_needed returned Err");
    assert!(second.skipped, "second run must skip when marker exists");
    assert_eq!(second.schemas_scanned, 0);
}

#[actix_web::test]
async fn env_var_opt_out_skips_migration_without_writing_marker() {
    use std::env;

    let svc = spawn_schema_service().await;
    let schema_url = svc.url.clone();

    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    config = config
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair))
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    let ingestion_service = Arc::new(
        IngestionService::new(
            Path::new("/nonexistent/folddb-test-config").to_path_buf(),
            fold_db_node::ingestion::IngestionConfig::default(),
        )
        .expect("construct IngestionService"),
    );

    let tmp = tempfile::tempdir().expect("tempdir");
    let config_dir = tmp.path();
    let marker = config_dir.join(".apple_consolidation_complete");

    // SAFETY: tests run with `cargo test`'s default thread sandbox; we set
    // and unset within the same scope to avoid leaking the env var to a
    // sibling test.
    unsafe {
        env::set_var("FOLDDB_SKIP_APPLE_NOTES_MIGRATION", "1");
    }
    let outcome = apple_consolidation::run_if_needed(&node, &ingestion_service, config_dir)
        .await
        .expect("run_if_needed returned Err");
    unsafe {
        env::remove_var("FOLDDB_SKIP_APPLE_NOTES_MIGRATION");
    }

    assert!(outcome.skipped, "opt-out should skip");
    assert!(
        !marker.exists(),
        "opt-out must NOT write the marker — operator may want migration to run later"
    );
}
