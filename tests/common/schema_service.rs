//! In-process schema service fixture for integration tests.
//!
//! Replaces hand-rolled actix mocks across the test suite. Backs the
//! real `SchemaServiceState` with a temp Sled directory, then mounts
//! the production `/v1/*` route table via
//! `schema_service_server_http::configure_routes`. That eliminates the
//! ~80 LOC of duplicated handler boilerplate every fingerprints/
//! ingestion test used to ship and keeps the wire contract under a
//! single source of truth as schema_service evolves.

use actix_web::dev::ServerHandle;
use actix_web::{web, App, HttpServer};
use schema_service_core::state::SchemaServiceState;
use schema_service_core::Embedder;
use schema_service_server_http::configure_routes;
use std::net::TcpListener;
use std::sync::Arc;
use tempfile::TempDir;

#[allow(dead_code)]
pub struct SpawnedSchemaService {
    pub url: String,
    pub handle: ServerHandle,
    /// Owned by the fixture so the Sled directory survives for the
    /// duration of the test. Drop the `SpawnedSchemaService` to clean up.
    pub temp_dir: TempDir,
}

/// Spawn an empty in-process schema service. Used by tests that
/// register their own schemas via the API.
#[allow(dead_code)]
pub async fn spawn_schema_service() -> SpawnedSchemaService {
    spawn_inner(
        false,
        Arc::new(::schema_service_core::embedder::MockEmbeddingModel),
    )
    .await
}

/// Spawn an in-process schema service pre-seeded with the Phase 1
/// built-in fingerprint schemas. Mirrors what
/// `SchemaServiceServer::new_with_builtins` does at production startup.
#[allow(dead_code)]
pub async fn spawn_schema_service_with_builtins() -> SpawnedSchemaService {
    spawn_inner(
        true,
        Arc::new(::schema_service_core::embedder::MockEmbeddingModel),
    )
    .await
}

/// Spawn an in-process schema service backed by fold_db's real
/// `FastEmbedModel` (AllMiniLML6V2). Used by tests that need actual
/// semantic similarity, not the byte-hash mock. The model downloads
/// on first run, so callers should `#[ignore]` these tests and gate
/// nightly invocations behind the FastEmbed runner.
#[allow(dead_code)]
pub async fn spawn_schema_service_with_fastembed() -> SpawnedSchemaService {
    use fold_db::db_operations::native_index::FastEmbedModel;
    spawn_inner(false, Arc::new(FastEmbedAdapter(FastEmbedModel::new()))).await
}

/// Wraps fold_db's `FastEmbedModel` so it satisfies
/// `schema_service_core::Embedder`. The production equivalent lives
/// in `schema_service_server_shared::FoldDbFastEmbedder`; we inline it
/// here to keep fold_db_node off that dependency. Mirrors the shim
/// in `tests/schema_service_semantic_match_test.rs`.
#[allow(dead_code)]
struct FastEmbedAdapter(fold_db::db_operations::native_index::FastEmbedModel);

impl Embedder for FastEmbedAdapter {
    fn embed_text(&self, text: &str) -> Result<Vec<f32>, schema_service_core::EmbedError> {
        use fold_db::db_operations::native_index::Embedder as _;
        self.0
            .embed_text(text)
            .map_err(|e| schema_service_core::EmbedError::EmbedFailed(e.to_string()))
    }
}

async fn spawn_inner(seed_builtins: bool, embedder: Arc<dyn Embedder>) -> SpawnedSchemaService {
    let temp_dir = TempDir::new().expect("create tempdir for schema service");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path, embedder).expect("create SchemaServiceState");
    if seed_builtins {
        schema_service_core::builtin_schemas::seed(&state)
            .await
            .expect("seed built-in schemas");
    }
    let state_data = web::Data::new(state);

    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind schema service listener");
    let port = listener.local_addr().expect("local_addr").port();

    let state_clone = state_data.clone();
    let server = HttpServer::new(move || {
        App::new()
            .app_data(state_clone.clone())
            .configure(configure_routes)
    })
    .listen(listener)
    .expect("listen on bound listener")
    .run();

    let handle = server.handle();
    actix_web::rt::spawn(server);
    actix_web::rt::time::sleep(std::time::Duration::from_millis(200)).await;

    SpawnedSchemaService {
        url: format!("http://127.0.0.1:{}", port),
        handle,
        temp_dir,
    }
}
