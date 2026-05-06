//! Schema creation and determination logic for IngestionService.

use super::{get_schema_manager, schema_err, IngestionService};
use crate::fold_node::FoldNode;
use crate::ingestion::{AISchemaResponse, IngestionError, IngestionResult};
use fold_db::schema::types::SchemaType;
use fold_db::schema::SchemaCore;
use serde_json::Value;
use std::collections::HashMap;

/// Default-fill `field_data_classifications` for any field listed in
/// `fields` that doesn't already have an entry. Schemas registered before
/// PR #361 (and any with corrupt persistence) reach the local node missing
/// this map; fold_db's loader rejects them with `"Schema 'X' has unclassified
/// fields …"`. Filling with `sensitivity_level=1, data_domain="general"`
/// matches the intent of the original PR — keep legacy schemas loadable
/// instead of becoming unrecoverable once they're already in the registry.
///
/// Mutates `schema_value` in place. No-op if `schema_value` isn't a JSON
/// object, if `fields` is missing/non-array, or if every field is already
/// classified.
fn backfill_field_data_classifications(schema_value: &mut Value) {
    let Some(obj) = schema_value.as_object_mut() else {
        return;
    };
    let field_names: Vec<String> = obj
        .get("fields")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if field_names.is_empty() {
        return;
    }
    let classifications = obj
        .entry("field_data_classifications".to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    // The map can be present-but-null (rare, from hand-edited JSON).
    // Replace null with an empty object before populating.
    if classifications.is_null() {
        *classifications = Value::Object(serde_json::Map::new());
    }
    let Some(classifications_obj) = classifications.as_object_mut() else {
        return;
    };
    for field_name in field_names {
        if !classifications_obj.contains_key(&field_name) {
            classifications_obj.insert(
                field_name,
                serde_json::json!({
                    "sensitivity_level": 1,
                    "data_domain": "general"
                }),
            );
        }
    }
}

impl IngestionService {
    /// Determine which schema to use based on AI response.
    /// Returns (schema_name, service_mutation_mappers) — the service mappers include
    /// any semantic field renames (e.g., "creator" → "artist") that must be merged
    /// with the AI's original mutation_mappers before generating mutations.
    pub(super) async fn determine_schema_to_use(
        &self,
        ai_response: &AISchemaResponse,
        sample_data: &Value,
        node: &FoldNode,
    ) -> IngestionResult<(String, HashMap<String, String>)> {
        // Always create a new schema from the AI definition
        if let Some(new_schema_def) = &ai_response.new_schemas {
            let (schema_name, service_mappers) = self
                .create_new_schema_with_node(new_schema_def, sample_data, node)
                .await?;
            return Ok((schema_name, service_mappers));
        }

        Err(IngestionError::ai_response_validation_error(
            "AI response did not provide a new schema definition",
        ))
    }

    /// Create a new schema using the FoldNode.
    /// Returns (schema_name, service_mutation_mappers) — service mappers include
    /// any semantic field renames from schema expansion.
    pub(crate) async fn create_new_schema_with_node(
        &self,
        schema_def: &Value,
        sample_data: &Value,
        node: &FoldNode,
    ) -> IngestionResult<(String, HashMap<String, String>)> {
        // Deserialize Value to Schema
        let mut schema: fold_db::schema::types::Schema = serde_json::from_value(schema_def.clone())
            .map_err(|error| {
                tracing::error!(
                target: "fold_node::ingestion",
                        "Schema deserialization failed: {}. Raw AI schema JSON: {}",
                        error,
                        serde_json::to_string_pretty(schema_def).unwrap_or_default()
                    );
                IngestionError::SchemaCreationError(format!(
                    "Failed to deserialize schema from AI response: {}",
                    error
                ))
            })?;

        tracing::info!(
            target: "fold_node::ingestion",
            "Deserialized schema with {} field classifications from AI",
            schema.field_classifications.len()
        );

        // Safety net: generate default field_descriptions for any fields missing them.
        // The AI prompt and validation retry loop should produce these, but if all
        // retries failed to include them, we generate defaults here so the schema
        // service doesn't reject the schema.
        if let Some(fields) = &schema.fields {
            for field_name in fields {
                schema
                    .field_descriptions
                    .entry(field_name.clone())
                    .or_insert_with(|| {
                        tracing::warn!(
            target: "fold_node::ingestion",
                            "AI did not provide field_description for '{}', using default",
                            field_name
                        );
                        format!("{} field", field_name.replace('_', " "))
                    });
            }
        }

        // Ensure default classifications for fields that are missing them
        if let Some(fields) = &schema.fields {
            let sample_for_defaults = if let Some(array) = sample_data.as_array() {
                array.first().unwrap_or(sample_data)
            } else {
                sample_data
            };

            for field_name in fields {
                let needs_default = schema
                    .field_classifications
                    .get(field_name)
                    .map(|v| v.is_empty())
                    .unwrap_or(true);
                if needs_default {
                    let default = match sample_for_defaults.get(field_name) {
                        Some(Value::Number(_)) => vec!["number".to_string()],
                        _ => vec!["word".to_string()],
                    };
                    tracing::info!(
                    target: "fold_node::ingestion",
                                "Added default classification {:?} to field '{}'",
                                default,
                                field_name
                            );
                    schema
                        .field_classifications
                        .insert(field_name.clone(), default);
                }
            }
        }

        // Infer field_types from sample data for any field that doesn't have
        // a declared type. The AI prompt doesn't ask for field_types, so they're
        // always empty — without this, all fields default to `Any` and the system
        // has no type information (e.g., can't distinguish String from Array<String>).
        if let Some(fields) = &schema.fields {
            let sample_obj = if let Some(array) = sample_data.as_array() {
                array.first().unwrap_or(sample_data)
            } else {
                sample_data
            };
            for field_name in fields {
                if !schema.field_types.contains_key(field_name) {
                    if let Some(sample_value) = sample_obj.get(field_name) {
                        let inferred = fold_db::schema::types::FieldValueType::infer(sample_value);
                        if inferred != fold_db::schema::types::FieldValueType::Null {
                            tracing::info!(
                            target: "fold_node::ingestion",
                                                "Inferred field_type for '{}': {}",
                                                field_name,
                                                inferred
                                            );
                            schema.field_types.insert(field_name.clone(), inferred);
                        }
                    }
                }
            }
        }

        // Ensure schema has key configuration for mutations to work
        if schema.key.is_none() {
            // Use the first field as the hash key
            let hash_field = if let Some(fields) = &schema.fields {
                fields.first().cloned()
            } else if !schema.field_classifications.is_empty() {
                schema.field_classifications.keys().next().cloned()
            } else {
                None
            };

            if let Some(field_name) = hash_field {
                schema.key = Some(fold_db::schema::types::KeyConfig::new(
                    Some(field_name.clone()),
                    None,
                ));
                tracing::info!(
                target: "fold_node::ingestion",
                        "Added default key configuration using field '{}' for schema",
                        field_name
                    );
            } else {
                return Err(IngestionError::SchemaCreationError(
                    "Cannot create schema without at least one field for key configuration"
                        .to_string(),
                ));
            }
        }

        // Compute identity_hash for structure-based deduplication (used by schema service)
        schema.compute_identity_hash();
        let identity_hash = schema
            .get_identity_hash()
            .ok_or_else(|| {
                IngestionError::SchemaCreationError(
                    "Schema must have identity_hash computed".to_string(),
                )
            })?
            .clone();

        // Keep the AI-provided semantic name (e.g., "customer_orders").
        // If the AI left it blank or used the placeholder "Schema", fall back to identity_hash.
        // Note: the schema service always overrides the name with identity_hash for dedup.
        // The human-readable name lives in descriptive_name (used by the UI).
        let ai_name = schema.name.trim().to_string();
        if ai_name.is_empty() || ai_name.eq_ignore_ascii_case("schema") {
            schema.name = identity_hash;
        }

        // Serialize schema creation: the schema service call, local load, and
        // block_and_supersede must happen atomically so concurrent ingestions
        // don't race on creating/expanding the same schema.
        let _lock = self.schema_creation_lock.lock().await;

        // Add schema to the schema service via the node
        let add_response = {
            node.add_schema_to_service(&schema).await.map_err(|error| {
                IngestionError::SchemaCreationError(format!(
                    "Failed to create schema via schema service: {}",
                    error
                ))
            })?
        };

        let schema_response = &add_response.schema;

        // Backfill default classifications for any unclassified fields. The schema
        // service may return legacy schemas missing `field_data_classifications` entries
        // (e.g. schemas created before PR #361 enforcement). Rather than rejecting at
        // load time, we default unclassified fields to sensitivity_level=1, domain=general.
        let mut schema_value = serde_json::to_value(schema_response).map_err(|error| {
            IngestionError::ai_response_validation_error(format!(
                "Failed to serialize schema definition: {}",
                error
            ))
        })?;
        backfill_field_data_classifications(&mut schema_value);
        let json_str = serde_json::to_string(&schema_value).map_err(|error| {
            IngestionError::ai_response_validation_error(format!(
                "Failed to serialize schema definition: {}",
                error
            ))
        })?;

        let schema_manager = get_schema_manager(node).await?;

        // Only load the schema if it doesn't already exist locally.
        // Re-loading from the schema service JSON would overwrite the cached schema's
        // molecule state (field_molecule_uuids, runtime_fields), causing subsequent
        // mutations to create new molecules instead of appending to existing ones.
        // Exception: if expansion happened, always reload since the schema name changed.
        let already_loaded = add_response.replaced_schema.is_none()
            && schema_manager
                .get_schema_metadata(&schema_response.name)
                .map(|opt| opt.is_some())
                .unwrap_or(false);

        // If expansion happened, ensure the old schema is loaded locally BEFORE
        // loading the new one. apply_field_mappers (triggered by approve) needs
        // the old schema's molecule UUIDs. In a fresh DB the old schema only
        // exists on the remote schema service.
        //
        // INVARIANT: schema expansion must be atomic. If we can't fetch or load
        // the old schema, we MUST abort ingestion rather than silently
        // continuing — otherwise apply_field_mappers runs without the source
        // molecule UUIDs and the new expanded schema coexists with an
        // unblocked old schema, producing split-brain writes.
        if let Some(ref old_name) = add_response.replaced_schema {
            let old_loaded = schema_manager
                .get_schema_metadata(old_name)
                .map(|opt| opt.is_some())
                .unwrap_or(false);
            if !old_loaded {
                if let Some(url) = node.schema_service_url() {
                    if !crate::fold_node::node::FoldNode::is_test_schema_service(&url) {
                        let client = crate::fold_node::SchemaServiceClient::new(&url);
                        let old_envelope = client.get_schema(old_name).await.map_err(|e| {
                            IngestionError::SchemaCreationError(format!(
                                "Schema expansion aborted: failed to fetch old schema '{}' \
                                 from schema service (new schema '{}' not loaded): {}",
                                old_name, schema_response.name, e
                            ))
                        })?;
                        // Serialize the inner Schema — the local
                        // `load_schema_from_json` path expects the Schema
                        // wire shape, not the envelope. The `system` flag
                        // on the envelope is not needed locally.
                        //
                        // Apply the same `field_data_classifications` backfill
                        // we apply to the new schema above. The dev cloud's
                        // schema-service S3 store has accumulated legacy
                        // schemas from pre-PR-#361 ingestions whose fields
                        // serialize without classifications; without backfill
                        // here, fold_db's validator rejects them and aborts
                        // expansion before the new schema can land. (Surfaced
                        // by the e2e face-discovery scenario hitting an old
                        // "Photography" entry from prior nightly runs.)
                        let mut old_value =
                            serde_json::to_value(&old_envelope.schema).map_err(schema_err)?;
                        backfill_field_data_classifications(&mut old_value);
                        let old_json = serde_json::to_string(&old_value).map_err(schema_err)?;
                        schema_manager
                            .load_schema_from_json(&old_json)
                            .await
                            .map_err(|e| {
                                IngestionError::SchemaCreationError(format!(
                                    "Schema expansion aborted: failed to load old schema '{}' \
                                     locally (new schema '{}' not loaded): {}",
                                    old_name, schema_response.name, e
                                ))
                            })?;
                    }
                }
            }
        }

        if !already_loaded {
            match schema_manager.load_schema_from_json(&json_str).await {
                Ok(_) => {}
                Err(error) => return Err(schema_err(error)),
            };
        }

        // Reject expansions whose field_mappers cross schema_type boundaries.
        // Apply must run BEFORE approve, since approve invokes
        // `apply_field_mappers` which would copy the source field's
        // `molecule_uuid` onto the target field — a `MoleculeHash` value
        // becomes unreadable when the target field is a HashRange (different
        // on-disk shape). Pre-fix this surfaced as
        // `FieldBase: skipping molecule ref … invalid type: string,
        // expected struct AtomEntry` and the new field silently returned
        // empty data on every read.
        validate_field_mapper_compatibility(&schema_response.name, schema_manager.as_ref()).await?;

        // Approve BEFORE blocking old schema — approval triggers apply_field_mappers
        // which needs to read the old schema's molecule UUIDs. If we block first,
        // the superseded_by redirect could cause circular resolution.
        schema_manager
            .approve(&schema_response.name)
            .await
            .map_err(schema_err)?;

        // Block the old schema AFTER approval, so field_mappers are already resolved.
        //
        // INVARIANT: if block_and_supersede fails, the new expanded schema is
        // live AND the old schema is still Approved — both accept writes,
        // creating split-brain state. We must propagate this error so the
        // caller can abort ingestion for this sample.
        if let Some(ref old_name) = add_response.replaced_schema {
            tracing::info!(
            target: "fold_node::ingestion",
                "Schema expansion: blocking old schema '{}', loaded expanded '{}'",
                old_name,
                schema_response.name
            );
            schema_manager
                .block_and_supersede(old_name, &schema_response.name)
                .await
                .map_err(|e| {
                    IngestionError::SchemaCreationError(format!(
                        "Schema expansion aborted: failed to block old schema '{}' \
                         after loading expanded '{}' (split-brain risk): {}",
                        old_name, schema_response.name, e
                    ))
                })?;
        }

        let schema_name = schema_response.name.clone();
        let service_mappers = add_response.mutation_mappers.clone();
        drop(_lock);

        Ok((schema_name, service_mappers))
    }

    /// Create an org-scoped copy of an existing schema.
    ///
    /// The schema service is unaware of orgs. This clones the personal schema
    /// definition, sets `org_hash`, and registers it locally under a namespaced
    /// name so mutations get org-prefixed storage keys.
    ///
    /// Returns the org-scoped schema name. No-ops if the org schema already exists.
    pub(super) async fn ensure_org_schema(
        &self,
        schema_name: &str,
        org_hash: &str,
        node: &FoldNode,
    ) -> IngestionResult<String> {
        let org_schema_name = format!("{}:{}", org_hash, schema_name);

        let schema_manager = get_schema_manager(node).await?;

        // Already loaded — return early
        if schema_manager
            .get_schema_metadata(&org_schema_name)
            .map(|opt| opt.is_some())
            .unwrap_or(false)
        {
            return Ok(org_schema_name);
        }

        // Clone the personal schema and set org fields
        let personal = schema_manager
            .get_schema(schema_name)
            .await
            .map_err(schema_err)?
            .ok_or_else(|| {
                IngestionError::SchemaCreationError(format!(
                    "Schema '{}' not found for org copy",
                    schema_name
                ))
            })?;

        let mut org_schema = personal;
        org_schema.name = org_schema_name.clone();
        org_schema.org_hash = Some(org_hash.to_string());
        // Clear inherited molecule state — org data uses prefixed keys so
        // personal molecule UUIDs don't apply. Fresh molecules will be created
        // on the first mutation.
        org_schema.field_molecule_uuids = None;
        org_schema.runtime_fields.clear();
        org_schema.populate_runtime_fields().map_err(schema_err)?;

        let org_json = serde_json::to_string(&org_schema).map_err(schema_err)?;
        schema_manager
            .load_schema_from_json(&org_json)
            .await
            .map_err(schema_err)?;
        schema_manager
            .approve(&org_schema_name)
            .await
            .map_err(schema_err)?;

        tracing::info!(
            target: "fold_node::ingestion",
            "Registered org-scoped schema '{}' for org {}",
            org_schema_name,
            org_hash
        );

        Ok(org_schema_name)
    }
}

/// Reject schema-expansion proposals whose `field_mappers` carry molecule
/// UUIDs across incompatible `schema_type` boundaries.
///
/// `apply_field_mappers` (invoked by `SchemaCore::approve`) copies the source
/// field's `molecule_uuid` onto the target field unconditionally. If the
/// source schema's `schema_type` differs from the target's — e.g. expanding
/// a `Hash` schema into a `HashRange` schema — the on-disk molecule shape no
/// longer matches what the target field tries to deserialize. Reads then fail
/// with `invalid type: string, expected struct AtomEntry`, which the upstream
/// `refresh_field_from_db` swallows as a WARN, leaving the field's molecule
/// `None` and silently returning empty data on every subsequent query.
///
/// Reject loud here so the schema-service mis-classification surfaces to the
/// caller (`IngestionError::SchemaCreationError`), rather than letting the
/// ingestion appear to succeed while shedding data. Same-type expansion
/// continues to work — the source field's molecule is reused as designed.
pub(super) async fn validate_field_mapper_compatibility(
    new_schema_name: &str,
    schema_manager: &SchemaCore,
) -> IngestionResult<()> {
    let new_schema = schema_manager
        .get_schema(new_schema_name)
        .await
        .map_err(schema_err)?
        .ok_or_else(|| {
            IngestionError::SchemaCreationError(format!(
                "Schema '{}' not found in cache after load_schema_from_json",
                new_schema_name
            ))
        })?;

    let Some(field_mappers) = new_schema.field_mappers.as_ref() else {
        return Ok(());
    };

    if field_mappers.is_empty() {
        return Ok(());
    }

    // Resolve each unique source schema once.
    let mut source_types: HashMap<String, SchemaType> = HashMap::new();
    let mut incompatibilities: Vec<String> = Vec::new();

    for (target_field, mapper) in field_mappers {
        let src_name = mapper.source_schema().to_string();
        if !source_types.contains_key(&src_name) {
            match schema_manager.get_schema(&src_name).await {
                Ok(Some(src_schema)) => {
                    source_types.insert(src_name.clone(), src_schema.schema_type.clone());
                }
                Ok(None) => {
                    // Source schema isn't registered locally — `apply_field_mappers`
                    // already warns and skips this entry, so no carry-over occurs
                    // and there's nothing to corrupt. Leave it be.
                    continue;
                }
                Err(e) => {
                    return Err(schema_err(e));
                }
            }
        }
        if let Some(src_type) = source_types.get(&src_name) {
            if *src_type != new_schema.schema_type {
                incompatibilities.push(format!(
                    "field '{}' inherits from '{}.{}' (schema_type={:?}) but target schema \
                     '{}' has schema_type={:?}",
                    target_field,
                    src_name,
                    mapper.source_field(),
                    src_type,
                    new_schema_name,
                    new_schema.schema_type,
                ));
            }
        }
    }

    if !incompatibilities.is_empty() {
        return Err(IngestionError::SchemaCreationError(format!(
            "Schema expansion rejected: cross-schema_type field_mapper(s) would corrupt \
             molecule storage on read. Incompatibilities: [{}]. The schema service must \
             not propose expansion across {{Single, Hash, Range, HashRange}} boundaries — \
             each schema_type stores molecules in a different on-disk shape.",
            incompatibilities.join("; "),
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{backfill_field_data_classifications, validate_field_mapper_compatibility};
    use serde_json::json;

    /// Build a serde_json schema definition for the regression tests below.
    /// `field_mappers` is `Some({target: "<src>.<src_field>"})` to mark fields
    /// inherited from another schema during expansion.
    fn make_schema_json(
        name: &str,
        schema_type: &str,
        key: serde_json::Value,
        fields: &[&str],
        field_mappers: Option<serde_json::Value>,
    ) -> serde_json::Value {
        let field_descriptions: serde_json::Map<String, serde_json::Value> = fields
            .iter()
            .map(|f| {
                (
                    (*f).to_string(),
                    serde_json::Value::String(format!("{} desc", f)),
                )
            })
            .collect();
        let field_data_classifications: serde_json::Map<String, serde_json::Value> = fields
            .iter()
            .map(|f| ((*f).to_string(), default_dc()))
            .collect();
        let mut obj = json!({
            "name": name,
            "descriptive_name": name,
            "schema_type": schema_type,
            "key": key,
            "fields": fields,
            "field_descriptions": field_descriptions,
            "field_data_classifications": field_data_classifications,
            "identity_hash": name,
            "source": "user",
        });
        if let Some(fm) = field_mappers {
            obj.as_object_mut()
                .unwrap()
                .insert("field_mappers".to_string(), fm);
        }
        obj
    }

    fn default_dc() -> serde_json::Value {
        json!({"sensitivity_level": 1, "data_domain": "general"})
    }

    /// Regression: pre-fix a `Hash` → `HashRange` schema expansion via
    /// `field_mappers` corrupted molecule reads with
    /// `invalid type: string, expected struct AtomEntry`. The validator
    /// must reject that proposal loudly so the bug surfaces instead of
    /// being swallowed by `FieldBase::refresh_field_from_db`.
    #[tokio::test]
    async fn rejects_hash_to_hashrange_field_mapper_carry_over() {
        let core = fold_db::schema::SchemaCore::new_for_testing()
            .await
            .expect("schema core init");

        // Source: Hash schema (HashField → MoleculeHash on disk)
        let src = make_schema_json(
            "RecipesV1",
            "Hash",
            json!({"hash_field": "source_file", "range_field": null}),
            &["source_file", "content", "file_type"],
            None,
        );
        core.load_schema_from_json(&serde_json::to_string(&src).unwrap())
            .await
            .expect("load source schema");

        // Target: HashRange schema (HashRangeField → MoleculeHashRange on disk)
        // with field_mappers carrying over `content`/`file_type`/`source_file`
        // from the Hash schema. Inheriting molecule_uuids across these two
        // shapes is the bug — same on-disk key, different deserialization
        // expectation.
        let tgt = make_schema_json(
            "RecipesV2",
            "HashRange",
            json!({"hash_field": "recipe", "range_field": "day"}),
            &[
                "recipe",
                "day",
                "meal",
                "content",
                "file_type",
                "source_file",
            ],
            Some(json!({
                "content": "RecipesV1.content",
                "file_type": "RecipesV1.file_type",
                "source_file": "RecipesV1.source_file",
            })),
        );
        core.load_schema_from_json(&serde_json::to_string(&tgt).unwrap())
            .await
            .expect("load target schema");

        let err = validate_field_mapper_compatibility("RecipesV2", &core)
            .await
            .expect_err("cross-schema_type expansion must be rejected");

        let msg = err.to_string();
        assert!(
            msg.contains("Schema expansion rejected"),
            "error must explain the rejection: {msg}"
        );
        assert!(
            msg.contains("RecipesV1") && msg.contains("RecipesV2"),
            "error must name both schemas: {msg}"
        );
        assert!(
            msg.contains("Hash") && msg.contains("HashRange"),
            "error must identify the incompatible schema_types: {msg}"
        );
    }

    /// Same-type expansion (Hash → Hash, just with extra fields) must still
    /// pass — the molecule_uuid carry-over is well-defined when shapes match.
    #[tokio::test]
    async fn accepts_same_schema_type_field_mapper_carry_over() {
        let core = fold_db::schema::SchemaCore::new_for_testing()
            .await
            .expect("schema core init");

        let src = make_schema_json(
            "ContactsV1",
            "Hash",
            json!({"hash_field": "id", "range_field": null}),
            &["id", "name", "email"],
            None,
        );
        core.load_schema_from_json(&serde_json::to_string(&src).unwrap())
            .await
            .expect("load source schema");

        let tgt = make_schema_json(
            "ContactsV2",
            "Hash",
            json!({"hash_field": "id", "range_field": null}),
            &["id", "name", "email", "phone"],
            Some(json!({
                "id": "ContactsV1.id",
                "name": "ContactsV1.name",
                "email": "ContactsV1.email",
            })),
        );
        core.load_schema_from_json(&serde_json::to_string(&tgt).unwrap())
            .await
            .expect("load target schema");

        validate_field_mapper_compatibility("ContactsV2", &core)
            .await
            .expect("same schema_type expansion must validate");
    }

    /// No field_mappers at all — the canonical first-ingestion path. Must
    /// be a no-op and never error.
    #[tokio::test]
    async fn accepts_schema_without_field_mappers() {
        let core = fold_db::schema::SchemaCore::new_for_testing()
            .await
            .expect("schema core init");

        let s = make_schema_json("Quotes", "Single", json!(null), &["author", "text"], None);
        core.load_schema_from_json(&serde_json::to_string(&s).unwrap())
            .await
            .expect("load schema");

        validate_field_mapper_compatibility("Quotes", &core)
            .await
            .expect("schema without field_mappers must validate");
    }

    /// The source schema referenced by a mapper isn't loaded locally.
    /// `apply_field_mappers` already warns and skips this case (no carry-over
    /// happens), so the validator must accept it — there's nothing to corrupt.
    #[tokio::test]
    async fn accepts_field_mapper_with_missing_source_schema() {
        let core = fold_db::schema::SchemaCore::new_for_testing()
            .await
            .expect("schema core init");

        let tgt = make_schema_json(
            "OrphanedExpansion",
            "HashRange",
            json!({"hash_field": "h", "range_field": "r"}),
            &["h", "r", "v"],
            Some(json!({
                "v": "NonExistent.v",
            })),
        );
        core.load_schema_from_json(&serde_json::to_string(&tgt).unwrap())
            .await
            .expect("load target schema");

        validate_field_mapper_compatibility("OrphanedExpansion", &core)
            .await
            .expect("missing source must be skipped, not rejected");
    }

    #[test]
    fn backfill_inserts_map_when_absent() {
        // Mirrors the on-wire shape returned by `client.get_schema()` for legacy
        // schemas: serde skips empty `field_data_classifications`, so the key
        // is missing entirely from the JSON object.
        let mut v = json!({
            "name": "legacy",
            "fields": ["a", "b"],
        });
        backfill_field_data_classifications(&mut v);
        let fdc = v.get("field_data_classifications").unwrap();
        assert_eq!(fdc.get("a").unwrap(), &default_dc());
        assert_eq!(fdc.get("b").unwrap(), &default_dc());
    }

    #[test]
    fn backfill_only_fills_missing_entries() {
        // Existing classifications must be preserved as-is; only fields without
        // an entry get the default. This guards against clobbering real
        // sensitivity levels assigned upstream.
        let mut v = json!({
            "fields": ["a", "b", "c"],
            "field_data_classifications": {
                "a": {"sensitivity_level": 3, "data_domain": "health"}
            }
        });
        backfill_field_data_classifications(&mut v);
        let fdc = v.get("field_data_classifications").unwrap();
        assert_eq!(
            fdc.get("a").unwrap(),
            &json!({"sensitivity_level": 3, "data_domain": "health"})
        );
        assert_eq!(fdc.get("b").unwrap(), &default_dc());
        assert_eq!(fdc.get("c").unwrap(), &default_dc());
    }

    #[test]
    fn backfill_replaces_null_value_with_object() {
        // Defensive: hand-edited fixtures occasionally have `null` instead of
        // omitting the key. Treat null as "no classifications" rather than
        // panicking or skipping.
        let mut v = json!({
            "fields": ["a"],
            "field_data_classifications": null,
        });
        backfill_field_data_classifications(&mut v);
        assert_eq!(
            v.get("field_data_classifications")
                .unwrap()
                .get("a")
                .unwrap(),
            &default_dc()
        );
    }

    #[test]
    fn backfill_no_op_when_all_fields_classified() {
        let original = json!({
            "fields": ["a"],
            "field_data_classifications": {
                "a": {"sensitivity_level": 0, "data_domain": "general"}
            }
        });
        let mut v = original.clone();
        backfill_field_data_classifications(&mut v);
        assert_eq!(v, original);
    }

    #[test]
    fn backfill_no_op_on_non_object_value() {
        let mut v = json!(["not", "an", "object"]);
        backfill_field_data_classifications(&mut v);
        assert_eq!(v, json!(["not", "an", "object"]));
    }

    #[test]
    fn backfill_no_op_when_fields_missing() {
        let mut v = json!({"name": "x"});
        backfill_field_data_classifications(&mut v);
        // No fields → nothing to classify; the map shouldn't get inserted just
        // to stay empty.
        assert!(v.get("field_data_classifications").is_none());
    }
}
