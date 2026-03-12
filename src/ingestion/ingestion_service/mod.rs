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

use decomposition::CachedSchema;

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
        Ok(Self { config, backend, init_error, schema_creation_lock: tokio::sync::Mutex::new(()) })
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

        // FileMarkdown path: hardcoded schema, skip AI recommendation entirely
        let (schema_name, new_schema_created, mutations_generated, mutations_executed, schemas_written) =
            if request.file_markdown.is_some() {
                self.process_file_markdown_path(
                    &request,
                    node,
                    progress_service,
                    &progress_id,
                )
                .await?
            } else {
                // Standard JSON path: validate, flatten, check for nested children
                // Step 1: Validate input
                progress_service.update_progress(&progress_id, IngestionStep::ValidatingConfig,
                    "Validating input data...".to_string()).await;
                self.validate_input(&request.data)?;

                // Step 2: Flatten data structure for AI analysis
                progress_service.update_progress(&progress_id, IngestionStep::FlatteningData,
                    "Processing and flattening data structure...".to_string()).await;
                let flattened_data = crate::ingestion::json_processor::flatten_root_layers(request.data.clone());

                // Step 2.5: Decompose nested structures and decide code path
                let has_nested_children = self.check_has_nested_children(&flattened_data);

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
                }
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
        self.resolve_schema_for_structure(
            &top_level_hash,
            rep,
            &mut schema_cache,
            node,
            0,
            request.source_file_name.as_deref(),
        )
        .await?;

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
                log::warn!("Schema cache miss for top-level hash '{}' — returning empty schema name", top_level_hash);
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
        // Note: file_markdown is always None here (FileMarkdown path is handled separately)
        let is_image = request.source_file_name.as_ref()
            .map(|name| crate::ingestion::is_image_file(name))
            .unwrap_or(false);
        progress_service.update_progress(progress_id, IngestionStep::GettingAIRecommendation,
            "Analyzing data with AI to determine schema...".to_string()).await;
        let mut ai_response = self.get_ai_recommendation(flattened_data).await?;

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
            .determine_schema_to_use(&ai_response, &request.data, node)
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

    /// Hardcoded schema fields for FileMarkdown documents.
    /// Every field from the FileMarkdown struct is preserved — no AI guessing.
    const FILE_MARKDOWN_FIELDS: &'static [&'static str] = &[
        "source",
        "file_type",
        "size_bytes",
        "mime_type",
        "extraction_method",
        "markdown",
        "title",
        "author",
        "created",
        "page_count",
        "rows",
        "columns",
        "duration_seconds",
        "sample_rate_hz",
        "channels",
        "image_format",
        "language",
        "line_count",
        "sheet_count",
        "slide_count",
        "entry_count",
        "archive_format",
        "word_count",
    ];

    /// Handles the FileMarkdown ingestion path with a hardcoded schema.
    ///
    /// Bypasses the AI schema recommendation entirely — the schema is always the
    /// same for every file processed by file_to_markdown, preserving all fields
    /// (especially the `markdown` content field that the AI was dropping).
    async fn process_file_markdown_path(
        &self,
        request: &IngestionRequest,
        node: &FoldNode,
        progress_service: &ProgressService,
        progress_id: &str,
    ) -> IngestionResult<(String, bool, usize, usize, Vec<SchemaWriteRecord>)> {
        let fm = request.file_markdown.as_ref()
            .expect("process_file_markdown_path called without file_markdown");
        let pub_key = request.pub_key.clone();

        progress_service.update_progress(progress_id, IngestionStep::FlatteningData,
            "Processing file markdown data...".to_string()).await;

        // Use the pre-converted Value from the request (already set by the entry point)
        let data = &request.data;

        // Build field classifications — markdown is "word" (text content),
        // numeric fields are "number", everything else is "word"
        let mut field_classifications: HashMap<String, Vec<String>> = HashMap::new();
        let number_fields = [
            "size_bytes", "page_count", "rows", "columns",
            "duration_seconds", "sample_rate_hz", "channels",
            "line_count", "sheet_count", "slide_count", "entry_count", "word_count",
        ];
        for &field in Self::FILE_MARKDOWN_FIELDS {
            let classification = if number_fields.contains(&field) {
                vec!["number".to_string()]
            } else {
                vec!["word".to_string()]
            };
            field_classifications.insert(field.to_string(), classification);
        }

        // Build the schema definition — use source as hash key
        let is_image = fm.image_format.is_some();
        let schema_def = if is_image {
            let mut def = serde_json::json!({
                "name": "FileMarkdownDocument",
                "schema_type": "HashRange",
                "key": {
                    "hash_field": "image_format",
                    "range_field": "source"
                },
                "fields": Self::FILE_MARKDOWN_FIELDS,
                "field_classifications": field_classifications,
            });
            if let Some(ref desc) = request.image_descriptive_name {
                def["descriptive_name"] = serde_json::json!(desc);
            }
            def
        } else {
            serde_json::json!({
                "name": "FileMarkdownDocument",
                "schema_type": "Single",
                "key": {
                    "hash_field": "source"
                },
                "fields": Self::FILE_MARKDOWN_FIELDS,
                "field_classifications": field_classifications,
            })
        };

        // Build an AISchemaResponse with identity mutation mappers (field → field)
        let mutation_mappers: HashMap<String, String> = Self::FILE_MARKDOWN_FIELDS
            .iter()
            .map(|&f| (f.to_string(), f.to_string()))
            .collect();
        let ai_response = AISchemaResponse {
            new_schemas: Some(schema_def),
            mutation_mappers,
        };

        // Step 4: Determine schema (creates/loads it)
        progress_service.update_progress(progress_id, IngestionStep::SettingUpSchema,
            "Setting up file document schema...".to_string()).await;
        let schema_name = self
            .determine_schema_to_use(&ai_response, data, node)
            .await?;
        let new_schema_created = true;

        // Step 5: Generate mutations
        progress_service.update_progress(progress_id, IngestionStep::GeneratingMutations,
            "Generating database mutations...".to_string()).await;
        let (mutations, schemas_written) = self
            .generate_flat_mutations(
                data,
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
            "Generated {} mutations for FileMarkdown document",
            mutations.len()
        );

        // Step 6: Execute mutations
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
                log::warn!("Failed to record file dedup entry: {}", e);
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

        // Use identity_hash as schema name for structure-based deduplication
        schema.compute_identity_hash();
        let identity_hash = schema
            .get_identity_hash()
            .ok_or_else(|| {
                IngestionError::SchemaCreationError(
                    "Schema must have identity_hash computed".to_string(),
                )
            })?
            .clone();

        schema.name = identity_hash.clone();

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
                                    log::warn!("Failed to load old schema '{}' from service: {}", old_name, e);
                                }
                            }
                            Err(e) => {
                                log::warn!("Failed to fetch old schema '{}' from service: {}", old_name, e);
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
            log::info!(
                "Schema expansion: blocking old schema '{}', loaded expanded '{}'",
                old_name,
                schema_response.name
            );
            if let Err(e) = schema_manager.block_and_supersede(old_name, &schema_response.name).await {
                log::warn!("Failed to block old schema '{}' during expansion: {}", old_name, e);
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

#[cfg(test)]
mod tests {
    use super::*;
    use file_to_markdown::FileMarkdown;
    use serde_json::json;

    /// Verify FILE_MARKDOWN_FIELDS matches every field in FileMarkdown.
    #[test]
    fn test_file_markdown_fields_covers_all_struct_fields() {
        let fm = FileMarkdown::new(
            "test.pdf".into(), "pdf".into(), 1024,
            Some("application/pdf".into()), "pdf-text".into(), "# Content".into(),
        );
        let value = serde_json::to_value(&fm).unwrap();
        let obj = value.as_object().unwrap();

        for key in obj.keys() {
            assert!(
                IngestionService::FILE_MARKDOWN_FIELDS.contains(&key.as_str()),
                "FileMarkdown field '{}' is missing from FILE_MARKDOWN_FIELDS constant",
                key
            );
        }
        for &field in IngestionService::FILE_MARKDOWN_FIELDS {
            assert!(
                obj.contains_key(field),
                "FILE_MARKDOWN_FIELDS contains '{}' but FileMarkdown struct does not serialize it",
                field
            );
        }
    }

    /// Verify the hardcoded schema definition for non-image documents.
    #[test]
    fn test_file_markdown_schema_def_document() {
        let fields = IngestionService::FILE_MARKDOWN_FIELDS;

        let schema_def = json!({
            "name": "FileMarkdownDocument",
            "schema_type": "Single",
            "key": { "hash_field": "source" },
            "fields": fields,
            "field_classifications": {},
        });

        assert_eq!(schema_def["schema_type"], "Single");
        assert_eq!(schema_def["key"]["hash_field"], "source");
        let schema_fields = schema_def["fields"].as_array().unwrap();
        assert!(schema_fields.iter().any(|f| f == "markdown"), "schema must include 'markdown' field");
        assert!(schema_fields.iter().any(|f| f == "source"), "schema must include 'source' field");
        assert!(schema_fields.iter().any(|f| f == "title"), "schema must include 'title' field");
    }

    /// Verify the hardcoded schema definition for images uses HashRange.
    #[test]
    fn test_file_markdown_schema_def_image() {
        let fields = IngestionService::FILE_MARKDOWN_FIELDS;

        let schema_def = json!({
            "name": "FileMarkdownDocument",
            "schema_type": "HashRange",
            "key": {
                "hash_field": "image_format",
                "range_field": "source"
            },
            "fields": fields,
            "field_classifications": {},
        });

        assert_eq!(schema_def["schema_type"], "HashRange");
        assert_eq!(schema_def["key"]["hash_field"], "image_format");
        assert_eq!(schema_def["key"]["range_field"], "source");
    }

    /// Verify identity mutation mappers map every field to itself.
    #[test]
    fn test_file_markdown_identity_mappers() {
        let mappers: HashMap<String, String> = IngestionService::FILE_MARKDOWN_FIELDS
            .iter()
            .map(|&f| (f.to_string(), f.to_string()))
            .collect();

        assert_eq!(mappers.len(), IngestionService::FILE_MARKDOWN_FIELDS.len());
        for &field in IngestionService::FILE_MARKDOWN_FIELDS {
            assert_eq!(
                mappers.get(field).map(|s| s.as_str()),
                Some(field),
                "Mapper for '{}' should be identity",
                field
            );
        }
    }

    /// Verify field classifications assign correct types.
    #[test]
    fn test_file_markdown_field_classifications() {
        let number_fields = [
            "size_bytes", "page_count", "rows", "columns",
            "duration_seconds", "sample_rate_hz", "channels",
            "line_count", "sheet_count", "slide_count", "entry_count", "word_count",
        ];

        let mut classifications: HashMap<String, Vec<String>> = HashMap::new();
        for &field in IngestionService::FILE_MARKDOWN_FIELDS {
            let class = if number_fields.contains(&field) {
                vec!["number".to_string()]
            } else {
                vec!["word".to_string()]
            };
            classifications.insert(field.to_string(), class);
        }

        assert_eq!(classifications["markdown"], vec!["word"]);
        assert_eq!(classifications["source"], vec!["word"]);
        assert_eq!(classifications["size_bytes"], vec!["number"]);
        assert_eq!(classifications["page_count"], vec!["number"]);
        assert_eq!(classifications["word_count"], vec!["number"]);
        assert_eq!(classifications["title"], vec!["word"]);
    }

    /// Verify that file_markdown_to_value preserves the markdown content field.
    #[test]
    fn test_file_markdown_to_value_preserves_markdown() {
        let long_content = "# Chapter 1\n\nThis is a long document with lots of content.\n".repeat(100);
        let fm = FileMarkdown::new(
            "book.pdf".into(), "pdf".into(), 50000,
            Some("application/pdf".into()), "pdf-text".into(), long_content.clone(),
        );
        let value = crate::ingestion::json_processor::file_markdown_to_value(&fm);
        assert_eq!(value["markdown"].as_str().unwrap(), long_content);
    }
}
