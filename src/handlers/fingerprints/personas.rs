//! Persona view-layer handlers: list summaries and fetch detailed
//! resolved clusters for the People tab UI.
//!
//! ## Endpoints this backs
//!
//! - `GET /api/fingerprints/personas` → `ListPersonasResponse`
//!   Lists every Persona record with summary stats (name,
//!   threshold, built_in flag, identity-linked flag, and counts
//!   of fingerprints/mentions/edges in the resolved cluster).
//!
//! - `GET /api/fingerprints/personas/{id}` → `PersonaDetailResponse`
//!   Returns the full resolved cluster for one Persona, including
//!   the fingerprint/edge/mention ids and any `ResolveDiagnostics`
//!   surfaced by the resolver.
//!
//! ## Canonical-name lookup
//!
//! Handlers look up `crate::fingerprints::schemas::PERSONA` through
//! `canonical_names::lookup()` before every query. This keeps the
//! indirection out of HTTP clients — they use the stable descriptive
//! name in the routes, and the handlers translate to runtime names
//! at call time. If `canonical_names` has not been populated
//! (subsystem startup hasn't run), every call fails loudly with a
//! clear error.

use crate::fingerprints::canonical_names;
use crate::fingerprints::resolver::{PersonaResolver, PersonaSpec, ResolveDiagnostics};
use crate::fingerprints::schemas::{EDGE, FINGERPRINT, MENTION, PERSONA};
use crate::fold_node::FoldNode;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult};
use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::{MutationType, Query};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

// ── Response types ───────────────────────────────────────────────

/// Summary row for the People tab list view.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaSummary {
    pub id: String,
    pub name: String,
    /// True if `identity_id` points at an Identity record — the
    /// UI renders a "verified" badge for these.
    pub identity_linked: bool,
    pub threshold: f32,
    pub relationship: String,
    pub trust_tier: i64,
    pub built_in: bool,
    pub user_confirmed: bool,
    /// Counts come from running the resolver. These are the live
    /// cluster sizes at the persona's current threshold; they
    /// reflect the graph as of the query, not a cached value.
    pub fingerprint_count: usize,
    pub edge_count: usize,
    pub mention_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListPersonasResponse {
    pub personas: Vec<PersonaSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonaDetailResponse {
    pub id: String,
    pub name: String,
    pub threshold: f32,
    pub relationship: String,
    pub trust_tier: i64,
    pub built_in: bool,
    pub user_confirmed: bool,
    pub identity_id: Option<String>,
    pub seed_fingerprint_ids: Vec<String>,
    pub aliases: Vec<String>,
    /// Full resolved fingerprint set (includes the seeds + every
    /// endpoint reachable via edges above threshold).
    pub fingerprint_ids: Vec<String>,
    pub edge_ids: Vec<String>,
    pub mention_ids: Vec<String>,
    /// Enriched fingerprint records for the resolved set. Same
    /// ordering as `fingerprint_ids`. Entries that could not be
    /// fetched (dangling reference) are omitted and surface via
    /// `ResolveDiagnostics.dangling_edge_ids` or a new
    /// `missing_fingerprint_ids` bucket on the frontend.
    pub fingerprints: Vec<FingerprintView>,
    pub edges: Vec<EdgeView>,
    pub mentions: Vec<MentionView>,
    /// `None` when the resolver reported a clean result;
    /// `Some(diagnostics)` when anything was missing, filtered,
    /// or excluded. The UI should surface diagnostics loudly.
    pub diagnostics: Option<ResolveDiagnostics>,
}

/// A Fingerprint record flattened for the Persona detail view.
/// Carries just enough for the UI to render "tom@acme.com (email)"
/// or "face embedding (512-dim)" without a second round trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FingerprintView {
    pub id: String,
    pub kind: String,
    /// Human-readable form. For scalar kinds (email, phone, name)
    /// this is the canonical value. For face embeddings we collapse
    /// the 512-float vector down to a short placeholder so the UI
    /// doesn't have to render 2 KB of floats — the full vector
    /// stays on the Fingerprint record, where the face-specific
    /// UI can fetch it if needed.
    pub display_value: String,
    pub first_seen: Option<String>,
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeView {
    pub id: String,
    pub a: String,
    pub b: String,
    pub kind: String,
    pub weight: f32,
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MentionView {
    pub id: String,
    pub source_schema: String,
    pub source_key: String,
    pub source_field: String,
    pub extractor: String,
    pub confidence: f32,
    pub created_at: Option<String>,
}

// ── Handlers ─────────────────────────────────────────────────────

/// List every Persona as a summary row.
///
/// Runs the resolver for each persona to compute live counts. On a
/// node with many personas this is N resolver calls; Phase 1 dogfood
/// has a handful of personas so this is fine. A future optimization
/// could cache counts or stream them.
pub async fn list_personas(node: Arc<FoldNode>) -> HandlerResult<ListPersonasResponse> {
    let started = std::time::Instant::now();
    let persona_canonical = canonical_names::lookup(PERSONA).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            PERSONA, e
        ))
    })?;

    let processor = crate::fold_node::OperationProcessor::new(node.clone());

    let query = Query {
        schema_name: persona_canonical,
        fields: persona_fields(),
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };

    let records = processor
        .execute_query_json(query)
        .await
        .map_err(|e| HandlerError::Internal(format!("persona list query failed: {}", e)))?;

    let resolver = PersonaResolver::new(node.clone());
    let mut personas = Vec::with_capacity(records.len());

    for record in records {
        let fields = record.get("fields").ok_or_else(|| {
            HandlerError::Internal("persona record missing 'fields' envelope".to_string())
        })?;
        let spec = persona_spec_from_fields(fields)?;
        let result = resolver
            .resolve(&spec)
            .await
            .map_err(|e| HandlerError::Internal(format!("persona resolve failed: {}", e)))?;

        let summary = PersonaSummary {
            id: spec.persona_id.clone(),
            name: string_field(fields, "name").unwrap_or_default(),
            identity_linked: spec.identity_id.is_some(),
            threshold: spec.threshold,
            relationship: string_field(fields, "relationship").unwrap_or_default(),
            trust_tier: int_field(fields, "trust_tier").unwrap_or(0),
            built_in: bool_field(fields, "built_in").unwrap_or(false),
            user_confirmed: bool_field(fields, "user_confirmed").unwrap_or(false),
            fingerprint_count: result.fingerprint_ids().len(),
            edge_count: result.edge_ids().len(),
            mention_count: result.mention_ids().len(),
        };
        personas.push(summary);
    }

    // Deterministic order: built-in first (Me should always appear
    // at the top), then by name. The frontend can re-sort, but
    // shipping a stable default is nicer than hash-map order.
    personas.sort_by(|a, b| match (a.built_in, b.built_in) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.cmp(&b.name),
    });

    log::info!(
        "fingerprints.handler: list_personas resolved {} personas in {:?}",
        personas.len(),
        started.elapsed()
    );

    Ok(ApiResponse::success(ListPersonasResponse { personas }))
}

/// Fetch a single Persona by id and return its resolved cluster +
/// diagnostics.
pub async fn get_persona(
    node: Arc<FoldNode>,
    persona_id: String,
) -> HandlerResult<PersonaDetailResponse> {
    let started = std::time::Instant::now();
    let persona_canonical = canonical_names::lookup(PERSONA).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            PERSONA, e
        ))
    })?;

    let processor = crate::fold_node::OperationProcessor::new(node.clone());

    let query = Query {
        schema_name: persona_canonical,
        fields: persona_fields(),
        filter: Some(fold_db::schema::types::field::HashRangeFilter::HashKey(
            persona_id.clone(),
        )),
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };

    let records = processor
        .execute_query_json(query)
        .await
        .map_err(|e| HandlerError::Internal(format!("persona query failed: {}", e)))?;

    let record = records
        .first()
        .ok_or_else(|| HandlerError::NotFound(format!("persona '{}' not found", persona_id)))?;
    let fields = record.get("fields").ok_or_else(|| {
        HandlerError::Internal("persona record missing 'fields' envelope".to_string())
    })?;

    let spec = persona_spec_from_fields(fields)?;

    let resolver = PersonaResolver::new(node.clone());
    let result = resolver
        .resolve(&spec)
        .await
        .map_err(|e| HandlerError::Internal(format!("persona resolve failed: {}", e)))?;

    let aliases = string_array_field(fields, "aliases");
    let mut fp_ids: Vec<String> = result.fingerprint_ids().iter().cloned().collect();
    let mut edge_ids: Vec<String> = result.edge_ids().iter().cloned().collect();
    let mut mention_ids: Vec<String> = result.mention_ids().iter().cloned().collect();
    fp_ids.sort();
    edge_ids.sort();
    mention_ids.sort();

    // Hydrate the resolved ID sets into full records so the UI has
    // something readable to show (emails, face-embedding placeholders,
    // source record pointers) instead of opaque hashes. One HashKey
    // query per record — fine for Phase 1 dogfood cluster sizes; a
    // future optimization can batch or cache.
    let fingerprints = fetch_fingerprint_views(&processor, &fp_ids).await?;
    let edges = fetch_edge_views(&processor, &edge_ids).await?;
    let mentions = fetch_mention_views(&processor, &mention_ids).await?;

    let diagnostics = result.diagnostics().cloned();

    log::info!(
        "fingerprints.handler: get_persona '{}' resolved in {:?} (fps={}, edges={}, mentions={}, clean={})",
        persona_id,
        started.elapsed(),
        fp_ids.len(),
        edge_ids.len(),
        mention_ids.len(),
        diagnostics.is_none()
    );

    Ok(ApiResponse::success(PersonaDetailResponse {
        id: spec.persona_id,
        name: string_field(fields, "name").unwrap_or_default(),
        threshold: spec.threshold,
        relationship: string_field(fields, "relationship").unwrap_or_default(),
        trust_tier: int_field(fields, "trust_tier").unwrap_or(0),
        built_in: bool_field(fields, "built_in").unwrap_or(false),
        user_confirmed: bool_field(fields, "user_confirmed").unwrap_or(false),
        identity_id: spec.identity_id,
        seed_fingerprint_ids: spec.seed_fingerprint_ids,
        aliases,
        fingerprint_ids: fp_ids,
        edge_ids,
        mention_ids,
        fingerprints,
        edges,
        mentions,
        diagnostics,
    }))
}

// ── Enrichment helpers ───────────────────────────────────────────
//
// Each helper runs one HashKey query per ID against the appropriate
// schema, flattens the result into a view struct, and skips IDs the
// store no longer has (dangling references are already surfaced via
// the resolver's diagnostics). Missing records are logged but do not
// fail the request — the goal is best-effort enrichment.

async fn fetch_fingerprint_views(
    processor: &crate::fold_node::OperationProcessor,
    ids: &[String],
) -> Result<Vec<FingerprintView>, HandlerError> {
    let canonical = canonical_names::lookup(FINGERPRINT).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            FINGERPRINT, e
        ))
    })?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let query = Query {
            schema_name: canonical.clone(),
            fields: vec![
                "id".to_string(),
                "kind".to_string(),
                "value".to_string(),
                "first_seen".to_string(),
                "last_seen".to_string(),
            ],
            filter: Some(fold_db::schema::types::field::HashRangeFilter::HashKey(
                id.clone(),
            )),
            as_of: None,
            rehydrate_depth: None,
            sort_order: None,
            value_filters: None,
        };
        let records = processor.execute_query_json(query).await.map_err(|e| {
            HandlerError::Internal(format!("fingerprint '{}' query failed: {}", id, e))
        })?;
        let Some(record) = records.first() else {
            log::warn!(
                "fingerprints.handler: fingerprint '{}' not found during enrichment",
                id
            );
            continue;
        };
        let Some(fields) = record.get("fields") else {
            continue;
        };
        let kind = string_field(fields, "kind").unwrap_or_default();
        let display_value = fingerprint_display_value(&kind, fields.get("value"));
        out.push(FingerprintView {
            id: id.clone(),
            kind,
            display_value,
            first_seen: string_field(fields, "first_seen"),
            last_seen: string_field(fields, "last_seen"),
        });
    }
    Ok(out)
}

async fn fetch_edge_views(
    processor: &crate::fold_node::OperationProcessor,
    ids: &[String],
) -> Result<Vec<EdgeView>, HandlerError> {
    let canonical = canonical_names::lookup(EDGE).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            EDGE, e
        ))
    })?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let query = Query {
            schema_name: canonical.clone(),
            fields: vec![
                "id".to_string(),
                "a".to_string(),
                "b".to_string(),
                "kind".to_string(),
                "weight".to_string(),
                "created_at".to_string(),
            ],
            filter: Some(fold_db::schema::types::field::HashRangeFilter::HashKey(
                id.clone(),
            )),
            as_of: None,
            rehydrate_depth: None,
            sort_order: None,
            value_filters: None,
        };
        let records = processor
            .execute_query_json(query)
            .await
            .map_err(|e| HandlerError::Internal(format!("edge '{}' query failed: {}", id, e)))?;
        let Some(record) = records.first() else {
            log::warn!(
                "fingerprints.handler: edge '{}' not found during enrichment",
                id
            );
            continue;
        };
        let Some(fields) = record.get("fields") else {
            continue;
        };
        let weight = fields
            .get("weight")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32)
            .unwrap_or(0.0);
        out.push(EdgeView {
            id: id.clone(),
            a: string_field(fields, "a").unwrap_or_default(),
            b: string_field(fields, "b").unwrap_or_default(),
            kind: string_field(fields, "kind").unwrap_or_default(),
            weight,
            created_at: string_field(fields, "created_at"),
        });
    }
    Ok(out)
}

async fn fetch_mention_views(
    processor: &crate::fold_node::OperationProcessor,
    ids: &[String],
) -> Result<Vec<MentionView>, HandlerError> {
    let canonical = canonical_names::lookup(MENTION).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            MENTION, e
        ))
    })?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let query = Query {
            schema_name: canonical.clone(),
            fields: vec![
                "id".to_string(),
                "source_schema".to_string(),
                "source_key".to_string(),
                "source_field".to_string(),
                "extractor".to_string(),
                "confidence".to_string(),
                "created_at".to_string(),
            ],
            filter: Some(fold_db::schema::types::field::HashRangeFilter::HashKey(
                id.clone(),
            )),
            as_of: None,
            rehydrate_depth: None,
            sort_order: None,
            value_filters: None,
        };
        let records = processor
            .execute_query_json(query)
            .await
            .map_err(|e| HandlerError::Internal(format!("mention '{}' query failed: {}", id, e)))?;
        let Some(record) = records.first() else {
            log::warn!(
                "fingerprints.handler: mention '{}' not found during enrichment",
                id
            );
            continue;
        };
        let Some(fields) = record.get("fields") else {
            continue;
        };
        let confidence = fields
            .get("confidence")
            .and_then(|v| v.as_f64())
            .map(|f| f as f32)
            .unwrap_or(0.0);
        out.push(MentionView {
            id: id.clone(),
            source_schema: string_field(fields, "source_schema").unwrap_or_default(),
            source_key: string_field(fields, "source_key").unwrap_or_default(),
            source_field: string_field(fields, "source_field").unwrap_or_default(),
            extractor: string_field(fields, "extractor").unwrap_or_default(),
            confidence,
            created_at: string_field(fields, "created_at"),
        });
    }
    Ok(out)
}

/// Reduce a Fingerprint.value cell into a human-readable string.
///
/// Scalar kinds (email, phone, full_name, first_name, handle,
/// node_pub_key) return the string as-is. Face embeddings are a
/// 512-float vector, ~2 KB serialized — we collapse them to a short
/// placeholder so the UI doesn't have to render the whole vector.
/// Unknown kinds fall through to a best-effort stringification.
fn fingerprint_display_value(kind: &str, value: Option<&Value>) -> String {
    let Some(value) = value else {
        return String::new();
    };
    if kind.eq_ignore_ascii_case("face_embedding") || kind.eq_ignore_ascii_case("faceembedding") {
        let dim = value.as_array().map(|a| a.len()).unwrap_or(0);
        return format!("face embedding ({} dims)", dim);
    }
    match value {
        Value::String(s) => s.clone(),
        Value::Array(items) => {
            if items.is_empty() {
                "[]".to_string()
            } else {
                format!("{} items", items.len())
            }
        }
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Update the threshold field on an existing Persona record, then
/// return the re-resolved detail. The frontend uses this to drive
/// the Persona detail threshold slider.
///
/// This is a read-modify-write against the Persona record:
/// 1. Fetch the existing record via the standard query path.
/// 2. Copy every field into a fresh mutation payload, overwriting
///    only `threshold` with the caller-supplied value.
/// 3. Execute a `MutationType::Update` that preserves the content-
///    addressed primary key (`id`) and writes the merged record.
/// 4. Re-run `get_persona` to return the updated detail with freshly
///    resolved counts and diagnostics.
///
/// Read-modify-write rather than a partial update because
/// `execute_mutation` expects all fields to be present in the
/// fields map, and we can't assume the caller's record is in the
/// writer's in-memory state.
pub async fn update_persona_threshold(
    node: Arc<FoldNode>,
    persona_id: String,
    new_threshold: f32,
) -> HandlerResult<PersonaDetailResponse> {
    // Validate range. Clients should already clamp but we defend
    // against malformed payloads here because a threshold outside
    // [0, 1] would silently break cluster resolution.
    if !new_threshold.is_finite() || !(0.0..=1.0).contains(&new_threshold) {
        return Err(HandlerError::BadRequest(format!(
            "threshold must be a finite number in [0.0, 1.0], got {}",
            new_threshold
        )));
    }

    let persona_canonical = canonical_names::lookup(PERSONA).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            PERSONA, e
        ))
    })?;

    let processor = crate::fold_node::OperationProcessor::new(node.clone());

    // 1. Fetch existing record by primary key.
    let query = Query {
        schema_name: persona_canonical.clone(),
        fields: persona_fields(),
        filter: Some(fold_db::schema::types::field::HashRangeFilter::HashKey(
            persona_id.clone(),
        )),
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let records = processor
        .execute_query_json(query)
        .await
        .map_err(|e| HandlerError::Internal(format!("persona query failed: {}", e)))?;
    let record = records
        .first()
        .ok_or_else(|| HandlerError::NotFound(format!("persona '{}' not found", persona_id)))?;
    let fields = record.get("fields").ok_or_else(|| {
        HandlerError::Internal("persona record missing 'fields' envelope".to_string())
    })?;

    // 2. Copy every field, overwriting only `threshold`.
    let mut payload: HashMap<String, Value> = HashMap::new();
    let Value::Object(field_map) = fields else {
        return Err(HandlerError::Internal(
            "persona record 'fields' envelope is not a JSON object".to_string(),
        ));
    };
    for (k, v) in field_map.iter() {
        payload.insert(k.clone(), v.clone());
    }
    payload.insert(
        "threshold".to_string(),
        serde_json::Number::from_f64(new_threshold as f64)
            .map(Value::Number)
            .unwrap_or(Value::Null),
    );

    // 3. Execute Update. Keep the same content-addressed primary key.
    let key_value = KeyValue::new(Some(persona_id.clone()), None);
    processor
        .execute_mutation(persona_canonical, payload, key_value, MutationType::Update)
        .await
        .map_err(|e| {
            HandlerError::Internal(format!(
                "failed to update persona '{}' threshold: {}",
                persona_id, e
            ))
        })?;

    log::info!(
        "fingerprints.handler: updated persona '{}' threshold to {:.3}",
        persona_id,
        new_threshold
    );

    // 4. Re-run the standard detail path so the caller gets the
    //    freshly-resolved counts + diagnostics.
    get_persona(node, persona_id).await
}

// ── Field-extraction helpers ─────────────────────────────────────

fn persona_fields() -> Vec<String> {
    vec![
        "id".to_string(),
        "name".to_string(),
        "seed_fingerprint_ids".to_string(),
        "threshold".to_string(),
        "excluded_mention_ids".to_string(),
        "excluded_edge_ids".to_string(),
        "included_mention_ids".to_string(),
        "aliases".to_string(),
        "relationship".to_string(),
        "trust_tier".to_string(),
        "identity_id".to_string(),
        "user_confirmed".to_string(),
        "built_in".to_string(),
        "created_at".to_string(),
    ]
}

fn persona_spec_from_fields(fields: &Value) -> Result<PersonaSpec, HandlerError> {
    let persona_id = string_field(fields, "id")
        .ok_or_else(|| HandlerError::Internal("persona record missing 'id' field".to_string()))?;
    let seed_fingerprint_ids = string_array_field(fields, "seed_fingerprint_ids");
    let threshold = fields
        .get("threshold")
        .and_then(|v| v.as_f64())
        .map(|n| n as f32)
        .unwrap_or(0.85);
    let excluded_edge_ids: HashSet<String> = string_array_field(fields, "excluded_edge_ids")
        .into_iter()
        .collect();
    let excluded_mention_ids: HashSet<String> = string_array_field(fields, "excluded_mention_ids")
        .into_iter()
        .collect();
    let included_mention_ids: HashSet<String> = string_array_field(fields, "included_mention_ids")
        .into_iter()
        .collect();

    // identity_id is stored as either null OR a SchemaRef reference
    // object `{"schema": "Identity", "key": "id_..."}`. Extract the
    // key string for the PersonaSpec.
    let identity_id = fields.get("identity_id").and_then(|v| {
        if v.is_null() {
            None
        } else {
            v.get("key").and_then(|k| k.as_str()).map(|s| s.to_string())
        }
    });

    Ok(PersonaSpec {
        persona_id,
        seed_fingerprint_ids,
        threshold,
        excluded_edge_ids,
        excluded_mention_ids,
        included_mention_ids,
        identity_id,
    })
}

fn string_field(fields: &Value, name: &str) -> Option<String> {
    fields
        .get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn bool_field(fields: &Value, name: &str) -> Option<bool> {
    fields.get(name).and_then(|v| v.as_bool())
}

fn int_field(fields: &Value, name: &str) -> Option<i64> {
    fields.get(name).and_then(|v| v.as_i64())
}

fn string_array_field(fields: &Value, name: &str) -> Vec<String> {
    fields
        .get(name)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

// Silence unused-import warning for HashMap — reserved for future
// field handling where HashMap-shaped records may appear.
#[allow(dead_code)]
fn _unused_hashmap() {
    let _: HashMap<String, String> = HashMap::new();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn persona_spec_extracts_all_fields_from_record() {
        let fields = json!({
            "id": "ps_me",
            "name": "Me",
            "seed_fingerprint_ids": ["fp_a", "fp_b"],
            "threshold": 0.9_f32,
            "excluded_mention_ids": ["mn_x"],
            "excluded_edge_ids": ["eg_y"],
            "included_mention_ids": [],
            "aliases": [],
            "relationship": "self",
            "trust_tier": 4,
            "identity_id": { "schema": "Identity", "key": "id_pubkey" },
            "user_confirmed": true,
            "built_in": true,
            "created_at": "2026-04-14T12:00:00Z",
        });
        let spec = persona_spec_from_fields(&fields).unwrap();
        assert_eq!(spec.persona_id, "ps_me");
        assert_eq!(spec.seed_fingerprint_ids, vec!["fp_a", "fp_b"]);
        assert!((spec.threshold - 0.9).abs() < 1e-6);
        assert_eq!(spec.excluded_edge_ids.len(), 1);
        assert!(spec.excluded_edge_ids.contains("eg_y"));
        assert_eq!(spec.excluded_mention_ids.len(), 1);
        assert!(spec.excluded_mention_ids.contains("mn_x"));
        assert_eq!(spec.included_mention_ids.len(), 0);
        assert_eq!(spec.identity_id.as_deref(), Some("id_pubkey"));
    }

    #[test]
    fn persona_spec_treats_null_identity_id_as_none() {
        let fields = json!({
            "id": "ps_other",
            "name": "Alice",
            "seed_fingerprint_ids": ["fp_a"],
            "threshold": 0.85_f32,
            "excluded_mention_ids": [],
            "excluded_edge_ids": [],
            "included_mention_ids": [],
            "aliases": [],
            "relationship": "friend",
            "trust_tier": 2,
            "identity_id": Value::Null,
            "user_confirmed": false,
            "built_in": false,
            "created_at": "2026-04-14T12:00:00Z",
        });
        let spec = persona_spec_from_fields(&fields).unwrap();
        assert_eq!(spec.identity_id, None);
    }

    #[test]
    fn persona_spec_returns_error_when_id_missing() {
        let fields = json!({
            "name": "Missing id",
            "seed_fingerprint_ids": [],
            "threshold": 0.85_f32,
        });
        let err = persona_spec_from_fields(&fields).unwrap_err();
        match err {
            HandlerError::Internal(msg) => assert!(msg.contains("missing 'id'")),
            _ => panic!("expected Internal error"),
        }
    }

    #[test]
    fn string_array_field_returns_empty_when_missing() {
        let fields = json!({});
        assert_eq!(string_array_field(&fields, "aliases"), Vec::<String>::new());
    }

    #[test]
    fn fingerprint_display_value_handles_scalar_kinds() {
        let v = json!("tom@acme.com");
        assert_eq!(fingerprint_display_value("email", Some(&v)), "tom@acme.com");
    }

    #[test]
    fn fingerprint_display_value_collapses_face_embedding_to_dim_count() {
        let vec: Vec<Value> = (0..512).map(|_| json!(0.1)).collect();
        let v = Value::Array(vec);
        assert_eq!(
            fingerprint_display_value("face_embedding", Some(&v)),
            "face embedding (512 dims)"
        );
    }

    #[test]
    fn fingerprint_display_value_missing_value_is_empty() {
        assert_eq!(fingerprint_display_value("email", None), "");
    }

    #[test]
    fn string_array_field_ignores_non_string_entries() {
        let fields = json!({ "aliases": ["a", 42, "b"] });
        assert_eq!(
            string_array_field(&fields, "aliases"),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
