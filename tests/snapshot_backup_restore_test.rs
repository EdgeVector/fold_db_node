//! End-to-end test for B4: snapshot backup + restore round-trip.
//!
//! Spins up a minimal mock storage_service (presign + list + storage PUT/GET)
//! using actix-web, then drives the real `fold_db::sync::SyncEngine`
//! `backup_snapshot()` and `bootstrap()` methods through it:
//!
//!   1. Populate node A's Sled-backed store with schemas and molecules.
//!   2. Call `SyncEngine::backup_snapshot()` → mock server stores `latest.enc`.
//!   3. Spin up node B with the same crypto key but an empty store.
//!   4. Call `SyncEngine::bootstrap()` on B → mock server serves `latest.enc`.
//!   5. Assert node B's store now contains exactly the same (k, v) pairs as node A.
//!
//! This exercises the real fold_db snapshot primitive, AuthClient presign path,
//! S3Client upload/download via presigned URLs, and the mock cloud storage is
//! an accurate-enough stand-in for the storage_service Lambda (same wire
//! protocol). No live AWS, DynamoDB, or B2/R2 calls.

use actix_web::{web, App, HttpResponse, HttpServer};
use fold_db::crypto::provider::LocalCryptoProvider;
use fold_db::crypto::CryptoProvider;
use fold_db::schema::SchemaState;
use fold_db::storage::inmemory_backend::InMemoryNamespacedStore;
use fold_db::storage::traits::NamespacedStore;
use fold_db::storage::SledNamespacedStore;
use fold_db::sync::auth::{AuthClient, AuthRefreshCallback, SyncAuth};
use fold_db::sync::s3::S3Client;
use fold_db::sync::{SyncConfig, SyncEngine};
use fold_db::test_helpers::TestSchemaBuilder;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

type Storage = Arc<Mutex<HashMap<String, Vec<u8>>>>;

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
    /// Number of times presign has been called (any action). Shared across
    /// handler invocations so tests can assert retry-count expectations.
    presign_calls: Arc<AtomicUsize>,
    /// Number of remaining presign calls to reject with 401 before serving
    /// normally. Simulates a stale API key that becomes valid after a refresh.
    fail_presigns_remaining: Arc<AtomicUsize>,
}

async fn handle_presign(
    ctx: web::Data<AppCtx>,
    body: web::Json<PresignBody>,
) -> actix_web::Result<HttpResponse> {
    ctx.presign_calls.fetch_add(1, Ordering::SeqCst);
    if ctx.fail_presigns_remaining.load(Ordering::SeqCst) > 0 {
        ctx.fail_presigns_remaining.fetch_sub(1, Ordering::SeqCst);
        return Ok(HttpResponse::Unauthorized().json(json!({
            "ok": false,
            "error": "stale api key",
        })));
    }

    let key = match body.action.as_str() {
        "presign_snapshot_upload" | "presign_snapshot_download" => {
            let name = body.snapshot_name.as_deref().unwrap_or("latest.enc");
            format!("{}/snapshots/{name}", ctx.user_prefix)
        }
        "presign_log_upload" | "presign_log_download" => {
            // One URL per seq; return the whole list
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

/// Handle to a running mock storage_service. Carries shared counters so
/// tests can assert how many presigns landed and trigger fail-first behavior.
struct MockServer {
    base_url: String,
    handle: actix_web::dev::ServerHandle,
    presign_calls: Arc<AtomicUsize>,
    fail_presigns_remaining: Arc<AtomicUsize>,
}

/// Start a mock storage_service on a free port. Returns the base URL and a
/// handle to the shared storage map (so tests can inspect it).
async fn start_mock_server(
    user_prefix: &str,
    storage: Storage,
) -> (String, actix_web::dev::ServerHandle) {
    let server = start_mock_server_full(user_prefix, storage).await;
    (server.base_url, server.handle)
}

async fn start_mock_server_full(user_prefix: &str, storage: Storage) -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let base_url = format!("http://127.0.0.1:{port}");
    let presign_calls = Arc::new(AtomicUsize::new(0));
    let fail_presigns_remaining = Arc::new(AtomicUsize::new(0));

    let ctx = web::Data::new(AppCtx {
        base_url: base_url.clone(),
        storage,
        user_prefix: user_prefix.to_string(),
        presign_calls: presign_calls.clone(),
        fail_presigns_remaining: fail_presigns_remaining.clone(),
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
    MockServer {
        base_url,
        handle,
        presign_calls,
        fail_presigns_remaining,
    }
}

fn crypto_for_key(key: [u8; 32]) -> Arc<dyn CryptoProvider> {
    Arc::new(LocalCryptoProvider::from_key(key))
}

fn make_engine(
    base_url: &str,
    crypto: Arc<dyn CryptoProvider>,
    store: Arc<dyn NamespacedStore>,
    device_id: &str,
) -> Arc<SyncEngine> {
    let http = Arc::new(reqwest::Client::new());
    let s3 = S3Client::new(http.clone());
    let auth = AuthClient::new(
        http,
        base_url.to_string(),
        SyncAuth::ApiKey("test".to_string()),
    );
    Arc::new(SyncEngine::new(
        device_id.to_string(),
        crypto,
        s3,
        auth,
        store,
        SyncConfig::default(),
    ))
}

async fn populate_node_a(store: &Arc<dyn NamespacedStore>) {
    let schemas = store.open_namespace("schemas").await.unwrap();
    schemas
        .put(b"schema:notes", br#"{"name":"notes"}"#.to_vec())
        .await
        .unwrap();
    schemas
        .put(b"schema:photos", br#"{"name":"photos"}"#.to_vec())
        .await
        .unwrap();

    let atoms = store.open_namespace("atoms").await.unwrap();
    for i in 0..10u32 {
        let key = format!("atom:{i:04}");
        let val = format!("atom-content-{i}");
        atoms
            .put(key.as_bytes(), val.as_bytes().to_vec())
            .await
            .unwrap();
    }

    let molecules = store.open_namespace("molecules").await.unwrap();
    for i in 0..5u32 {
        let key = format!("mol:{i}");
        let val = format!(r#"{{"id":{i},"ref":"atom:{i:04}"}}"#);
        molecules
            .put(key.as_bytes(), val.as_bytes().to_vec())
            .await
            .unwrap();
    }
}

async fn snapshot_contents(
    store: &Arc<dyn NamespacedStore>,
) -> Vec<(String, Vec<(Vec<u8>, Vec<u8>)>)> {
    let mut out = Vec::new();
    let mut names = store.list_namespaces().await.unwrap();
    names.sort();
    for name in names {
        if name == "__sled__default" {
            continue;
        }
        let kv = store.open_namespace(&name).await.unwrap();
        let mut entries = kv.scan_prefix(&[]).await.unwrap();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        out.push((name, entries));
    }
    out
}

/// Full round-trip: populate node A, backup, restore into empty node B,
/// assert data parity.
#[actix_web::test]
async fn snapshot_backup_restore_roundtrip() {
    let user_prefix = "test_user_hash";
    let storage: Storage = Arc::new(Mutex::new(HashMap::new()));
    let (base_url, handle) = start_mock_server(user_prefix, storage.clone()).await;

    // Same crypto key on both nodes — simulates BIP39-derived unified identity.
    let crypto = crypto_for_key([0x42u8; 32]);

    // --- Node A: populate + backup -----------------------------------------
    let store_a: Arc<dyn NamespacedStore> = Arc::new(InMemoryNamespacedStore::new());
    populate_node_a(&store_a).await;
    let expected = snapshot_contents(&store_a).await;

    let engine_a = make_engine(&base_url, crypto.clone(), store_a.clone(), "node-a");
    let seq = engine_a.backup_snapshot().await.expect("backup_snapshot");
    assert_eq!(seq, 0, "fresh engine should upload at seq 0");

    // latest.enc must exist in mock cloud storage.
    {
        let st = storage.lock().unwrap();
        let latest_key = format!("{user_prefix}/snapshots/latest.enc");
        let seq_key = format!("{user_prefix}/snapshots/0.enc");
        assert!(
            st.contains_key(&latest_key),
            "mock storage should have latest.enc, has keys: {:?}",
            st.keys().collect::<Vec<_>>()
        );
        assert!(
            st.contains_key(&seq_key),
            "mock storage should also have seq-specific snapshot {seq_key}"
        );
        // The two should be byte-identical (same sealed bytes uploaded twice).
        assert_eq!(st[&latest_key], st[&seq_key]);
    }

    // --- Node B: empty store, same key → bootstrap --------------------------
    let store_b: Arc<dyn NamespacedStore> = Arc::new(InMemoryNamespacedStore::new());
    assert!(
        snapshot_contents(&store_b).await.is_empty(),
        "node B store must start empty"
    );

    let engine_b = make_engine(&base_url, crypto.clone(), store_b.clone(), "node-b");
    let last_seq = engine_b.bootstrap().await.expect("bootstrap");
    assert_eq!(
        last_seq, 0,
        "bootstrap should return the snapshot's last_seq"
    );

    // --- Assert data parity ------------------------------------------------
    let restored = snapshot_contents(&store_b).await;
    assert_eq!(
        restored, expected,
        "restored store on node B must match node A exactly"
    );

    handle.stop(true).await;
}

/// Regression for alpha-e2e-dogfood gap db973: `folddb snapshot backup` CLI
/// returned a 500 auth error when the daemon's cached API key had gone stale,
/// even though the auth-refresh callback was wired. The periodic sync path
/// retried on auth error; the on-demand backup path didn't.
///
/// This test puts the mock storage_service into a "reject the first presign
/// with 401" state and confirms that `SyncEngine::backup_snapshot` now
/// invokes the auth-refresh callback and retries once, completing the upload.
#[actix_web::test]
async fn backup_snapshot_retries_once_on_stale_auth() {
    let user_prefix = "db973_stale_auth_user";
    let storage: Storage = Arc::new(Mutex::new(HashMap::new()));
    let mock = start_mock_server_full(user_prefix, storage.clone()).await;

    // Make the very first presign return 401. The refresh callback fires,
    // SyncEngine retries, and the second presign succeeds.
    mock.fail_presigns_remaining.store(1, Ordering::SeqCst);

    let crypto = crypto_for_key([0x42u8; 32]);
    let store: Arc<dyn NamespacedStore> = Arc::new(InMemoryNamespacedStore::new());
    populate_node_a(&store).await;

    // Build the engine manually so we can wire an auth_refresh callback that
    // tracks invocations.
    let http = Arc::new(reqwest::Client::new());
    let s3 = S3Client::new(http.clone());
    let auth = AuthClient::new(
        http,
        mock.base_url.clone(),
        SyncAuth::ApiKey("stale-key".to_string()),
    );
    let mut engine = SyncEngine::new(
        "stale-device".to_string(),
        crypto,
        s3,
        auth,
        store,
        SyncConfig::default(),
    );
    let refresh_invocations = Arc::new(AtomicUsize::new(0));
    let cb_counter = refresh_invocations.clone();
    let cb: AuthRefreshCallback = Arc::new(move || {
        let cb_counter = cb_counter.clone();
        Box::pin(async move {
            cb_counter.fetch_add(1, Ordering::SeqCst);
            Ok(SyncAuth::ApiKey("fresh-key".to_string()))
        })
    });
    engine.set_auth_refresh(cb);
    let engine = Arc::new(engine);

    let seq = engine
        .backup_snapshot()
        .await
        .expect("backup should succeed after auth refresh + retry");
    assert_eq!(seq, 0, "fresh engine uploads at seq 0");

    assert_eq!(
        refresh_invocations.load(Ordering::SeqCst),
        1,
        "auth-refresh callback must fire exactly once per stale-auth retry"
    );
    // First presign rejected, then two successful presigns (one for seq
    // file, one for latest.enc) = 3 total server-side calls.
    assert_eq!(
        mock.presign_calls.load(Ordering::SeqCst),
        3,
        "expected 1 rejected + 2 accepted presigns, got {}",
        mock.presign_calls.load(Ordering::SeqCst)
    );
    assert_eq!(
        mock.fail_presigns_remaining.load(Ordering::SeqCst),
        0,
        "fail budget must be exhausted"
    );

    // Snapshot actually landed in mock cloud storage.
    {
        let st = storage.lock().unwrap();
        assert!(
            st.contains_key(&format!("{user_prefix}/snapshots/latest.enc")),
            "latest.enc must exist after retry succeeded"
        );
    }

    mock.handle.stop(true).await;
}

/// Without a wired auth-refresh callback, a stale-key presign failure must
/// bubble out as `SyncError::Auth` — no silent retry and no partial write.
#[actix_web::test]
async fn backup_snapshot_surfaces_auth_error_without_refresh_callback() {
    let user_prefix = "db973_no_refresh_user";
    let storage: Storage = Arc::new(Mutex::new(HashMap::new()));
    let mock = start_mock_server_full(user_prefix, storage.clone()).await;
    mock.fail_presigns_remaining.store(1, Ordering::SeqCst);

    let crypto = crypto_for_key([0x42u8; 32]);
    let store: Arc<dyn NamespacedStore> = Arc::new(InMemoryNamespacedStore::new());
    populate_node_a(&store).await;

    let engine = make_engine(&mock.base_url, crypto, store, "no-refresh-device");
    let err = engine
        .backup_snapshot()
        .await
        .expect_err("backup must fail without a refresh callback");
    assert!(
        format!("{err:?}").contains("Auth"),
        "expected SyncError::Auth, got: {err:?}"
    );

    assert!(
        storage.lock().unwrap().is_empty(),
        "no snapshot objects must be written when presign fails"
    );

    mock.handle.stop(true).await;
}

/// Wrong key on node B must fail snapshot decryption — critical safety check.
#[actix_web::test]
async fn snapshot_restore_fails_with_wrong_key() {
    let user_prefix = "wrong_key_user";
    let storage: Storage = Arc::new(Mutex::new(HashMap::new()));
    let (base_url, handle) = start_mock_server(user_prefix, storage.clone()).await;

    // Node A uploads with key 0x42
    let crypto_a = crypto_for_key([0x42u8; 32]);
    let store_a: Arc<dyn NamespacedStore> = Arc::new(InMemoryNamespacedStore::new());
    populate_node_a(&store_a).await;
    let engine_a = make_engine(&base_url, crypto_a, store_a, "node-a");
    engine_a.backup_snapshot().await.expect("backup");

    // Node B tries to restore with key 0x99 — must error, not silently succeed.
    let crypto_b = crypto_for_key([0x99u8; 32]);
    let store_b: Arc<dyn NamespacedStore> = Arc::new(InMemoryNamespacedStore::new());
    let engine_b = make_engine(&base_url, crypto_b, store_b.clone(), "node-b");
    let result = engine_b.bootstrap().await;
    assert!(
        result.is_err(),
        "bootstrap with wrong crypto key must fail, got {result:?}"
    );

    // Node B's store must still be empty — no partial restore.
    assert!(
        snapshot_contents(&store_b).await.is_empty(),
        "failed restore must not leave partial data in node B"
    );

    handle.stop(true).await;
}

/// Regression for papercut 99e8a (Alpha E2E Dogfood Run 5, Flow 2).
///
/// After `POST /api/snapshot/restore` writes schemas to Sled, `GET /api/schemas`
/// returned only the 12 canonical schemas until the node was restarted —
/// restored org-tagged schemas were invisible. The root cause is that
/// `SyncEngine::bootstrap_all` only invokes the registered schema reloader
/// when post-snapshot *log entries* include schemas; a snapshot-only restore
/// (the common case: 0 log deltas) skipped the reload and left the SchemaCore
/// cache stale.
///
/// The fix in `handlers::snapshot::restore` adds an unconditional
/// `SchemaCore::reload_from_store()` call after `bootstrap_all`. This test
/// drives the actual handler end-to-end: snapshot a schema on node A, restore
/// onto fresh node B via the handler, then assert the schema is visible via
/// `schema_manager().get_schemas()` WITHOUT restarting node B.
#[actix_web::test]
async fn snapshot_restore_refreshes_schema_cache_without_restart() {
    let user_prefix = "papercut_99e8a_user";
    let storage: Storage = Arc::new(Mutex::new(HashMap::new()));
    let (base_url, handle) = start_mock_server(user_prefix, storage.clone()).await;

    // Same E2E key on both nodes — simulates BIP39-derived unified identity.
    let crypto = crypto_for_key([0x99u8; 32]);

    // Schema that exists on node A but NOT in the canonical 12 preloaded on
    // node B — its visibility after restore is the assertion that matters.
    const SCHEMA_NAME: &str = "papercut_99e8a_notes";
    let schema_json = TestSchemaBuilder::new(SCHEMA_NAME)
        .fields(&["body"])
        .hash_key("title")
        .range_key("date")
        .build_json();

    // --- Node A: real FoldNode, register schema, back up via SyncEngine ----
    let tmp_a = tempfile::tempdir().unwrap();
    let node_a = make_node(tmp_a.path().to_str().unwrap()).await;
    let db_a = node_a.get_fold_db().expect("get fold_db A");

    db_a.schema_manager()
        .load_schema_from_json(&schema_json)
        .await
        .expect("load schema on A");
    db_a.schema_manager()
        .set_schema_state(SCHEMA_NAME, SchemaState::Approved)
        .await
        .expect("approve schema on A");

    let pool_a = db_a.sled_pool().cloned().expect("A sled pool");
    let store_a: Arc<dyn NamespacedStore> = Arc::new(SledNamespacedStore::new(pool_a));
    let engine_a = make_engine(&base_url, crypto.clone(), store_a, "node-a");
    engine_a
        .backup_snapshot()
        .await
        .expect("backup_snapshot on A");

    // --- Node B: fresh FoldNode, attach sync engine pointing at same mock --
    let tmp_b = tempfile::tempdir().unwrap();
    let node_b = make_node(tmp_b.path().to_str().unwrap()).await;
    let db_b = node_b.get_fold_db().expect("get fold_db B");

    let pool_b = db_b.sled_pool().cloned().expect("B sled pool");
    let store_b: Arc<dyn NamespacedStore> = Arc::new(SledNamespacedStore::new(pool_b));
    let engine_b = make_engine(&base_url, crypto, store_b, "node-b");
    // `set_sync_engine` also wires the schema reloader callback — but that
    // callback only fires when bootstrap_all sees schemas in *log entries*,
    // which is exactly the bug we're guarding against.
    db_b.set_sync_engine(engine_b).await;

    // Sanity: the schema is NOT in node B's cache yet.
    let before = db_b.schema_manager().get_schemas().expect("get_schemas B");
    assert!(
        !before.contains_key(SCHEMA_NAME),
        "precondition: fresh node B must not yet have '{SCHEMA_NAME}', got: {:?}",
        before.keys().collect::<Vec<_>>()
    );

    // --- Invoke the actual handler (the code path the bug fix landed in) --
    let envelope = fold_db_node::handlers::snapshot::restore("test_user", &node_b)
        .await
        .expect("restore handler must succeed");
    let response = envelope.data.expect("handler envelope must contain data");
    assert!(response.success);
    assert_eq!(
        response.targets_restored, 1,
        "personal target is the only one configured"
    );
    assert_eq!(
        response.entries_replayed, 0,
        "snapshot-only restore — the exact path the bug hid in"
    );
    assert!(
        response.message.contains("Restored snapshot"),
        "message should state that the snapshot was restored, got: {:?}",
        response.message
    );

    // --- The regression assertion: schema visible WITHOUT restart ---------
    let after = db_b.schema_manager().get_schemas().expect("get_schemas B");
    assert!(
        after.contains_key(SCHEMA_NAME),
        "restored schema '{SCHEMA_NAME}' must be visible in SchemaCore after restore \
         (no node restart) — got: {:?}",
        after.keys().collect::<Vec<_>>()
    );

    handle.stop(true).await;
}

async fn make_node(db_path: &str) -> FoldNode {
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(db_path.into())
        .with_schema_service_url("test://mock")
        .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
    FoldNode::new(config).await.expect("create FoldNode")
}
