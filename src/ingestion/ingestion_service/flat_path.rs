//! Flat (non-nested) ingestion path for IngestionService.

use super::{get_schema_manager, IngestionService};
use crate::ingestion::progress::{IngestionStep, ProgressService, SchemaWriteRecord};
use crate::ingestion::{AISchemaResponse, IngestionRequest, IngestionResult};
use crate::fold_node::FoldNode;
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::schema::types::Mutation;
use serde_json::Value;

impl IngestionService {
    /// Handles the flat (non-nested) ingestion path: AI recommendation, mutation generation, execution.
    /// Returns (schema_name, new_schema_created, mutations_generated, mutations_executed, schemas_written).
    pub(crate) async fn process_flat_path(
        &self,
        flattened_data: &Value,
        request: &IngestionRequest,
        node: &FoldNode,
        progress_service: &ProgressService,
        progress_id: &str,
    ) -> IngestionResult<(String, bool, usize, usize, Vec<SchemaWriteRecord>)> {
        let pub_key = request.pub_key.clone();

        // Step 3: Get AI recommendation (with image override)
        let is_image = request
            .source_file_name
            .as_ref()
            .map(|name| crate::ingestion::is_image_file(name))
            .unwrap_or(false);
        progress_service.update_progress(progress_id, IngestionStep::GettingAIRecommendation,
            "Analyzing data with AI to determine schema...".to_string()).await;
        let mut ai_response = self.get_ai_recommendation(flattened_data).await?;

        // If the AI didn't provide field_descriptions, do a second AI call
        // focused just on generating descriptions from the JSON structure.
        self.fill_missing_field_descriptions(&mut ai_response, flattened_data).await?;

        if is_image {
            super::apply_image_schema_override(
                &mut ai_response,
                request.image_descriptive_name.as_deref(),
            );
        }

        // Step 4: Determine schema to use
        progress_service.update_progress(progress_id, IngestionStep::SettingUpSchema,
            "Setting up schema and preparing for data storage...".to_string()).await;
        let (schema_name, service_mappers) = self
            .determine_schema_to_use(&ai_response, flattened_data, node)
            .await?;
        // Merge schema service's semantic field renames into AI's mutation_mappers.
        // Service mappers (e.g., "creator" → "artist") take precedence since they
        // reflect the canonical field names on the actual expanded schema.
        for (from, to) in &service_mappers {
            ai_response.mutation_mappers.insert(from.clone(), to.clone());
        }
        let new_schema_created = ai_response.new_schemas.is_some();

        // Enrich image data with source_file_name, created_at, image_type so
        // mutations include these key fields. The HTTP routes do this before
        // calling us, but direct callers (integration tests, admin_ops) may not.
        let enriched_data = if is_image {
            let mut data = flattened_data.clone();
            if let Some(ref sfn) = request.source_file_name {
                let dummy_path = std::path::PathBuf::from(sfn);
                crate::ingestion::json_processor::enrich_image_json(
                    &mut data,
                    &dummy_path,
                    Some(sfn.as_str()),
                );
            }
            data
        } else {
            flattened_data.clone()
        };

        // Step 5: Generate mutations
        progress_service.update_progress(progress_id, IngestionStep::GeneratingMutations,
            "Generating database mutations...".to_string()).await;
        let (mutations, schemas_written) = self
            .generate_flat_mutations(
                &enriched_data,
                &schema_name,
                &ai_response,
                request,
                &pub_key,
                node,
                progress_service,
                progress_id,
            )
            .await?;

        log_feature!(
            LogFeature::Ingestion,
            info,
            "Generated {} mutations",
            mutations.len()
        );

        // Step 6: Execute mutations if requested
        progress_service.update_progress(progress_id, IngestionStep::ExecutingMutations,
            "Executing mutations to store data...".to_string()).await;

        let mutations_len = mutations.len();

        let mutations_executed = if request.auto_execute {
            self.execute_mutations_with_node_and_progress(
                mutations,
                node,
                progress_service,
                progress_id,
            )
            .await?
        } else {
            0
        };

        Ok((schema_name, new_schema_created, mutations_len, mutations_executed, schemas_written))
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
        progress_service: &ProgressService,
        progress_id: &str,
    ) -> IngestionResult<(Vec<Mutation>, Vec<SchemaWriteRecord>)> {
        // Get schema manager for key extraction
        let schema_manager = get_schema_manager(node).await?;

        let metadata = Self::build_ingestion_metadata(&request.file_hash, progress_id);

        // Collect items to process — normalize single object to a one-element slice
        let items: Vec<&serde_json::Map<String, Value>> = if let Some(array) = flattened_data.as_array() {
            array
                .iter()
                .filter_map(|item| item.as_object())
                .collect()
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
                &ai_response.mutation_mappers,
                &schema_manager,
                pub_key,
                request.source_file_name.clone(),
                metadata.clone(),
            )
            .await?;

            mutations.extend(item_mutations);

            // Update progress every 10 items (only meaningful for arrays)
            if total_items > 1 && ((idx + 1) % 10 == 0 || idx + 1 == total_items) {
                let percent_of_step = ((idx + 1) as f32 / total_items as f32 * 15.0) as u8;
                let progress_percent = 75 + percent_of_step;
                progress_service
                    .update_progress_with_percentage(
                        progress_id,
                        IngestionStep::GeneratingMutations,
                        format!("Generating mutations... ({}/{})", idx + 1, total_items),
                        progress_percent,
                    )
                    .await;
            }
        }

        // Collect schemas_written from generated mutations
        let schemas_written = super::schemas_written_from(&mutations);

        Ok((mutations, schemas_written))
    }
}
