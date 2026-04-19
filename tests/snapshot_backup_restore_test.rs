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
use fold_db::storage::inmemory_backend::InMemoryNamespacedStore;
use fold_db::storage::traits::NamespacedStore;
use fold_db::sync::auth::{AuthClient, SyncAuth};
use fold_db::sync::s3::S3Client;
use fold_db::sync::{SyncConfig, SyncEngine};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::net::TcpListener;
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

/// Start a mock storage_service on a free port. Returns the base URL and a
/// handle to the shared storage map (so tests can inspect it).
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
