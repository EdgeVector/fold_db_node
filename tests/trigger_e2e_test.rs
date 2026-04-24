//! End-to-end: a view registered through the schema service with an
//! `OnWrite` trigger fires on source mutations, writes a `TriggerFiring`
//! audit row, and re-materializes its output.
//!
//! Covers the full composition across three repos:
//! - fold_db PR #584 (TriggerRunner + dispatch) — the runtime path that
//!   translates a mutation into a fire + audit-row write.
//! - fold_db_node PR #642 (cascade `triggers` field to `AddViewRequest` /
//!   `StoredView` call sites) — the declaration path.
//! - fold_db_node PR #647 (delete `trigger_adapter.rs`, pass-through
//!   canonical `Trigger`) — trigger shapes must reach the fold_db
//!   registry byte-identical via `load_view_from_service`.
//!
//! Existing `view_loading_test.rs::load_view_passes_canonical_triggers_through_unchanged`
//! covers the static round-trip (service → local registry). Existing
//! fold_db `tests/trigger_runner_integration_test.rs` covers the dispatch
//! path on a directly-registered view. This test is the only one that
//! exercises the full mutation → dispatch → fire → audit → rematerialize
//! chain through the fold_db_node schema-service pathway.

use actix_web::{web, App, HttpResponse, HttpServer};
use fold_db::schema::types::data_classification::DataClassification;
use fold_db::schema::types::field_value_type::FieldValueType;
use fold_db::schema::types::key_config::KeyConfig;
use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::Query;
use fold_db::schema::types::schema::DeclarativeSchemaType as SchemaType;
use fold_db::schema::types::Schema;
use fold_db::schema::SchemaState;
use fold_db::storage::config::DatabaseConfig;
use fold_db::triggers::{fields as firing_fields, status, TRIGGER_FIRING_SCHEMA_NAME};
use fold_db::MutationType;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::{FoldNode, OperationProcessor};
use schema_service_core::types::{SchemaEnvelope, StoredView, Trigger};
use serde_json::json;
use std::collections::HashMap;
use std::net::TcpListener;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod common;

/// Spawn a mock schema service that serves pre-configured schemas and views.
/// Identical in shape to the helper in `view_loading_test.rs`; kept inline
/// because the two files only need the read endpoints and a full refactor
/// to share via `common/` would drag in unrelated wiring.
async fn spawn_mock_service(
    schemas: HashMap<String, Schema>,
    views: HashMap<String, StoredView>,
) -> (String, actix_web::dev::ServerHandle) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = listener.local_addr().unwrap();

    let schemas_data = web::Data::new(schemas);
    let views_data = web::Data::new(views);

    let server = HttpServer::new(move || {
        App::new()
            .app_data(schemas_data.clone())
            .app_data(views_data.clone())
            .route(
                "/v1/schema/{name}",
                web::get().to(
                    |path: web::Path<String>,
                     schemas: web::Data<HashMap<String, Schema>>| async move {
                        let name = path.into_inner();
                        match schemas.get(&name) {
                            Some(schema) => HttpResponse::Ok().json(SchemaEnvelope {
                                schema: schema.clone(),
                                system: false,
                            }),
                            None => HttpResponse::NotFound()
                                .json(serde_json::json!({"error": "not found"})),
                        }
                    },
                ),
            )
            .route(
                "/v1/view/{name}",
                web::get().to(
                    |path: web::Path<String>,
                     views: web::Data<HashMap<String, StoredView>>| async move {
                        let name = path.into_inner();
                        match views.get(&name) {
                            Some(view) => HttpResponse::Ok().json(view),
                            None => HttpResponse::NotFound()
                                .json(serde_json::json!({"error": "not found"})),
                        }
                    },
                ),
            )
    })
    .listen(listener)
    .unwrap()
    .run();

    let handle = server.handle();
    actix_web::rt::spawn(server);
    tokio::time::sleep(Duration::from_millis(50)).await;

    (format!("http://{}", addr), handle)
}

/// Build a minimal Range schema with the given fields and a `date` range key.
fn make_range_schema(name: &str, fields: &[&str]) -> Schema {
    let mut schema = Schema::new(
        name.to_string(),
        SchemaType::Range,
        Some(KeyConfig::new(None, Some("date".to_string()))),
        Some(fields.iter().map(|f| f.to_string()).collect()),
        None,
        None,
    );
    schema.descriptive_name = Some(name.to_string());
    let classification =
        DataClassification::new(0, "general".to_string()).expect("valid classification");
    for field in fields {
        schema
            .field_types
            .insert(field.to_string(), FieldValueType::Any);
        schema
            .field_data_classifications
            .insert(field.to_string(), classification.clone());
    }
    schema
}

/// Build a StoredView describing an identity projection over
/// `source_schema.source_fields` with the provided triggers attached.
/// `transform_hash` and `wasm_bytes` are `None` so the view is identity —
/// invalidation + lazy recompute is enough to verify the fire path.
fn make_identity_view(
    name: &str,
    source_schema: &str,
    source_fields: &[&str],
    output_schema_name: &str,
    triggers: Vec<Trigger>,
) -> StoredView {
    StoredView {
        name: name.to_string(),
        input_queries: vec![Query::new(
            source_schema.to_string(),
            source_fields.iter().map(|f| f.to_string()).collect(),
        )],
        transform_hash: None,
        wasm_bytes: None,
        output_schema_name: output_schema_name.to_string(),
        schema_type: SchemaType::Range,
        triggers,
    }
}

async fn make_node(schema_service_url: &str) -> FoldNode {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().to_path_buf();
    // The tempdir must outlive the node — leak it for the test's lifetime.
    std::mem::forget(dir);

    let (private_key, public_key) = common::test_identity_b64();
    let config = NodeConfig {
        database: DatabaseConfig::local(db_path),
        schema_service_url: Some(schema_service_url.to_string()),
        seed_identity: Some(fold_db_node::identity::NodeIdentity {
            private_key,
            public_key,
        }),
        ..Default::default()
    };
    FoldNode::new(config).await.unwrap()
}

/// Scan the internal TriggerFiring schema and reshape the field-oriented
/// result map into a flat list of rows keyed by (hash, range).
async fn scan_firings(node: &FoldNode) -> Vec<HashMap<String, serde_json::Value>> {
    let db = node.get_fold_db().expect("get fold_db");
    let q = Query::new(
        TRIGGER_FIRING_SCHEMA_NAME.to_string(),
        vec![
            firing_fields::TRIGGER_ID.to_string(),
            firing_fields::VIEW_NAME.to_string(),
            firing_fields::FIRED_AT.to_string(),
            firing_fields::STATUS.to_string(),
            firing_fields::ERROR_MESSAGE.to_string(),
        ],
    );
    let results = db
        .query_executor()
        .query(q)
        .await
        .expect("query TriggerFiring");

    let mut rows: HashMap<(Option<String>, Option<String>), HashMap<String, serde_json::Value>> =
        HashMap::new();
    for (field_name, entries) in results {
        for (kv, fv) in entries {
            let key = (kv.hash.clone(), kv.range.clone());
            rows.entry(key)
                .or_default()
                .insert(field_name.clone(), fv.value.clone());
        }
    }
    rows.into_values().collect()
}

#[actix_web::test(flavor = "multi_thread", worker_threads = 2)]
async fn on_write_trigger_fires_and_view_rematerializes_end_to_end() {
    // Source: `Note { body, date }` with `date` as the range key.
    let source_schema = make_range_schema("Note", &["body", "date"]);
    // Output schema for the view projects just `body`.
    let output_schema = make_range_schema("NoteBodyView_output", &["body"]);

    let mut schemas = HashMap::new();
    schemas.insert("Note".to_string(), source_schema);
    schemas.insert("NoteBodyView_output".to_string(), output_schema);

    let triggers = vec![Trigger::OnWrite {
        schemas: vec!["Note".to_string()],
    }];

    let mut views = HashMap::new();
    views.insert(
        "NoteBodyView".to_string(),
        make_identity_view(
            "NoteBodyView",
            "Note",
            &["body"],
            "NoteBodyView_output",
            triggers,
        ),
    );

    let (url, server_handle) = spawn_mock_service(schemas, views).await;
    let node = make_node(&url).await;

    // Register the view through the fold_db_node pass-through path.
    // This invokes `load_view_from_service`, which is the composition
    // point we want under test (StoredView → TransformView with
    // triggers attached, then `register_view` on the local SchemaCore).
    node.load_view_from_service("NoteBodyView")
        .await
        .expect("load NoteBodyView from mock schema service");

    // `load_view_from_service` loads source schemas as `Available`.
    // Mutations target Approved schemas.
    let db = node.get_fold_db().expect("get fold_db");
    db.schema_manager()
        .set_schema_state("Note", SchemaState::Approved)
        .await
        .expect("approve Note source schema");

    // Baseline: view registration must not have written a firing row.
    assert!(
        scan_firings(&node).await.is_empty(),
        "unexpected TriggerFiring rows before any mutation"
    );

    // Drive a mutation through the same path production uses.
    let processor = OperationProcessor::new(Arc::new(node.clone()));
    let mut note_fields = HashMap::new();
    note_fields.insert("body".to_string(), json!("hello world"));
    note_fields.insert("date".to_string(), json!("2026-04-22"));
    processor
        .execute_mutation(
            "Note".to_string(),
            note_fields,
            KeyValue::new(None, Some("2026-04-22".to_string())),
            MutationType::Create,
        )
        .await
        .expect("mutate Note");

    // Poll for the TriggerFiring audit row. The OnWrite path is inline —
    // the row should be visible essentially immediately — but poll with
    // a generous budget so a slow CI runner doesn't flake.
    let deadline = Instant::now() + Duration::from_secs(2);
    let firing = loop {
        let rows = scan_firings(&node).await;
        if let Some(row) = rows
            .iter()
            .find(|r| r.get(firing_fields::VIEW_NAME) == Some(&json!("NoteBodyView")))
        {
            break row.clone();
        }
        if Instant::now() >= deadline {
            panic!(
                "no TriggerFiring row for NoteBodyView within 2s — observed rows: {:?}",
                rows
            );
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };

    assert_eq!(
        firing.get(firing_fields::STATUS),
        Some(&json!(status::SUCCESS)),
        "fire must be recorded as successful, got {:?}",
        firing.get(firing_fields::STATUS)
    );
    assert_eq!(
        firing.get(firing_fields::TRIGGER_ID),
        Some(&json!("NoteBodyView:0")),
        "trigger_id uses `<view>:<trigger_index>` shape"
    );
    assert!(
        matches!(
            firing.get(firing_fields::FIRED_AT),
            Some(serde_json::Value::Number(n)) if n.as_i64().map(|v| v > 0).unwrap_or(false)
        ),
        "fired_at should be a positive epoch-ms value, got {:?}",
        firing.get(firing_fields::FIRED_AT)
    );
    assert!(
        matches!(
            firing.get(firing_fields::ERROR_MESSAGE),
            Some(serde_json::Value::Null)
        ),
        "successful fire must have null error_message, got {:?}",
        firing.get(firing_fields::ERROR_MESSAGE)
    );

    // Query the view — the cache was invalidated by the fire, so this is
    // a lazy recompute that must surface the just-mutated row.
    let view_results = processor
        .execute_query_json(Query::new(
            "NoteBodyView".to_string(),
            vec!["body".to_string()],
        ))
        .await
        .expect("query NoteBodyView");
    assert_eq!(
        view_results.len(),
        1,
        "expected exactly one materialized row, got {:?}",
        view_results
    );
    let body = view_results[0]
        .get("fields")
        .and_then(|f| f.get("body"))
        .and_then(|v| v.as_str())
        .expect("fields.body string present on materialized row");
    assert_eq!(
        body, "hello world",
        "view output must reflect the mutated source value"
    );

    server_handle.stop(true).await;
}
