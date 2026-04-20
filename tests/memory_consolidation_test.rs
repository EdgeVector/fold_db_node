//! Integration test: register memory schema + `TopicClusters` TransformView,
//! ingest semantically-clustered memories, query the view, assert clusters.
//!
//! Proves the Phase 1a thesis from `docs/design/memory_agent.md`:
//! consolidation is a TransformView whose WASM emits one row per cluster,
//! reactively recomputing on source mutations.
//!
//! Gated on `transform-wasm` because the clustering WASM is compiled at
//! runtime from Rust source via `fold_node::wasm_compiler`. That requires
//! `wasm32-unknown-unknown` installed locally (`rustup target add
//! wasm32-unknown-unknown`). First run takes ~30s for the cargo build;
//! subsequent runs are faster if the sub-project's target dir is hot.
//!
//! Run manually:
//!   cargo test -p fold_db_node --features transform-wasm --test memory_consolidation_test

#![cfg(feature = "transform-wasm")]

use actix_web::{web, App, HttpResponse, HttpServer};
use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::{MutationType, Query};
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::{FoldNode, OperationProcessor};
use fold_db_node::memory::consolidation::{
    cluster_fields, register_topic_clusters_view, TOPIC_CLUSTERS_VIEW_NAME,
};
use fold_db_node::memory::{self, fields};
use fold_db_node::schema_service::server::{
    AddSchemaResponse, SchemaAddOutcome, SchemaServiceState,
};
use fold_db_node::schema_service::types::{
    AddViewRequest, AddViewResponse, RegisterTransformRequest, RegisterTransformResponse,
    TransformAddOutcome, ViewAddOutcome,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::net::TcpListener;
use std::sync::Arc;
use tempfile::TempDir;

// ── Schema-service scaffolding (lifted from memory_roundtrip_test.rs) ───

async fn handle_add_schema(
    payload: web::Json<serde_json::Value>,
    state: web::Data<SchemaServiceState>,
) -> HttpResponse {
    let req = payload.into_inner();
    let schema: fold_db::schema::types::Schema = match serde_json::from_value(req["schema"].clone())
    {
        Ok(s) => s,
        Err(e) => {
            return HttpResponse::BadRequest().json(json!({
                "error": format!("deserialize schema: {}", e)
            }))
        }
    };
    let mappers: HashMap<String, String> =
        serde_json::from_value(req["mutation_mappers"].clone()).unwrap_or_default();
    match state.add_schema(schema, mappers).await {
        Ok(outcome) => match outcome {
            SchemaAddOutcome::Added(s, m) => HttpResponse::Created().json(AddSchemaResponse {
                schema: s,
                mutation_mappers: m,
                replaced_schema: None,
            }),
            SchemaAddOutcome::AlreadyExists(s, _) => HttpResponse::Ok().json(AddSchemaResponse {
                schema: s,
                mutation_mappers: HashMap::new(),
                replaced_schema: None,
            }),
            SchemaAddOutcome::Expanded(old, s, m) => {
                HttpResponse::Created().json(AddSchemaResponse {
                    schema: s,
                    mutation_mappers: m,
                    replaced_schema: Some(old),
                })
            }
        },
        Err(e) => HttpResponse::BadRequest().json(json!({ "error": e.to_string() })),
    }
}

async fn handle_get_schema(
    path: web::Path<String>,
    state: web::Data<SchemaServiceState>,
) -> HttpResponse {
    let name = path.into_inner();
    match state.get_schema_by_name(&name) {
        Ok(Some(s)) => HttpResponse::Ok().json(s),
        _ => HttpResponse::NotFound().json(json!({"error": "not found"})),
    }
}

async fn handle_list_schemas(state: web::Data<SchemaServiceState>) -> HttpResponse {
    match state.get_schema_names() {
        Ok(names) => HttpResponse::Ok().json(json!({"schemas": names})),
        Err(e) => HttpResponse::InternalServerError().json(json!({"error": e.to_string()})),
    }
}

async fn handle_available(state: web::Data<SchemaServiceState>) -> HttpResponse {
    match state.get_all_schemas_cached() {
        Ok(s) => HttpResponse::Ok().json(json!({"schemas": s})),
        Err(e) => HttpResponse::InternalServerError().json(json!({"error": e.to_string()})),
    }
}

async fn handle_register_transform(
    payload: web::Json<RegisterTransformRequest>,
    state: web::Data<SchemaServiceState>,
) -> HttpResponse {
    let req = payload.into_inner();
    match state.register_transform(req).await {
        Ok((record, TransformAddOutcome::Added)) => {
            HttpResponse::Created().json(RegisterTransformResponse {
                hash: record.hash.clone(),
                record,
                outcome: TransformAddOutcome::Added,
            })
        }
        Ok((record, TransformAddOutcome::AlreadyExists)) => {
            HttpResponse::Ok().json(RegisterTransformResponse {
                hash: record.hash.clone(),
                record,
                outcome: TransformAddOutcome::AlreadyExists,
            })
        }
        Err(e) => HttpResponse::BadRequest().json(json!({ "error": e.to_string() })),
    }
}

async fn handle_add_view(
    payload: web::Json<AddViewRequest>,
    state: web::Data<SchemaServiceState>,
) -> HttpResponse {
    let req = payload.into_inner();
    match state.add_view(req).await {
        Ok(ViewAddOutcome::Added(view, schema)) => HttpResponse::Created().json(AddViewResponse {
            view,
            output_schema: schema,
            replaced_schema: None,
        }),
        Ok(ViewAddOutcome::AddedWithExistingSchema(view, schema)) => {
            HttpResponse::Ok().json(AddViewResponse {
                view,
                output_schema: schema,
                replaced_schema: None,
            })
        }
        Ok(ViewAddOutcome::Expanded(view, schema, old_name)) => {
            HttpResponse::Created().json(AddViewResponse {
                view,
                output_schema: schema,
                replaced_schema: Some(old_name),
            })
        }
        Err(e) => HttpResponse::BadRequest().json(json!({ "error": e.to_string() })),
    }
}

async fn handle_get_view(
    path: web::Path<String>,
    state: web::Data<SchemaServiceState>,
) -> HttpResponse {
    let name = path.into_inner();
    match state.get_view_by_name(&name) {
        Ok(Some(v)) => HttpResponse::Ok().json(v),
        Ok(None) => {
            HttpResponse::NotFound().json(json!({"error": format!("view `{}` not found", name)}))
        }
        Err(e) => HttpResponse::InternalServerError().json(json!({"error": e.to_string()})),
    }
}

struct SpawnedService {
    url: String,
    _temp_dir: TempDir,
}

async fn spawn_schema_service() -> SpawnedService {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir
        .path()
        .join("memory_consolidation_test_registry")
        .to_string_lossy()
        .to_string();
    let state = SchemaServiceState::new(db_path).unwrap();
    fold_db_node::schema_service::builtin_schemas::seed(&state)
        .await
        .expect("seed built-in schemas");
    let state_data = web::Data::new(state);
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    let state_clone = state_data.clone();

    let server = HttpServer::new(move || {
        App::new().app_data(state_clone.clone()).service(
            web::scope("/api")
                .route("/schemas", web::get().to(handle_list_schemas))
                .route("/schemas", web::post().to(handle_add_schema))
                .route("/schemas/available", web::get().to(handle_available))
                .route("/schema/{name}", web::get().to(handle_get_schema))
                .route("/transforms", web::post().to(handle_register_transform))
                .route("/views", web::post().to(handle_add_view))
                .route("/view/{name}", web::get().to(handle_get_view)),
        )
    })
    .listen(listener)
    .unwrap()
    .run();

    actix_web::rt::spawn(server);
    actix_web::rt::time::sleep(std::time::Duration::from_millis(200)).await;

    SpawnedService {
        url: format!("http://127.0.0.1:{}", port),
        _temp_dir: temp_dir,
    }
}

async fn create_node(schema_service_url: &str) -> (Arc<FoldNode>, TempDir) {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_string_lossy().to_string();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(path.into())
        .with_schema_service_url(schema_service_url)
        .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
    let node = FoldNode::new(config).await.expect("create FoldNode");
    (Arc::new(node), tmp)
}

// ── Helpers ────────────────────────────────────────────────────────────

fn memory_fields(id: &str, body: &str, kind: &str) -> HashMap<String, Value> {
    let mut f = HashMap::new();
    f.insert(fields::ID.to_string(), json!(id));
    f.insert(fields::BODY.to_string(), json!(body));
    f.insert(fields::KIND.to_string(), json!(kind));
    f.insert(fields::STATUS.to_string(), json!("live"));
    f.insert(fields::TAGS.to_string(), json!([] as [String; 0]));
    f.insert(fields::SOURCE.to_string(), json!("integration_test"));
    f.insert(
        fields::CREATED_AT.to_string(),
        json!("2026-04-20T12:00:00Z"),
    );
    f.insert(fields::DERIVED_FROM.to_string(), json!([] as [String; 0]));
    f
}

async fn write_memory(
    processor: &OperationProcessor,
    canonical_name: &str,
    id: &str,
    body: &str,
    kind: &str,
) {
    processor
        .execute_mutation(
            canonical_name.to_string(),
            memory_fields(id, body, kind),
            KeyValue::new(Some(id.to_string()), None),
            MutationType::Create,
        )
        .await
        .unwrap_or_else(|e| panic!("failed to write memory `{}`: {}", id, e));
}

/// Flatten the view's typed query result into a list of
/// (signature, derived_from) pairs. `execute_query_json` returns
/// `Vec<Value>` where each entry has `{ "key": {hash, range}, "fields": {...} }`.
fn collect_clusters(results: &[Value]) -> Vec<(String, Vec<String>)> {
    let mut out = Vec::new();
    for r in results {
        let fields = match r.get("fields").and_then(|v| v.as_object()) {
            Some(o) => o,
            None => continue,
        };
        let sig = fields
            .get(cluster_fields::SIGNATURE)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string();
        let derived: Vec<String> = fields
            .get(cluster_fields::DERIVED_FROM)
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|s| s.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if !sig.is_empty() {
            out.push((sig, derived));
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    out
}

// ── Tests ──────────────────────────────────────────────────────────────

/// Core Phase 1a verification: register the memory schema + TopicClusters
/// view, ingest memories that form two obvious topical clusters plus a
/// lone memory, query the view, assert two clusters with the right
/// `derived_from` sets.
///
/// This test takes ~30s on first run (runtime WASM compile). Set
/// `RUST_LOG=info` if you want to watch the pipeline.
#[actix_web::test]
async fn topic_clusters_view_emits_one_row_per_cluster() {
    let service = spawn_schema_service().await;
    let (node, _tmp) = create_node(&service.url).await;

    // 1. Register the memory schema.
    let canonical = memory::register_memory_schema(&node)
        .await
        .expect("register memory schema");

    // 2. Write two clustered sets + one lone memory.
    let processor = OperationProcessor::new(node.clone());
    let deploy_memories = [
        (
            "mem_deploy_1",
            "Always rebase on the base branch before pushing a PR. Rebasing keeps history linear and prevents merge queue conflicts.",
            "feedback",
        ),
        (
            "mem_deploy_2",
            "Auto-merge with squash flag is required. Rebase first then push then enable auto-merge. Pull requests must follow this flow.",
            "feedback",
        ),
        (
            "mem_deploy_3",
            "When a pull request merges the task moves to trash. Switch back to master branch and pull after rebase.",
            "feedback",
        ),
    ];
    let hiking_memories = [
        (
            "mem_hike_1",
            "Mount Rainier Paradise hiking trail is crowded on weekends. Weekday hikes have better photos and less traffic.",
            "project",
        ),
        (
            "mem_hike_2",
            "Snow Lake hiking trail near Snoqualmie Pass has beautiful mountain views. Bring water and a warm jacket.",
            "project",
        ),
        (
            "mem_hike_3",
            "Cascade Pass hiking trail in North Cascades is stunning. Best months are July and August. Trail is long and steep.",
            "project",
        ),
    ];
    let lone = (
        "mem_lone",
        "The espresso machine needs descaling every three months. Citric acid works well for cleaning.",
        "reference",
    );

    for (id, body, kind) in deploy_memories.iter().chain(hiking_memories.iter()) {
        write_memory(&processor, &canonical, id, body, kind).await;
    }
    write_memory(&processor, &canonical, lone.0, lone.1, lone.2).await;
    node.wait_for_background_tasks(std::time::Duration::from_secs(10))
        .await;

    // 3. Register the TopicClusters view. This compiles the WASM (slow on
    //    first call), registers it with the Global Transform Registry on
    //    the schema service (hash + classification), then creates + approves
    //    the local view.
    let registration = register_topic_clusters_view(&node, &canonical)
        .await
        .expect("register topic clusters view");
    assert_eq!(registration.view_name, TOPIC_CLUSTERS_VIEW_NAME);
    assert!(
        !registration.transform_hash.is_empty(),
        "schema service must return a non-empty content-hash for the WASM"
    );
    assert_eq!(
        registration.transform_hash.len(),
        64,
        "hash should be sha256 hex (64 chars), got `{}`",
        registration.transform_hash
    );
    eprintln!(
        "Global Transform Registry confirmed TopicClusters: hash={} outcome={:?}",
        registration.transform_hash, registration.outcome
    );

    // View orchestrator hasn't seen a source mutation since the view was
    // registered, so its cache is Empty. Querying will trigger compute.

    // 4. Query the view.
    let query = Query::new(
        TOPIC_CLUSTERS_VIEW_NAME.to_string(),
        vec![
            cluster_fields::SIGNATURE.to_string(),
            cluster_fields::DERIVED_FROM.to_string(),
            cluster_fields::SIZE.to_string(),
            cluster_fields::BODY.to_string(),
        ],
    );
    let results = processor
        .execute_query_json(query)
        .await
        .expect("query TopicClusters view");

    let clusters = collect_clusters(&results);
    eprintln!("clusters emitted:");
    for (sig, members) in &clusters {
        eprintln!("  {}  members={:?}", sig, members);
    }

    // 5. Assertions.
    assert_eq!(
        clusters.len(),
        2,
        "expected 2 clusters (deploy + hiking), got {}. Clusters: {:?}",
        clusters.len(),
        clusters
    );

    let deploy_ids: HashSet<String> = deploy_memories
        .iter()
        .map(|(id, _, _)| id.to_string())
        .collect();
    let hiking_ids: HashSet<String> = hiking_memories
        .iter()
        .map(|(id, _, _)| id.to_string())
        .collect();

    let mut saw_deploy = false;
    let mut saw_hiking = false;
    for (sig, members) in &clusters {
        let member_set: HashSet<String> = members.iter().cloned().collect();
        if member_set == deploy_ids {
            saw_deploy = true;
            assert!(
                sig.contains("mem_deploy"),
                "deploy cluster signature should contain deploy ids, got: {}",
                sig
            );
        } else if member_set == hiking_ids {
            saw_hiking = true;
            assert!(
                sig.contains("mem_hike"),
                "hiking cluster signature should contain hike ids, got: {}",
                sig
            );
        } else {
            panic!(
                "unexpected cluster: signature={}, members={:?}. Expected deploy_ids={:?} or hiking_ids={:?}",
                sig, members, deploy_ids, hiking_ids
            );
        }
    }
    assert!(saw_deploy, "missing deploy cluster");
    assert!(saw_hiking, "missing hiking cluster");

    // Lone memory must not appear in any cluster.
    for (_, members) in &clusters {
        assert!(
            !members.contains(&lone.0.to_string()),
            "lone memory `{}` leaked into a cluster: {:?}",
            lone.0,
            members
        );
    }
}
