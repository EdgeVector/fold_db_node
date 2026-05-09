//! Consolidate fragmented Apple-source records into the canonical schemas.
//!
//! PR #946 (`fix(apple-import): pin Apple Notes/Calendar/Contacts/Reminders to
//! canonical schemas`) stops new imports from fragmenting via LLM-classifier
//! non-determinism. But on installs that imported before that fix landed,
//! records already live in classifier-picked schemas (e.g. "Project Notes",
//! "Content Documents", "Documents" for 132 Apple notes split 1+40+62 = 103
//! — the dogfood symptom from 2026-05-09 where an LLM agent answering "How
//! many notes did I import?" returned "40" because it queried only the
//! "Content Documents" partition).
//!
//! This migration runs once per install: every non-canonical, non-blocked
//! schema is scanned for records whose `source` field equals one of the four
//! Apple sentinel values (`apple_notes`, `apple_calendar`, `apple_contacts`,
//! `apple_reminders`). Matching records are re-ingested into the canonical
//! `Apple Notes / Apple Calendar / Apple Contacts / Apple Reminders` schema
//! via the same forced-schema path PR #946 wired in
//! (`IngestionRequest::forced_schema_descriptive_name`). The forced-schema
//! synthesis path is byte-deterministic on the same `(descriptive_name,
//! field_set)`, so concurrent re-ingestions consolidate into a single schema
//! per source kind, and the canonical schema dedupes per-record via its
//! `content_hash` hash key.
//!
//! **Originals are not removed.** fold_db's mutation pipeline is append-only:
//! `MutationType::Delete` writes a sync-log marker and shows up in trigger
//! retention semantics, but it does NOT remove molecule entries on local
//! storage. Trying to scrub the fragmented records would therefore be a no-op
//! at the storage layer while creating false confidence at the API layer.
//! The migration's user-visible contract is "the canonical schema is the
//! source of truth post-migration, and queries against it return the full
//! set of imported records." The legacy fragmented schemas remain reachable
//! by direct schema name but contain a strict subset of the canonical's data.
//!
//! Idempotent: the marker file `<config_dir>/.apple_consolidation_complete`
//! is written on success. Re-ingestion is itself idempotent because the
//! forced-schema synthesis hashes records into the canonical schema's
//! `content_hash` hash field — running the migration a second time without
//! the marker would simply re-write the same atoms into the same molecule
//! slots. Operators can opt out per-boot via
//! `FOLDDB_SKIP_APPLE_NOTES_MIGRATION=1` to handle weird intermediate states
//! manually.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use fold_db::schema::types::Query;
use fold_db::schema::SchemaState;
use serde_json::Value;

use crate::fold_node::{FoldNode, OperationProcessor};
use crate::ingestion::ingestion_service::IngestionService;
use crate::ingestion::{create_progress_tracker, IngestionRequest, ProgressService};

/// `(record-source-sentinel, canonical-schema-descriptive-name)`. Source order
/// matches the order PR #946 wired in `apple_import.rs`. Keep in lockstep:
/// the LHS values are emitted by `apple_import::*::to_json_records` (e.g.
/// `notes.rs:38`) and the RHS values are passed to
/// `IngestionRequest::forced_schema_descriptive_name` from the same file.
const SOURCE_TO_CANONICAL: &[(&str, &str)] = &[
    ("apple_notes", "Apple Notes"),
    ("apple_calendar", "Apple Calendar"),
    ("apple_contacts", "Apple Contacts"),
    ("apple_reminders", "Apple Reminders"),
];

const MARKER_FILE_NAME: &str = ".apple_consolidation_complete";
const SKIP_ENV_VAR: &str = "FOLDDB_SKIP_APPLE_NOTES_MIGRATION";

#[derive(Debug, Default, PartialEq, Eq)]
pub struct AppleConsolidationOutcome {
    pub skipped: bool,
    /// Records re-ingested into a canonical schema. The `content_hash` hash
    /// key dedupes against any records that already landed in the canonical
    /// schema via a post-#946 import, so this is "writes attempted" — the
    /// schema's record count converges regardless.
    pub migrated: usize,
    /// Schemas that had a `source` field and were eligible for scanning.
    /// Useful for log forensics — a node with no Apple imports reports `0`,
    /// a node with 3 fragmented schemas reports `3`.
    pub schemas_scanned: usize,
}

/// Run the migration if the marker is absent and the opt-out is not set.
///
/// On success, writes the marker. On failure, leaves the marker absent so the
/// next boot retries. Re-ingestion is content-hash-keyed, so retries don't
/// duplicate.
pub async fn run_if_needed(
    node: &FoldNode,
    ingestion_service: &Arc<IngestionService>,
    config_dir: &Path,
) -> Result<AppleConsolidationOutcome, String> {
    if env_skip() {
        tracing::info!(
            target: "fold_node::migrations",
            "Apple consolidation skipped via {}",
            SKIP_ENV_VAR
        );
        return Ok(AppleConsolidationOutcome {
            skipped: true,
            ..Default::default()
        });
    }

    let marker = config_dir.join(MARKER_FILE_NAME);
    if marker.exists() {
        return Ok(AppleConsolidationOutcome {
            skipped: true,
            ..Default::default()
        });
    }

    let outcome = run(node, ingestion_service).await?;

    if let Some(parent) = marker.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Cannot create marker parent dir '{}': {e}",
                parent.display()
            )
        })?;
    }
    std::fs::write(&marker, "1")
        .map_err(|e| format!("Cannot write marker '{}': {e}", marker.display()))?;

    Ok(outcome)
}

/// Migration logic without the marker / opt-out wrapping. Exposed for tests
/// that want to drive the migration directly without managing a config dir.
pub async fn run(
    node: &FoldNode,
    ingestion_service: &Arc<IngestionService>,
) -> Result<AppleConsolidationOutcome, String> {
    let processor = OperationProcessor::new(Arc::new(node.clone()));
    let all_schemas = processor
        .list_schemas()
        .await
        .map_err(|e| format!("list_schemas failed: {e}"))?;

    let canonical_set: HashSet<&str> = SOURCE_TO_CANONICAL.iter().map(|(_, c)| *c).collect();

    let mut to_migrate: HashMap<&'static str, Vec<Value>> = HashMap::new();
    let mut schemas_scanned = 0usize;

    for sws in &all_schemas {
        if sws.state == SchemaState::Blocked {
            continue;
        }
        let s = &sws.schema;

        if let Some(d) = s.descriptive_name.as_deref() {
            if canonical_set.contains(d) {
                continue;
            }
        }
        // Records that originated from Apple-source ingestion always carry a
        // `source` field (see `apple_import::*::to_json_records`). If the
        // schema doesn't declare one, no record here can match — skip cheaply.
        if !s.runtime_fields.contains_key("source") {
            continue;
        }

        schemas_scanned += 1;

        let field_names: Vec<String> = s.runtime_fields.keys().cloned().collect();
        let query = Query::new(s.name.clone(), field_names);
        let results = match processor.execute_query_json(query).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    target: "fold_node::migrations",
                    schema = %s.name,
                    error = %e,
                    "execute_query_json failed during apple-consolidation scan; skipping schema"
                );
                continue;
            }
        };

        for record in results {
            let Some(source) = record
                .get("fields")
                .and_then(|f| f.get("source"))
                .and_then(|v| v.as_str())
            else {
                continue;
            };

            let Some(canonical) = SOURCE_TO_CANONICAL
                .iter()
                .find(|(src, _)| *src == source)
                .map(|(_, c)| *c)
            else {
                continue;
            };

            let Some(fields) = record.get("fields").cloned() else {
                continue;
            };

            // Strip molecule-internal `_rk` if present — it would survive the
            // round-trip into ingestion as a phantom field, but it's a query-
            // shape artifact, not record content.
            let cleaned_fields = strip_internal_keys(fields);

            to_migrate
                .entry(canonical)
                .or_default()
                .push(cleaned_fields);
        }
    }

    if to_migrate.is_empty() {
        return Ok(AppleConsolidationOutcome {
            schemas_scanned,
            ..Default::default()
        });
    }

    let progress_service = ProgressService::new(create_progress_tracker().await);
    let pub_key = node.get_node_public_key().to_string();

    let mut migrated = 0usize;

    for (canonical_name, records) in to_migrate {
        let count = records.len();
        let progress_id = format!(
            "apple-consolidation-{}",
            canonical_name.replace(' ', "-").to_lowercase()
        );

        let request = IngestionRequest {
            data: Value::Array(records),
            auto_execute: true,
            pub_key: pub_key.clone(),
            source_file_name: None,
            progress_id: Some(progress_id.clone()),
            file_hash: None,
            source_folder: None,
            image_descriptive_name: None,
            org_hash: None,
            image_bytes: None,
            forced_schema_descriptive_name: Some(canonical_name.to_string()),
        };

        let response = ingestion_service
            .process_json_with_node_and_progress(request, node, &progress_service, progress_id)
            .await
            .map_err(|e| format!("re-ingestion for '{canonical_name}': {e}"))?;

        if !response.success {
            return Err(format!(
                "re-ingestion for '{canonical_name}' failed: {:?}",
                response.errors
            ));
        }

        migrated += count;
    }

    tracing::info!(
        target: "fold_node::migrations",
        migrated = migrated,
        schemas_scanned = schemas_scanned,
        "Apple-source consolidation completed"
    );

    Ok(AppleConsolidationOutcome {
        skipped: false,
        migrated,
        schemas_scanned,
    })
}

/// Drop molecule-internal field names that the query layer surfaces but that
/// aren't part of the record's user-visible content. Today only `_rk` (range
/// key marker) qualifies; if more leak in, add them here.
fn strip_internal_keys(fields: Value) -> Value {
    match fields {
        Value::Object(mut map) => {
            map.remove("_rk");
            Value::Object(map)
        }
        other => other,
    }
}

fn env_skip() -> bool {
    std::env::var(SKIP_ENV_VAR)
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "True"))
        .unwrap_or(false)
}
