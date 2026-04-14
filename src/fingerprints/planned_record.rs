//! `PlannedRecord` — the schema-agnostic output type of every
//! extractor's planning layer.
//!
//! Extractors produce a `Vec<PlannedRecord>` describing every
//! Fingerprint / Mention / Edge / junction / support record that
//! should be persisted. The writer layer (`crate::fingerprints::writer`)
//! consumes that vector and materializes each record via
//! `OperationProcessor::execute_mutation`, resolving the schema's
//! runtime name through `canonical_names::lookup()`.
//!
//! Planning layers stay I/O-free and unit-testable; the writer handles
//! all mutation calls, canonical-name resolution, and error routing.

use serde_json::Value;
use std::collections::HashMap;

/// A single record an extractor plans to write.
///
/// **IMPORTANT: `descriptive_schema` is NOT a runtime schema name.**
/// It holds a descriptive_name constant from
/// `crate::fingerprints::schemas` (e.g. `"Fingerprint"`, `"Edge"`).
/// The writer layer must resolve it to the canonical runtime name
/// via `canonical_names::lookup(descriptive_schema)` before calling
/// `execute_mutation`. Planning layers deliberately do not know about
/// canonical names so plans remain deterministic and unit-testable
/// without a running schema service.
#[derive(Debug, Clone)]
pub struct PlannedRecord {
    /// Descriptive schema name (e.g. `"Fingerprint"`). Resolve to the
    /// runtime canonical name at write time via
    /// `canonical_names::lookup(descriptive_schema)`.
    pub descriptive_schema: &'static str,
    pub fields: HashMap<String, Value>,
    /// The value of the schema's declared hash_field (so the writer
    /// can pass the correct KeyValue to execute_mutation).
    pub hash_key: String,
    /// For HashRange schemas only — the value of the declared
    /// range_field.
    pub range_key: Option<String>,
}

impl PlannedRecord {
    /// Build a PlannedRecord for a Hash schema (hash_key only).
    pub fn hash(
        descriptive_schema: &'static str,
        hash_key: String,
        fields: HashMap<String, Value>,
    ) -> Self {
        Self {
            descriptive_schema,
            fields,
            hash_key,
            range_key: None,
        }
    }

    /// Build a PlannedRecord for a HashRange schema (hash_key + range_key).
    pub fn hash_range(
        descriptive_schema: &'static str,
        hash_key: String,
        range_key: String,
        fields: HashMap<String, Value>,
    ) -> Self {
        Self {
            descriptive_schema,
            fields,
            hash_key,
            range_key: Some(range_key),
        }
    }
}
