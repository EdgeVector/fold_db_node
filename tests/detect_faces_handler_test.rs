//! Integration test: `detect_faces` handler happy-path against the real ONNX
//! face pipeline.
//!
//! Closes the TODO at `src/handlers/fingerprints/detect_faces.rs` by replacing
//! an empty `#[ignore]`d stub with a test that actually runs. Spins up a
//! `FoldNode` with the `face-detection` feature compiled in (which auto-wires
//! the production `OnnxFaceProcessor` via `fold_db::fold_db_core::factory`),
//! base64-encodes the `alice_01.jpg` fixture, and asserts the handler returns
//! at least one detected face with a 512-dim embedding, a normalized bbox, and
//! a confidence in `[0, 1]`.
//!
//! Mocking the `FaceProcessor` was the original plan, but the production
//! factory installs `OnnxFaceProcessor` into a `OnceCell` at FoldNode init —
//! `set_face_processor`'s `let _ = ...set(...)` then silently no-ops, so the
//! mock would never run. Asserting against the real pipeline is also more
//! honest: it covers model download, image decode, SCRFD detection, ArcFace
//! embedding, and the handler's `FaceEmbedding → DetectedFaceDto` shape.
//!
//! ## Cost
//!
//! First run downloads ~50 MB of SCRFD + ArcFace ONNX weights; subsequent runs
//! reuse the cached models from the FoldNode home dir. Not part of the default
//! `cargo test --workspace --lib` invocation in CI — only runs under
//! `cargo test --features face-detection`. The face-detection-aware build is
//! exercised today by the `e2e-cloud` workflow.

#![cfg(feature = "face-detection")]

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use fold_db_node::handlers::fingerprints::{detect_faces, DetectFacesRequest};
use std::sync::Arc;
use tempfile::TempDir;

mod common;
use common::schema_service::spawn_schema_service;

async fn create_node(schema_service_url: &str) -> (Arc<FoldNode>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_string_lossy().to_string();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(path.into())
        .with_schema_service_url(schema_service_url)
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair));
    let node = FoldNode::new(config).await.expect("create FoldNode");
    (Arc::new(node), tmp)
}

#[actix_web::test]
async fn detect_faces_finds_a_face_in_the_alice_fixture() {
    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;

    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("test-framework/fixtures/faces/alice_01.jpg");
    let fixture_bytes = std::fs::read(&fixture_path)
        .unwrap_or_else(|e| panic!("read {}: {}", fixture_path.display(), e));
    let image_b64 = BASE64_STANDARD.encode(&fixture_bytes);

    let response = detect_faces(
        node.clone(),
        DetectFacesRequest {
            image_base64: image_b64,
        },
    )
    .await
    .expect("handler must succeed on a valid face image");

    let body = response.data.expect("response body must contain data");

    assert!(
        !body.faces.is_empty(),
        "alice_01.jpg should contain at least one detectable face"
    );

    for (i, face) in body.faces.iter().enumerate() {
        assert_eq!(
            face.embedding.len(),
            512,
            "face {i} embedding must be 512-dim ArcFace output"
        );
        // bbox is normalized [x1, y1, x2, y2] in [0, 1] with x2 > x1, y2 > y1
        let [x1, y1, x2, y2] = face.bbox;
        assert!(
            (0.0..=1.0).contains(&x1) && (0.0..=1.0).contains(&y1),
            "face {i} bbox top-left must be normalized: {:?}",
            face.bbox
        );
        assert!(
            (0.0..=1.0).contains(&x2) && (0.0..=1.0).contains(&y2),
            "face {i} bbox bottom-right must be normalized: {:?}",
            face.bbox
        );
        assert!(
            x2 > x1 && y2 > y1,
            "face {i} bbox must have positive area: {:?}",
            face.bbox
        );
        assert!(
            (0.0..=1.0).contains(&face.confidence),
            "face {i} confidence must be in [0, 1]: {}",
            face.confidence
        );
    }
}
