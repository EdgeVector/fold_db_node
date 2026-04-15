//! Suggested Personas — the dense-subgraph sweep from
//! `docs/designs/fingerprints.md` §"System-proposed clusters".
//!
//! Scans every Fingerprint, runs BFS over Edges with weight ≥ 0.85
//! (skipping `UserForbidden`), and emits components with ≥ `MIN_FPS`
//! fingerprints and ≥ `MIN_MENTIONS` mentions. Components already
//! fully covered by an existing Persona are filtered out.
//!
//! Dismissals are frontend-only soft state per the design doc —
//! there is no `dismissed_cluster` table, no write on Dismiss. The
//! `suggested_id` is a deterministic SHA-256 of the sorted
//! fingerprint ids so the frontend can keep a local dismiss set
//! across refreshes if it wants to.
//!
//! Endpoints this backs:
//! - `GET  /api/fingerprints/suggestions` — list candidate clusters
//! - `POST /api/fingerprints/suggestions/accept` — promote a candidate
//!   into a real Persona record and return its `PersonaDetailResponse`

use crate::fingerprints::canonical_names;
use crate::fingerprints::keys::edge_kind;
use crate::fingerprints::schemas::{EDGE, EDGE_BY_FINGERPRINT, FINGERPRINT, PERSONA};
use crate::fold_node::{FoldNode, OperationProcessor};
use crate::handlers::fingerprints::personas::{
    get_persona, FingerprintView, PersonaDetailResponse,
};
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult};
use fold_db::schema::types::field::HashRangeFilter;
use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::{MutationType, Query};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Minimum fingerprint count before a component is worth proposing.
/// Matches the design doc's N ≥ 3 "degree" gate loosely — we count
/// component size, not degree, because a clique of 3 is a better
/// signal than one hub with 3 leaves.
pub const MIN_FINGERPRINTS: usize = 3;

/// Minimum mention count before a component is worth proposing.
/// Matches the design doc's M ≥ 5 mention gate.
pub const MIN_MENTIONS: usize = 5;

/// Minimum edge weight for BFS expansion. Matches the design doc's
/// hard-coded 0.85 strong-match floor.
pub const MIN_EDGE_WEIGHT: f32 = 0.85;

// ── Response types ───────────────────────────────────────────────

/// One proposed cluster in the suggestions list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedPersonaView {
    /// Deterministic hash of the sorted fingerprint ids in this
    /// component. Stable across sweeps as long as the component is
    /// unchanged, so the frontend can use it as a dismissal key.
    pub suggested_id: String,
    pub suggested_name: String,
    pub fingerprint_ids: Vec<String>,
    pub fingerprint_count: usize,
    pub edge_count: usize,
    pub mention_count: usize,
    /// Up to 5 enriched fingerprints so the UI can render a
    /// "tom@acme.com · face embedding · …" preview without a
    /// second round trip.
    pub sample_fingerprints: Vec<FingerprintView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListSuggestedResponse {
    pub suggestions: Vec<SuggestedPersonaView>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AcceptSuggestedRequest {
    pub fingerprint_ids: Vec<String>,
    pub name: String,
    pub relationship: Option<String>,
}

// ── Handlers ─────────────────────────────────────────────────────

/// Run the dense-subgraph sweep and return every candidate cluster
/// that passes the MIN_FINGERPRINTS / MIN_MENTIONS gates and is not
/// already covered by an existing Persona.
pub async fn list_suggested_personas(node: Arc<FoldNode>) -> HandlerResult<ListSuggestedResponse> {
    let started = std::time::Instant::now();
    let processor = OperationProcessor::new(node.clone());

    // 1. Load every Fingerprint id. Phase 1 dogfood scales are small,
    //    so a full scan is fine. A future optimization can keep an
    //    in-memory index.
    let all_fingerprint_ids = load_all_fingerprint_ids(&processor).await?;

    // 2. Load every existing Persona's seed fingerprint ids so we can
    //    filter out components already covered.
    let existing_persona_seed_sets = load_persona_seed_sets(&processor).await?;

    // 3. BFS every fingerprint exactly once into components.
    let mut visited: HashSet<String> = HashSet::new();
    let mut components: Vec<Component> = Vec::new();
    for fp_id in &all_fingerprint_ids {
        if visited.contains(fp_id) {
            continue;
        }
        let component = walk_component(&processor, fp_id, &mut visited).await?;
        if component.fingerprint_ids.len() >= MIN_FINGERPRINTS {
            components.push(component);
        }
    }

    // 4. Build suggestion views — count mentions, fetch sample fps,
    //    apply the mentions gate, skip covered components.
    let mut suggestions: Vec<SuggestedPersonaView> = Vec::new();
    for component in components {
        if component_covered(&component, &existing_persona_seed_sets) {
            continue;
        }

        let mention_count = count_component_mentions(&processor, &component).await?;
        if mention_count < MIN_MENTIONS {
            continue;
        }

        let sample_ids: Vec<String> = component.fingerprint_ids.iter().take(5).cloned().collect();
        let sample_fingerprints =
            crate::handlers::fingerprints::personas::fetch_fingerprint_views_for_ids(
                &processor,
                &sample_ids,
            )
            .await?;

        let suggested_name = most_common_full_name(&sample_fingerprints)
            .unwrap_or_else(|| "Unnamed cluster".to_string());

        let mut sorted_fps = component.fingerprint_ids.clone();
        sorted_fps.sort();
        let suggested_id = hash_fingerprint_set(&sorted_fps);

        suggestions.push(SuggestedPersonaView {
            suggested_id,
            suggested_name,
            fingerprint_ids: sorted_fps,
            fingerprint_count: component.fingerprint_ids.len(),
            edge_count: component.edge_ids.len(),
            mention_count,
            sample_fingerprints,
        });
    }

    // Deterministic order: largest cluster first, then by name.
    suggestions.sort_by(|a, b| {
        b.fingerprint_count
            .cmp(&a.fingerprint_count)
            .then_with(|| a.suggested_name.cmp(&b.suggested_name))
    });

    log::info!(
        "fingerprints.handler: list_suggested_personas returned {} candidates in {:?}",
        suggestions.len(),
        started.elapsed()
    );

    Ok(ApiResponse::success(ListSuggestedResponse { suggestions }))
}

/// Promote a suggestion into a real Persona record and return its
/// freshly-resolved `PersonaDetailResponse`.
pub async fn accept_suggested_persona(
    node: Arc<FoldNode>,
    req: AcceptSuggestedRequest,
) -> HandlerResult<PersonaDetailResponse> {
    if req.fingerprint_ids.is_empty() {
        return Err(HandlerError::BadRequest(
            "fingerprint_ids must not be empty".to_string(),
        ));
    }
    let trimmed_name = req.name.trim();
    if trimmed_name.is_empty() {
        return Err(HandlerError::BadRequest(
            "name must not be empty".to_string(),
        ));
    }

    let persona_canonical = canonical_names::lookup(PERSONA).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            PERSONA, e
        ))
    })?;

    let processor = OperationProcessor::new(node.clone());
    let now = chrono::Utc::now().to_rfc3339();
    let persona_id = format!("ps_{}", uuid::Uuid::new_v4().simple());
    let relationship = req
        .relationship
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or("unknown")
        .to_string();

    let mut fields: HashMap<String, Value> = HashMap::new();
    fields.insert("id".to_string(), json!(persona_id));
    fields.insert("name".to_string(), json!(trimmed_name));
    fields.insert(
        "seed_fingerprint_ids".to_string(),
        json!(req.fingerprint_ids),
    );
    fields.insert("threshold".to_string(), json!(0.85_f32));
    fields.insert(
        "excluded_mention_ids".to_string(),
        json!(Vec::<String>::new()),
    );
    fields.insert("excluded_edge_ids".to_string(), json!(Vec::<String>::new()));
    fields.insert(
        "included_mention_ids".to_string(),
        json!(Vec::<String>::new()),
    );
    fields.insert("aliases".to_string(), json!(Vec::<String>::new()));
    fields.insert("relationship".to_string(), json!(relationship));
    fields.insert("trust_tier".to_string(), json!(0));
    fields.insert("identity_id".to_string(), Value::Null);
    fields.insert("user_confirmed".to_string(), json!(true));
    fields.insert("built_in".to_string(), json!(false));
    fields.insert("created_at".to_string(), json!(now));

    processor
        .execute_mutation(
            persona_canonical,
            fields,
            KeyValue::new(Some(persona_id.clone()), None),
            MutationType::Create,
        )
        .await
        .map_err(|e| {
            HandlerError::Internal(format!(
                "failed to create accepted persona '{}': {}",
                persona_id, e
            ))
        })?;

    log::info!(
        "fingerprints.handler: accepted suggested persona '{}' with {} seeds",
        persona_id,
        req.fingerprint_ids.len()
    );

    get_persona(node, persona_id).await
}

// ── Internals ────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct Component {
    fingerprint_ids: Vec<String>,
    edge_ids: HashSet<String>,
}

async fn load_all_fingerprint_ids(
    processor: &OperationProcessor,
) -> Result<Vec<String>, HandlerError> {
    let canonical = canonical_names::lookup(FINGERPRINT).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            FINGERPRINT, e
        ))
    })?;
    let query = Query {
        schema_name: canonical,
        fields: vec!["id".to_string()],
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let records = processor
        .execute_query_json(query)
        .await
        .map_err(|e| HandlerError::Internal(format!("fingerprint scan failed: {}", e)))?;
    let mut ids: Vec<String> = Vec::with_capacity(records.len());
    for record in records {
        if let Some(fields) = record.get("fields") {
            if let Some(id) = fields.get("id").and_then(|v| v.as_str()) {
                ids.push(id.to_string());
            }
        }
    }
    Ok(ids)
}

async fn load_persona_seed_sets(
    processor: &OperationProcessor,
) -> Result<Vec<HashSet<String>>, HandlerError> {
    let canonical = canonical_names::lookup(PERSONA).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            PERSONA, e
        ))
    })?;
    let query = Query {
        schema_name: canonical,
        fields: vec!["seed_fingerprint_ids".to_string()],
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let records = processor
        .execute_query_json(query)
        .await
        .map_err(|e| HandlerError::Internal(format!("persona scan failed: {}", e)))?;
    let mut sets: Vec<HashSet<String>> = Vec::new();
    for record in records {
        let Some(fields) = record.get("fields") else {
            continue;
        };
        let arr = fields
            .get("seed_fingerprint_ids")
            .and_then(|v| v.as_array());
        let set: HashSet<String> = match arr {
            Some(items) => items
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect(),
            None => HashSet::new(),
        };
        if !set.is_empty() {
            sets.push(set);
        }
    }
    Ok(sets)
}

async fn walk_component(
    processor: &OperationProcessor,
    start_fp: &str,
    global_visited: &mut HashSet<String>,
) -> Result<Component, HandlerError> {
    let mut component = Component::default();
    let mut queue: Vec<String> = vec![start_fp.to_string()];

    while let Some(fp_id) = queue.pop() {
        if global_visited.contains(&fp_id) {
            continue;
        }
        global_visited.insert(fp_id.clone());
        component.fingerprint_ids.push(fp_id.clone());

        let edge_ids = edge_ids_touching(processor, &fp_id).await?;
        for edge_id in edge_ids {
            if component.edge_ids.contains(&edge_id) {
                continue;
            }
            let Some((a, b, kind, weight)) = fetch_edge_endpoints(processor, &edge_id).await?
            else {
                continue;
            };
            if kind == edge_kind::USER_FORBIDDEN {
                continue;
            }
            if weight < MIN_EDGE_WEIGHT {
                continue;
            }
            component.edge_ids.insert(edge_id);
            let other = if a == fp_id { b } else { a };
            if !global_visited.contains(&other) {
                queue.push(other);
            }
        }
    }
    Ok(component)
}

async fn edge_ids_touching(
    processor: &OperationProcessor,
    fp_id: &str,
) -> Result<Vec<String>, HandlerError> {
    let canonical = canonical_names::lookup(EDGE_BY_FINGERPRINT).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            EDGE_BY_FINGERPRINT, e
        ))
    })?;
    let query = Query {
        schema_name: canonical,
        fields: vec!["edge_id".to_string()],
        filter: Some(HashRangeFilter::HashKey(fp_id.to_string())),
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let results = processor
        .execute_query_json(query)
        .await
        .map_err(|e| HandlerError::Internal(format!("edge-by-fp query failed: {}", e)))?;
    Ok(results
        .into_iter()
        .filter_map(|r| {
            r.get("fields")
                .and_then(|f| f.get("edge_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        })
        .collect())
}

async fn fetch_edge_endpoints(
    processor: &OperationProcessor,
    edge_id: &str,
) -> Result<Option<(String, String, String, f32)>, HandlerError> {
    let canonical = canonical_names::lookup(EDGE).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            EDGE, e
        ))
    })?;
    let query = Query {
        schema_name: canonical,
        fields: vec![
            "a".to_string(),
            "b".to_string(),
            "kind".to_string(),
            "weight".to_string(),
        ],
        filter: Some(HashRangeFilter::HashKey(edge_id.to_string())),
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let records = processor
        .execute_query_json(query)
        .await
        .map_err(|e| HandlerError::Internal(format!("edge fetch failed: {}", e)))?;
    let Some(record) = records.first() else {
        return Ok(None);
    };
    let Some(fields) = record.get("fields") else {
        return Ok(None);
    };
    let a = fields
        .get("a")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let b = fields
        .get("b")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let kind = fields
        .get("kind")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let weight = fields
        .get("weight")
        .and_then(|v| v.as_f64())
        .map(|f| f as f32)
        .unwrap_or(0.0);
    Ok(Some((a, b, kind, weight)))
}

async fn count_component_mentions(
    processor: &OperationProcessor,
    component: &Component,
) -> Result<usize, HandlerError> {
    let canonical = canonical_names::lookup(crate::fingerprints::schemas::MENTION_BY_FINGERPRINT)
        .map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            crate::fingerprints::schemas::MENTION_BY_FINGERPRINT,
            e
        ))
    })?;
    let mut seen: HashSet<String> = HashSet::new();
    for fp_id in &component.fingerprint_ids {
        let query = Query {
            schema_name: canonical.clone(),
            fields: vec!["mention_id".to_string()],
            filter: Some(HashRangeFilter::HashKey(fp_id.clone())),
            as_of: None,
            rehydrate_depth: None,
            sort_order: None,
            value_filters: None,
        };
        let results = processor
            .execute_query_json(query)
            .await
            .map_err(|e| HandlerError::Internal(format!("mention-by-fp query failed: {}", e)))?;
        for r in results {
            if let Some(id) = r
                .get("fields")
                .and_then(|f| f.get("mention_id"))
                .and_then(|v| v.as_str())
            {
                seen.insert(id.to_string());
            }
        }
    }
    Ok(seen.len())
}

fn component_covered(component: &Component, seed_sets: &[HashSet<String>]) -> bool {
    let component_fps: HashSet<&String> = component.fingerprint_ids.iter().collect();
    for seed_set in seed_sets {
        if !seed_set.is_empty() && seed_set.iter().all(|s| component_fps.contains(s)) {
            return true;
        }
    }
    false
}

fn hash_fingerprint_set(sorted_ids: &[String]) -> String {
    let mut hasher = Sha256::new();
    for id in sorted_ids {
        hasher.update(id.as_bytes());
        hasher.update(b"|");
    }
    format!("sg_{:x}", hasher.finalize())
}

/// Pick the most common `FullName`-kind display value across the
/// provided fingerprints. Returns `None` when no FullName fingerprint
/// is present — the caller falls back to a generic label.
fn most_common_full_name(fingerprints: &[FingerprintView]) -> Option<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for fp in fingerprints {
        if fp.kind.eq_ignore_ascii_case("full_name") && !fp.display_value.is_empty() {
            *counts.entry(fp.display_value.clone()).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(name, _)| name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(kind: &str, value: &str) -> FingerprintView {
        FingerprintView {
            id: format!("fp_{}_{}", kind, value),
            kind: kind.to_string(),
            display_value: value.to_string(),
            first_seen: None,
            last_seen: None,
        }
    }

    #[test]
    fn hash_fingerprint_set_is_deterministic() {
        let a = hash_fingerprint_set(&["fp_a".to_string(), "fp_b".to_string(), "fp_c".to_string()]);
        let b = hash_fingerprint_set(&["fp_a".to_string(), "fp_b".to_string(), "fp_c".to_string()]);
        assert_eq!(a, b);
        assert!(a.starts_with("sg_"));
    }

    #[test]
    fn hash_fingerprint_set_changes_with_membership() {
        let a = hash_fingerprint_set(&["fp_a".to_string(), "fp_b".to_string()]);
        let b = hash_fingerprint_set(&["fp_a".to_string(), "fp_c".to_string()]);
        assert_ne!(a, b);
    }

    #[test]
    fn most_common_full_name_picks_majority() {
        let fps = vec![
            fp("full_name", "Tom Tang"),
            fp("full_name", "Tom Tang"),
            fp("full_name", "Other"),
            fp("email", "tom@acme.com"),
        ];
        assert_eq!(most_common_full_name(&fps), Some("Tom Tang".to_string()));
    }

    #[test]
    fn most_common_full_name_returns_none_when_absent() {
        let fps = vec![fp("email", "tom@acme.com"), fp("phone", "+1")];
        assert_eq!(most_common_full_name(&fps), None);
    }

    #[test]
    fn component_covered_matches_all_seeds_in_component() {
        let component = Component {
            fingerprint_ids: vec!["fp_a".into(), "fp_b".into(), "fp_c".into()],
            edge_ids: HashSet::new(),
        };
        let seed_sets = vec![vec!["fp_a".to_string(), "fp_b".to_string()]
            .into_iter()
            .collect::<HashSet<_>>()];
        assert!(component_covered(&component, &seed_sets));
    }

    #[test]
    fn component_covered_rejects_partial_overlap() {
        let component = Component {
            fingerprint_ids: vec!["fp_a".into()],
            edge_ids: HashSet::new(),
        };
        let seed_sets = vec![vec!["fp_a".to_string(), "fp_b".to_string()]
            .into_iter()
            .collect::<HashSet<_>>()];
        // Seed set references fp_b, which is NOT in the component,
        // so the persona is not fully "covered" by the component.
        assert!(!component_covered(&component, &seed_sets));
    }
}
