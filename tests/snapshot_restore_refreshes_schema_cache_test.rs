//! Regression test for alpha-e2e dogfood run 5 flow 2 papercut (task 99e8a):
//! `POST /api/snapshot/restore` used to write schemas to Sled but leave the
//! in-memory SchemaCore cache stale, so `/api/schemas` kept returning the
//! pre-restore view until the node restarted.
//!
//! `SyncEngine::bootstrap_all` fires the schema reloader only when *log*
//! entries include a schemas namespace. A snapshot-only restore
//! (entries_replayed == 0) writes schemas straight into Sled without
//! tripping the reloader, so the handler now defensively calls
//! `SchemaCore::reload_from_store` after `bootstrap_all` succeeds.
//!
//! The test drives two real FoldNode instances against a mock storage
//! service:
//!   1. Node A populates `SchemaOne` and backs up a snapshot.
//!   2. Node B spins up with cloud sync. The factory's auto-bootstrap gate
//!      (`has_user_data`) is already `true` at that point because
//!      `NodeConfigStore::with_crypto_key` opens the `node_config` Sled
//!      tree before the bootstrap check, so SchemaCore initializes empty —
//!      matching the "on-demand restore on a running node" scenario in the
//!      bug report.
//!   3. Node B calls the restore handler. Without the fix, SchemaCore
//!      would see an empty cache even though the snapshot restore wrote
//!      `SchemaOne` into Sled. With the fix, `reload_from_store` picks it
//!      up and `/api/schemas` immediately reflects the restored state.

use actix_web::{web, App, HttpResponse, HttpServer};
use fold_db::storage::config::{CloudSyncConfig, DatabaseConfig};
use fold_db_node::fold_node::{FoldNode, NodeConfig};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

type Storage = Arc<Mutex<HashMap<String, Vec<u8>>>>;

/// Serialize tests that mutate `FOLDDB_HOME` (a process-global env var) so
/// they don't race each other when run in parallel.
fn env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("env lock poisoned")
}

#[derive(Deserialize)]
struct PresignBody {
    action: String,
    #[serde(default)]
    snapshot_name: Option<String>,
    #[serde(default)]
    seq_numbers: Vec<u64>,
}

#[derive(Deserialize)]
struct ListBody {
    #[allow(dead_code)]
    action: String,
    prefix: String,
}

struct AppCtx {
    base_url: String,
    storage: Storage,
    user_prefix: String,
}

async fn handle_presign(
    ctx: web::Data<AppCtx>,
    body: web::Json<PresignBody>,
) -> actix_web::Result<HttpResponse> {
    let key = match body.action.as_str() {
        "presign_snapshot_upload" | "presign_snapshot_download" => {
            let name = body.snapshot_name.as_deref().unwrap_or("latest.enc");
            format!("{}/snapshots/{name}", ctx.user_prefix)
        }
        "presign_log_upload" | "presign_log_download" => {
            let urls: Vec<_> = body
                .seq_numbers
                .iter()
                .map(|seq| {
                    let key = format!("{}/log/{seq}.enc", ctx.user_prefix);
                    let method = if body.action == "presign_log_upload" {
                        "PUT"
                    } else {
                        "GET"
                    };
                    json!({
                        "url": format!("{}/storage/{}", ctx.base_url, key),
                        "method": method,
                        "expires_in_secs": 900,
                    })
                })
                .collect();
            return Ok(HttpResponse::Ok().json(json!({ "ok": true, "urls": urls })));
        }
        other => {
            return Ok(HttpResponse::BadRequest()
                .json(json!({ "ok": false, "error": format!("unsupported action: {other}") })));
        }
    };

    let method = if body.action.contains("upload") {
        "PUT"
    } else {
        "GET"
    };
    Ok(HttpResponse::Ok().json(json!({
        "ok": true,
        "urls": [{
            "url": format!("{}/storage/{}", ctx.base_url, key),
            "method": method,
            "expires_in_secs": 900,
        }],
    })))
}

async fn handle_list(
    ctx: web::Data<AppCtx>,
    body: web::Json<ListBody>,
) -> actix_web::Result<HttpResponse> {
    let full_prefix = format!("{}/{}", ctx.user_prefix, body.prefix);
    let storage = ctx.storage.lock().unwrap();
    let objects: Vec<_> = storage
        .keys()
        .filter(|k| k.starts_with(&full_prefix))
        .map(|k| {
            let stripped = k
                .strip_prefix(&format!("{}/", ctx.user_prefix))
                .unwrap_or(k);
            json!({
                "key": stripped,
                "size": storage[k].len(),
                "last_modified": "2026-04-19T00:00:00Z",
            })
        })
        .collect();
    Ok(HttpResponse::Ok().json(json!({ "ok": true, "objects": objects })))
}

async fn handle_put(
    ctx: web::Data<AppCtx>,
    path: web::Path<String>,
    body: web::Bytes,
) -> HttpResponse {
    let key = path.into_inner();
    ctx.storage.lock().unwrap().insert(key, body.to_vec());
    HttpResponse::Ok().finish()
}

async fn handle_get(ctx: web::Data<AppCtx>, path: web::Path<String>) -> HttpResponse {
    let key = path.into_inner();
    match ctx.storage.lock().unwrap().get(&key).cloned() {
        Some(bytes) => HttpResponse::Ok()
            .content_type("application/octet-stream")
            .body(bytes),
        None => HttpResponse::NotFound().finish(),
    }
}

async fn start_mock_server(
    user_prefix: &str,
    storage: Storage,
) -> (String, actix_web::dev::ServerHandle) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{port}");

    let ctx = web::Data::new(AppCtx {
        base_url: base_url.clone(),
        storage,
        user_prefix: user_prefix.to_string(),
    });

    let server = HttpServer::new(move || {
        App::new()
            .app_data(ctx.clone())
            .app_data(web::JsonConfig::default().limit(64 * 1024 * 1024))
            .app_data(web::PayloadConfig::default().limit(64 * 1024 * 1024))
            .route("/api/sync/presign", web::post().to(handle_presign))
            .route("/api/sync/list", web::post().to(handle_list))
            .route("/storage/{key:.*}", web::put().to(handle_put))
            .route("/storage/{key:.*}", web::get().to(handle_get))
    })
    .listen(listener)
    .unwrap()
    .run();

    let handle = server.handle();
    tokio::spawn(server);
    (base_url, handle)
}

fn node_cfg_with_cloud_sync(
    home: &Path,
    api_url: &str,
    user_hash: &str,
    pub_key: &str,
    priv_key: &str,
) -> NodeConfig {
    let data_dir = home.join("data");
    NodeConfig {
        database: DatabaseConfig::with_cloud_sync(
            data_dir.clone(),
            CloudSyncConfig {
                api_url: api_url.to_string(),
                api_key: "test-key".to_string(),
                session_token: None,
                user_hash: Some(user_hash.to_string()),
            },
        ),
        storage_path: Some(data_dir),
        network_listen_address: "/ip4/127.0.0.1/tcp/0".to_string(),
        security_config: Default::default(),
        schema_service_url: Some("test://mock".to_string()),
        public_key: Some(pub_key.to_string()),
        private_key: Some(priv_key.to_string()),
        config_dir: Some(home.join("config")),
    }
}

async fn load_schema_by_json(node: &FoldNode, json_str: &str) {
    let declarative: fold_db::schema::types::DeclarativeSchemaDefinition =
        serde_json::from_str(json_str).expect("parse declarative schema");
    let schema =
        fold_db::schema::SchemaInterpreter::interpret(declarative).expect("interpret schema");
    node.get_fold_db()
        .expect("fold_db")
        .schema_manager()
        .load_schema_internal(schema)
        .await
        .expect("load_schema_internal");
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::await_holding_lock)]
async fn restore_handler_refreshes_schema_cache_without_restart() {
    let _guard = env_lock();

    let user_hash = "snapshot_refresh_user";
    let storage: Storage = Arc::new(Mutex::new(HashMap::new()));
    let (base_url, handle) = start_mock_server(user_hash, storage.clone()).await;

    // Shared identity — both nodes derive the same E2E key from this keypair,
    // matching the BIP39-based "restore on a new device" flow.
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let pub_key = keypair.public_key_base64();
    let priv_key = keypair.secret_key_base64();

    let node_a_home = tempfile::tempdir().unwrap();
    let node_b_home = tempfile::tempdir().unwrap();

    // --- Node A: populate SchemaOne, back up a snapshot -------------------
    std::env::set_var("FOLDDB_HOME", node_a_home.path());
    let node_a = FoldNode::new(node_cfg_with_cloud_sync(
        node_a_home.path(),
        &base_url,
        user_hash,
        &pub_key,
        &priv_key,
    ))
    .await
    .expect("build node A");

    load_schema_by_json(
        &node_a,
        r#"{
            "name": "SchemaOne",
            "key": { "range_field": "created_at" },
            "fields": { "title": {}, "created_at": {} }
        }"#,
    )
    .await;

    let backup = fold_db_node::handlers::snapshot::backup(user_hash, &node_a)
        .await
        .expect("backup A");
    assert!(backup.ok, "backup handler failed: {:?}", backup.error);

    // --- Node B: separate FOLDDB_HOME, same identity ----------------------
    // The factory's `has_user_data` gate is already `true` at construction
    // time (the `node_config` Sled tree is opened before the bootstrap
    // check), so auto-bootstrap is skipped and SchemaCore initializes empty
    // — which is exactly the "on-demand restore on a running node"
    // scenario the papercut was filed against.
    std::env::set_var("FOLDDB_HOME", node_b_home.path());
    let node_b = FoldNode::new(node_cfg_with_cloud_sync(
        node_b_home.path(),
        &base_url,
        user_hash,
        &pub_key,
        &priv_key,
    ))
    .await
    .expect("build node B");

    let before = node_b
        .get_fold_db()
        .unwrap()
        .schema_manager()
        .get_schemas()
        .expect("B schemas before restore");
    assert!(
        !before.contains_key("SchemaOne"),
        "node B must not have SchemaOne before restore, got: {:?}",
        before.keys().collect::<Vec<_>>()
    );

    // --- Call the restore handler — this is the path under test ----------
    let restore = fold_db_node::handlers::snapshot::restore(user_hash, &node_b)
        .await
        .expect("restore handler");
    assert!(
        restore.ok,
        "restore handler must succeed: {:?}",
        restore.error
    );
    let data = restore.data.as_ref().expect("restore response body");
    assert!(data.success, "restore reported failure");
    // Sanity: the restore must arrive purely via the snapshot. If log
    // entries were replayed, the engine's own reloader path would fire —
    // which wouldn't exercise the handler's defensive reload.
    assert_eq!(
        data.entries_replayed, 0,
        "test must exercise the snapshot-only restore path; \
         saw {} log entries",
        data.entries_replayed
    );
    assert!(
        data.schemas_refreshed >= 1,
        "restore must refresh at least 1 schema in the cache, \
         got schemas_refreshed={}",
        data.schemas_refreshed
    );

    // Core assertion: SchemaCore on B now sees the restored schema without
    // a node restart. Before the fix, this assertion would fail because
    // `bootstrap_all` skips the reloader on snapshot-only restores.
    let after = node_b
        .get_fold_db()
        .unwrap()
        .schema_manager()
        .get_schemas()
        .expect("B schemas after restore");
    assert!(
        after.contains_key("SchemaOne"),
        "restored schema must be visible without a node restart, \
         got: {:?}",
        after.keys().collect::<Vec<_>>()
    );

    handle.stop(true).await;
}
