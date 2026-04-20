//! Local semantic memory store.
//!
//! A `memory` is a piece of content (the `body` field) with lightweight
//! metadata (type, status, tags, source, timestamp). Memories are ordinary
//! fold_db molecules on a `Memory` schema. The schema is proposed to the
//! schema service on first registration; the service canonicalizes, and
//! the node caches the canonical name for subsequent mutations and queries.
//!
//! ## Why a schema, not a bespoke store
//!
//! Using fold_db as-is means memories get:
//!   * Auto-embedding by `NativeIndexManager::index_record` on every write
//!   * Cross-device sync via the unified sync path
//!   * Schema expansion when new fields appear (e.g., `claim`, `evidence`,
//!     `confidence` on later consolidations)
//!   * Content-addressable molecule identity
//!
//! We don't add a Model Registry or an `embed_body` transform — the native
//! index already embeds every text fragment of every field automatically.
//! See `docs/design/memory_agent.md`.
//!
//! ## Fields
//!
//! | Field          | Type            | Purpose |
//! |----------------|-----------------|---------|
//! | `id`           | String          | Primary key `mem_<uuid>` |
//! | `body`         | String          | Memory content as free text |
//! | `kind`         | String          | Semantic category: feedback, project, reference, decision, consolidation_proposal, approved_consolidation |
//! | `status`       | String          | Lifecycle: `live`, `pending`, `approved`, `rejected`, `edited` |
//! | `tags`         | Array<String>   | Free-form tags |
//! | `source`       | String          | Origin: conversation-id, file path, `manual`, etc. |
//! | `created_at`   | String          | ISO-8601 timestamp |
//! | `derived_from` | Array<String>   | Source memory IDs — populated on consolidation proposals, empty for raw memories |
//!
//! `kind` is used instead of `type` because `type` is a reserved word in
//! some fold_db codepaths (serde tag collisions have bitten us elsewhere).

use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::schema::types::data_classification::DataClassification;
use fold_db::schema::types::field_value_type::FieldValueType;
use fold_db::schema::types::key_config::KeyConfig;
use fold_db::schema::types::schema::DeclarativeSchemaType as SchemaType;
use fold_db::schema::types::Schema;
use fold_db::schema::SchemaState;

use crate::fold_node::FoldNode;

/// Consolidation TransformViews over memories. See `consolidation.rs` for
/// the architecture — gated on `transform-wasm` because it depends on the
/// runtime WASM compiler.
#[cfg(feature = "transform-wasm")]
pub mod consolidation;

/// Descriptive name the node uses to look up the memory schema. The schema
/// service canonicalizes every schema to its identity_hash on insert, so the
/// runtime `name` field on the schema will be a hash — call
/// `register_memory_schema` to obtain the canonical runtime name.
pub const MEMORY_DESCRIPTIVE_NAME: &str = "Memory";

/// Field names on the memory schema. Consumers (handler, agent) reference
/// these by constant so a rename on either side fails the type checker.
pub mod fields {
    pub const ID: &str = "id";
    pub const BODY: &str = "body";
    pub const KIND: &str = "kind";
    pub const STATUS: &str = "status";
    pub const TAGS: &str = "tags";
    pub const SOURCE: &str = "source";
    pub const CREATED_AT: &str = "created_at";
    pub const DERIVED_FROM: &str = "derived_from";
}

/// Build the memory schema as a local Rust value. The schema service will
/// canonicalize this on `register_memory_schema` — the returned canonical
/// is what mutations and queries actually target.
pub fn memory_schema() -> Schema {
    let fields: Vec<(&'static str, FieldValueType, &'static str)> = vec![
        (
            fields::ID,
            FieldValueType::String,
            "Stable primary key mem_<uuid> for the memory molecule",
        ),
        (
            fields::BODY,
            FieldValueType::String,
            "Memory content as free text. Auto-embedded by NativeIndexManager on write.",
        ),
        (
            fields::KIND,
            FieldValueType::String,
            "Semantic category: feedback, project, reference, decision, consolidation_proposal, approved_consolidation",
        ),
        (
            fields::STATUS,
            FieldValueType::String,
            "Lifecycle status: live (raw memories), pending (proposals awaiting review), approved, rejected, edited",
        ),
        (
            fields::TAGS,
            FieldValueType::Array(Box::new(FieldValueType::String)),
            "Free-form tags for grouping and filtering",
        ),
        (
            fields::SOURCE,
            FieldValueType::String,
            "Where the memory came from: conversation-id, file path, 'manual', etc.",
        ),
        (
            fields::CREATED_AT,
            FieldValueType::String,
            "ISO-8601 timestamp when the memory was written",
        ),
        (
            fields::DERIVED_FROM,
            FieldValueType::Array(Box::new(FieldValueType::String)),
            "Source memory IDs for consolidation proposals. Empty for raw memories.",
        ),
    ];

    let field_names: Vec<String> = fields.iter().map(|(name, _, _)| name.to_string()).collect();

    let mut schema = Schema::new(
        MEMORY_DESCRIPTIVE_NAME.to_string(),
        SchemaType::Hash,
        Some(KeyConfig::new(Some(fields::ID.to_string()), None)),
        Some(field_names),
        None,
        None,
    );

    schema.descriptive_name = Some(MEMORY_DESCRIPTIVE_NAME.to_string());

    for (name, ty, description) in fields {
        schema.field_types.insert(name.to_string(), ty);
        schema
            .field_descriptions
            .insert(name.to_string(), description.to_string());
        schema.field_data_classifications.insert(
            name.to_string(),
            DataClassification {
                sensitivity_level: 0,
                data_domain: "general".to_string(),
            },
        );
        // Default classification so the schema service's field-classification
        // validation path doesn't reject. Matches what builtin_schemas uses.
        schema
            .field_classifications
            .insert(name.to_string(), vec!["word".to_string()]);
    }

    schema.compute_identity_hash();
    schema
}

/// Propose the memory schema to the schema service, load the canonical
/// locally, and approve it so mutations can write. Returns the canonical
/// runtime name — callers must use this name (not `MEMORY_DESCRIPTIVE_NAME`)
/// for all subsequent mutations and queries.
///
/// Idempotent: if the schema service already has this schema (same
/// identity_hash), the `add_schema` call returns `AlreadyExists` and the
/// existing canonical is reused.
pub async fn register_memory_schema(node: &FoldNode) -> FoldDbResult<String> {
    let schema = memory_schema();
    log::info!(
        "memory: proposing schema '{}' (identity_hash={}) to schema service",
        MEMORY_DESCRIPTIVE_NAME,
        schema.name
    );

    let response = node.add_schema_to_service(&schema).await?;
    let canonical = response.schema.clone();
    let canonical_name = canonical.name.clone();

    let fold_db = node.get_fold_db()?;
    let schema_manager = fold_db.schema_manager();

    let canonical_json = serde_json::to_string(&canonical).map_err(|e| {
        FoldDbError::Config(format!(
            "memory: failed to serialize canonical schema '{}': {}",
            canonical_name, e
        ))
    })?;

    schema_manager
        .load_schema_from_json(&canonical_json)
        .await
        .map_err(|e| {
            FoldDbError::Config(format!(
                "memory: failed to load canonical schema '{}' locally: {}",
                canonical_name, e
            ))
        })?;

    schema_manager
        .set_schema_state(&canonical_name, SchemaState::Approved)
        .await
        .map_err(|e| {
            FoldDbError::Config(format!(
                "memory: failed to approve canonical schema '{}': {}",
                canonical_name, e
            ))
        })?;

    log::info!(
        "memory: registered schema, canonical_name={}",
        canonical_name
    );

    Ok(canonical_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_schema_has_expected_fields() {
        let schema = memory_schema();
        let expected = [
            fields::ID,
            fields::BODY,
            fields::KIND,
            fields::STATUS,
            fields::TAGS,
            fields::SOURCE,
            fields::CREATED_AT,
            fields::DERIVED_FROM,
        ];
        for field in expected {
            assert!(
                schema.field_types.contains_key(field),
                "memory schema missing field `{}`",
                field
            );
            assert!(
                schema.field_descriptions.contains_key(field),
                "memory schema missing description for `{}`",
                field
            );
        }
    }

    #[test]
    fn memory_schema_has_identity_hash() {
        let schema = memory_schema();
        assert!(
            !schema.name.is_empty(),
            "identity_hash was not computed — schema.name is empty"
        );
    }

    #[test]
    fn memory_schema_keyed_by_id() {
        let schema = memory_schema();
        let key = schema
            .key
            .as_ref()
            .expect("memory schema should declare a KeyConfig");
        assert_eq!(
            key.hash_field.as_deref(),
            Some(fields::ID),
            "memory schema should be keyed by `id`"
        );
    }

    #[test]
    fn memory_schema_is_stable() {
        // Identity hash must be deterministic — two calls return the same
        // schema.name. The schema service relies on this for dedup across
        // node restarts.
        let a = memory_schema();
        let b = memory_schema();
        assert_eq!(a.name, b.name, "memory schema identity_hash must be stable");
    }

    #[test]
    fn tags_and_derived_from_are_string_arrays() {
        let schema = memory_schema();
        let tags_ty = schema.field_types.get(fields::TAGS).unwrap();
        match tags_ty {
            FieldValueType::Array(inner) => matches!(**inner, FieldValueType::String)
                .then_some(())
                .expect("tags should be Array<String>"),
            _ => panic!("tags should be Array<String>, got {:?}", tags_ty),
        }

        let derived_ty = schema.field_types.get(fields::DERIVED_FROM).unwrap();
        match derived_ty {
            FieldValueType::Array(inner) => matches!(**inner, FieldValueType::String)
                .then_some(())
                .expect("derived_from should be Array<String>"),
            _ => panic!("derived_from should be Array<String>, got {:?}", derived_ty),
        }
    }
}
