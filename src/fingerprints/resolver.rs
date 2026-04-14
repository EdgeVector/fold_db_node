//! Persona resolver — graph traversal over the fingerprint graph.
//!
//! A `Persona` is a saved query: "the connected component reachable
//! from `seed_fingerprint_ids`, traversing `Edge` records with
//! `weight >= threshold` and whose id is not in `excluded_edge_ids`,
//! plus mentions in `included_mention_ids`, minus mentions in
//! `excluded_mention_ids`". This module resolves that query into a
//! concrete `ResolveResult`.
//!
//! ## Algorithm
//!
//! Breadth-first traversal of the fingerprint graph starting from
//! the Persona's seeds. At each fingerprint, we look up the edges
//! incident to it via the `EdgeByFingerprint` junction schema (one
//! `HashKey` query per fingerprint), filter them by threshold /
//! excluded_edge_ids / UserForbidden, and enqueue any unvisited
//! endpoints.
//!
//! ```text
//!   start: visited = {}, queue = seed_fingerprint_ids
//!
//!   while queue not empty:
//!     fp = queue.pop()
//!     if fp in visited:       continue
//!     if fp not in Fingerprint schema:
//!         diagnostics.missing_seed_fingerprint_ids.push(fp)
//!         continue
//!     visited.insert(fp)
//!
//!     for edge in edges_touching(fp):
//!         if edge.id in excluded_edge_ids:    diag.excluded_edges++; skip
//!         if edge.kind == UserForbidden:      diag.forbidden_edges++; skip
//!         if edge.weight < threshold:         diag.below_threshold++; skip
//!         visited_edges.insert(edge.id)
//!         other = edge.other_endpoint(fp)
//!         if other not in visited:           queue.push(other)
//!
//!   collect mentions:
//!     for fp in visited:
//!         for mention_id in mentions_touching(fp):
//!             if mention_id in excluded_mention_ids:
//!                 diag.excluded_mentions++; skip
//!             visited_mentions.insert(mention_id)
//!     for mention_id in included_mention_ids:
//!         visited_mentions.insert(mention_id)
//! ```
//!
//! ## Diagnostics are first-class
//!
//! Per the Section 2 review decision (TODO-5), every `resolve()`
//! returns a `ResolveResult` that is either `Resolved { ... }` (clean)
//! or `ResolvedWithDiagnostics { ..., diagnostics }` (filter hits
//! were non-zero or seeds were missing). This surfaces the "why is
//! my Persona showing fewer mentions than I expected?" question as
//! explicit state rather than silent empty results.
//!
//! ## Fetching is lazy, not eager
//!
//! The resolver does one HashKey query per visited fingerprint (for
//! EdgeByFingerprint) and one HashKey query per visited fingerprint
//! (for MentionByFingerprint). For a Persona with N visited
//! fingerprints, that's 2N HashKey queries, plus M point-fetches
//! for the Edge records themselves (where M is the number of
//! unique non-excluded edges). This is acceptable for Phase 1
//! dogfood data; a future optimization could batch queries or
//! maintain a process-wide edge cache if the latency becomes a
//! problem.

use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::schema::types::field::HashRangeFilter;
use fold_db::schema::types::operations::Query;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;

use crate::fingerprints::canonical_names;
use crate::fingerprints::keys::edge_kind;
use crate::fingerprints::schemas::{
    EDGE, EDGE_BY_FINGERPRINT, FINGERPRINT, MENTION_BY_FINGERPRINT, PERSONA,
};
use crate::fold_node::{FoldNode, OperationProcessor};

/// Input to the resolver. Captures the mutable fields of a Persona
/// record that affect traversal. Constructed either by reading a
/// Persona record back from fold_db or built inline for tests.
#[derive(Debug, Clone)]
pub struct PersonaSpec {
    pub persona_id: String,
    pub seed_fingerprint_ids: Vec<String>,
    pub threshold: f32,
    pub excluded_edge_ids: HashSet<String>,
    pub excluded_mention_ids: HashSet<String>,
    pub included_mention_ids: HashSet<String>,
    pub identity_id: Option<String>,
}

/// An Edge record materialized for traversal. Small subset of the
/// full Edge schema — just what the resolver needs.
#[derive(Debug, Clone)]
pub struct ResolvedEdge {
    pub id: String,
    pub a: String,
    pub b: String,
    pub kind: String,
    pub weight: f32,
}

impl ResolvedEdge {
    /// Returns the endpoint of this edge that is NOT `fp`. Returns
    /// `None` if `fp` is neither endpoint — an inconsistency that
    /// should never happen because the EdgeByFingerprint junction is
    /// keyed by endpoint, so any edge we fetch via "edges touching
    /// fp_X" must have fp_X as one of its endpoints.
    pub fn other_endpoint(&self, fp: &str) -> Option<&str> {
        if self.a == fp {
            Some(&self.b)
        } else if self.b == fp {
            Some(&self.a)
        } else {
            None
        }
    }
}

/// Per-Persona traversal diagnostics. Zero values everywhere means
/// a clean resolve; any non-zero means something was filtered,
/// missing, or excluded, and the UI should surface it.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ResolveDiagnostics {
    /// Seed fingerprint IDs that do not exist in the Fingerprint
    /// schema. This is a hard inconsistency — a Persona should
    /// never reference a missing fingerprint — and the UI should
    /// show a loud warning.
    pub missing_seed_fingerprint_ids: Vec<String>,
    /// Count of edges skipped because they were in excluded_edge_ids.
    pub excluded_edge_count: usize,
    /// Count of edges skipped because kind == UserForbidden.
    pub forbidden_edge_count: usize,
    /// Count of edges skipped because weight < threshold.
    pub below_threshold_edge_count: usize,
    /// Count of mentions skipped because they were in excluded_mention_ids.
    pub excluded_mention_count: usize,
    /// Count of edge references in junctions whose Edge records do
    /// not exist (dangling junction rows — another hard inconsistency).
    pub dangling_edge_ids: Vec<String>,
}

impl ResolveDiagnostics {
    pub fn is_clean(&self) -> bool {
        self.missing_seed_fingerprint_ids.is_empty()
            && self.excluded_edge_count == 0
            && self.forbidden_edge_count == 0
            && self.below_threshold_edge_count == 0
            && self.excluded_mention_count == 0
            && self.dangling_edge_ids.is_empty()
    }
}

/// Output of a Persona resolve.
#[derive(Debug, Clone)]
pub enum ResolveResult {
    /// Clean resolve — no diagnostics to surface.
    Resolved {
        persona_id: String,
        fingerprint_ids: HashSet<String>,
        edge_ids: HashSet<String>,
        mention_ids: HashSet<String>,
        identity_id: Option<String>,
    },
    /// Resolve succeeded but something was missing, filtered, or
    /// excluded. Callers MUST surface the diagnostics to the user
    /// per the no-silent-failures invariant.
    ResolvedWithDiagnostics {
        persona_id: String,
        fingerprint_ids: HashSet<String>,
        edge_ids: HashSet<String>,
        mention_ids: HashSet<String>,
        identity_id: Option<String>,
        diagnostics: ResolveDiagnostics,
    },
}

impl ResolveResult {
    pub fn fingerprint_ids(&self) -> &HashSet<String> {
        match self {
            Self::Resolved {
                fingerprint_ids, ..
            }
            | Self::ResolvedWithDiagnostics {
                fingerprint_ids, ..
            } => fingerprint_ids,
        }
    }

    pub fn edge_ids(&self) -> &HashSet<String> {
        match self {
            Self::Resolved { edge_ids, .. } | Self::ResolvedWithDiagnostics { edge_ids, .. } => {
                edge_ids
            }
        }
    }

    pub fn mention_ids(&self) -> &HashSet<String> {
        match self {
            Self::Resolved { mention_ids, .. }
            | Self::ResolvedWithDiagnostics { mention_ids, .. } => mention_ids,
        }
    }

    pub fn diagnostics(&self) -> Option<&ResolveDiagnostics> {
        match self {
            Self::Resolved { .. } => None,
            Self::ResolvedWithDiagnostics { diagnostics, .. } => Some(diagnostics),
        }
    }

    pub fn is_clean(&self) -> bool {
        matches!(self, Self::Resolved { .. })
    }
}

/// The resolver. Holds a handle on the node so it can issue queries.
pub struct PersonaResolver {
    processor: OperationProcessor,
}

impl PersonaResolver {
    pub fn new(node: Arc<FoldNode>) -> Self {
        Self {
            processor: OperationProcessor::new(node),
        }
    }

    /// Resolve a Persona into a concrete cluster by BFS over the
    /// fingerprint graph. See module docs for the algorithm.
    pub async fn resolve(&self, spec: &PersonaSpec) -> FoldDbResult<ResolveResult> {
        let mut diagnostics = ResolveDiagnostics::default();
        let mut visited_fps: HashSet<String> = HashSet::new();
        let mut visited_edges: HashSet<String> = HashSet::new();

        // BFS queue. We enqueue fingerprint ids and dedupe via
        // visited_fps on pop.
        let mut queue: Vec<String> = spec.seed_fingerprint_ids.clone();

        while let Some(fp_id) = queue.pop() {
            if visited_fps.contains(&fp_id) {
                continue;
            }

            // Verify the fingerprint exists before we claim it. A
            // missing seed is a hard inconsistency — a Persona should
            // never reference a fingerprint that doesn't exist.
            if !self.fingerprint_exists(&fp_id).await? {
                if !diagnostics.missing_seed_fingerprint_ids.contains(&fp_id) {
                    diagnostics.missing_seed_fingerprint_ids.push(fp_id.clone());
                }
                continue;
            }

            visited_fps.insert(fp_id.clone());

            // Fetch edges incident to this fingerprint via the
            // EdgeByFingerprint junction. Returns edge_ids; we then
            // point-fetch each Edge record.
            let edge_ids = self.edge_ids_touching(&fp_id).await?;

            for edge_id in edge_ids {
                if visited_edges.contains(&edge_id) {
                    continue;
                }

                if spec.excluded_edge_ids.contains(&edge_id) {
                    diagnostics.excluded_edge_count += 1;
                    continue;
                }

                let edge = match self.fetch_edge(&edge_id).await? {
                    Some(e) => e,
                    None => {
                        // The junction pointed at an edge that doesn't
                        // exist. Record and continue.
                        if !diagnostics.dangling_edge_ids.contains(&edge_id) {
                            diagnostics.dangling_edge_ids.push(edge_id.clone());
                        }
                        continue;
                    }
                };

                if edge.kind == edge_kind::USER_FORBIDDEN {
                    diagnostics.forbidden_edge_count += 1;
                    continue;
                }

                if edge.weight < spec.threshold {
                    diagnostics.below_threshold_edge_count += 1;
                    continue;
                }

                visited_edges.insert(edge_id.clone());

                if let Some(other) = edge.other_endpoint(&fp_id) {
                    if !visited_fps.contains(other) {
                        queue.push(other.to_string());
                    }
                }
            }
        }

        // Collect mentions from every visited fingerprint, respecting
        // the Persona's excluded_mention_ids + included_mention_ids.
        let mut mention_ids: HashSet<String> = HashSet::new();
        for fp_id in &visited_fps {
            let ms = self.mention_ids_touching(fp_id).await?;
            for m in ms {
                if spec.excluded_mention_ids.contains(&m) {
                    diagnostics.excluded_mention_count += 1;
                    continue;
                }
                mention_ids.insert(m);
            }
        }
        for m in &spec.included_mention_ids {
            mention_ids.insert(m.clone());
        }

        if diagnostics.is_clean() {
            Ok(ResolveResult::Resolved {
                persona_id: spec.persona_id.clone(),
                fingerprint_ids: visited_fps,
                edge_ids: visited_edges,
                mention_ids,
                identity_id: spec.identity_id.clone(),
            })
        } else {
            Ok(ResolveResult::ResolvedWithDiagnostics {
                persona_id: spec.persona_id.clone(),
                fingerprint_ids: visited_fps,
                edge_ids: visited_edges,
                mention_ids,
                identity_id: spec.identity_id.clone(),
                diagnostics,
            })
        }
    }

    // ── fold_db lookup helpers ─────────────────────────────────

    async fn fingerprint_exists(&self, fp_id: &str) -> FoldDbResult<bool> {
        let canonical = canonical_names::lookup(FINGERPRINT)?;
        let query = Query {
            schema_name: canonical,
            fields: vec!["id".to_string()],
            filter: Some(HashRangeFilter::HashKey(fp_id.to_string())),
            as_of: None,
            rehydrate_depth: None,
            sort_order: None,
            value_filters: None,
        };
        let results = self.processor.execute_query_json(query).await?;
        Ok(!results.is_empty())
    }

    async fn edge_ids_touching(&self, fp_id: &str) -> FoldDbResult<Vec<String>> {
        let canonical = canonical_names::lookup(EDGE_BY_FINGERPRINT)?;
        let query = Query {
            schema_name: canonical,
            fields: vec!["edge_id".to_string()],
            filter: Some(HashRangeFilter::HashKey(fp_id.to_string())),
            as_of: None,
            rehydrate_depth: None,
            sort_order: None,
            value_filters: None,
        };
        let results = self.processor.execute_query_json(query).await?;
        Ok(results
            .into_iter()
            .filter_map(|r| extract_string_field(&r, "edge_id"))
            .collect())
    }

    async fn mention_ids_touching(&self, fp_id: &str) -> FoldDbResult<Vec<String>> {
        let canonical = canonical_names::lookup(MENTION_BY_FINGERPRINT)?;
        let query = Query {
            schema_name: canonical,
            fields: vec!["mention_id".to_string()],
            filter: Some(HashRangeFilter::HashKey(fp_id.to_string())),
            as_of: None,
            rehydrate_depth: None,
            sort_order: None,
            value_filters: None,
        };
        let results = self.processor.execute_query_json(query).await?;
        Ok(results
            .into_iter()
            .filter_map(|r| extract_string_field(&r, "mention_id"))
            .collect())
    }

    async fn fetch_edge(&self, edge_id: &str) -> FoldDbResult<Option<ResolvedEdge>> {
        let canonical = canonical_names::lookup(EDGE)?;
        let query = Query {
            schema_name: canonical,
            fields: vec![
                "id".to_string(),
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
        let results = self.processor.execute_query_json(query).await?;
        if results.is_empty() {
            return Ok(None);
        }
        let record = &results[0];
        let fields = record
            .get("fields")
            .ok_or_else(|| FoldDbError::Config("Edge record missing 'fields' envelope".into()))?;
        let id = extract_field_as_string(fields, "id")?;
        let a = extract_field_as_string(fields, "a")?;
        let b = extract_field_as_string(fields, "b")?;
        let kind = extract_field_as_string(fields, "kind")?;
        let weight = extract_field_as_f32(fields, "weight")?;
        Ok(Some(ResolvedEdge {
            id,
            a,
            b,
            kind,
            weight,
        }))
    }
}

// ── Record extraction helpers ────────────────────────────────────
//
// fold_db's execute_query_json returns an array of records, each
// wrapped in a `{"fields": {...}}` envelope. These helpers pull
// typed values out of that envelope and fail loudly on unexpected
// shapes rather than silently defaulting.

fn extract_string_field(record: &Value, field_name: &str) -> Option<String> {
    record
        .get("fields")
        .and_then(|f| f.get(field_name))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn extract_field_as_string(fields: &Value, field_name: &str) -> FoldDbResult<String> {
    fields
        .get(field_name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            FoldDbError::Config(format!(
                "resolver: field '{}' missing or non-string in record",
                field_name
            ))
        })
}

fn extract_field_as_f32(fields: &Value, field_name: &str) -> FoldDbResult<f32> {
    fields
        .get(field_name)
        .and_then(|v| v.as_f64())
        .map(|n| n as f32)
        .ok_or_else(|| {
            FoldDbError::Config(format!(
                "resolver: field '{}' missing or non-numeric in record",
                field_name
            ))
        })
}

// Quiet the unused-import warning for the PERSONA constant. It is
// referenced indirectly by callers that look up the canonical Persona
// name, but this module itself does not query the Persona schema —
// it takes a PersonaSpec constructed by the caller. Re-exporting the
// constant here keeps the import graph stable without needing a
// downstream `use` in every consumer.
#[allow(dead_code)]
const _PERSONA: &str = PERSONA;

#[cfg(test)]
mod tests {
    use super::*;

    // ── ResolvedEdge::other_endpoint ────────────────────────────

    #[test]
    fn other_endpoint_returns_b_when_given_a() {
        let e = ResolvedEdge {
            id: "eg_1".into(),
            a: "fp_A".into(),
            b: "fp_B".into(),
            kind: "StrongMatch".into(),
            weight: 0.95,
        };
        assert_eq!(e.other_endpoint("fp_A"), Some("fp_B"));
    }

    #[test]
    fn other_endpoint_returns_a_when_given_b() {
        let e = ResolvedEdge {
            id: "eg_1".into(),
            a: "fp_A".into(),
            b: "fp_B".into(),
            kind: "StrongMatch".into(),
            weight: 0.95,
        };
        assert_eq!(e.other_endpoint("fp_B"), Some("fp_A"));
    }

    #[test]
    fn other_endpoint_returns_none_when_given_unrelated_fp() {
        let e = ResolvedEdge {
            id: "eg_1".into(),
            a: "fp_A".into(),
            b: "fp_B".into(),
            kind: "StrongMatch".into(),
            weight: 0.95,
        };
        assert_eq!(e.other_endpoint("fp_C"), None);
    }

    // ── Diagnostics::is_clean ──────────────────────────────────

    #[test]
    fn diagnostics_empty_is_clean() {
        let d = ResolveDiagnostics::default();
        assert!(d.is_clean());
    }

    #[test]
    fn diagnostics_with_any_count_is_not_clean() {
        let d = ResolveDiagnostics {
            excluded_edge_count: 1,
            ..Default::default()
        };
        assert!(!d.is_clean());

        let d = ResolveDiagnostics {
            forbidden_edge_count: 1,
            ..Default::default()
        };
        assert!(!d.is_clean());

        let d = ResolveDiagnostics {
            below_threshold_edge_count: 1,
            ..Default::default()
        };
        assert!(!d.is_clean());

        let d = ResolveDiagnostics {
            excluded_mention_count: 1,
            ..Default::default()
        };
        assert!(!d.is_clean());
    }

    #[test]
    fn diagnostics_with_missing_seed_is_not_clean() {
        let d = ResolveDiagnostics {
            missing_seed_fingerprint_ids: vec!["fp_missing".into()],
            ..Default::default()
        };
        assert!(!d.is_clean());
    }

    #[test]
    fn diagnostics_with_dangling_edge_is_not_clean() {
        let d = ResolveDiagnostics {
            dangling_edge_ids: vec!["eg_dangling".into()],
            ..Default::default()
        };
        assert!(!d.is_clean());
    }

    // ── ResolveResult accessor consistency ─────────────────────

    #[test]
    fn resolve_result_accessors_return_same_data_for_both_variants() {
        let fps: HashSet<String> = vec!["fp_A".to_string()].into_iter().collect();
        let eds: HashSet<String> = vec!["eg_1".to_string()].into_iter().collect();
        let mns: HashSet<String> = vec!["mn_1".to_string()].into_iter().collect();
        let clean = ResolveResult::Resolved {
            persona_id: "ps_1".into(),
            fingerprint_ids: fps.clone(),
            edge_ids: eds.clone(),
            mention_ids: mns.clone(),
            identity_id: None,
        };
        let dirty = ResolveResult::ResolvedWithDiagnostics {
            persona_id: "ps_1".into(),
            fingerprint_ids: fps.clone(),
            edge_ids: eds.clone(),
            mention_ids: mns.clone(),
            identity_id: None,
            diagnostics: ResolveDiagnostics {
                excluded_edge_count: 1,
                ..Default::default()
            },
        };
        assert_eq!(clean.fingerprint_ids(), &fps);
        assert_eq!(dirty.fingerprint_ids(), &fps);
        assert_eq!(clean.edge_ids(), &eds);
        assert_eq!(dirty.edge_ids(), &eds);
        assert_eq!(clean.mention_ids(), &mns);
        assert_eq!(dirty.mention_ids(), &mns);
        assert!(clean.diagnostics().is_none());
        assert!(dirty.diagnostics().is_some());
        assert!(clean.is_clean());
        assert!(!dirty.is_clean());
    }

    // ── extract_string_field ────────────────────────────────────

    #[test]
    fn extract_string_field_pulls_from_envelope() {
        let rec = serde_json::json!({
            "fields": { "edge_id": "eg_42" }
        });
        assert_eq!(
            extract_string_field(&rec, "edge_id"),
            Some("eg_42".to_string())
        );
    }

    #[test]
    fn extract_string_field_returns_none_on_missing() {
        let rec = serde_json::json!({
            "fields": { "other": "eg_42" }
        });
        assert_eq!(extract_string_field(&rec, "edge_id"), None);
    }

    #[test]
    fn extract_field_as_string_fails_loudly_on_missing() {
        let fields = serde_json::json!({});
        let err = extract_field_as_string(&fields, "id").unwrap_err();
        assert!(format!("{}", err).contains("missing or non-string"));
    }

    #[test]
    fn extract_field_as_f32_handles_integer_and_float() {
        let fields = serde_json::json!({ "weight": 0.85 });
        let v = extract_field_as_f32(&fields, "weight").unwrap();
        assert!((v - 0.85).abs() < 1e-6);

        let fields = serde_json::json!({ "weight": 1 });
        let v = extract_field_as_f32(&fields, "weight").unwrap();
        assert!((v - 1.0).abs() < 1e-6);
    }
}
