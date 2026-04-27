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
use crate::fingerprints::schemas::{EDGE, FINGERPRINT, MENTION, MENTION_BY_FINGERPRINT, PERSONA};
use crate::fold_node::FoldNode;
use crate::handlers::response::{require_non_empty, ApiResponse, HandlerError, HandlerResult};
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
    /// Aliases (free-form alternate names) on the persona record. Surfaced
    /// in the list summary so the UI can filter against them client-side.
    pub aliases: Vec<String>,
    /// ISO-8601 timestamp the persona record was created. Used by the UI
    /// to sort by recency. `None` for older records that pre-date the field.
    pub created_at: Option<String>,
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
    /// Raw exclusion lists from the Persona record. The UI renders
    /// these as a collapsible "excluded items" panel so the user
    /// can undo any ✂ action they regret. Ordered as stored.
    pub excluded_edge_ids: Vec<String>,
    pub excluded_mention_ids: Vec<String>,
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
/// or "face embedding (512-dim) · Photos:IMG_1234" without a
/// second round trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FingerprintView {
    pub id: String,
    /// First 8 hex chars of the Fingerprint key (after the `fp_`
    /// prefix). Lets the UI distinguish otherwise-identical face
    /// embedding rows at a glance, which is friction point F2 from
    /// the Phase 1 walkthrough findings.
    pub short_id: String,
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
    /// A sample source record that references this fingerprint,
    /// formatted as `"<source_schema>:<source_key>"`. Populated from
    /// the most recent Mention via the MentionByFingerprint
    /// junction. `None` when no mentions touch this fingerprint
    /// (e.g. a Persona seed fingerprint the user added directly).
    ///
    /// The UI uses this to render face rows like "face_embedding
    /// (512 dims) · Photos:IMG_1234 · 2026-04-15" so five
    /// otherwise-identical face fingerprints become distinguishable
    /// by the photo each came from.
    pub sample_source: Option<String>,
    /// `source_field` on the sample Mention (e.g. "face").
    pub sample_source_field: Option<String>,
    /// `created_at` on the sample Mention — when this fingerprint
    /// was first observed on the sample source record.
    pub sample_mention_at: Option<String>,
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
            aliases: string_array_field(fields, "aliases"),
            created_at: string_field(fields, "created_at"),
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

    tracing::info!(
        "fingerprints.handler: list_personas resolved {} personas in {:?}",
        personas.len(),
        started.elapsed()
    );

    Ok(ApiResponse::success(ListPersonasResponse { personas }))
}

/// Delete a Persona by id. Refuses to delete a built-in persona
/// (the Me persona) because those are sourced from the IdentityCard
/// and should only disappear when the user resets their node
/// identity, not through a routine delete click.
///
/// Underlying Fingerprint / Mention / Edge records are NOT touched —
/// they are observed facts, and their persistence is the point per
/// the design doc §Operations ("Deleting a Persona removes the
/// Persona record. Underlying ... records are untouched"). A new
/// Persona seeded from the same fingerprints will resolve to the
/// same cluster.
pub async fn delete_persona(
    node: Arc<FoldNode>,
    persona_id: String,
) -> HandlerResult<serde_json::Value> {
    let persona_canonical = canonical_names::lookup(PERSONA).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            PERSONA, e
        ))
    })?;

    let processor = crate::fold_node::OperationProcessor::new(node.clone());

    // Fetch the persona to check built_in before deleting. A clean
    // NotFound here is better than a silent mutation error.
    let query = Query {
        schema_name: persona_canonical.clone(),
        fields: vec!["id".to_string(), "built_in".to_string()],
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
        .map_err(|e| HandlerError::Internal(format!("persona lookup failed: {}", e)))?;
    let record = records
        .first()
        .ok_or_else(|| HandlerError::NotFound(format!("persona '{}' not found", persona_id)))?;
    let fields = record.get("fields").ok_or_else(|| {
        HandlerError::Internal("persona record missing 'fields' envelope".to_string())
    })?;
    let built_in = fields
        .get("built_in")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if built_in {
        return Err(HandlerError::BadRequest(
            "cannot delete a built-in persona (Me); reset your node identity instead".to_string(),
        ));
    }

    let key_value = KeyValue::new(Some(persona_id.clone()), None);
    processor
        .execute_mutation(
            persona_canonical,
            HashMap::new(),
            key_value,
            MutationType::Delete,
        )
        .await
        .map_err(|e| {
            HandlerError::Internal(format!("failed to delete persona '{}': {}", persona_id, e))
        })?;

    tracing::info!(
        "fingerprints.handler: deleted persona '{}' (underlying fingerprints/edges/mentions untouched)",
        persona_id
    );

    Ok(ApiResponse::success(serde_json::json!({
        "deleted_persona_id": persona_id,
    })))
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

    tracing::info!(
        "fingerprints.handler: get_persona '{}' resolved in {:?} (fps={}, edges={}, mentions={}, clean={})",
        persona_id,
        started.elapsed(),
        fp_ids.len(),
        edge_ids.len(),
        mention_ids.len(),
        diagnostics.is_none()
    );

    let excluded_edge_ids_out = string_array_field(fields, "excluded_edge_ids");
    let excluded_mention_ids_out = string_array_field(fields, "excluded_mention_ids");

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
        excluded_edge_ids: excluded_edge_ids_out,
        excluded_mention_ids: excluded_mention_ids_out,
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

/// Public alias used by the Suggested Personas handler so it can
/// enrich its sample fingerprints without duplicating this loop.
/// Thin wrapper so the signature stays inside the personas module.
pub(crate) async fn fetch_fingerprint_views_for_ids(
    processor: &crate::fold_node::OperationProcessor,
    ids: &[String],
) -> Result<Vec<FingerprintView>, HandlerError> {
    fetch_fingerprint_views(processor, ids).await
}

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
            tracing::warn!(
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
        let sample = fetch_sample_mention_for_fingerprint(processor, id).await?;
        out.push(FingerprintView {
            id: id.clone(),
            short_id: short_fingerprint_id(id),
            kind,
            display_value,
            first_seen: string_field(fields, "first_seen"),
            last_seen: string_field(fields, "last_seen"),
            sample_source: sample
                .as_ref()
                .map(|m| format!("{}:{}", m.source_schema, m.source_key)),
            sample_source_field: sample.as_ref().and_then(|m| {
                if m.source_field.is_empty() {
                    None
                } else {
                    Some(m.source_field.clone())
                }
            }),
            sample_mention_at: sample.and_then(|m| m.created_at),
        });
    }
    Ok(out)
}

/// Join MentionByFingerprint → Mention to find a representative
/// source record for a given fingerprint. Returns the first
/// successfully-fetched Mention for the fingerprint, or `None` when
/// the fingerprint is not referenced by any Mention (e.g. a Persona
/// seed fingerprint that was hand-added via the Suggestions accept
/// endpoint).
///
/// "First" is the junction's natural order — range keys are mention
/// ids which are deterministic per source record, so the ordering
/// is stable across calls even though it isn't explicitly sorted.
/// A future enhancement could return the MOST RECENT mention by
/// `created_at`; the current implementation is cheaper and meets
/// the friction-point fix for F2/F8 (at-a-glance distinguishability
/// for otherwise-identical face rows).
async fn fetch_sample_mention_for_fingerprint(
    processor: &crate::fold_node::OperationProcessor,
    fingerprint_id: &str,
) -> Result<Option<MentionView>, HandlerError> {
    let junction_canonical = canonical_names::lookup(MENTION_BY_FINGERPRINT).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            MENTION_BY_FINGERPRINT, e
        ))
    })?;
    let junction_query = Query {
        schema_name: junction_canonical,
        fields: vec!["mention_id".to_string()],
        filter: Some(fold_db::schema::types::field::HashRangeFilter::HashKey(
            fingerprint_id.to_string(),
        )),
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let junction_records = processor
        .execute_query_json(junction_query)
        .await
        .map_err(|e| {
            HandlerError::Internal(format!(
                "sample-mention junction query failed for '{}': {}",
                fingerprint_id, e
            ))
        })?;

    let Some(first) = junction_records.first() else {
        return Ok(None);
    };
    let Some(mention_id) = first
        .get("fields")
        .and_then(|f| f.get("mention_id"))
        .and_then(|v| v.as_str())
    else {
        return Ok(None);
    };

    // Reuse the existing Mention fetcher with a single-id slice so we
    // get exactly the same view type the UI already consumes for the
    // mentions section. Empty result means a dangling junction row —
    // logged and treated as "no sample available".
    let mentions =
        fetch_mention_views(processor, std::slice::from_ref(&mention_id.to_string())).await?;
    Ok(mentions.into_iter().next())
}

/// Extract a short display id for the UI.
/// Takes everything after the first underscore (if any) and keeps
/// the first 8 chars, giving ~32 bits of visual distinguishability
/// across otherwise-identical face embedding rows. Falls back to
/// the raw id when it's shorter than 8 chars or has no underscore.
fn short_fingerprint_id(full: &str) -> String {
    let tail = full.split_once('_').map(|(_, rest)| rest).unwrap_or(full);
    tail.chars().take(8).collect()
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
            tracing::warn!(
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
            tracing::warn!(
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

/// Declarative patch applied by `apply_persona_patch`. Every field is
/// optional; callers populate only the ops they want to run. Ops are
/// applied in a stable order within a single read-modify-write cycle
/// so a caller can, e.g., set the threshold AND exclude an edge in
/// one round trip.
#[derive(Debug, Clone, Default)]
pub struct PersonaPatch {
    pub threshold: Option<f32>,
    pub add_excluded_edge_id: Option<String>,
    pub remove_excluded_edge_id: Option<String>,
    pub add_excluded_mention_id: Option<String>,
    pub remove_excluded_mention_id: Option<String>,
    /// Rename the persona. Ignored when `built_in == true` — the
    /// "Me" persona's name is always the node owner's display name
    /// and should only change via the IdentityCard flow.
    pub name: Option<String>,
    /// Change the relationship category. Accepts
    /// `self | family | colleague | friend | acquaintance | unknown`.
    pub relationship: Option<String>,
    /// Replace the aliases array wholesale. Pass an empty vec to
    /// clear all aliases; `None` leaves the existing list alone.
    pub aliases: Option<Vec<String>>,
    /// Set the user_confirmed flag. The auto-create sweep produces
    /// Personas with `user_confirmed: false`; the user flips this
    /// to `true` via the Confirm action in the Persona detail UI.
    /// There is no path to flip true → false; once confirmed, a
    /// Persona stays confirmed until deleted.
    pub user_confirmed: Option<bool>,
    /// Link this persona to a verified Identity record. The value is
    /// the `id_<pub_key>` key of an Identity record that already
    /// exists on this node (typically written by the import-identity-
    /// card handler after a successful signature verification). Sets
    /// `identity_id` on the persona to the SchemaRef shape the schema
    /// requires. Pass `None` to leave the link unchanged; pass
    /// `clear_identity_id: true` to null it out.
    pub link_identity_id: Option<String>,
    /// Clear `identity_id` on the persona (set to JSON null). Used
    /// when the user realizes a link was set on the wrong persona,
    /// or wants to downgrade a verified persona back to assumed.
    /// Rejected on built-in personas — the Me persona's identity_id
    /// is the self-Identity and must not be cleared.
    ///
    /// `link_identity_id` and `clear_identity_id = true` MUST NOT
    /// be used in the same patch. The handler rejects that
    /// combination with a 400 so the caller doesn't get a silent
    /// "clear wins" or "link wins" surprise.
    pub clear_identity_id: bool,
}

impl PersonaPatch {
    pub fn is_empty(&self) -> bool {
        self.threshold.is_none()
            && self.add_excluded_edge_id.is_none()
            && self.remove_excluded_edge_id.is_none()
            && self.add_excluded_mention_id.is_none()
            && self.remove_excluded_mention_id.is_none()
            && self.name.is_none()
            && self.relationship.is_none()
            && self.aliases.is_none()
            && self.user_confirmed.is_none()
            && self.link_identity_id.is_none()
            && !self.clear_identity_id
    }
}

/// Canonical relationship values accepted by the patch handler.
/// Keeping this list here, close to the validator, means the UI
/// can fetch the same set via an OpenAPI spec later without
/// having to duplicate it in the React client.
const ALLOWED_RELATIONSHIPS: &[&str] = &[
    "self",
    "family",
    "colleague",
    "friend",
    "acquaintance",
    "unknown",
];

/// Update the threshold field on an existing Persona record.
/// Retained as a thin compat wrapper over `apply_persona_patch`
/// so older callers keep working; new callers should use the patch
/// API directly.
pub async fn update_persona_threshold(
    node: Arc<FoldNode>,
    persona_id: String,
    new_threshold: f32,
) -> HandlerResult<PersonaDetailResponse> {
    apply_persona_patch(
        node,
        persona_id,
        PersonaPatch {
            threshold: Some(new_threshold),
            ..Default::default()
        },
    )
    .await
}

/// Apply a [`PersonaPatch`] to an existing Persona record and return
/// the freshly-resolved detail.
///
/// Read-modify-write:
/// 1. Fetch the existing record.
/// 2. Copy every field into a mutation payload.
/// 3. For each Some-op in the patch, mutate the relevant field in
///    place (threshold scalar, or array-add / array-remove).
/// 4. Execute a `MutationType::Update` preserving the content-keyed
///    primary key.
/// 5. Re-run `get_persona` so the caller sees the updated counts +
///    diagnostics in one round trip.
///
/// Edge/mention exclusion ops are idempotent: adding an id that's
/// already excluded is a no-op; removing an id that isn't excluded
/// is a no-op. Both still write the record so the caller gets a
/// fresh resolve back — useful for "refresh and re-render" flows.
pub async fn apply_persona_patch(
    node: Arc<FoldNode>,
    persona_id: String,
    patch: PersonaPatch,
) -> HandlerResult<PersonaDetailResponse> {
    if patch.is_empty() {
        return Err(HandlerError::BadRequest(
            "persona patch must contain at least one mutable field".to_string(),
        ));
    }
    if patch.link_identity_id.is_some() && patch.clear_identity_id {
        return Err(HandlerError::BadRequest(
            "link_identity_id and clear_identity_id are mutually exclusive — \
             pick one op per patch"
                .to_string(),
        ));
    }
    if let Some(threshold) = patch.threshold {
        if !threshold.is_finite() || !(0.0..=threshold_upper()).contains(&threshold) {
            return Err(HandlerError::BadRequest(format!(
                "threshold must be a finite number in [0.0, 1.0], got {}",
                threshold
            )));
        }
    }
    if let Some(ref name) = patch.name {
        require_non_empty(name, "name must not be empty")?;
    }
    if let Some(ref relationship) = patch.relationship {
        if !ALLOWED_RELATIONSHIPS.contains(&relationship.as_str()) {
            return Err(HandlerError::BadRequest(format!(
                "relationship must be one of {:?}, got '{}'",
                ALLOWED_RELATIONSHIPS, relationship
            )));
        }
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

    // 2. Clone every field into a mutation payload.
    let mut payload: HashMap<String, Value> = HashMap::new();
    let Value::Object(field_map) = fields else {
        return Err(HandlerError::Internal(
            "persona record 'fields' envelope is not a JSON object".to_string(),
        ));
    };
    for (k, v) in field_map.iter() {
        payload.insert(k.clone(), v.clone());
    }

    // 3. Apply patch ops in-place on the payload.
    if let Some(threshold) = patch.threshold {
        payload.insert(
            "threshold".to_string(),
            serde_json::Number::from_f64(threshold as f64)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
    }
    if let Some(edge_id) = &patch.add_excluded_edge_id {
        apply_array_add(&mut payload, "excluded_edge_ids", edge_id);
    }
    if let Some(edge_id) = &patch.remove_excluded_edge_id {
        apply_array_remove(&mut payload, "excluded_edge_ids", edge_id);
    }
    if let Some(mention_id) = &patch.add_excluded_mention_id {
        apply_array_add(&mut payload, "excluded_mention_ids", mention_id);
    }
    if let Some(mention_id) = &patch.remove_excluded_mention_id {
        apply_array_remove(&mut payload, "excluded_mention_ids", mention_id);
    }

    // Metadata fields. `built_in` personas reject name renames
    // because the Me persona's name is sourced from the IdentityCard
    // and would drift from the rest of the identity flow if edited
    // in place. Relationship and aliases on Me are user-editable —
    // per the design doc, Me is mutable except for its
    // seed_fingerprint_ids and built_in flag.
    if let Some(name) = &patch.name {
        let built_in_now = payload
            .get("built_in")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if built_in_now {
            return Err(HandlerError::BadRequest(
                "cannot rename a built-in persona (Me) — update the IdentityCard display name instead".to_string(),
            ));
        }
        payload.insert("name".to_string(), Value::String(name.trim().to_string()));
    }
    if let Some(relationship) = &patch.relationship {
        payload.insert(
            "relationship".to_string(),
            Value::String(relationship.clone()),
        );
    }
    if let Some(aliases) = &patch.aliases {
        payload.insert(
            "aliases".to_string(),
            Value::Array(aliases.iter().map(|s| Value::String(s.clone())).collect()),
        );
    }
    if let Some(user_confirmed) = patch.user_confirmed {
        // One-way transition: false→true is "Confirm this tentative
        // persona". We deliberately reject true→false to prevent an
        // accidental de-confirmation, which would be confusing state.
        let was_confirmed = payload
            .get("user_confirmed")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if was_confirmed && !user_confirmed {
            return Err(HandlerError::BadRequest(
                "cannot un-confirm a persona; delete it if you want to reject it".to_string(),
            ));
        }
        payload.insert("user_confirmed".to_string(), Value::Bool(user_confirmed));
    }
    if let Some(identity_id) = &patch.link_identity_id {
        // Persona schema declares `identity_id` as
        // `OneOf([SchemaRef("Identity"), Null])`. SchemaRef requires
        // the reference-object shape — see `me_persona_record` for
        // the canonical example. We write the DESCRIPTIVE schema
        // name `"Identity"` because that's what the SchemaRef variant
        // carries; the writer path will translate to the canonical
        // name downstream.
        payload.insert(
            "identity_id".to_string(),
            serde_json::json!({ "schema": "Identity", "key": identity_id }),
        );
    }
    if patch.clear_identity_id {
        // Me persona's identity_id is the self-Identity and MUST
        // NOT be cleared; doing so would leave the built-in persona
        // in a "claims to be Me but no signed identity" state that
        // breaks trust decisions. Reject early with a clear message.
        let built_in_now = payload
            .get("built_in")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if built_in_now {
            return Err(HandlerError::BadRequest(
                "cannot clear identity_id on a built-in persona (Me). \
                 Rotate the IdentityCard if you need to change it."
                    .to_string(),
            ));
        }
        payload.insert("identity_id".to_string(), Value::Null);
    }

    // 4. Execute Update. Keep the same content-addressed primary key.
    let key_value = KeyValue::new(Some(persona_id.clone()), None);
    processor
        .execute_mutation(persona_canonical, payload, key_value, MutationType::Update)
        .await
        .map_err(|e| {
            HandlerError::Internal(format!("failed to update persona '{}': {}", persona_id, e))
        })?;

    tracing::info!(
        "fingerprints.handler: applied patch to persona '{}' \
         (threshold={:?}, add_edge={:?}, rm_edge={:?}, add_mention={:?}, rm_mention={:?})",
        persona_id,
        patch.threshold,
        patch.add_excluded_edge_id,
        patch.remove_excluded_edge_id,
        patch.add_excluded_mention_id,
        patch.remove_excluded_mention_id,
    );

    // 4. Re-run the standard detail path so the caller gets the
    //    freshly-resolved counts + diagnostics.
    get_persona(node, persona_id).await
}

// ── Patch application helpers ────────────────────────────────────

fn threshold_upper() -> f32 {
    1.0
}

/// Append `value` to the string array at `field_name` on `payload`
/// unless it is already present. If the payload field is missing or
/// not an array, replace it with a single-element array — the array
/// shape on the schema is fixed so this is a safe recovery.
fn apply_array_add(payload: &mut HashMap<String, Value>, field_name: &str, value: &str) {
    let existing = payload
        .get(field_name)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let mut items: Vec<Value> = existing;
    let already_present = items
        .iter()
        .any(|v| v.as_str().map(|s| s == value).unwrap_or(false));
    if !already_present {
        items.push(Value::String(value.to_string()));
    }
    payload.insert(field_name.to_string(), Value::Array(items));
}

/// Remove every occurrence of `value` from the string array at
/// `field_name`. No-op when the field is missing, not an array, or
/// does not contain the value. Writes the (possibly unchanged) array
/// back so the mutation payload is self-consistent.
fn apply_array_remove(payload: &mut HashMap<String, Value>, field_name: &str, value: &str) {
    let existing = payload
        .get(field_name)
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let items: Vec<Value> = existing
        .into_iter()
        .filter(|v| v.as_str().map(|s| s != value).unwrap_or(true))
        .collect();
    payload.insert(field_name.to_string(), Value::Array(items));
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

/// Request body for [`merge_personas`]. The URL names the survivor
/// (`{id}` in `POST /personas/{id}/merge`); the body names the
/// persona whose seeds + aliases get folded in and then deleted.
#[derive(Debug, Clone, Deserialize)]
pub struct MergePersonasRequest {
    pub absorbed_persona_id: String,
}

/// Fold one Persona into another.
///
/// Product shape: the user has two Personas that they've realized
/// are the same person (e.g. the text pipeline created "Tom Tang"
/// from emails and the face pipeline auto-created "Unknown-23"
/// from shared photos, and a Mention connects them). Merging
/// unions their seed_fingerprint_ids, unions their exclusion
/// lists, appends the absorbed persona's name to the survivor's
/// aliases, and deletes the absorbed record. The resolver then
/// produces one cluster from the combined seeds.
///
/// Conflict policy:
///
/// - Both personas must be non-built-in. The Me persona never
///   merges — its identity_id is the self-Identity and folding
///   another persona into it would muddy that anchor.
/// - If both personas have a non-null identity_id AND those
///   identities differ, the merge is rejected. That configuration
///   means the user is trying to merge two *verified* identities,
///   which is a data-integrity bug, not a merge. The only sane
///   move is to unlink one first.
/// - If only one persona has identity_id, the survivor inherits it
///   (so merging a verified Persona into an unverified one does
///   not silently drop the cryptographic link).
/// - Thresholds are NOT averaged — the survivor's threshold wins.
///   Threshold is a UX knob, not a cluster property.
///
/// Returns the survivor's freshly-resolved detail.
pub async fn merge_personas(
    node: Arc<FoldNode>,
    survivor_id: String,
    request: MergePersonasRequest,
) -> HandlerResult<PersonaDetailResponse> {
    let absorbed_id = request.absorbed_persona_id;
    if survivor_id == absorbed_id {
        return Err(HandlerError::BadRequest(
            "cannot merge a persona into itself".to_string(),
        ));
    }

    let persona_canonical = canonical_names::lookup(PERSONA).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            PERSONA, e
        ))
    })?;
    let processor = crate::fold_node::OperationProcessor::new(node.clone());

    // Load both personas up front. Fail fast on NotFound so we
    // never half-apply a merge.
    let survivor = fetch_persona_fields(&processor, &persona_canonical, &survivor_id).await?;
    let absorbed = fetch_persona_fields(&processor, &persona_canonical, &absorbed_id).await?;

    if bool_field(&survivor, "built_in").unwrap_or(false)
        || bool_field(&absorbed, "built_in").unwrap_or(false)
    {
        return Err(HandlerError::BadRequest(
            "cannot merge a built-in persona (Me); rotate the IdentityCard instead".to_string(),
        ));
    }

    // Identity conflict check. identity_id is stored as a SchemaRef
    // object `{"schema": "Identity", "key": "id_..."}` OR JSON null.
    // We only care about the `key` field for this comparison.
    let survivor_identity_key = extract_identity_key(&survivor);
    let absorbed_identity_key = extract_identity_key(&absorbed);
    match (&survivor_identity_key, &absorbed_identity_key) {
        (Some(a), Some(b)) if a != b => {
            return Err(HandlerError::BadRequest(format!(
                "both personas are linked to different verified identities ({} vs {}); \
                 unlink one before merging",
                a, b
            )));
        }
        _ => {}
    }

    // Build the merged survivor payload.
    let mut payload: HashMap<String, Value> = HashMap::new();
    if let Value::Object(map) = &survivor {
        for (k, v) in map.iter() {
            payload.insert(k.clone(), v.clone());
        }
    } else {
        return Err(HandlerError::Internal(
            "survivor persona fields is not a JSON object".to_string(),
        ));
    }

    // Union the seed fingerprints — the survivor's cluster widens
    // to include every fingerprint that was driving the absorbed
    // persona's resolve.
    let merged_seeds = union_string_arrays(
        string_array_field(&survivor, "seed_fingerprint_ids"),
        string_array_field(&absorbed, "seed_fingerprint_ids"),
    );
    payload.insert(
        "seed_fingerprint_ids".to_string(),
        json_string_array(merged_seeds),
    );

    // Union exclusion lists so edges/mentions the user previously
    // hid on either persona stay hidden on the merged cluster.
    let merged_excl_edges = union_string_arrays(
        string_array_field(&survivor, "excluded_edge_ids"),
        string_array_field(&absorbed, "excluded_edge_ids"),
    );
    payload.insert(
        "excluded_edge_ids".to_string(),
        json_string_array(merged_excl_edges),
    );
    let merged_excl_mentions = union_string_arrays(
        string_array_field(&survivor, "excluded_mention_ids"),
        string_array_field(&absorbed, "excluded_mention_ids"),
    );
    payload.insert(
        "excluded_mention_ids".to_string(),
        json_string_array(merged_excl_mentions),
    );

    // Aliases: union + append the absorbed name when it differs
    // from any existing alias / the survivor's name. Users expect
    // "Tom Tang" merged into "Tom" to leave both names searchable.
    let mut aliases = string_array_field(&survivor, "aliases");
    for alias in string_array_field(&absorbed, "aliases") {
        if !aliases.contains(&alias) {
            aliases.push(alias);
        }
    }
    let survivor_name = string_field(&survivor, "name").unwrap_or_default();
    let absorbed_name = string_field(&absorbed, "name").unwrap_or_default();
    if !absorbed_name.is_empty()
        && absorbed_name != survivor_name
        && !aliases.contains(&absorbed_name)
    {
        aliases.push(absorbed_name);
    }
    payload.insert("aliases".to_string(), json_string_array(aliases));

    // Inherit identity_id if absorbed had one and survivor didn't.
    // The identity-conflict guard above already rejected the
    // differing-keys case; here we only need to promote.
    if survivor_identity_key.is_none() {
        if let Some(key) = absorbed_identity_key {
            payload.insert(
                "identity_id".to_string(),
                serde_json::json!({ "schema": "Identity", "key": key }),
            );
        }
    }

    // Write the merged survivor, then delete the absorbed record.
    // We write first because if the delete failed after an update,
    // the user still has a merged survivor and a stale duplicate —
    // recoverable. The other order leaves us with a missing
    // survivor + an orphan absorbed record which is worse.
    let survivor_key = KeyValue::new(Some(survivor_id.clone()), None);
    processor
        .execute_mutation(
            persona_canonical.clone(),
            payload,
            survivor_key,
            MutationType::Update,
        )
        .await
        .map_err(|e| {
            HandlerError::Internal(format!(
                "failed to write merged survivor persona '{}': {}",
                survivor_id, e
            ))
        })?;

    let absorbed_key = KeyValue::new(Some(absorbed_id.clone()), None);
    processor
        .execute_mutation(
            persona_canonical,
            HashMap::new(),
            absorbed_key,
            MutationType::Delete,
        )
        .await
        .map_err(|e| {
            HandlerError::Internal(format!(
                "survivor merged but failed to delete absorbed persona '{}': {}",
                absorbed_id, e
            ))
        })?;

    tracing::info!(
        "fingerprints.handler: merged persona '{}' into '{}'",
        absorbed_id,
        survivor_id
    );

    get_persona(node, survivor_id).await
}

async fn fetch_persona_fields(
    processor: &crate::fold_node::OperationProcessor,
    persona_canonical: &str,
    persona_id: &str,
) -> Result<Value, HandlerError> {
    let query = Query {
        schema_name: persona_canonical.to_string(),
        fields: persona_fields(),
        filter: Some(fold_db::schema::types::field::HashRangeFilter::HashKey(
            persona_id.to_string(),
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
    record
        .get("fields")
        .cloned()
        .ok_or_else(|| HandlerError::Internal("persona record missing 'fields' envelope".into()))
}

fn extract_identity_key(fields: &Value) -> Option<String> {
    fields
        .get("identity_id")
        .and_then(|v| v.as_object())
        .and_then(|obj| obj.get("key"))
        .and_then(|v| v.as_str())
        .map(String::from)
}

fn union_string_arrays(a: Vec<String>, b: Vec<String>) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::with_capacity(a.len() + b.len());
    for s in a.into_iter().chain(b) {
        if seen.insert(s.clone()) {
            out.push(s);
        }
    }
    out
}

fn json_string_array(values: Vec<String>) -> Value {
    Value::Array(values.into_iter().map(Value::String).collect())
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
    fn apply_array_add_appends_when_missing() {
        let mut payload: HashMap<String, Value> = HashMap::new();
        payload.insert("excluded_edge_ids".to_string(), json!(["eg_a"]));
        apply_array_add(&mut payload, "excluded_edge_ids", "eg_b");
        assert_eq!(
            payload.get("excluded_edge_ids").unwrap(),
            &json!(["eg_a", "eg_b"])
        );
    }

    #[test]
    fn apply_array_add_is_idempotent() {
        let mut payload: HashMap<String, Value> = HashMap::new();
        payload.insert("excluded_edge_ids".to_string(), json!(["eg_a"]));
        apply_array_add(&mut payload, "excluded_edge_ids", "eg_a");
        assert_eq!(payload.get("excluded_edge_ids").unwrap(), &json!(["eg_a"]));
    }

    #[test]
    fn apply_array_add_initializes_missing_field() {
        let mut payload: HashMap<String, Value> = HashMap::new();
        apply_array_add(&mut payload, "excluded_mention_ids", "mn_a");
        assert_eq!(
            payload.get("excluded_mention_ids").unwrap(),
            &json!(["mn_a"])
        );
    }

    #[test]
    fn apply_array_remove_strips_matching_value() {
        let mut payload: HashMap<String, Value> = HashMap::new();
        payload.insert(
            "excluded_mention_ids".to_string(),
            json!(["mn_a", "mn_b", "mn_c"]),
        );
        apply_array_remove(&mut payload, "excluded_mention_ids", "mn_b");
        assert_eq!(
            payload.get("excluded_mention_ids").unwrap(),
            &json!(["mn_a", "mn_c"])
        );
    }

    #[test]
    fn apply_array_remove_is_noop_when_missing() {
        let mut payload: HashMap<String, Value> = HashMap::new();
        payload.insert("excluded_edge_ids".to_string(), json!(["eg_a"]));
        apply_array_remove(&mut payload, "excluded_edge_ids", "eg_nope");
        assert_eq!(payload.get("excluded_edge_ids").unwrap(), &json!(["eg_a"]));
    }

    #[test]
    fn persona_patch_is_empty_when_all_none() {
        let patch = PersonaPatch::default();
        assert!(patch.is_empty());
    }

    #[test]
    fn persona_patch_not_empty_when_any_set() {
        let patch = PersonaPatch {
            threshold: Some(0.5),
            ..Default::default()
        };
        assert!(!patch.is_empty());
    }

    #[test]
    fn short_fingerprint_id_takes_8_chars_after_underscore() {
        assert_eq!(
            short_fingerprint_id("fp_cd0c9614db89af2080dd4e99d5fe021699ab43fa"),
            "cd0c9614"
        );
    }

    #[test]
    fn short_fingerprint_id_handles_missing_underscore() {
        assert_eq!(short_fingerprint_id("abcdef0123456789"), "abcdef01");
    }

    #[test]
    fn short_fingerprint_id_handles_short_input() {
        assert_eq!(short_fingerprint_id("fp_abc"), "abc");
    }

    #[test]
    fn short_fingerprint_id_handles_empty() {
        assert_eq!(short_fingerprint_id(""), "");
    }

    #[test]
    fn persona_patch_not_empty_when_only_name_set() {
        let patch = PersonaPatch {
            name: Some("Tom".to_string()),
            ..Default::default()
        };
        assert!(!patch.is_empty());
    }

    #[test]
    fn persona_patch_not_empty_when_only_relationship_set() {
        let patch = PersonaPatch {
            relationship: Some("friend".to_string()),
            ..Default::default()
        };
        assert!(!patch.is_empty());
    }

    #[test]
    fn persona_patch_not_empty_when_only_aliases_set() {
        let patch = PersonaPatch {
            aliases: Some(vec!["tommy".to_string()]),
            ..Default::default()
        };
        assert!(!patch.is_empty());
    }

    #[test]
    fn persona_patch_not_empty_when_only_clear_identity_id_set() {
        let patch = PersonaPatch {
            clear_identity_id: true,
            ..Default::default()
        };
        assert!(!patch.is_empty());
    }

    #[test]
    fn persona_patch_not_empty_when_only_link_identity_id_set() {
        let patch = PersonaPatch {
            link_identity_id: Some("id_abc".to_string()),
            ..Default::default()
        };
        assert!(!patch.is_empty());
    }

    #[test]
    fn allowed_relationships_includes_every_design_doc_value() {
        for r in &[
            "self",
            "family",
            "colleague",
            "friend",
            "acquaintance",
            "unknown",
        ] {
            assert!(
                ALLOWED_RELATIONSHIPS.contains(r),
                "expected '{}' in ALLOWED_RELATIONSHIPS",
                r
            );
        }
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

    // ── merge helpers ─────────────────────────────────────────

    #[test]
    fn union_string_arrays_preserves_first_occurrence_order() {
        let a = vec!["x".to_string(), "y".to_string()];
        let b = vec!["y".to_string(), "z".to_string()];
        assert_eq!(
            union_string_arrays(a, b),
            vec!["x".to_string(), "y".to_string(), "z".to_string()]
        );
    }

    #[test]
    fn union_string_arrays_empty_inputs() {
        assert!(union_string_arrays(Vec::new(), Vec::new()).is_empty());
    }

    #[test]
    fn extract_identity_key_returns_key_for_schemaref_object() {
        let fields = json!({
            "identity_id": { "schema": "Identity", "key": "id_abc" }
        });
        assert_eq!(extract_identity_key(&fields).as_deref(), Some("id_abc"));
    }

    #[test]
    fn extract_identity_key_returns_none_for_null() {
        let fields = json!({ "identity_id": null });
        assert!(extract_identity_key(&fields).is_none());
    }

    #[test]
    fn extract_identity_key_returns_none_for_missing() {
        let fields = json!({});
        assert!(extract_identity_key(&fields).is_none());
    }

    #[test]
    fn bool_field_is_none_on_missing_and_non_bool() {
        assert!(bool_field(&json!({}), "x").is_none());
        assert!(bool_field(&json!({ "x": "yes" }), "x").is_none());
        assert!(bool_field(&json!({ "x": 1 }), "x").is_none());
        assert_eq!(bool_field(&json!({ "x": true }), "x"), Some(true));
    }

    #[test]
    fn string_array_field_returns_empty_on_missing() {
        assert!(string_array_field(&json!({}), "seeds").is_empty());
    }

    #[test]
    fn string_array_field_skips_non_strings_entries() {
        let fields = json!({ "seeds": ["fp_a", 9, null, "fp_b"] });
        assert_eq!(
            string_array_field(&fields, "seeds"),
            vec!["fp_a".to_string(), "fp_b".to_string()]
        );
    }
}
