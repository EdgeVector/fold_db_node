//! Memory consolidation via reactive TransformViews.
//!
//! Consolidation is not a Rust agent — it's a [`TransformView`] whose WASM
//! body does cluster detection and row emission. See
//! [`docs/design/memory_agent.md`](https://github.com/EdgeVector/exemem-workspace/blob/master/docs/design/memory_agent.md)
//! for the full architecture.
//!
//! Phase 1a ships one view — `TopicClusters` — with a deterministic
//! bag-of-words + union-find WASM that emits one row per cluster. Phase 2
//! swaps the WASM for an LLM-backed summarizer once the Model Registry's
//! host-import shim exists.
//!
//! ## Feature gate
//!
//! This module is behind `transform-wasm` because it depends on the runtime
//! WASM compiler (`fold_node::wasm_compiler`), which in turn requires the
//! `wasm32-unknown-unknown` target on the build host. For local dev: `rustup
//! target add wasm32-unknown-unknown`. For CI: gated, runs when explicitly
//! requested with `--features transform-wasm`.
//!
//! ## Flow
//!
//! ```text
//! register_topic_clusters_view(node, memory_canonical)
//!   │
//!   ├── compile transform source → WASM bytes (one-shot cargo build, ~10-30s)
//!   │
//!   ├── build TransformView:
//!   │     input_queries = [ Memory(all fields) ]
//!   │     output_fields = { body, derived_from, size, signature }
//!   │     wasm_transform = compiled bytes
//!   │
//!   ├── processor.create_view(view)
//!   │
//!   └── processor.approve_view(name)
//!
//! After that:
//!   - Any mutation on Memory invalidates this view's cache.
//!   - Querying TopicClusters triggers recompute if cache is empty.
//!   - Background precomputation rehydrates deep views automatically.
//! ```

use std::collections::HashMap;

use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::schema::types::field_value_type::FieldValueType;
use fold_db::schema::types::key_config::KeyConfig;
use fold_db::schema::types::operations::Query;
use fold_db::schema::types::schema::DeclarativeSchemaType as SchemaType;
use fold_db::view::types::TransformView;

use crate::fold_node::{wasm_compiler, FoldNode, OperationProcessor};
use crate::memory::fields;
use crate::schema_service::types::{RegisterTransformRequest, TransformAddOutcome};

/// Descriptive name of the TopicClusters view.
pub const TOPIC_CLUSTERS_VIEW_NAME: &str = "TopicClusters";

/// Field names on the TopicClusters output schema.
pub mod cluster_fields {
    /// Stub cluster body — Phase 2 swaps in an LLM summary.
    pub const BODY: &str = "body";
    /// Sorted list of source memory IDs (Array<String>).
    pub const DERIVED_FROM: &str = "derived_from";
    /// Cluster size (Integer).
    pub const SIZE: &str = "size";
    /// Stable signature derived from sorted memory IDs (String).
    pub const SIGNATURE: &str = "signature";
}

/// Rust source for the clustering WASM. The scaffold in
/// `wasm_compiler::compile_rust_to_wasm` wraps this with memory-management
/// exports (`alloc`, `transform`) and serde_json helpers.
///
/// Contract:
///   Input:  { "inputs": { <schema_name>: { <field>: { <key>: value } } } }
///   Output: { "fields": { <output_field>: { <cluster_signature>: value } } }
///
/// Algorithm:
///   1. Extract live memories (status == "live" or absent)
///   2. Tokenize each body, skip stop words, build bag-of-words term-frequency
///      vectors
///   3. Pairwise cosine similarity; union-find over pairs above threshold
///   4. Emit one output row per connected component of size >= MIN_CLUSTER_SIZE
///
/// The threshold and stop list are deliberately conservative starting points;
/// tune with the dogfood harness's `eval` command.
const CLUSTER_MEMORIES_TRANSFORM_SRC: &str = r#"
fn transform_impl(input: Value) -> Value {
    let empty_map = serde_json::Map::new();

    // 1. Navigate the input envelope. The view has exactly one input query
    //    against the Memory schema, so take the first (and only) schema
    //    entry — we don't know its canonical name statically.
    let inputs = match input.get("inputs").and_then(|v| v.as_object()) {
        Some(o) => o,
        None => return empty_output(),
    };
    let (_schema_name, schema_fields) = match inputs.iter().next() {
        Some(entry) => entry,
        None => return empty_output(),
    };
    let schema_obj = match schema_fields.as_object() {
        Some(o) => o,
        None => return empty_output(),
    };

    let bodies = schema_obj.get("body").and_then(|v| v.as_object()).unwrap_or(&empty_map);
    let ids = schema_obj.get("id").and_then(|v| v.as_object()).unwrap_or(&empty_map);
    let statuses = schema_obj.get("status").and_then(|v| v.as_object()).unwrap_or(&empty_map);
    let kinds = schema_obj.get("kind").and_then(|v| v.as_object()).unwrap_or(&empty_map);

    // 2. Collect (key, id, body) for live memories. Skip proposals + rejected.
    let mut memories: Vec<(String, String, String)> = Vec::new();
    for (key, body_val) in bodies {
        let body = match body_val.as_str() { Some(s) => s, None => continue };
        let status = statuses.get(key).and_then(|v| v.as_str()).unwrap_or("live");
        if status != "live" { continue; }
        let kind = kinds.get(key).and_then(|v| v.as_str()).unwrap_or("");
        // Skip anything that's already a consolidation — don't cluster clusters.
        if kind == "consolidation_proposal" || kind == "approved_consolidation" { continue; }
        let id = ids.get(key).and_then(|v| v.as_str()).unwrap_or(key).to_string();
        memories.push((key.clone(), id, body.to_string()));
    }

    const MIN_CLUSTER_SIZE: usize = 3;
    const THRESHOLD: f32 = 0.15;

    if memories.len() < MIN_CLUSTER_SIZE {
        return empty_output();
    }

    // 3. Bag-of-words term-frequency vectors, stop-words stripped.
    let stop_words: std::collections::HashSet<&'static str> = [
        "a", "an", "and", "are", "as", "at", "be", "but", "by", "do", "for",
        "from", "has", "have", "in", "is", "it", "its", "of", "on", "or",
        "that", "the", "to", "was", "were", "will", "with", "this", "these",
        "those", "not", "so", "if", "which", "when", "where", "why", "how",
        "what", "who", "then", "you", "your", "our", "they", "them", "their",
    ].into_iter().collect();

    let bags: Vec<std::collections::HashMap<String, f32>> = memories.iter().map(|(_, _, body)| {
        let mut bag: std::collections::HashMap<String, f32> = std::collections::HashMap::new();
        for token in body.to_lowercase().split(|c: char| !c.is_alphanumeric()) {
            if token.len() < 3 { continue; }
            if stop_words.contains(token) { continue; }
            *bag.entry(token.to_string()).or_insert(0.0) += 1.0;
        }
        bag
    }).collect();

    // 4. Union-find over pairs above threshold.
    let n = memories.len();
    let mut parent: Vec<usize> = (0..n).collect();

    for i in 0..n {
        for j in (i + 1)..n {
            let sim = cosine(&bags[i], &bags[j]);
            if sim > THRESHOLD {
                let ri = find(&mut parent, i);
                let rj = find(&mut parent, j);
                if ri != rj {
                    parent[ri] = rj;
                }
            }
        }
    }

    // 5. Group by component root.
    let mut components: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        components.entry(root).or_default().push(i);
    }

    // 6. Emit one row per component of size >= MIN_CLUSTER_SIZE.
    let mut body_out = serde_json::Map::new();
    let mut derived_out = serde_json::Map::new();
    let mut size_out = serde_json::Map::new();
    let mut sig_out = serde_json::Map::new();

    for members in components.values() {
        if members.len() < MIN_CLUSTER_SIZE { continue; }
        let mut member_ids: Vec<String> = members.iter().map(|&i| memories[i].1.clone()).collect();
        member_ids.sort();
        let signature = format!("cluster_{}", member_ids.join("_"));

        let body_text = format!(
            "Cluster of {} memories (Phase 1a stub; Phase 2 summary pending): {}",
            member_ids.len(),
            member_ids.join(", ")
        );

        body_out.insert(signature.clone(), serde_json::Value::String(body_text));
        derived_out.insert(
            signature.clone(),
            serde_json::Value::Array(
                member_ids.iter().cloned().map(serde_json::Value::String).collect()
            )
        );
        size_out.insert(
            signature.clone(),
            serde_json::Value::Number((member_ids.len() as u64).into())
        );
        sig_out.insert(signature.clone(), serde_json::Value::String(signature.clone()));
    }

    serde_json::json!({
        "fields": {
            "body": body_out,
            "derived_from": derived_out,
            "size": size_out,
            "signature": sig_out,
        }
    })
}

fn empty_output() -> Value {
    serde_json::json!({
        "fields": {
            "body": {},
            "derived_from": {},
            "size": {},
            "signature": {},
        }
    })
}

fn find(parent: &mut [usize], x: usize) -> usize {
    let mut root = x;
    while parent[root] != root { root = parent[root]; }
    // Path compression.
    let mut cur = x;
    while parent[cur] != root {
        let next = parent[cur];
        parent[cur] = root;
        cur = next;
    }
    root
}

fn cosine(a: &std::collections::HashMap<String, f32>, b: &std::collections::HashMap<String, f32>) -> f32 {
    let dot: f32 = a.iter().map(|(k, va)| va * b.get(k).copied().unwrap_or(0.0)).sum();
    let na: f32 = a.values().map(|v| v * v).sum::<f32>().sqrt();
    let nb: f32 = b.values().map(|v| v * v).sum::<f32>().sqrt();
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na * nb) }
}
"#;

/// Compile the clustering transform source to WASM bytes.
///
/// This calls `cargo build --target wasm32-unknown-unknown` under the hood,
/// so it's slow (~10-30s) on first invocation. Callers should compile once
/// and cache the result when possible.
pub fn compile_cluster_memories_transform() -> FoldDbResult<Vec<u8>> {
    wasm_compiler::compile_rust_to_wasm(CLUSTER_MEMORIES_TRANSFORM_SRC).map_err(|e| {
        FoldDbError::Config(format!(
            "memory: failed to compile cluster_memories transform: {}",
            e
        ))
    })
}

/// Result of registering the TopicClusters consolidation end-to-end.
#[derive(Debug, Clone)]
pub struct TopicClustersRegistration {
    /// Local view name (always `TOPIC_CLUSTERS_VIEW_NAME`).
    pub view_name: String,
    /// Schema service's canonical content-hash for the WASM (sha256).
    /// Use this to audit, share, or dedup the transform across nodes.
    pub transform_hash: String,
    /// Whether the schema service added the transform or returned an
    /// existing record with the same hash.
    pub outcome: TransformAddOutcome,
}

/// Register the `TopicClusters` TransformView end-to-end:
///
/// 1. Compile the clustering WASM via `fold_node::wasm_compiler`.
/// 2. Register the WASM with the Global Transform Registry on the schema
///    service via `node.register_transform_on_service`. The service
///    content-addresses it by sha256 and classifies it against the input
///    queries' data classifications. This is the audit / cross-node
///    sharing layer.
/// 3. Build a local [`TransformView`] referencing the same WASM bytes inline.
/// 4. `create_view` + `approve_view` on the local node so the view
///    orchestrator can execute it.
///
/// Both sides hold the same bytes until the runtime fetches bytes by hash
/// on demand (a future platform improvement). The registry step is what
/// makes the transform auditable and content-addressed globally.
///
/// Idempotent: re-registering with identical WASM + input queries returns
/// `TransformAddOutcome::AlreadyExists` from the registry, and
/// `create_view` on the local side will error if the view already exists
/// (caller should catch and re-approve if needed). For the dogfood flow,
/// run `reset` before re-registering.
pub async fn register_topic_clusters_view(
    node: &FoldNode,
    memory_canonical: &str,
) -> FoldDbResult<TopicClustersRegistration> {
    log::info!("memory: compiling cluster_memories WASM transform (may take 10-30s on first call)");
    let wasm_bytes = compile_cluster_memories_transform()?;
    log::info!(
        "memory: compiled cluster_memories WASM ({} bytes)",
        wasm_bytes.len()
    );

    // Input query: pull every field we need for clustering + output identity.
    let input_query = Query::new(
        memory_canonical.to_string(),
        vec![
            fields::ID.to_string(),
            fields::BODY.to_string(),
            fields::KIND.to_string(),
            fields::STATUS.to_string(),
        ],
    );

    // Output schema: one row per cluster, keyed by signature.
    let mut output_fields: HashMap<String, FieldValueType> = HashMap::new();
    output_fields.insert(cluster_fields::BODY.to_string(), FieldValueType::String);
    output_fields.insert(
        cluster_fields::DERIVED_FROM.to_string(),
        FieldValueType::Array(Box::new(FieldValueType::String)),
    );
    output_fields.insert(cluster_fields::SIZE.to_string(), FieldValueType::Integer);
    output_fields.insert(
        cluster_fields::SIGNATURE.to_string(),
        FieldValueType::String,
    );

    // Step 1: Register with the Global Transform Registry on the schema
    // service. This is the audit + classification + cross-node sharing
    // layer. The service hashes the WASM by sha256 and classifies it
    // against the input queries' data classifications.
    let registry_request = RegisterTransformRequest {
        name: TOPIC_CLUSTERS_VIEW_NAME.to_string(),
        version: "0.1.0".to_string(),
        description: Some(
            "Memory consolidation via bag-of-words + union-find clustering. \
             Deterministic; Phase 2 replaces with an LLM-backed summarizer."
                .to_string(),
        ),
        input_queries: vec![input_query.clone()],
        output_fields: output_fields.clone(),
        source_url: None,
        wasm_bytes: wasm_bytes.clone(),
    };

    let registry_response = node
        .register_transform_on_service(&registry_request)
        .await
        .map_err(|e| {
            FoldDbError::Config(format!(
                "memory: failed to register `{}` WASM with the Global Transform Registry: {}",
                TOPIC_CLUSTERS_VIEW_NAME, e
            ))
        })?;

    log::info!(
        "memory: Global Transform Registry confirmed `{}` — hash={} outcome={:?} assigned_classification={:?}",
        TOPIC_CLUSTERS_VIEW_NAME,
        registry_response.hash,
        registry_response.outcome,
        registry_response.record.assigned_classification
    );

    // Step 2: Build the local TransformView referencing the same WASM bytes.
    let view = TransformView::new(
        TOPIC_CLUSTERS_VIEW_NAME,
        // WASM-backed views must use Range or Single — Hash isn't supported
        // (see `ViewResolver::execute_wasm_transform`, which builds
        // `KeyValue::new(None, Some(key_str))` for every emitted key).
        SchemaType::Range,
        Some(KeyConfig::new(
            None,
            Some(cluster_fields::SIGNATURE.to_string()),
        )),
        vec![input_query],
        Some(wasm_bytes),
        output_fields,
    );

    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    processor.create_view(view).await.map_err(|e| {
        FoldDbError::Config(format!(
            "memory: failed to register local `{}` view: {}",
            TOPIC_CLUSTERS_VIEW_NAME, e
        ))
    })?;
    processor
        .approve_view(TOPIC_CLUSTERS_VIEW_NAME)
        .await
        .map_err(|e| {
            FoldDbError::Config(format!(
                "memory: failed to approve local `{}` view: {}",
                TOPIC_CLUSTERS_VIEW_NAME, e
            ))
        })?;

    log::info!(
        "memory: local `{}` view registered + approved on source schema `{}`",
        TOPIC_CLUSTERS_VIEW_NAME,
        memory_canonical
    );

    Ok(TopicClustersRegistration {
        view_name: TOPIC_CLUSTERS_VIEW_NAME.to_string(),
        transform_hash: registry_response.hash,
        outcome: registry_response.outcome,
    })
}
