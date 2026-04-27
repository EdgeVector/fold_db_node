//! IngestionError view-layer handlers for the Failed records panel.
//!
//! ## Endpoints this backs
//!
//! - `GET /api/fingerprints/ingestion-errors` →
//!   `ListIngestionErrorsResponse` — every IngestionError row flattened
//!   into a view struct. Supports an optional `?include_resolved=1`
//!   query param (resolved rows are hidden by default so the panel
//!   shows only actionable failures).
//!
//! - `PATCH /api/fingerprints/ingestion-errors/{id}` → marks a single
//!   row as `resolved: true` (used by the Dismiss button in the UI).
//!   Retry will eventually re-run the extractor — for Phase 1 the
//!   Retry action calls this same endpoint so dismissing and
//!   "retrying" are both idempotent clears from the panel's POV.
//!
//! Canonical-name lookup follows the same pattern as
//! `handlers::fingerprints::personas`: we take the descriptive name
//! (`INGESTION_ERROR`) and resolve it to the runtime schema name via
//! `canonical_names::lookup` before every query, so HTTP callers never
//! need to know the runtime hash.

use crate::fingerprints::canonical_names;
use crate::fingerprints::schemas::INGESTION_ERROR;
use crate::fold_node::FoldNode;
use crate::handlers::response::{ApiResponse, HandlerError, HandlerResult};
use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::{MutationType, Query};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

// ── Response types ───────────────────────────────────────────────

/// Flattened IngestionError row for the Failed records panel. Matches
/// the schema in `schema_service_core::builtin_schemas` 1:1 minus
/// the `last_retry_at` optional timestamp which the Phase 1 UI does
/// not yet render.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestionErrorView {
    pub id: String,
    pub source_schema: String,
    pub source_key: String,
    pub extractor: String,
    pub error_class: String,
    pub error_msg: String,
    pub retry_count: i64,
    pub resolved: bool,
    pub created_at: String,
    pub last_retry_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListIngestionErrorsResponse {
    pub errors: Vec<IngestionErrorView>,
}

// ── Handlers ─────────────────────────────────────────────────────

/// List every IngestionError row. `include_resolved=false` (the
/// default) hides any row where `resolved == true`; the Failed panel
/// is primarily a to-do list of live failures.
///
/// Rows are sorted newest-first by `created_at` so the most recent
/// failures are at the top of the panel. Phase 1 dogfood volumes are
/// small enough that sorting in-memory is fine.
pub async fn list_ingestion_errors(
    node: Arc<FoldNode>,
    include_resolved: bool,
) -> HandlerResult<ListIngestionErrorsResponse> {
    let started = std::time::Instant::now();
    let canonical = canonical_names::lookup(INGESTION_ERROR).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            INGESTION_ERROR, e
        ))
    })?;

    let processor = crate::fold_node::OperationProcessor::new(node.clone());

    let query = Query {
        schema_name: canonical,
        fields: ingestion_error_fields(),
        filter: None,
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };

    let records = processor
        .execute_query_json(query)
        .await
        .map_err(|e| HandlerError::Internal(format!("ingestion-error query failed: {}", e)))?;

    let mut errors: Vec<IngestionErrorView> = Vec::with_capacity(records.len());
    for record in records {
        let Some(fields) = record.get("fields") else {
            continue;
        };
        let view = ingestion_error_view_from_fields(fields)?;
        if !include_resolved && view.resolved {
            continue;
        }
        errors.push(view);
    }

    // Newest first. Empty strings sort to the bottom naturally.
    errors.sort_by(|a, b| b.created_at.cmp(&a.created_at));

    tracing::info!(
        "fingerprints.handler: list_ingestion_errors returned {} rows in {:?} (include_resolved={})",
        errors.len(),
        started.elapsed(),
        include_resolved,
    );

    Ok(ApiResponse::success(ListIngestionErrorsResponse { errors }))
}

/// Set the `resolved` flag on a single IngestionError row.
///
/// Callers pass `resolved: true` to dismiss and `resolved: false`
/// to restore a previously-dismissed row back into the active
/// Failed panel. The toggle is idempotent — setting `true` on an
/// already-resolved row is a no-op, and vice versa.
///
/// Read-modify-write so every field is carried through untouched
/// except `resolved` (and `last_retry_at` when we later wire a real
/// retry). Mirrors `personas::apply_persona_patch`.
pub async fn resolve_ingestion_error(
    node: Arc<FoldNode>,
    error_id: String,
    resolved: bool,
) -> HandlerResult<IngestionErrorView> {
    let canonical = canonical_names::lookup(INGESTION_ERROR).map_err(|e| {
        HandlerError::Internal(format!(
            "fingerprints: canonical_names not initialized for '{}': {}",
            INGESTION_ERROR, e
        ))
    })?;

    let processor = crate::fold_node::OperationProcessor::new(node.clone());

    // 1. Fetch the existing row so we can preserve every field.
    let query = Query {
        schema_name: canonical.clone(),
        fields: ingestion_error_fields(),
        filter: Some(fold_db::schema::types::field::HashRangeFilter::HashKey(
            error_id.clone(),
        )),
        as_of: None,
        rehydrate_depth: None,
        sort_order: None,
        value_filters: None,
    };
    let records = processor
        .execute_query_json(query)
        .await
        .map_err(|e| HandlerError::Internal(format!("ingestion-error query failed: {}", e)))?;
    let record = records.first().ok_or_else(|| {
        HandlerError::NotFound(format!("ingestion error '{}' not found", error_id))
    })?;
    let fields = record.get("fields").ok_or_else(|| {
        HandlerError::Internal("ingestion-error record missing 'fields' envelope".to_string())
    })?;
    let Value::Object(field_map) = fields else {
        return Err(HandlerError::Internal(
            "ingestion-error 'fields' envelope is not a JSON object".to_string(),
        ));
    };

    // 2. Copy every field, flip `resolved`.
    let mut payload: HashMap<String, Value> = HashMap::new();
    for (k, v) in field_map.iter() {
        payload.insert(k.clone(), v.clone());
    }
    payload.insert("resolved".to_string(), Value::Bool(resolved));

    // 3. Execute Update keeping the same primary key.
    let key_value = KeyValue::new(Some(error_id.clone()), None);
    processor
        .execute_mutation(canonical, payload, key_value, MutationType::Update)
        .await
        .map_err(|e| {
            HandlerError::Internal(format!(
                "failed to update ingestion error '{}': {}",
                error_id, e
            ))
        })?;

    tracing::info!(
        "fingerprints.handler: ingestion error '{}' resolved={}",
        error_id,
        resolved
    );

    // 4. Return the updated row so the UI can swap it in place.
    let mut view = ingestion_error_view_from_fields(fields)?;
    view.resolved = resolved;
    Ok(ApiResponse::success(view))
}

// ── Field helpers ────────────────────────────────────────────────

fn ingestion_error_fields() -> Vec<String> {
    vec![
        "id".to_string(),
        "source_schema".to_string(),
        "source_key".to_string(),
        "extractor".to_string(),
        "error_class".to_string(),
        "error_msg".to_string(),
        "retry_count".to_string(),
        "resolved".to_string(),
        "created_at".to_string(),
        "last_retry_at".to_string(),
    ]
}

fn ingestion_error_view_from_fields(fields: &Value) -> Result<IngestionErrorView, HandlerError> {
    let id = string_field(fields, "id").ok_or_else(|| {
        HandlerError::Internal("ingestion-error record missing 'id' field".to_string())
    })?;
    Ok(IngestionErrorView {
        id,
        source_schema: string_field(fields, "source_schema").unwrap_or_default(),
        source_key: string_field(fields, "source_key").unwrap_or_default(),
        extractor: string_field(fields, "extractor").unwrap_or_default(),
        error_class: string_field(fields, "error_class").unwrap_or_default(),
        error_msg: string_field(fields, "error_msg").unwrap_or_default(),
        retry_count: fields
            .get("retry_count")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
        resolved: fields
            .get("resolved")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        created_at: string_field(fields, "created_at").unwrap_or_default(),
        last_retry_at: fields.get("last_retry_at").and_then(|v| {
            if v.is_null() {
                None
            } else {
                v.as_str().map(|s| s.to_string())
            }
        }),
    })
}

fn string_field(fields: &Value, name: &str) -> Option<String> {
    fields
        .get(name)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn view_extracts_every_field() {
        let fields = json!({
            "id": "ie_abc",
            "source_schema": "Photos",
            "source_key": "IMG_1234",
            "extractor": "face_detect",
            "error_class": "FaceDetectorTimeout",
            "error_msg": "timed out after 30s",
            "retry_count": 2,
            "resolved": false,
            "created_at": "2026-04-15T10:00:00Z",
            "last_retry_at": "2026-04-15T10:05:00Z",
        });
        let view = ingestion_error_view_from_fields(&fields).unwrap();
        assert_eq!(view.id, "ie_abc");
        assert_eq!(view.source_schema, "Photos");
        assert_eq!(view.source_key, "IMG_1234");
        assert_eq!(view.extractor, "face_detect");
        assert_eq!(view.error_class, "FaceDetectorTimeout");
        assert_eq!(view.error_msg, "timed out after 30s");
        assert_eq!(view.retry_count, 2);
        assert!(!view.resolved);
        assert_eq!(view.created_at, "2026-04-15T10:00:00Z");
        assert_eq!(view.last_retry_at.as_deref(), Some("2026-04-15T10:05:00Z"));
    }

    #[test]
    fn view_treats_null_last_retry_at_as_none() {
        let fields = json!({
            "id": "ie_x",
            "source_schema": "Notes",
            "source_key": "n_1",
            "extractor": "ner_llm",
            "error_class": "",
            "error_msg": "",
            "retry_count": 0,
            "resolved": false,
            "created_at": "",
            "last_retry_at": Value::Null,
        });
        let view = ingestion_error_view_from_fields(&fields).unwrap();
        assert_eq!(view.last_retry_at, None);
    }

    #[test]
    fn missing_id_errors() {
        let fields = json!({ "source_schema": "x" });
        let err = ingestion_error_view_from_fields(&fields).unwrap_err();
        match err {
            HandlerError::Internal(msg) => assert!(msg.contains("missing 'id'")),
            _ => panic!("expected Internal error"),
        }
    }
}
