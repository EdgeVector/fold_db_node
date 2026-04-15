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
use crate::fingerprints::schemas::PERSONA;
use crate::fold_node::FoldNode;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult};
use fold_db::schema::types::operations::Query;
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
    /// `None` when the resolver reported a clean result;
    /// `Some(diagnostics)` when anything was missing, filtered,
    /// or excluded. The UI should surface diagnostics loudly.
    pub diagnostics: Option<ResolveDiagnostics>,
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
        diagnostics,
    }))
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
    fn string_array_field_ignores_non_string_entries() {
        let fields = json!({ "aliases": ["a", 42, "b"] });
        assert_eq!(
            string_array_field(&fields, "aliases"),
            vec!["a".to_string(), "b".to_string()]
        );
    }
}
