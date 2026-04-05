//! Flat (non-nested) ingestion path for IngestionService.

use super::{get_schema_manager, IngestionService};
use crate::fold_node::FoldNode;
use crate::ingestion::progress::{IngestionPhase, PhaseTracker, SchemaWriteRecord};
use crate::ingestion::{AISchemaResponse, IngestionRequest, IngestionResult};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::schema::types::Mutation;
use serde_json::Value;
use std::collections::HashMap;

impl IngestionService {
    /// Handles the flat (non-nested) ingestion path: AI recommendation, mutation generation, execution.
    /// Returns (schema_name, new_schema_created, mutations_generated, mutations_executed, schemas_written).
    pub(crate) async fn process_flat_path(
        &self,
        flattened_data: &Value,
        request: &IngestionRequest,
        node: &FoldNode,
        tracker: &mut PhaseTracker<'_>,
    ) -> IngestionResult<(String, bool, usize, usize, Vec<SchemaWriteRecord>)> {
        let pub_key = request.pub_key.clone();

        // AI recommendation (with image override)
        let is_image = request
            .source_file_name
            .as_ref()
            .map(|name| crate::ingestion::is_image_file(name))
            .unwrap_or(false);
        tracker
            .enter_phase(
                IngestionPhase::AIRecommendation,
                "Analyzing data with AI to determine schema...".to_string(),
            )
            .await;
        let mut ai_response = self.get_ai_recommendation(flattened_data).await?;

        // If the AI didn't provide field_descriptions, do a second AI call
        // focused just on generating descriptions from the JSON structure.
        self.fill_missing_field_descriptions(&mut ai_response, flattened_data)
            .await?;

        if is_image {
            super::apply_image_schema_override(
                &mut ai_response,
                request.image_descriptive_name.as_deref(),
            );
        }

        // Schema resolution
        tracker
            .enter_phase(
                IngestionPhase::SchemaResolution,
                "Setting up schema and preparing for data storage...".to_string(),
            )
            .await;
        let (mut schema_name, service_mappers) = self
            .determine_schema_to_use(&ai_response, flattened_data, node)
            .await?;

        // If ingesting into an org, create an org-scoped copy of the schema
        if let Some(ref org_hash) = request.org_hash {
            schema_name = self.ensure_org_schema(&schema_name, org_hash, node).await?;
        }
        // Merge schema service's semantic field renames into AI's mutation_mappers.
        // Service mappers (e.g., "creator" → "artist") take precedence since they
        // reflect the canonical field names on the actual expanded schema.
        for (from, to) in &service_mappers {
            ai_response
                .mutation_mappers
                .insert(from.clone(), to.clone());
        }
        let new_schema_created = ai_response.new_schemas.is_some();

        // Enrich image data with source_file_name, created_at, image_type so
        // mutations include these key fields. The HTTP routes do this before
        // calling us, but direct callers (integration tests, admin_ops) may not.
        let enriched_data = if is_image {
            let mut data = flattened_data.clone();
            if let Some(ref sfn) = request.source_file_name {
                let dummy_path = std::path::PathBuf::from(sfn);
                crate::ingestion::file_handling::json_processor::enrich_image_json(
                    &mut data,
                    &dummy_path,
                    Some(sfn.as_str()),
                );
            }
            // Classify photo visibility using AI
            if data.get("visibility").and_then(|v| v.as_str()).is_none() {
                match crate::ingestion::file_handling::json_processor::classify_visibility(
                    &data, self,
                )
                .await
                {
                    Ok(visibility) => {
                        if let Value::Object(ref mut map) = data {
                            map.insert("visibility".to_string(), Value::String(visibility));
                        }
                    }
                    Err(e) => {
                        log_feature!(
                            LogFeature::Ingestion,
                            warn,
                            "Visibility classification failed, skipping: {}",
                            e
                        );
                    }
                }
            }
            data
        } else {
            flattened_data.clone()
        };

        // Mutation generation
        tracker
            .enter_phase(
                IngestionPhase::MutationGeneration,
                "Generating database mutations...".to_string(),
            )
            .await;
        let (mutations, schemas_written) = self
            .generate_flat_mutations(
                &enriched_data,
                &schema_name,
                &ai_response,
                request,
                &pub_key,
                node,
                tracker,
            )
            .await?;

        log_feature!(
            LogFeature::Ingestion,
            info,
            "Generated {} mutations",
            mutations.len()
        );

        // Mutation execution
        tracker
            .enter_phase(
                IngestionPhase::MutationExecution,
                "Executing mutations to store data...".to_string(),
            )
            .await;

        let mutations_len = mutations.len();

        let mutations_executed = if request.auto_execute {
            self.execute_mutations_with_tracking(mutations, node, tracker)
                .await?
        } else {
            0
        };

        Ok((
            schema_name,
            new_schema_created,
            mutations_len,
            mutations_executed,
            schemas_written,
        ))
    }

    /// Generates mutations for flat (non-nested) data items.
    /// Returns (mutations, schemas_written).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn generate_flat_mutations(
        &self,
        flattened_data: &Value,
        schema_name: &str,
        ai_response: &AISchemaResponse,
        request: &IngestionRequest,
        pub_key: &str,
        node: &FoldNode,
        tracker: &PhaseTracker<'_>,
    ) -> IngestionResult<(Vec<Mutation>, Vec<SchemaWriteRecord>)> {
        // Get schema manager for key extraction
        let schema_manager = get_schema_manager(node).await?;

        let metadata = Self::build_ingestion_metadata(&request.file_hash, tracker.progress_id());

        // Filter mutation mappers to only reference fields that exist in the
        // schema's runtime_fields. The AI may include mappers for fields it
        // dropped from the schema definition, causing write failures.
        let mut mutation_mappers = ai_response.mutation_mappers.clone();
        if let Ok(Some(schema_meta)) = schema_manager.get_schema_metadata(schema_name) {
            let schema_fields: std::collections::HashSet<String> =
                schema_meta.runtime_fields.keys().cloned().collect();
            let before = mutation_mappers.len();
            mutation_mappers.retain(|_json_field, schema_field| {
                let target = if schema_field.contains('.') {
                    schema_field.rsplit('.').next().unwrap_or(schema_field)
                } else {
                    schema_field.as_str()
                };
                schema_fields.contains(target)
            });
            let dropped = before - mutation_mappers.len();
            if dropped > 0 {
                log_feature!(
                    LogFeature::Ingestion,
                    debug,
                    "Dropped {} mutation mapper(s) for schema '{}' — target fields not in runtime_fields",
                    dropped,
                    schema_name
                );
            }
        }

        // Collect items to process — normalize single object to a one-element slice
        let items: Vec<&serde_json::Map<String, Value>> =
            if let Some(array) = flattened_data.as_array() {
                array.iter().filter_map(|item| item.as_object()).collect()
            } else if let Some(obj) = flattened_data.as_object() {
                vec![obj]
            } else {
                vec![]
            };

        let total_items = items.len();
        let mut mutations = Vec::new();
        for (idx, obj) in items.into_iter().enumerate() {
            let item_mutations = super::generate_mutations_for_item(
                obj,
                schema_name,
                &mutation_mappers,
                &schema_manager,
                pub_key,
                request.source_file_name.clone(),
                metadata.clone(),
            )
            .await?;

            mutations.extend(item_mutations);

            // Update progress every 10 items (only meaningful for arrays)
            if total_items > 1 && ((idx + 1) % 10 == 0 || idx + 1 == total_items) {
                let fraction = (idx + 1) as f32 / total_items as f32;
                tracker
                    .sub_progress(
                        fraction,
                        format!("Generating mutations... ({}/{})", idx + 1, total_items),
                    )
                    .await;
            }
        }

        // Detect key collisions — two records mapping to the same key means
        // the second will silently overwrite the first. Log a warning so the
        // operator knows data was lost.
        {
            let mut seen: HashMap<(String, fold_db::schema::types::KeyValue), usize> =
                HashMap::new();
            for m in &mutations {
                let key = (m.schema_name.clone(), m.key_value.clone());
                let count = seen.entry(key).or_insert(0);
                *count += 1;
            }
            for ((schema, key_val), count) in &seen {
                if *count > 1 {
                    log_feature!(
                        LogFeature::Ingestion,
                        warn,
                        "Key collision: {} records in schema '{}' share key {:?} — \
                         later records will overwrite earlier ones. \
                         Consider using a unique ID field as hash_field.",
                        count,
                        schema,
                        key_val
                    );
                }
            }
        }

        // Collect schemas_written from generated mutations
        let schemas_written = super::schemas_written_from(&mutations);

        Ok((mutations, schemas_written))
    }
}
