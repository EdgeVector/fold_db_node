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
use fold_db::schema_service::state::SchemaServiceState;
use schema_service_server_http::configure_routes;
use std::net::TcpListener;
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
    spawn_inner(false).await
}

/// Spawn an in-process schema service pre-seeded with the Phase 1
/// built-in fingerprint schemas. Mirrors what
/// `SchemaServiceServer::new_with_builtins` does at production startup.
#[allow(dead_code)]
pub async fn spawn_schema_service_with_builtins() -> SpawnedSchemaService {
    spawn_inner(true).await
}

async fn spawn_inner(seed_builtins: bool) -> SpawnedSchemaService {
    let temp_dir = TempDir::new().expect("create tempdir for schema service");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path).expect("create SchemaServiceState");
    if seed_builtins {
        fold_db::schema_service::builtin_schemas::seed(&state)
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
