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
//! returns the canonical name. Ambiguous descriptive labels (two schemas
//! sharing the same `descriptive_name`) become a 400 with the matching
//! canonical hashes listed.

use crate::fold_node::OperationProcessor;
use crate::handlers::response::{HandlerError, IntoTypedHandlerError};

/// Resolve `requested` to a canonical runtime schema name.
///
/// Resolution order:
/// 1. If a schema exists with `name == requested`, return `requested`.
/// 2. Otherwise, scan active schemas for `descriptive_name == requested`.
///    - exactly one match → return its canonical name
///    - zero matches → return `requested` unchanged so downstream emits
///      its own "not found" error (preserves existing behavior for
///      truly unknown names)
///    - 2+ matches → 400 with all candidate canonical hashes listed
pub async fn resolve_schema_name(
    processor: &OperationProcessor,
    requested: &str,
) -> Result<String, HandlerError> {
    if processor
        .get_schema(requested)
        .await
        .typed_handler_err()?
        .is_some()
    {
        return Ok(requested.to_string());
    }

    let schemas = processor.list_schemas().await.typed_handler_err()?;
    let matches: Vec<String> = schemas
        .into_iter()
        .filter(|s| s.schema.descriptive_name.as_deref() == Some(requested))
        .map(|s| s.schema.name)
        .collect();

    match matches.len() {
        0 => Ok(requested.to_string()),
        1 => Ok(matches.into_iter().next().unwrap()),
        _ => Err(HandlerError::BadRequest(format!(
            "ambiguous descriptive_name '{}': matches {:?}",
            requested, matches
        ))),
    }
}
