//! Schema creation and determination logic for IngestionService.

use super::{get_schema_manager, schema_err, IngestionService};
use crate::fold_node::FoldNode;
use crate::ingestion::{AISchemaResponse, IngestionError, IngestionResult};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde_json::Value;
use std::collections::HashMap;

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
                IngestionError::SchemaCreationError(format!(
                    "Failed to deserialize schema from AI response: {}",
                    error
                ))
            })?;

        log_feature!(
            LogFeature::Ingestion,
            info,
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
                        log_feature!(
                            LogFeature::Ingestion,
                            warn,
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
                    log_feature!(
                        LogFeature::Ingestion,
                        info,
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
                log_feature!(
                    LogFeature::Ingestion,
                    info,
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
        if schema.get_identity_hash().is_none() {
            return Err(IngestionError::SchemaCreationError(
                "Schema must have identity_hash computed".to_string(),
            ));
        }

        // Keep the AI-provided semantic name (e.g., "customer_orders").
        // If the AI left it blank or used the placeholder "Schema", fall back to identity_hash.
        let ai_name = schema.name.trim().to_string();
        if ai_name.is_empty() || ai_name.eq_ignore_ascii_case("schema") {
            schema.name = schema.get_identity_hash().unwrap().clone();
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

        let json_str = serde_json::to_string(schema_response).map_err(|error| {
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
        if let Some(ref old_name) = add_response.replaced_schema {
            let old_loaded = schema_manager
                .get_schema_metadata(old_name)
                .map(|opt| opt.is_some())
                .unwrap_or(false);
            if !old_loaded {
                if let Some(url) = node.schema_service_url() {
                    if !crate::fold_node::node::FoldNode::is_test_schema_service(&url) {
                        let client = crate::fold_node::SchemaServiceClient::new(&url);
                        match client.get_schema(old_name).await {
                            Ok(old_schema) => {
                                let old_json =
                                    serde_json::to_string(&old_schema).map_err(schema_err)?;
                                if let Err(e) =
                                    schema_manager.load_schema_from_json(&old_json).await
                                {
                                    log_feature!(
                                        LogFeature::Ingestion,
                                        warn,
                                        "Failed to load old schema '{}' from service: {}",
                                        old_name,
                                        e
                                    );
                                }
                            }
                            Err(e) => {
                                log_feature!(
                                    LogFeature::Ingestion,
                                    warn,
                                    "Failed to fetch old schema '{}' from service: {}",
                                    old_name,
                                    e
                                );
                            }
                        }
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

        // Approve BEFORE blocking old schema — approval triggers apply_field_mappers
        // which needs to read the old schema's molecule UUIDs. If we block first,
        // the superseded_by redirect could cause circular resolution.
        schema_manager
            .approve(&schema_response.name)
            .await
            .map_err(schema_err)?;

        // Block the old schema AFTER approval, so field_mappers are already resolved.
        if let Some(ref old_name) = add_response.replaced_schema {
            log_feature!(
                LogFeature::Ingestion,
                info,
                "Schema expansion: blocking old schema '{}', loaded expanded '{}'",
                old_name,
                schema_response.name
            );
            if let Err(e) = schema_manager
                .block_and_supersede(old_name, &schema_response.name)
                .await
            {
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
                    "Failed to block old schema '{}' during expansion: {}",
                    old_name,
                    e
                );
            }
        }

        let schema_name = schema_response.name.clone();
        let service_mappers = add_response.mutation_mappers.clone();
        drop(_lock);

        Ok((schema_name, service_mappers))
    }
}
