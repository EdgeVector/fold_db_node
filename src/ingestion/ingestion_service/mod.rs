//! AI-powered ingestion service that works with FoldNode
//!
//! Handles JSON data ingestion with AI schema recommendation, mutation generation,
//! and execution. Refactored to take &FoldNode references for flexible locking.

mod decomposition;

use crate::fold_node::{FileIngestionRecord, FoldNode};
use crate::ingestion::ai_client::{build_backend, AiBackend};
use crate::ingestion::config::AIProvider;
use crate::ingestion::decomposer;
use crate::ingestion::key_extraction::extract_key_values_from_data;
use crate::ingestion::mutation_generator;
use crate::ingestion::progress::{
    IngestionResults, IngestionStep, ProgressService, SchemaWriteRecord,
};
use crate::ingestion::IngestionRequest;
use crate::ingestion::{
    AISchemaResponse, IngestionConfig, IngestionError, IngestionResponse, IngestionResult,
    IngestionStatus,
};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::schema::types::{KeyValue, Mutation};
use fold_db::schema::SchemaCore;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use crate::schema_service::types::{BatchSchemaReuseResponse, SchemaLookupEntry};
use decomposition::{AiProposal, CachedSchema};

/// Shorthand to wrap any `Display` error as a `SchemaCreationError`.
pub(super) fn schema_err(e: impl std::fmt::Display) -> IngestionError {
    IngestionError::SchemaCreationError(e.to_string())
}

/// Acquire a clone of the SchemaCore from the node without holding the DB lock.
async fn get_schema_manager(node: &FoldNode) -> IngestionResult<Arc<SchemaCore>> {
    let db_guard = node.get_fold_db().await.map_err(schema_err)?;
    let manager = db_guard.schema_manager.clone();
    drop(db_guard);
    Ok(manager)
}

/// AI-powered ingestion service that works with FoldNode
pub struct IngestionService {
    config: IngestionConfig,
    backend: Option<Arc<dyn AiBackend>>,
    /// Stores the reason if the configured provider failed to initialise.
    init_error: Option<String>,
    /// Serializes schema creation to prevent race conditions when multiple
    /// files are ingested concurrently. The AI call (slow) happens outside
    /// the lock; only the schema-service call + local load + block is locked.
    schema_creation_lock: tokio::sync::Mutex<()>,
    /// Cross-file cache: structure hash → resolved schema. Persists across files
    /// in a batch so the second file with the same JSON shape skips AI entirely.
    structure_schema_cache: std::sync::RwLock<HashMap<String, CachedSchema>>,
}

impl IngestionService {
    /// Create an ingestion service from environment configuration
    pub fn from_env() -> IngestionResult<Self> {
        let config = IngestionConfig::from_env()?;
        Self::new(config)
    }

    /// Create a new ingestion service.
    /// The backend is initialised best-effort: if validation fails
    /// (e.g. missing API key) the service is still created so that
    /// `get_status()` can report the correct provider/model — actual
    /// ingestion calls will fail at runtime with a clear error.
    pub fn new(config: IngestionConfig) -> IngestionResult<Self> {
        let (backend, init_error) = build_backend(&config);
        Ok(Self {
            config,
            backend,
            init_error,
            schema_creation_lock: tokio::sync::Mutex::new(()),
            structure_schema_cache: std::sync::RwLock::new(HashMap::new()),
        })
    }

    /// Look up a structure hash in the service-level cache.
    fn get_cached_schema(&self, structure_hash: &str) -> Option<CachedSchema> {
        self.structure_schema_cache
            .read()
            .ok()
            .and_then(|cache| {
                cache.get(structure_hash).map(|c| CachedSchema {
                    schema_name: c.schema_name.clone(),
                    mutation_mappers: c.mutation_mappers.clone(),
                })
            })
    }

    /// Store a resolved schema in the service-level cache for cross-file reuse.
    fn cache_schema(
        &self,
        structure_hash: &str,
        schema_name: &str,
        mutation_mappers: &HashMap<String, String>,
    ) {
        if let Ok(mut cache) = self.structure_schema_cache.write() {
            cache.insert(
                structure_hash.to_string(),
                CachedSchema {
                    schema_name: schema_name.to_string(),
                    mutation_mappers: mutation_mappers.clone(),
                },
            );
        }
    }

    /// Process JSON ingestion using a FoldNode with progress tracking
    /// Accepts a reference to FoldNode, making it compatible with both Mutex and RwLock guards
    pub async fn process_json_with_node_and_progress(
        &self,
        request: IngestionRequest,
        node: &FoldNode,
        progress_service: &ProgressService,
        progress_id: String,
    ) -> IngestionResult<IngestionResponse> {
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Starting JSON ingestion process with FoldNode (progress_id: {})",
            progress_id
        );

        if !self.config.is_ready() {
            progress_service
                .fail_progress(
                    &progress_id,
                    "Ingestion module is not properly configured or disabled".to_string(),
                )
                .await;
            return Ok(IngestionResponse::failure(vec![
                "Ingestion module is not properly configured or disabled".to_string(),
            ]));
        }

        // Step 1: Validate input
        progress_service.update_progress(&progress_id, IngestionStep::ValidatingConfig,
            "Validating input data...".to_string()).await;
        self.validate_input(&request.data)?;

        // Step 2: Flatten data structure for AI analysis
        progress_service.update_progress(&progress_id, IngestionStep::FlatteningData,
            "Processing and flattening data structure...".to_string()).await;
        let mut flattened_data = crate::ingestion::json_processor::flatten_root_layers(request.data.clone());

        // Enrich text-file JSON with source path context so the AI can propose
        // semantic schema names (e.g., "recipes" instead of "document_content").
        enrich_with_source_context(&mut flattened_data, request.source_file_name.as_deref());

        // Step 2.5: Decompose nested structures and decide code path
        let has_nested_children = self.check_has_nested_children(&flattened_data);

        let (schema_name, new_schema_created, mutations_generated, mutations_executed, schemas_written) =
            if has_nested_children {
                progress_service.update_progress(&progress_id, IngestionStep::GettingAIRecommendation,
                    "Decomposing nested data structures...".to_string()).await;
                self.process_decomposed_path(&flattened_data, &request, node, &progress_id).await?
            } else {
                self.process_flat_path(
                    &flattened_data,
                    &request,
                    node,
                    progress_service,
                    &progress_id,
                )
                .await?
            };

        // Shared finalization: record file dedup + complete progress
        self.record_file_ingested(&request, node).await;

        let results = IngestionResults {
            schema_name: schema_name.clone(),
            new_schema_created,
            mutations_generated,
            mutations_executed,
            schemas_written: schemas_written.clone(),
        };
        progress_service
            .complete_progress(&progress_id, results)
            .await;

        Ok(IngestionResponse::success_with_progress(
            progress_id,
            schema_name,
            new_schema_created,
            mutations_generated,
            mutations_executed,
            schemas_written,
        ))
    }

    // --- Helpers for process_json_with_node_and_progress ---

    /// Checks whether the flattened data contains nested arrays-of-objects
    /// that require recursive decomposition.
    fn check_has_nested_children(&self, flattened_data: &Value) -> bool {
        let representative = if let Some(arr) = flattened_data.as_array() {
            arr.first().cloned()
        } else {
            Some(flattened_data.clone())
        };

        representative
            .as_ref()
            .map(|rep| !decomposer::decompose(rep).children.is_empty())
            .unwrap_or(false)
    }

    /// Handles the recursive decomposition path for data with nested arrays-of-objects.
    /// Returns (schema_name, new_schema_created, mutations_generated, mutations_executed, schemas_written).
    async fn process_decomposed_path(
        &self,
        flattened_data: &Value,
        request: &IngestionRequest,
        node: &FoldNode,
        progress_id: &str,
    ) -> IngestionResult<(String, bool, usize, usize, Vec<SchemaWriteRecord>)> {
        let pub_key = request.pub_key.clone();
        let mut schema_cache: HashMap<String, CachedSchema> = HashMap::new();
        let mut total_mutations_generated: usize = 0;
        let mut total_mutations_executed: usize = 0;
        let mut schemas_written_map: HashMap<String, Vec<KeyValue>> = HashMap::new();

        // Collect items: either array elements or the single object
        let items: Vec<Value> = if let Some(arr) = flattened_data.as_array() {
            arr.clone()
        } else {
            vec![flattened_data.clone()]
        };

        // Resolve schemas for the representative's structure tree.
        let representative = if let Some(arr) = flattened_data.as_array() {
            arr.first().cloned()
        } else {
            Some(flattened_data.clone())
        };
        let rep = representative.as_ref()
            .expect("representative is Some when has_nested_children is true");
        let top_level_hash = crate::ingestion::decomposer::compute_structure_hash(rep);

        // Phase 1: Collect all AI proposals recursively (no schema creation).
        // Skips AI for structure hashes already in the service-level cache.
        let mut proposals: HashMap<String, AiProposal> = HashMap::new();
        self.collect_ai_proposals_recursive(
            &top_level_hash,
            rep,
            &mut proposals,
            0,
            request.source_file_name.as_deref(),
        )
        .await?;

        // Phase 2: Batch check reuse for ALL proposals at once
        let entries: Vec<SchemaLookupEntry> = proposals
            .values()
            .filter_map(|p| extract_lookup_entry(&p.ai_response))
            .collect();
        let batch_result = node.batch_check_schema_reuse(&entries).await
            .unwrap_or_else(|e| {
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
                    "Batch schema reuse check failed, falling back to creation: {}",
                    e
                );
                BatchSchemaReuseResponse {
                    matches: HashMap::new(),
                }
            });

        // Phase 3: Resolve schemas using batch results (reuse fast path or create slow path)
        self.resolve_schemas_with_reuse(
            &top_level_hash,
            rep,
            &mut schema_cache,
            &proposals,
            &batch_result,
            node,
            0,
            request.source_file_name.as_deref(),
        )
        .await?;

        // Cache all resolved schemas in the service-level cache for cross-file reuse
        for (hash, cached) in &schema_cache {
            self.cache_schema(hash, &cached.schema_name, &cached.mutation_mappers);
        }

        let metadata = Self::build_ingestion_metadata(&request.file_hash, progress_id);

        // Process each item: recursively handle children, then generate parent mutation.
        for item in &items {
            let (gen, exec, _key_value) = self
                .ingest_decomposed_item(
                    item,
                    &top_level_hash,
                    &mut schema_cache,
                    node,
                    &pub_key,
                    request.source_file_name.clone(),
                    metadata.clone(),
                    request.auto_execute,
                    0,
                    &mut schemas_written_map,
                )
                .await?;
            total_mutations_generated += gen;
            total_mutations_executed += exec;
        }

        // Determine the top-level schema name for the response
        let top_schema_name = schema_cache
            .get(&top_level_hash)
            .map(|c| c.schema_name.clone())
            .unwrap_or_else(|| {
                log_feature!(LogFeature::Ingestion, warn, "Schema cache miss for top-level hash '{}' — returning empty schema name", top_level_hash);
                String::new()
            });

        let schemas_written = schemas_written_from_map(schemas_written_map);

        Ok((top_schema_name, true, total_mutations_generated, total_mutations_executed, schemas_written))
    }

    /// Handles the flat (non-nested) ingestion path: AI recommendation, mutation generation, execution.
    /// Returns (schema_name, new_schema_created, mutations_generated, mutations_executed, schemas_written).
    async fn process_flat_path(
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

        // CRITICAL: Images MUST use HashRange(source_file_name, created_at).
        // Using source_file_name as hash ensures each image file gets a unique key.
        // (image_type is too coarse — all photos share the same value.)
        if is_image {
            if let Some(ref mut schema_def) = ai_response.new_schemas {
                schema_def["schema_type"] = serde_json::json!("HashRange");
                schema_def["key"] = serde_json::json!({
                    "hash_field": "source_file_name",
                    "range_field": "created_at"
                });
                // Ensure source_file_name is in the schema fields.
                // The AI may provide fields as an array OR only in field_classifications.
                // Handle both cases.
                if let Some(fields) = schema_def.get_mut("fields").and_then(|f| f.as_array_mut()) {
                    let sfn = serde_json::json!("source_file_name");
                    if !fields.contains(&sfn) {
                        fields.push(sfn);
                    }
                } else {
                    // fields key doesn't exist or isn't an array — create it from
                    // field_classifications keys + source_file_name
                    let mut field_names: Vec<String> = schema_def
                        .get("field_classifications")
                        .and_then(|fc| fc.as_object())
                        .map(|obj| obj.keys().cloned().collect())
                        .unwrap_or_default();
                    if !field_names.contains(&"source_file_name".to_string()) {
                        field_names.push("source_file_name".to_string());
                    }
                    schema_def["fields"] = serde_json::json!(field_names);
                }
                // Also ensure source_file_name has a classification
                if let Some(fc) = schema_def.get_mut("field_classifications").and_then(|f| f.as_object_mut()) {
                    fc.entry("source_file_name").or_insert_with(|| serde_json::json!(["word"]));
                }
                if let Some(ref desc) = request.image_descriptive_name {
                    schema_def["descriptive_name"] = serde_json::json!(desc);
                }
            }
            // Ensure mutation_mappers include source_file_name so it gets written
            // during mutation execution (the enriched JSON has this field).
            ai_response
                .mutation_mappers
                .entry("source_file_name".to_string())
                .or_insert_with(|| "source_file_name".to_string());
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
    async fn generate_flat_mutations(
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
            let fields_and_values: HashMap<String, Value> =
                obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

            let keys_and_values = extract_key_values_from_data(
                &fields_and_values,
                schema_name,
                &schema_manager,
            )
            .await?;

            let item_mutations = mutation_generator::generate_mutations(
                schema_name,
                &keys_and_values,
                &fields_and_values,
                &ai_response.mutation_mappers,
                pub_key.to_string(),
                request.source_file_name.clone(),
                metadata.clone(),
            )?;

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
        let schemas_written = schemas_written_from(&mutations);

        Ok((mutations, schemas_written))
    }

    /// Builds metadata HashMap from file_hash and progress_id for mutations.
    fn build_ingestion_metadata(
        file_hash: &Option<String>,
        progress_id: &str,
    ) -> Option<HashMap<String, String>> {
        let mut meta = HashMap::new();
        if let Some(ref hash) = file_hash {
            meta.insert("file_hash".to_string(), hash.clone());
        }
        meta.insert("progress_id".to_string(), progress_id.to_string());
        Some(meta)
    }

    /// Records a file as ingested for per-user file-level dedup.
    async fn record_file_ingested(
        &self,
        request: &IngestionRequest,
        node: &FoldNode,
    ) {
        if let Some(ref fh) = request.file_hash {
            let record = FileIngestionRecord {
                ingested_at: chrono::Utc::now().to_rfc3339(),
                source_folder: request.source_folder.clone(),
                source_file_name: request.source_file_name.clone(),
                progress_id: request.progress_id.clone(),
            };
            if let Err(e) = node.mark_file_ingested(&request.pub_key, fh, record).await {
                log_feature!(LogFeature::Ingestion, warn, "Failed to record file dedup entry: {}", e);
            }
        }
    }

    /// Call the underlying AI API with a raw prompt string.
    ///
    /// This is the low-level API used by smart_folder scanning and other
    /// components that need raw AI text completion without schema parsing.
    pub async fn call_ai_raw(&self, prompt: &str) -> IngestionResult<String> {
        let detail = self.init_error.as_deref().unwrap_or("unknown reason");
        self.backend
            .as_ref()
            .ok_or_else(|| IngestionError::configuration_error(
                format!("{:?} backend not initialized ({})", self.config.provider, detail),
            ))?
            .call(prompt)
            .await
    }

    /// Get AI schema recommendation with validation retries.
    ///
    /// Builds the prompt once, then retries the AI call if response parsing fails
    /// (e.g., malformed JSON, missing required fields). Network-level retries are
    /// handled separately inside `call_ai_raw`.
    pub(super) async fn get_ai_recommendation(
        &self,
        json_data: &Value,
    ) -> IngestionResult<AISchemaResponse> {
        use crate::ingestion::ai_helpers::{analyze_and_build_prompt, parse_ai_response};

        let prompt = analyze_and_build_prompt(json_data)?;
        let max_validation_attempts = self.config.max_retries.clamp(1, 3);
        let mut last_error = None;

        for attempt in 1..=max_validation_attempts {
            let raw_response = self.call_ai_raw(&prompt).await?;

            match parse_ai_response(&raw_response) {
                Ok(response) => return Ok(response),
                Err(e) => {
                    log_feature!(
                        LogFeature::Ingestion,
                        warn,
                        "AI response validation failed on attempt {}/{}: {}",
                        attempt,
                        max_validation_attempts,
                        e
                    );
                    last_error = Some(e);

                    if attempt < max_validation_attempts {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            IngestionError::ai_response_validation_error(
                "All AI attempts returned invalid responses",
            )
        }))
    }

    /// Second AI pass: fill in missing field_descriptions on the schema.
    ///
    /// If the initial schema proposal is missing field_descriptions, this method
    /// calls the AI with a focused prompt that only asks for descriptions, given
    /// the JSON data and field names. This is more reliable than expecting the
    /// schema proposal prompt to always produce descriptions.
    pub(super) async fn fill_missing_field_descriptions(
        &self,
        ai_response: &mut AISchemaResponse,
        json_data: &Value,
    ) -> IngestionResult<()> {
        let schema_def = match ai_response.new_schemas.as_mut() {
            Some(def) => def,
            None => return Ok(()),
        };

        let fields: Vec<String> = schema_def
            .get("fields")
            .and_then(|f| f.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect())
            .unwrap_or_default();

        if fields.is_empty() {
            return Ok(());
        }

        let existing_descriptions = schema_def
            .get("field_descriptions")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let missing: Vec<&String> = fields
            .iter()
            .filter(|f| !existing_descriptions.contains_key(f.as_str()))
            .collect();

        if missing.is_empty() {
            return Ok(());
        }

        log_feature!(
            LogFeature::Ingestion,
            info,
            "Schema missing field_descriptions for {:?}, calling AI for descriptions",
            missing
        );

        // Build a compact sample for the prompt
        let sample = if let Some(array) = json_data.as_array() {
            serde_json::json!(array.iter().take(2).collect::<Vec<_>>())
        } else {
            json_data.clone()
        };

        let prompt = crate::ingestion::prompts::FIELD_DESCRIPTIONS_PROMPT
            .replace("{sample}", &serde_json::to_string_pretty(&sample).unwrap_or_default())
            .replace("{fields}", &format!("{:?}", missing));

        match self.call_ai_raw(&prompt).await {
            Ok(raw_response) => {
                match crate::ingestion::ai_helpers::extract_json_from_response(&raw_response) {
                    Ok(json_str) => {
                        if let Ok(descriptions) = serde_json::from_str::<serde_json::Map<String, Value>>(&json_str) {
                            let fd = schema_def
                                .as_object_mut()
                                .unwrap()
                                .entry("field_descriptions")
                                .or_insert_with(|| Value::Object(serde_json::Map::new()));
                            if let Some(fd_obj) = fd.as_object_mut() {
                                for (field, desc) in descriptions {
                                    if desc.is_string() {
                                        fd_obj.entry(&field).or_insert(desc);
                                    }
                                }
                            }
                            log_feature!(
                                LogFeature::Ingestion,
                                info,
                                "AI provided field descriptions for missing fields"
                            );
                        }
                    }
                    Err(e) => {
                        log_feature!(
                            LogFeature::Ingestion,
                            warn,
                            "Failed to parse field descriptions AI response: {}",
                            e
                        );
                    }
                }
            }
            Err(e) => {
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
                    "Failed to get field descriptions from AI: {}",
                    e
                );
            }
        }

        Ok(())
    }

    /// Validate JSON input
    pub fn validate_input(&self, data: &Value) -> IngestionResult<()> {
        if data.is_null() {
            return Err(IngestionError::invalid_input("Input data cannot be null"));
        }

        if !data.is_object() && !data.is_array() {
            return Err(IngestionError::invalid_input(
                "Input data must be a JSON object or array",
            ));
        }

        Ok(())
    }

    /// Returns `true` when the configured AI provider runs locally (e.g. Ollama)
    /// and therefore incurs no per-token cost.
    pub fn is_local_provider(&self) -> bool {
        matches!(self.config.provider, AIProvider::Ollama)
    }

    /// Returns the provider name as a string.
    pub fn provider_name(&self) -> &str {
        match self.config.provider {
            AIProvider::Ollama => "Ollama",
            AIProvider::Anthropic => "Anthropic",
        }
    }

    /// Get status information
    pub fn get_status(&self) -> IngestionResult<IngestionStatus> {
        let (provider_name, model) = match self.config.provider {
            AIProvider::Ollama => ("Ollama".to_string(), self.config.ollama.model.clone()),
            AIProvider::Anthropic => ("Anthropic".to_string(), self.config.anthropic.model.clone()),
        };

        Ok(IngestionStatus {
            enabled: self.config.enabled,
            configured: self.config.is_ready(),
            provider: provider_name,
            model,
            auto_execute_mutations: self.config.auto_execute_mutations,
        })
    }

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
    async fn create_new_schema_with_node(
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
                schema.field_descriptions.entry(field_name.clone())
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
                let needs_default = schema.field_classifications
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
                    schema.field_classifications.insert(field_name.clone(), default);
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
                    "Cannot create schema without at least one field for key configuration".to_string(),
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
            IngestionError::schema_parsing_error(format!(
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
                    if !url.starts_with("test://") && !url.starts_with("mock://") {
                        let client = crate::fold_node::SchemaServiceClient::new(&url);
                        match client.get_schema(old_name).await {
                            Ok(old_schema) => {
                                let old_json = serde_json::to_string(&old_schema)
                                    .map_err(schema_err)?;
                                if let Err(e) = schema_manager.load_schema_from_json(&old_json).await {
                                    log_feature!(LogFeature::Ingestion, warn, "Failed to load old schema '{}' from service: {}", old_name, e);
                                }
                            }
                            Err(e) => {
                                log_feature!(LogFeature::Ingestion, warn, "Failed to fetch old schema '{}' from service: {}", old_name, e);
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
                LogFeature::Ingestion, info,
                "Schema expansion: blocking old schema '{}', loaded expanded '{}'",
                old_name,
                schema_response.name
            );
            if let Err(e) = schema_manager.block_and_supersede(old_name, &schema_response.name).await {
                log_feature!(LogFeature::Ingestion, warn, "Failed to block old schema '{}' during expansion: {}", old_name, e);
            }
        }

        let schema_name = schema_response.name.clone();
        let service_mappers = add_response.mutation_mappers.clone();
        drop(_lock);

        Ok((schema_name, service_mappers))
    }

    /// Execute mutations with progress tracking
    async fn execute_mutations_with_node_and_progress(
        &self,
        mutations: Vec<Mutation>,
        node: &FoldNode,
        progress_service: &ProgressService,
        progress_id: &str,
    ) -> IngestionResult<usize> {
        if mutations.is_empty() {
            return Ok(0);
        }

        let total_mutations = mutations.len();

        progress_service
            .update_progress_with_percentage(
                progress_id,
                IngestionStep::ExecutingMutations,
                format!("Submitting {} mutations...", total_mutations),
                90,
            )
            .await;

        // Execute all mutations in a batch using FoldNode directly
        // mutate_batch runs the MutationPreprocessor (keyword extraction) then writes
        let result = node.mutate_batch(mutations)
            .await
            .map(|mutation_ids| mutation_ids.len())
            .map_err(|e| {
                IngestionError::SchemaSystemError(fold_db::schema::SchemaError::InvalidData(
                    e.to_string(),
                ))
            });

        if let Ok(count) = &result {
            progress_service
                .update_progress_with_percentage(
                    progress_id,
                    IngestionStep::ExecutingMutations,
                    format!("Completed {} mutations", count),
                    95,
                )
                .await;
        }

        result
    }
}

/// Enrich text-file JSON with source path context so the AI sees the category
/// and full source path instead of just the bare filename. This allows it to
/// propose semantic schema names like "recipes" instead of "document_content".
///
/// For text files wrapped by `wrap_text_content`, the `source_file` field only
/// has the filename (e.g., "grandmas_cookies.txt"). The `source_file_name` from
/// the `IngestionRequest` has the full relative path (e.g., "recipes/grandmas_cookies.txt").
fn enrich_with_source_context(data: &mut Value, source_file_name: Option<&str>) {
    let source_path = match source_file_name {
        Some(s) if !s.is_empty() => s,
        _ => return,
    };

    // Derive category from parent directory (e.g., "recipes/cookies.txt" → "recipes")
    let category = std::path::Path::new(source_path)
        .parent()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty() && s != "." && s != "..");

    let enrich_obj = |obj: &mut serde_json::Map<String, Value>| {
        // Only enrich objects that look like text-file wrappers (have "content" and "file_type")
        if !obj.contains_key("content") || !obj.contains_key("file_type") {
            return;
        }
        // Update source_file to include the full path
        obj.insert("source_file".to_string(), Value::String(source_path.to_string()));
        // Add category hint from the directory name
        if let Some(ref cat) = category {
            obj.entry("category").or_insert_with(|| Value::String(cat.clone()));
        }
    };

    match data {
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                if let Some(obj) = item.as_object_mut() {
                    enrich_obj(obj);
                }
            }
        }
        Value::Object(obj) => enrich_obj(obj),
        _ => {}
    }
}

/// Extract a `SchemaLookupEntry` from an AI response for batch reuse checking.
fn extract_lookup_entry(ai_response: &AISchemaResponse) -> Option<SchemaLookupEntry> {
    let schema_def = ai_response.new_schemas.as_ref()?;
    let descriptive_name = schema_def
        .get("descriptive_name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())?;
    let fields: Vec<String> = schema_def
        .get("fields")
        .and_then(|v| v.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        .or_else(|| {
            schema_def
                .get("field_classifications")
                .and_then(|v| v.as_object())
                .map(|obj| obj.keys().cloned().collect())
        })?;
    Some(SchemaLookupEntry {
        descriptive_name,
        fields,
    })
}

/// Build a `Vec<SchemaWriteRecord>` from a mutations slice, deduplicating keys.
fn schemas_written_from(mutations: &[Mutation]) -> Vec<SchemaWriteRecord> {
    let mut map: HashMap<String, Vec<KeyValue>> = HashMap::new();
    for m in mutations {
        let keys = map.entry(m.schema_name.clone()).or_default();
        if !keys.contains(&m.key_value) {
            keys.push(m.key_value.clone());
        }
    }
    schemas_written_from_map(map)
}

/// Convert a `HashMap<schema_name, keys>` into `Vec<SchemaWriteRecord>`.
fn schemas_written_from_map(map: HashMap<String, Vec<KeyValue>>) -> Vec<SchemaWriteRecord> {
    map.into_iter()
        .map(|(name, keys)| SchemaWriteRecord { schema_name: name, keys_written: keys })
        .collect()
}
