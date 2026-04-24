//! Integration tests for TODO-6: zero-extractor-yield meta-error.
//!
//! The batch ingest handlers (`ingest_photo_faces_batch` and
//! `ingest_text_signals_batch`) accept a per-record
//! `expected_to_yield` flag. When a flagged record's extractor runs
//! empty AND no per-extractor IngestionError was emitted, the
//! handler writes a meta `IngestionError` with
//! `error_class: "ZeroExtractorYield"` so the Failed panel surfaces
//! the silent-gap case.
//!
//! The unit tests in `src/handlers/fingerprints/ingest*.rs` cover DTO
//! shape and the deterministic-id-doesn't-collide invariant. These
//! integration tests exercise the end-to-end write path against a
//! live node with Phase 1 schemas registered.

use std::sync::Arc;

use fold_db::security::Ed25519KeyPair;
use fold_db_node::fingerprints::canonical_names;
use fold_db_node::fingerprints::registration::register_phase_1_schemas;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use fold_db_node::handlers::fingerprints::ingest::{
    ingest_photo_faces_batch, IngestPhotoFacesRequest, PhotoFacesDto,
};
use fold_db_node::handlers::fingerprints::ingest_text::{
    ingest_text_signals_batch, IngestTextSignalsRequest, TextRecordDto,
};
use fold_db_node::handlers::fingerprints::ingestion_errors::list_ingestion_errors;
use tempfile::TempDir;

mod common;
use common::schema_service::spawn_schema_service_with_builtins;

async fn setup_node() -> (
    Arc<FoldNode>,
    TempDir,
    common::schema_service::SpawnedSchemaService,
) {
    canonical_names::reset_for_tests();

    let service = spawn_schema_service_with_builtins().await;
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_string_lossy().to_string();
    let keypair = Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(path.into())
        .with_schema_service_url(&service.url)
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair));
    let node = FoldNode::new(config).await.expect("create FoldNode");
    let node = Arc::new(node);
    register_phase_1_schemas(&node)
        .await
        .expect("register Phase 1 schemas");
    (node, tmp, service)
}

fn collect_error_classes(
    errors: &[fold_db_node::handlers::fingerprints::ingestion_errors::IngestionErrorView],
) -> Vec<String> {
    errors.iter().map(|e| e.error_class.clone()).collect()
}

// ── Photo ingest ──────────────────────────────────────────────────

#[actix_web::test]
async fn photo_flagged_expected_to_yield_with_zero_faces_emits_meta_error() {
    let (node, _tmp, _svc) = setup_node().await;

    let request = IngestPhotoFacesRequest {
        source_schema: "Photos".to_string(),
        photos: vec![PhotoFacesDto {
            source_key: "IMG_WITH_PERSON".to_string(),
            faces: vec![],
            expected_to_yield: true,
        }],
    };
    let resp = ingest_photo_faces_batch(node.clone(), request)
        .await
        .expect("batch ok");
    let body = resp.data.expect("response body present");
    assert_eq!(body.successful_photos, 1);
    assert_eq!(body.total_faces, 0);
    assert!(body.per_photo[0].ran_empty);

    let errors = list_ingestion_errors(node, false)
        .await
        .expect("list ingestion errors")
        .data
        .expect("list body");
    let classes = collect_error_classes(&errors.errors);
    assert!(
        classes.iter().any(|c| c == "ZeroExtractorYield"),
        "expected meta error; got {:?}",
        classes
    );
    let meta = errors
        .errors
        .iter()
        .find(|e| e.error_class == "ZeroExtractorYield")
        .unwrap();
    assert_eq!(meta.source_schema, "Photos");
    assert_eq!(meta.source_key, "IMG_WITH_PERSON");
    assert_eq!(meta.extractor, "meta_zero_yield");
}

#[actix_web::test]
async fn photo_without_expected_to_yield_and_zero_faces_emits_no_meta_error() {
    let (node, _tmp, _svc) = setup_node().await;

    let request = IngestPhotoFacesRequest {
        source_schema: "Photos".to_string(),
        photos: vec![PhotoFacesDto {
            source_key: "IMG_SCENERY".to_string(),
            faces: vec![],
            expected_to_yield: false,
        }],
    };
    ingest_photo_faces_batch(node.clone(), request)
        .await
        .expect("batch ok");

    let errors = list_ingestion_errors(node, false)
        .await
        .expect("list ingestion errors")
        .data
        .expect("list body");
    // No meta row for a panorama that never claimed to have faces —
    // the whole point of the silencing rule.
    assert!(
        !errors
            .errors
            .iter()
            .any(|e| e.error_class == "ZeroExtractorYield"),
        "meta error was emitted for a non-expected-to-yield record; got {:?}",
        collect_error_classes(&errors.errors)
    );
}

// ── Text ingest ───────────────────────────────────────────────────

#[actix_web::test]
async fn text_flagged_expected_to_yield_with_no_signals_emits_meta_error() {
    let (node, _tmp, _svc) = setup_node().await;

    let request = IngestTextSignalsRequest {
        source_schema: "Emails".to_string(),
        records: vec![TextRecordDto {
            source_key: "msg_42".to_string(),
            // Body with no parseable email or phone. If the caller
            // believed this email record would contain structured
            // identity content, this is suspicious and should surface.
            text: "thanks for the update".to_string(),
            expected_to_yield: true,
        }],
    };
    let resp = ingest_text_signals_batch(node.clone(), request)
        .await
        .expect("batch ok");
    let body = resp.data.expect("response body present");
    assert_eq!(body.successful_records, 1);
    assert_eq!(body.total_signals, 0);
    assert!(body.per_record[0].ran_empty);

    let errors = list_ingestion_errors(node, false)
        .await
        .expect("list ingestion errors")
        .data
        .expect("list body");
    let meta = errors
        .errors
        .iter()
        .find(|e| e.error_class == "ZeroExtractorYield")
        .unwrap_or_else(|| {
            panic!(
                "expected meta error; got {:?}",
                collect_error_classes(&errors.errors)
            )
        });
    assert_eq!(meta.source_schema, "Emails");
    assert_eq!(meta.source_key, "msg_42");
    assert_eq!(meta.extractor, "meta_zero_yield");
}

#[actix_web::test]
async fn text_with_parseable_email_does_not_emit_meta_error_even_when_flagged() {
    let (node, _tmp, _svc) = setup_node().await;

    let request = IngestTextSignalsRequest {
        source_schema: "Emails".to_string(),
        records: vec![TextRecordDto {
            source_key: "msg_42".to_string(),
            text: "contact me at tom@example.com".to_string(),
            expected_to_yield: true,
        }],
    };
    let resp = ingest_text_signals_batch(node.clone(), request)
        .await
        .expect("batch ok");
    let body = resp.data.expect("response body present");
    assert_eq!(body.successful_records, 1);
    assert!(body.total_signals >= 1);
    assert!(!body.per_record[0].ran_empty);

    let errors = list_ingestion_errors(node, false)
        .await
        .expect("list ingestion errors")
        .data
        .expect("list body");
    // The extractor yielded at least one signal — zero-yield gate
    // must NOT fire, even though the record was flagged.
    assert!(
        !errors
            .errors
            .iter()
            .any(|e| e.error_class == "ZeroExtractorYield"),
        "meta error was emitted despite non-empty extraction; got {:?}",
        collect_error_classes(&errors.errors)
    );
}
