//! Writer layer — persists `PlannedRecord`s to fold_db via the
//! standard mutation path.
//!
//! Every extractor produces a `Vec<PlannedRecord>` describing the
//! records that should be written. This module consumes that vector
//! and executes each record as a mutation, resolving descriptive
//! schema names to canonical runtime names via the process-wide
//! `canonical_names` registry.
//!
//! ## Why a separate module
//!
//! Planning layers are pure and unit-testable. The writer is the
//! only place that talks to the node, the canonical-name registry,
//! and the mutation path. Keeping them separate means:
//!
//!   - Planners never depend on a running schema service
//!   - Writers are generic across extractors (face today, NER
//!     tomorrow, etc.) because they only touch the opaque
//!     PlannedRecord type
//!   - Integration tests can swap real writes for dry-runs by
//!     constructing a plan and inspecting it before calling
//!     `write_records`
//!
//! ## Failure posture
//!
//! Per the project's no-silent-failures invariant, this module
//! NEVER swallows a mutation error. Options on failure:
//!
//!   - Propagate the first error to the caller (current default)
//!   - Attempt to record an IngestionError for the failed record
//!     and continue (TODO, once the caller has a photo-level
//!     context to attach to the error)
//!
//! For now, the Phase 1 writer takes the first option. The caller
//! (typically the photo ingestion pipeline) decides how to surface
//! the failure — probably by writing its own IngestionError record
//! with the originating source_schema/source_key context that the
//! writer doesn't have.
//!
//! ## The canonical-name hop
//!
//! ```text
//!   PlannedRecord {                          Mutation {
//!     descriptive_schema: "Fingerprint",       schema_name:
//!     hash_key: "fp_abc",        │                "sh_<hash>",
//!     range_key: None,           │              ← canonical_names::lookup("Fingerprint")
//!     fields: {...},             │              fields: {...},
//!   }                            │              key_value: KeyValue::new(
//!                                ▼                Some("fp_abc"), None),
//!                                              mutation_type: Create,
//!                                            }
//! ```
//!
//! The writer never hard-codes a schema name in a mutation call.
//! Every call goes through `canonical_names::lookup()`. If the
//! registry is uninitialized or the descriptive name is unknown,
//! the write fails loudly — not silently bypassed.

use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::schema::types::key_value::KeyValue;
use fold_db::MutationType;
use std::sync::Arc;

use crate::fingerprints::canonical_names;
use crate::fingerprints::planned_record::PlannedRecord;
use crate::fold_node::{FoldNode, OperationProcessor};

/// Outcome of writing a single `PlannedRecord`.
#[derive(Debug, Clone)]
pub struct WrittenRecord {
    /// The descriptive_schema the record was planned under.
    pub descriptive_schema: &'static str,
    /// The canonical runtime name the record was actually written under.
    pub canonical_schema: String,
    /// The hash_key the record was written under.
    pub hash_key: String,
}

/// Outcome of writing a batch of records.
#[derive(Debug, Clone, Default)]
pub struct WriteOutcome {
    pub written: Vec<WrittenRecord>,
}

impl WriteOutcome {
    pub fn total(&self) -> usize {
        self.written.len()
    }

    pub fn count_for_descriptive(&self, descriptive: &str) -> usize {
        self.written
            .iter()
            .filter(|r| r.descriptive_schema == descriptive)
            .count()
    }
}

/// Persist a batch of `PlannedRecord`s to the node via
/// `OperationProcessor::execute_mutation`.
///
/// Every record's `descriptive_schema` is resolved to a canonical
/// runtime name via `canonical_names::lookup()`. If the registry has
/// not been populated (i.e. `register_phase_1_schemas()` has not
/// run), every lookup fails and the first one surfaces to the
/// caller — partial writes are never left in place.
///
/// Fails on the first error. The caller is responsible for deciding
/// whether to write an IngestionError record (with photo-level
/// context) or to retry.
pub async fn write_records(
    node: Arc<FoldNode>,
    records: &[PlannedRecord],
) -> FoldDbResult<WriteOutcome> {
    let processor = OperationProcessor::new(node);
    let mut outcome = WriteOutcome::default();
    let started = std::time::Instant::now();
    let input_count = records.len();

    tracing::info!(
        "fingerprints.writer: starting batch write ({} records)",
        input_count
    );

    for record in records {
        let canonical = canonical_names::lookup(record.descriptive_schema).map_err(|e| {
            FoldDbError::Config(format!(
                "writer: cannot resolve descriptive_schema '{}' — {}. \
                 Did register_phase_1_schemas() run at subsystem startup?",
                record.descriptive_schema, e
            ))
        })?;

        let key_value = KeyValue::new(Some(record.hash_key.clone()), record.range_key.clone());

        processor
            .execute_mutation(
                canonical.clone(),
                record.fields.clone(),
                key_value,
                MutationType::Create,
            )
            .await
            .map_err(|e| {
                FoldDbError::Config(format!(
                    "writer: failed to persist record on '{}' (canonical '{}'), \
                     hash_key='{}': {}",
                    record.descriptive_schema, canonical, record.hash_key, e
                ))
            })?;

        outcome.written.push(WrittenRecord {
            descriptive_schema: record.descriptive_schema,
            canonical_schema: canonical,
            hash_key: record.hash_key.clone(),
        });
    }

    let fp_count = outcome.count_for_descriptive("Fingerprint");
    let mn_count = outcome.count_for_descriptive("Mention");
    let eg_count = outcome.count_for_descriptive("Edge");
    let other_count = outcome
        .total()
        .saturating_sub(fp_count + mn_count + eg_count);
    tracing::info!(
        "fingerprints.writer: batch write complete in {:?}: \
         {} total (Fingerprint={}, Mention={}, Edge={}, other={})",
        started.elapsed(),
        outcome.total(),
        fp_count,
        mn_count,
        eg_count,
        other_count
    );

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_outcome_total_is_zero_on_empty() {
        let outcome = WriteOutcome::default();
        assert_eq!(outcome.total(), 0);
    }

    #[test]
    fn count_for_descriptive_filters_correctly() {
        let outcome = WriteOutcome {
            written: vec![
                WrittenRecord {
                    descriptive_schema: "Fingerprint",
                    canonical_schema: "sh_a".to_string(),
                    hash_key: "fp_1".to_string(),
                },
                WrittenRecord {
                    descriptive_schema: "Fingerprint",
                    canonical_schema: "sh_a".to_string(),
                    hash_key: "fp_2".to_string(),
                },
                WrittenRecord {
                    descriptive_schema: "Mention",
                    canonical_schema: "sh_b".to_string(),
                    hash_key: "mn_1".to_string(),
                },
            ],
        };
        assert_eq!(outcome.count_for_descriptive("Fingerprint"), 2);
        assert_eq!(outcome.count_for_descriptive("Mention"), 1);
        assert_eq!(outcome.count_for_descriptive("Edge"), 0);
    }
}
