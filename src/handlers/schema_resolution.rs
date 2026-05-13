//! Resolve a user-supplied schema identifier to a canonical runtime name.
//!
//! The schema service canonicalizes every loaded schema by replacing its
//! `name` field with an identity hash (e.g. `76a65df7…`). The
//! human-readable label survives in `descriptive_name` and is what the UI,
//! humans, and the AI agent naturally use ("Apple Reminders"). The query
//! executor only knows canonical names, so a request that names a schema
//! by its descriptive label fails with "not found as schema or view".
//!
//! [`resolve_schema_name`] closes that gap: it accepts either form and
//! returns either a single canonical name or — when two or more active
//! schemas share the same `descriptive_name` on a user's machine — surfaces
//! the conflict to the caller so the HTTP layer can map it to a 409 with a
//! structured `{ambiguous_schemas: [...]}` body. Picking arbitrarily
//! would silently route the query at one of several schemas and the
//! caller would have no way to know.

use crate::fold_node::OperationProcessor;
use crate::handlers::response::{HandlerError, IntoTypedHandlerError};

/// Outcome of resolving a user-supplied schema identifier.
///
/// `Canonical` covers both the "input was already canonical" and "exactly
/// one descriptive_name match" cases — and also the "no match" pass-through,
/// where we return the input unchanged so the downstream query executor
/// emits its own "not found as schema or view" message rather than us
/// pre-empting it. `Ambiguous` carries every candidate canonical hash so
/// the caller can pin a future query to one of them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaResolution {
    Canonical(String),
    Ambiguous {
        input: String,
        candidates: Vec<String>,
    },
}

impl SchemaResolution {
    /// Collapse to a single canonical name, mapping `Ambiguous` to a 400
    /// [`HandlerError`]. Use this in non-HTTP code paths (Lambda, internal
    /// callers) that can't surface a 409 with a structured body — the HTTP
    /// route layer should match on the enum directly instead.
    pub fn into_canonical_or_err(self) -> Result<String, HandlerError> {
        match self {
            SchemaResolution::Canonical(name) => Ok(name),
            SchemaResolution::Ambiguous { input, candidates } => {
                Err(HandlerError::BadRequest(format!(
                    "ambiguous descriptive_name '{}': matches {:?}",
                    input, candidates
                )))
            }
        }
    }
}

/// Resolve `requested` to either a single canonical runtime schema name or
/// a conflict listing the matching canonical hashes.
///
/// Resolution order:
/// 1. If a schema exists with `name == requested`, return it as
///    [`SchemaResolution::Canonical`].
/// 2. Otherwise, scan active (non-Blocked) schemas for matching
///    `descriptive_name`:
///    - exactly one match → [`SchemaResolution::Canonical`] with its
///      canonical name
///    - zero matches → [`SchemaResolution::Canonical`] with `requested`
///      unchanged so downstream emits its own "not found" error
///      (preserves existing behavior for truly unknown names)
///    - 2+ matches → [`SchemaResolution::Ambiguous`] listing all candidate
///      canonical hashes so the caller can disambiguate by hash
pub async fn resolve_schema_name(
    processor: &OperationProcessor,
    requested: &str,
) -> Result<SchemaResolution, HandlerError> {
    let schemas = processor.list_schemas().await.typed_handler_err()?;

    // Exact canonical match wins — `requested` is already the schema's
    // runtime `name`, no substitution needed. Cannot delegate to
    // [`OperationProcessor::get_schema`] here: that method does its own
    // descriptive_name fallback internally and would return `Some` for the
    // descriptive label too, making the canonical check spuriously succeed
    // and the descriptive label pass through unchanged (the PR #975 bug
    // that left descriptive_name queries failing in prod).
    if schemas.iter().any(|s| s.schema.name == requested) {
        return Ok(SchemaResolution::Canonical(requested.to_string()));
    }

    let mut matches: Vec<String> = schemas
        .into_iter()
        .filter(|s| s.schema.descriptive_name.as_deref() == Some(requested))
        .map(|s| s.schema.name)
        .collect();

    match matches.len() {
        0 => Ok(SchemaResolution::Canonical(requested.to_string())),
        1 => Ok(SchemaResolution::Canonical(matches.pop().unwrap())),
        _ => {
            // Sort so the payload is stable across runs (HashMap ordering
            // in the upstream cache is non-deterministic) — both for
            // logging and for any caller that wants to diff two responses.
            matches.sort();
            Ok(SchemaResolution::Ambiguous {
                input: requested.to_string(),
                candidates: matches,
            })
        }
    }
}
