//! AI-powered ingestion service that works with FoldNode
//!
//! Handles JSON data ingestion with AI schema recommendation, mutation generation,
//! and execution. Refactored to take &FoldNode references for flexible locking.

mod ai_methods;
mod decomposition;
mod flat_path;
mod schema_creation;

use crate::fold_node::{FileIngestionRecord, FoldNode};
use crate::ingestion::ai::client::{build_backend, AiBackend};
use crate::ingestion::config::AIProvider;
use crate::ingestion::decomposer;
use crate::ingestion::progress::{
    IngestionPhase, IngestionResults, PhaseTracker, ProgressService, SchemaWriteRecord,
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
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;

use crate::schema_service::types::{BatchSchemaReuseResponse, SchemaLookupEntry};
use decomposition::{AiProposal, CachedSchema, SchemaCache};

/// Shorthand to wrap any `Display` error as a `SchemaCreationError`.
pub(crate) fn schema_err(e: impl std::fmt::Display) -> IngestionError {
    IngestionError::SchemaCreationError(e.to_string())
}

/// Apply image-specific overrides to an AI schema response.
///
/// Images use `Hash(source_file_name)` so each image file gets a unique key.
/// This function:
/// 1. Sets `schema_type` to `Hash` and configures the key field
/// 2. Ensures `source_file_name` is in the schema's `fields` array and `field_classifications`
/// 3. Optionally sets a custom `descriptive_name`
/// 4. Adds `source_file_name` to `mutation_mappers` so the value gets written
pub(crate) fn apply_image_schema_override(
    ai_response: &mut AISchemaResponse,
    descriptive_name: Option<&str>,
) {
    if let Some(ref mut schema_def) = ai_response.new_schemas {
        schema_def["schema_type"] = serde_json::json!("Hash");
        schema_def["key"] = serde_json::json!({
            "hash_field": "source_file_name"
        });
        // Ensure source_file_name is in the fields list
        if let Some(fields) = schema_def.get_mut("fields").and_then(|f| f.as_array_mut()) {
            let sfn = serde_json::json!("source_file_name");
            if !fields.contains(&sfn) {
                fields.push(sfn);
            }
        } else {
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
        if let Some(fc) = schema_def
            .get_mut("field_classifications")
            .and_then(|f| f.as_object_mut())
        {
            fc.entry("source_file_name")
                .or_insert_with(|| serde_json::json!(["word"]));
            fc.entry("visibility")
                .or_insert_with(|| serde_json::json!(["word"]));
        }
        // Ensure visibility is in the fields list
        if let Some(fields) = schema_def.get_mut("fields").and_then(|f| f.as_array_mut()) {
            let vis = serde_json::json!("visibility");
            if !fields.contains(&vis) {
                fields.push(vis);
            }
        }
        // Ensure visibility has a field description
        if let Some(fd) = schema_def
            .get_mut("field_descriptions")
            .and_then(|f| f.as_object_mut())
        {
            fd.entry("visibility").or_insert_with(|| {
                serde_json::json!(
                    "AI-classified photo visibility: public (suitable for social feed) or private (sensitive content)"
                )
            });
        }
        if let Some(desc) = descriptive_name {
            schema_def["descriptive_name"] = serde_json::json!(desc);
        }
    }
    ai_response
        .mutation_mappers
        .entry("source_file_name".to_string())
        .or_insert_with(|| "source_file_name".to_string());
    ai_response
        .mutation_mappers
        .entry("visibility".to_string())
        .or_insert_with(|| "visibility".to_string());
}

/// Acquire a clone of the SchemaCore from the node without holding the DB lock.
pub(crate) async fn get_schema_manager(node: &FoldNode) -> IngestionResult<Arc<SchemaCore>> {
    let db_guard = node.get_fold_db().await.map_err(schema_err)?;
    let manager = db_guard.schema_manager.clone();
    drop(db_guard);
    Ok(manager)
}

/// AI-powered ingestion service that works with FoldNode
pub struct IngestionService {
    pub(super) config: IngestionConfig,
    pub(super) backend: Option<Arc<dyn AiBackend>>,
    /// Stores the reason if the configured provider failed to initialise.
    pub(super) init_error: Option<String>,
    /// Serializes schema creation to prevent race conditions when multiple
    /// files are ingested concurrently. The AI call (slow) happens outside
    /// the lock; only the schema-service call + local load + block is locked.
    pub(super) schema_creation_lock: tokio::sync::Mutex<()>,
    /// Shared cross-file cache backing store. Per-call `SchemaCache` instances
    /// wrap this with a local scope and flush back on `commit()`.
    shared_schema_cache: Arc<std::sync::RwLock<HashMap<String, CachedSchema>>>,
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
            shared_schema_cache: Arc::new(std::sync::RwLock::new(HashMap::new())),
        })
    }

    /// Create a new per-call `SchemaCache` backed by the shared cross-file store.
    fn new_schema_cache(&self) -> SchemaCache {
        SchemaCache::new(self.shared_schema_cache.clone())
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

        let mut tracker = PhaseTracker::new(progress_service, progress_id);

        if !self.config.is_ready() {
            tracker
                .fail("Ingestion module is not properly configured or disabled".to_string())
                .await;
            return Ok(IngestionResponse::failure(vec![
                "Ingestion module is not properly configured or disabled".to_string(),
            ]));
        }

        // Phase 1: Validate input
        tracker
            .enter_phase(
                IngestionPhase::Validating,
                "Validating input data...".to_string(),
            )
            .await;
        self.validate_input(&request.data)?;

        // Phase 2: Flatten data structure for AI analysis
        tracker
            .enter_phase(
                IngestionPhase::Flattening,
                "Processing and flattening data structure...".to_string(),
            )
            .await;
        let mut flattened_data =
            crate::ingestion::file_handling::json_processor::flatten_root_layers(
                request.data.clone(),
            );

        // Enrich text-file JSON with source path context so the AI can propose
        // semantic schema names (e.g., "recipes" instead of "document_content").
        enrich_with_source_context(&mut flattened_data, request.source_file_name.as_deref());

        // Inject content_hash for document/note data to prevent key collisions
        // when multiple items share the same title (e.g., dated journal entries).
        inject_content_hashes(&mut flattened_data);

        // Decide code path based on data shape
        let has_nested_children = self.check_has_nested_children(&flattened_data);

        // Phases 3-6: Delegate to path-specific handler
        let (
            schema_name,
            new_schema_created,
            mutations_generated,
            mutations_executed,
            schemas_written,
        ) = if has_nested_children {
            tracker
                .enter_phase(
                    IngestionPhase::AIRecommendation,
                    "Decomposing nested data structures...".to_string(),
                )
                .await;
            self.process_decomposed_path(&flattened_data, &request, node, &tracker)
                .await?
        } else {
            self.process_flat_path(&flattened_data, &request, node, &mut tracker)
                .await?
        };

        // Finalize: record file dedup + complete progress
        self.record_file_ingested(&request, node).await;

        let results = IngestionResults {
            schema_name: schema_name.clone(),
            new_schema_created,
            mutations_generated,
            mutations_executed,
            schemas_written: schemas_written.clone(),
        };
        tracker.complete(results).await;

        Ok(IngestionResponse::success_with_progress(
            tracker.progress_id().to_string(),
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
        tracker: &PhaseTracker<'_>,
    ) -> IngestionResult<(String, bool, usize, usize, Vec<SchemaWriteRecord>)> {
        let pub_key = request.pub_key.clone();
        let mut schema_cache = self.new_schema_cache();
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
        let rep = representative
            .as_ref()
            .expect("representative is Some when has_nested_children is true");
        let top_level_hash = crate::ingestion::decomposer::compute_structure_hash(rep);

        // AI proposal collection (part of the AIRecommendation phase, already entered by caller)
        tracker
            .sub_progress(
                0.5,
                "Collecting AI proposals for nested structures...".to_string(),
            )
            .await;
        let mut proposals: HashMap<String, AiProposal> = HashMap::new();
        self.collect_ai_proposals_recursive(
            &top_level_hash,
            rep,
            &mut proposals,
            &schema_cache,
            0,
            request.source_file_name.as_deref(),
        )
        .await?;

        // Schema resolution phase: batch check reuse then resolve
        tracker
            .sub_progress(1.0, "Batch-checking schema reuse...".to_string())
            .await;
        let entries: Vec<SchemaLookupEntry> = proposals
            .values()
            .filter_map(|p| extract_lookup_entry(&p.ai_response))
            .collect();
        let batch_result = node
            .batch_check_schema_reuse(&entries)
            .await
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

        // Resolve schemas using batch results (reuse fast path or create slow path)
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

        // Flush resolved schemas to the shared cross-file cache
        schema_cache.commit();

        // Mutation generation + execution for each item
        let metadata = Self::build_ingestion_metadata(&request.file_hash, tracker.progress_id());

        for (idx, item) in items.iter().enumerate() {
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

            // Report progress at the item level
            if items.len() > 1 {
                let fraction = (idx + 1) as f32 / items.len() as f32;
                tracker
                    .sub_progress(
                        fraction,
                        format!("Processing item {}/{}", idx + 1, items.len()),
                    )
                    .await;
            }
        }

        // Determine the top-level schema name for the response
        let top_schema_name = schema_cache
            .get(&top_level_hash)
            .map(|c| c.schema_name)
            .unwrap_or_else(|| {
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
                    "Schema cache miss for top-level hash '{}' — returning empty schema name",
                    top_level_hash
                );
                String::new()
            });

        let schemas_written = schemas_written_from_map(schemas_written_map);

        Ok((
            top_schema_name,
            true,
            total_mutations_generated,
            total_mutations_executed,
            schemas_written,
        ))
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
    async fn record_file_ingested(&self, request: &IngestionRequest, node: &FoldNode) {
        if let Some(ref fh) = request.file_hash {
            let record = FileIngestionRecord {
                ingested_at: chrono::Utc::now().to_rfc3339(),
                source_folder: request.source_folder.clone(),
                source_file_name: request.source_file_name.clone(),
                progress_id: request.progress_id.clone(),
            };
            if let Err(e) = node.mark_file_ingested(&request.pub_key, fh, record).await {
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
                    "Failed to record file dedup entry: {}",
                    e
                );
            }
        }
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

    /// Execute mutations with progress tracking via PhaseTracker.
    async fn execute_mutations_with_tracking(
        &self,
        mutations: Vec<Mutation>,
        node: &FoldNode,
        tracker: &PhaseTracker<'_>,
    ) -> IngestionResult<usize> {
        if mutations.is_empty() {
            return Ok(0);
        }

        let total_mutations = mutations.len();
        tracker
            .sub_progress(0.0, format!("Submitting {} mutations...", total_mutations))
            .await;

        // Execute all mutations in a batch using FoldNode directly
        // mutate_batch runs the MutationPreprocessor (keyword extraction) then writes
        let result = node
            .mutate_batch(mutations)
            .await
            .map(|mutation_ids| mutation_ids.len())
            .map_err(|e| {
                IngestionError::SchemaSystemError(fold_db::schema::SchemaError::InvalidData(
                    e.to_string(),
                ))
            });

        if let Ok(count) = &result {
            tracker
                .sub_progress(1.0, format!("Completed {} mutations", count))
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
        // Enrich objects from native parsers: text-file wrappers (content + file_type)
        // and code metadata (source_file + file_type, with functions/classes/comments).
        if !obj.contains_key("file_type") {
            return;
        }
        let is_text_wrapper = obj.contains_key("content");
        let is_code_metadata = obj.contains_key("source_file");
        if !is_text_wrapper && !is_code_metadata {
            return;
        }
        // Update source_file to use the original filename
        obj.insert(
            "source_file".to_string(),
            Value::String(source_path.to_string()),
        );
        // Add category hint from the directory name
        if let Some(ref cat) = category {
            obj.entry("category")
                .or_insert_with(|| Value::String(cat.clone()));
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
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
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

/// Generate mutations for a single JSON object: extract keys then build mutations.
///
/// Shared by both the flat and decomposed ingestion paths.
pub(crate) async fn generate_mutations_for_item(
    obj: &serde_json::Map<String, Value>,
    schema_name: &str,
    mutation_mappers: &HashMap<String, String>,
    schema_manager: &Arc<SchemaCore>,
    pub_key: &str,
    source_file_name: Option<String>,
    metadata: Option<HashMap<String, String>>,
) -> IngestionResult<Vec<Mutation>> {
    use crate::ingestion::key_extraction::extract_key_values_from_data;
    use crate::ingestion::mutation_generator;

    // Flatten nested objects into dot-notation keys so that mapper paths like
    // "budget_breakdown.flights" resolve correctly. Without this, only top-level
    // scalar fields are preserved and nested objects are silently dropped.
    let fields_and_values: HashMap<String, Value> = mutation_generator::flatten_json_object(obj);

    let keys_and_values =
        extract_key_values_from_data(&fields_and_values, schema_name, schema_manager).await?;

    mutation_generator::generate_mutations(
        schema_name,
        &keys_and_values,
        &fields_and_values,
        mutation_mappers,
        pub_key.to_string(),
        source_file_name,
        metadata,
    )
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

/// Inject a `content_hash` field into objects to prevent key collisions.
///
/// For objects with a `content` or `body` field, hashes that text.
/// For all other objects, hashes all field values so that structurally
/// distinct records (e.g., two flights on the same date with different
/// flight_ids) get unique disambiguation keys.
fn inject_content_hash(obj: &mut serde_json::Map<String, Value>) {
    // Prefer content/body text when available (original behavior)
    let content_str = obj
        .get("content")
        .or_else(|| obj.get("body"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let hash_input = if let Some(text) = content_str {
        text
    } else {
        // Hash all field values for structured data without content/body
        let mut sorted_keys: Vec<&String> = obj.keys().collect();
        sorted_keys.sort();
        let mut parts = String::new();
        for k in sorted_keys {
            parts.push_str(k);
            parts.push(':');
            parts.push_str(&obj[k].to_string());
            parts.push('\n');
        }
        parts
    };

    let mut hasher = Sha256::new();
    hasher.update(hash_input.as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    obj.insert(
        "content_hash".to_string(),
        Value::String(hash[..16].to_string()),
    );
}

/// Apply `inject_content_hash` to every object in an array or a single object.
fn inject_content_hashes(data: &mut Value) {
    match data {
        Value::Array(arr) => {
            for item in arr.iter_mut() {
                if let Some(obj) = item.as_object_mut() {
                    inject_content_hash(obj);
                }
            }
        }
        Value::Object(obj) => {
            inject_content_hash(obj);
        }
        _ => {}
    }
}

/// Convert a `HashMap<schema_name, keys>` into `Vec<SchemaWriteRecord>`.
fn schemas_written_from_map(map: HashMap<String, Vec<KeyValue>>) -> Vec<SchemaWriteRecord> {
    map.into_iter()
        .map(|(name, keys)| SchemaWriteRecord {
            schema_name: name,
            keys_written: keys,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_apply_image_schema_override_includes_visibility() {
        let mut response = AISchemaResponse {
            new_schemas: Some(json!({
                "name": "social_photos",
                "fields": ["markdown", "image_type"],
                "field_classifications": {"markdown": ["word"], "image_type": ["word"]},
                "field_descriptions": {"markdown": "description", "image_type": "type"}
            })),
            mutation_mappers: HashMap::new(),
        };
        apply_image_schema_override(&mut response, Some("Photos"));

        let schema = response.new_schemas.unwrap();
        let fields = schema["fields"].as_array().unwrap();
        assert!(
            fields.contains(&json!("visibility")),
            "visibility must be in fields"
        );
        assert!(
            fields.contains(&json!("source_file_name")),
            "source_file_name must be in fields"
        );
        assert!(
            schema["field_classifications"]["visibility"].is_array(),
            "visibility must have field_classifications"
        );
        assert!(
            schema["field_descriptions"]["visibility"].is_string(),
            "visibility must have field_descriptions"
        );
        assert!(
            response.mutation_mappers.contains_key("visibility"),
            "visibility must be in mutation_mappers"
        );
        assert!(
            response.mutation_mappers.contains_key("source_file_name"),
            "source_file_name must be in mutation_mappers"
        );
    }

    #[test]
    fn test_inject_content_hash_with_content_field() {
        let mut obj = serde_json::Map::new();
        obj.insert("title".to_string(), json!("Note"));
        obj.insert("content".to_string(), json!("Hello world"));
        inject_content_hash(&mut obj);
        assert!(obj.contains_key("content_hash"));
        let hash = obj["content_hash"].as_str().unwrap();
        assert_eq!(hash.len(), 16);
    }

    #[test]
    fn test_inject_content_hash_without_content_field() {
        // Structured data like flights should still get a content_hash
        let mut obj = serde_json::Map::new();
        obj.insert("flight_id".to_string(), json!("AA100"));
        obj.insert("airline".to_string(), json!("American Airlines"));
        obj.insert("departure_date".to_string(), json!("2026-04-01"));
        inject_content_hash(&mut obj);
        assert!(
            obj.contains_key("content_hash"),
            "Structured data without content/body should still get a content_hash"
        );
        let hash = obj["content_hash"].as_str().unwrap();
        assert_eq!(hash.len(), 16);
    }

    #[test]
    fn test_inject_content_hash_different_data_produces_different_hashes() {
        let mut obj1 = serde_json::Map::new();
        obj1.insert("flight_id".to_string(), json!("AA100"));
        obj1.insert("departure_date".to_string(), json!("2026-04-01"));

        let mut obj2 = serde_json::Map::new();
        obj2.insert("flight_id".to_string(), json!("AA200"));
        obj2.insert("departure_date".to_string(), json!("2026-04-01"));

        inject_content_hash(&mut obj1);
        inject_content_hash(&mut obj2);

        assert_ne!(
            obj1["content_hash"].as_str().unwrap(),
            obj2["content_hash"].as_str().unwrap(),
            "Different records should produce different content hashes"
        );
    }
}
