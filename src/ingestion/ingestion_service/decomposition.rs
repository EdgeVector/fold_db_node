//! Recursive decomposition ingestion for nested data structures.
//!
//! Handles ingestion of data with nested arrays-of-objects by recursively
//! decomposing, resolving schemas, and building parent-child references.

use crate::ingestion::decomposer;
use crate::ingestion::{AISchemaResponse, IngestionError, IngestionResult};
use crate::schema_service::types::BatchSchemaReuseResponse;
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::schema::types::KeyValue;
use serde_json::Value;
use std::collections::HashMap;

use super::IngestionService;

/// Maximum recursion depth for decomposition to prevent unbounded nesting.
const MAX_DECOMPOSITION_DEPTH: usize = 10;

/// Cached result of AI schema determination for a given structure.
#[derive(Clone)]
pub(super) struct CachedSchema {
    pub(super) schema_name: String,
    pub(super) mutation_mappers: HashMap<String, String>,
}

/// Unified schema cache with cross-file persistence and per-call scoping.
///
/// Consolidates the service-level `structure_schema_cache` and per-call
/// `schema_cache` into a single type with clear lifecycle:
/// 1. `get()` checks local scope first, then shared cross-file cache
/// 2. `insert()` writes to local scope only (safe during partial resolution)
/// 3. `commit()` flushes local scope to shared cache (call after successful resolution)
pub(super) struct SchemaCache {
    shared: std::sync::Arc<std::sync::RwLock<HashMap<String, CachedSchema>>>,
    local: HashMap<String, CachedSchema>,
}

impl SchemaCache {
    /// Create a new cache backed by a shared cross-file store.
    pub(super) fn new(
        shared: std::sync::Arc<std::sync::RwLock<HashMap<String, CachedSchema>>>,
    ) -> Self {
        Self {
            shared,
            local: HashMap::new(),
        }
    }

    /// Look up a structure hash. Checks local first, then shared.
    pub(super) fn get(&self, structure_hash: &str) -> Option<CachedSchema> {
        if let Some(cached) = self.local.get(structure_hash) {
            return Some(cached.clone());
        }
        self.shared
            .read()
            .ok()
            .and_then(|cache| cache.get(structure_hash).cloned())
    }

    /// Returns true if the structure hash is in either local or shared cache.
    pub(super) fn contains_key(&self, structure_hash: &str) -> bool {
        self.local.contains_key(structure_hash)
            || self
                .shared
                .read()
                .ok()
                .map(|cache| cache.contains_key(structure_hash))
                .unwrap_or(false)
    }

    /// Insert into local scope only (not yet visible to other ingestion calls).
    pub(super) fn insert(&mut self, structure_hash: String, cached: CachedSchema) {
        self.local.insert(structure_hash, cached);
    }

    /// Return all unique schema names in the local cache.
    pub(super) fn local_schema_names(&self) -> Vec<String> {
        self.local.values().map(|c| c.schema_name.clone()).collect()
    }

    /// Remap all cached schema names to org-scoped versions.
    /// Called after schema resolution when ingesting into an org.
    pub(super) fn remap_to_org(&mut self, name_map: &HashMap<String, String>) {
        for cached in self.local.values_mut() {
            if let Some(org_name) = name_map.get(&cached.schema_name) {
                cached.schema_name = org_name.clone();
            }
        }
    }

    /// Flush all local entries to the shared cross-file cache.
    /// Returns Err if the lock is poisoned (entries not flushed).
    pub(super) fn commit(&self) -> Result<(), String> {
        match self.shared.write() {
            Ok(mut shared) => {
                for (hash, cached) in &self.local {
                    shared.insert(hash.clone(), cached.clone());
                }
                Ok(())
            }
            Err(e) => {
                let msg = format!(
                    "Schema cache lock poisoned — {} local entries NOT flushed: {}",
                    self.local.len(),
                    e
                );
                log::error!("{}", msg);
                Err(msg)
            }
        }
    }
}

/// Pre-collected AI proposal for a structure hash (before schema creation).
pub(super) struct AiProposal {
    pub(super) ai_response: AISchemaResponse,
    pub(super) parent_data: Value,
}

/// Merge service/batch field renames into AI's mutation mappers.
/// Service mappers take precedence since they reflect canonical field names.
fn merge_mappers(
    ai_mappers: &HashMap<String, String>,
    extra: impl IntoIterator<Item = (String, String)>,
) -> HashMap<String, String> {
    let mut merged = ai_mappers.clone();
    for (from, to) in extra {
        merged.insert(from, to);
    }
    merged
}

/// Update a parent schema's `ref_fields` and `fields` to include child references.
///
/// Shared by `resolve_schema_for_structure` and `resolve_schemas_with_reuse`.
async fn update_ref_fields(
    schema_name: &str,
    children: &[crate::ingestion::decomposer::ChildGroup],
    schema_cache: &SchemaCache,
    node: &crate::fold_node::FoldNode,
) -> IngestionResult<()> {
    let schema_manager = super::get_schema_manager(node).await?;

    let mut schema = match schema_manager.get_schema_metadata(schema_name) {
        Ok(Some(s)) => s,
        Ok(None) => {
            return Err(IngestionError::SchemaCreationError(format!(
                "Schema '{}' not found when updating ref_fields",
                schema_name
            )));
        }
        Err(e) => {
            return Err(IngestionError::SchemaCreationError(format!(
                "Failed to get schema '{}' for ref_fields update: {}",
                schema_name, e
            )));
        }
    };

    for child_group in children {
        let child_schema_name = schema_cache
            .get(&child_group.structure_hash)
            .map(|c| c.schema_name)
            .ok_or_else(|| {
                IngestionError::SchemaCreationError(format!(
                    "No cached schema for child structure hash '{}' (field '{}')",
                    child_group.structure_hash, child_group.field_name
                ))
            })?;
        schema
            .ref_fields
            .insert(child_group.field_name.clone(), child_schema_name);

        if let Some(ref mut fields) = schema.fields {
            if !fields.contains(&child_group.field_name) {
                fields.push(child_group.field_name.clone());
            }
        } else {
            schema.fields = Some(vec![child_group.field_name.clone()]);
        }
    }

    if let Err(e) = schema.populate_runtime_fields() {
        log_feature!(
            LogFeature::Ingestion,
            warn,
            "Failed to populate runtime fields for schema '{}': {}",
            schema_name,
            e
        );
    }

    schema_manager.update_schema(&schema).await.map_err(|e| {
        IngestionError::SchemaCreationError(format!(
            "Failed to update schema with ref_fields: {}",
            e
        ))
    })?;

    Ok(())
}

impl IngestionService {
    /// Resolve the schema for a given structure hash.
    ///
    /// If not cached, decomposes the representative item (recursively resolving
    /// its own children), then sends the flat representative to AI to determine
    /// the schema. Caches the result for reuse by all items sharing the same
    /// structure.
    pub(super) async fn resolve_schema_for_structure(
        &self,
        structure_hash: &str,
        representative: &Value,
        schema_cache: &mut SchemaCache,
        node: &crate::fold_node::FoldNode,
        depth: usize,
        source_file_name: Option<&str>,
    ) -> IngestionResult<String> {
        // Return cached result if available (checks both local and shared)
        if let Some(cached) = schema_cache.get(structure_hash) {
            return Ok(cached.schema_name);
        }

        // Decompose the representative to handle its own nested children
        let rep_decomp = decomposer::decompose(representative);

        // Depth guard: if we've recursed too deep, skip children and treat as flat
        if depth >= MAX_DECOMPOSITION_DEPTH {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Decomposition depth limit ({}) reached for structure hash '{}' — treating as flat",
                MAX_DECOMPOSITION_DEPTH,
                structure_hash
            );
        } else {
            // Recursively resolve schemas for the representative's children (depth-first)
            // Children are never images themselves, so pass None for source_file_name
            for child_group in &rep_decomp.children {
                let representative = child_group.items.first().ok_or_else(|| {
                    IngestionError::InvalidInput("Empty child group in decomposition".to_string())
                })?;
                Box::pin(self.resolve_schema_for_structure(
                    &child_group.structure_hash,
                    representative,
                    schema_cache,
                    node,
                    depth + 1,
                    None,
                ))
                .await?;
            }
        }

        // Get AI recommendation for the flat parent (no array-of-object fields)
        let mut ai_response = self.get_ai_recommendation(&rep_decomp.parent).await?;

        // If the AI didn't provide field_descriptions, do a second AI call
        self.fill_missing_field_descriptions(&mut ai_response, &rep_decomp.parent)
            .await?;

        // Apply image override at depth 0 (top-level parent is the image schema)
        if depth == 0
            && source_file_name
                .map(crate::ingestion::is_image_file)
                .unwrap_or(false)
        {
            super::apply_image_schema_override(&mut ai_response, None);
        }

        // Create the schema via the standard path
        let (schema_name, service_mappers) = self
            .determine_schema_to_use(&ai_response, &rep_decomp.parent, node)
            .await?;

        let merged_mappers = merge_mappers(&ai_response.mutation_mappers, service_mappers);

        log_feature!(
            LogFeature::Ingestion,
            info,
            "Cached schema '{}' for structure hash {}",
            schema_name,
            structure_hash
        );

        schema_cache.insert(
            structure_hash.to_string(),
            CachedSchema {
                schema_name: schema_name.clone(),
                mutation_mappers: merged_mappers,
            },
        );

        // Update the parent schema with ref_fields for each decomposed child field.
        if !rep_decomp.children.is_empty() && depth < MAX_DECOMPOSITION_DEPTH {
            update_ref_fields(&schema_name, &rep_decomp.children, schema_cache, node).await?;
        }

        Ok(schema_name)
    }

    /// Pre-pass: recursively decompose and collect AI recommendations for all
    /// unique structure hashes WITHOUT creating schemas.
    ///
    /// Skips AI calls for structure hashes already in the service-level cache.
    pub(super) async fn collect_ai_proposals_recursive(
        &self,
        structure_hash: &str,
        representative: &Value,
        proposals: &mut HashMap<String, AiProposal>,
        schema_cache: &SchemaCache,
        depth: usize,
        source_file_name: Option<&str>,
    ) -> IngestionResult<()> {
        // Already collected or already cached (local or cross-file) — skip
        if proposals.contains_key(structure_hash) {
            return Ok(());
        }
        if schema_cache.get(structure_hash).is_some() {
            return Ok(());
        }

        let rep_decomp = decomposer::decompose(representative);

        // Recursively collect children (depth-first)
        if depth < MAX_DECOMPOSITION_DEPTH {
            for child_group in &rep_decomp.children {
                let representative = child_group.items.first().ok_or_else(|| {
                    IngestionError::InvalidInput("Empty child group in decomposition".to_string())
                })?;
                Box::pin(self.collect_ai_proposals_recursive(
                    &child_group.structure_hash,
                    representative,
                    proposals,
                    schema_cache,
                    depth + 1,
                    None,
                ))
                .await?;
            }
        }

        // Get AI recommendation for the flat parent
        let mut ai_response = self.get_ai_recommendation(&rep_decomp.parent).await?;
        self.fill_missing_field_descriptions(&mut ai_response, &rep_decomp.parent)
            .await?;

        // Apply image override at depth 0
        if depth == 0
            && source_file_name
                .map(crate::ingestion::is_image_file)
                .unwrap_or(false)
        {
            super::apply_image_schema_override(&mut ai_response, None);
        }

        proposals.insert(
            structure_hash.to_string(),
            AiProposal {
                ai_response,
                parent_data: rep_decomp.parent,
            },
        );

        Ok(())
    }

    /// Resolve schemas using batch reuse results: for each structure hash, try
    /// the service-level cache first, then batch reuse results, then fall back
    /// to creating via `determine_schema_to_use`.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn resolve_schemas_with_reuse(
        &self,
        structure_hash: &str,
        representative: &Value,
        schema_cache: &mut SchemaCache,
        proposals: &HashMap<String, AiProposal>,
        batch_result: &BatchSchemaReuseResponse,
        node: &crate::fold_node::FoldNode,
        depth: usize,
        _source_file_name: Option<&str>,
    ) -> IngestionResult<String> {
        // Return cached result if available (checks both local and shared)
        if let Some(cached) = schema_cache.get(structure_hash) {
            return Ok(cached.schema_name);
        }

        let rep_decomp = decomposer::decompose(representative);

        // Recursively resolve children first (depth-first)
        if depth < MAX_DECOMPOSITION_DEPTH {
            for child_group in &rep_decomp.children {
                let representative = child_group.items.first().ok_or_else(|| {
                    IngestionError::InvalidInput("Empty child group in decomposition".to_string())
                })?;
                Box::pin(self.resolve_schemas_with_reuse(
                    &child_group.structure_hash,
                    representative,
                    schema_cache,
                    proposals,
                    batch_result,
                    node,
                    depth + 1,
                    None,
                ))
                .await?;
            }
        }

        // Get the AI proposal for this structure hash
        let proposal = proposals.get(structure_hash).ok_or_else(|| {
            IngestionError::SchemaCreationError(format!(
                "No AI proposal for structure hash '{}'",
                structure_hash
            ))
        })?;

        // Try batch reuse: check if the descriptive name matched an existing schema
        let batch_reuse_result = proposal
            .ai_response
            .new_schemas
            .as_ref()
            .and_then(|sd| sd.get("descriptive_name"))
            .and_then(|v| v.as_str())
            .and_then(|desc_name| batch_result.matches.get(desc_name));

        let (schema_name, merged_mappers) = if let Some(reuse_match) = batch_reuse_result {
            if reuse_match.is_superset {
                log_feature!(
                    LogFeature::Ingestion,
                    info,
                    "Batch reuse hit for structure hash '{}': reusing schema '{}' (superset match)",
                    structure_hash,
                    reuse_match.schema.name
                );
                // Reuse: load schema locally if needed, approve, build mappers
                let schema_manager = super::get_schema_manager(node).await?;
                let already_loaded = schema_manager
                    .get_schema_metadata(&reuse_match.schema.name)
                    .map(|opt| opt.is_some())
                    .unwrap_or(false);
                if !already_loaded {
                    let json_str =
                        serde_json::to_string(&reuse_match.schema).map_err(super::schema_err)?;
                    schema_manager
                        .load_schema_from_json(&json_str)
                        .await
                        .map_err(super::schema_err)?;
                    schema_manager
                        .approve(&reuse_match.schema.name)
                        .await
                        .map_err(super::schema_err)?;
                }
                let mappers = merge_mappers(
                    &proposal.ai_response.mutation_mappers,
                    reuse_match
                        .field_rename_map
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone())),
                );
                (reuse_match.schema.name.clone(), mappers)
            } else {
                // Not a superset — fall through to creation
                let (name, service_mappers) = self
                    .determine_schema_to_use(&proposal.ai_response, &proposal.parent_data, node)
                    .await?;
                (
                    name.clone(),
                    merge_mappers(&proposal.ai_response.mutation_mappers, service_mappers),
                )
            }
        } else {
            // No batch match — create via standard path
            let (name, service_mappers) = self
                .determine_schema_to_use(&proposal.ai_response, &proposal.parent_data, node)
                .await?;
            (
                name.clone(),
                merge_mappers(&proposal.ai_response.mutation_mappers, service_mappers),
            )
        };

        log_feature!(
            LogFeature::Ingestion,
            info,
            "Resolved schema '{}' for structure hash {}",
            schema_name,
            structure_hash
        );

        schema_cache.insert(
            structure_hash.to_string(),
            CachedSchema {
                schema_name: schema_name.clone(),
                mutation_mappers: merged_mappers,
            },
        );

        // Update parent schema with ref_fields for children
        if !rep_decomp.children.is_empty() && depth < MAX_DECOMPOSITION_DEPTH {
            update_ref_fields(&schema_name, &rep_decomp.children, schema_cache, node).await?;
        }

        Ok(schema_name)
    }

    /// Process a single item through decomposition: recursively handle its
    /// children, then generate and execute a mutation for the flat parent.
    ///
    /// `structure_hash` is the identity hash of the full item (before decomposition),
    /// matching the key used in `resolve_schema_for_structure`.
    ///
    /// Returns (mutations_generated, mutations_executed, own_key_value).
    /// The third element is the `key_value` from the mutation generated for this item
    /// (None if item was empty). This lets the caller (parent) build references to this child.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn ingest_decomposed_item(
        &self,
        item: &Value,
        structure_hash: &str,
        schema_cache: &mut SchemaCache,
        node: &crate::fold_node::FoldNode,
        pub_key: &str,
        source_file_name: Option<String>,
        metadata: Option<HashMap<String, String>>,
        auto_execute: bool,
        depth: usize,
        schemas_written_map: &mut HashMap<String, Vec<KeyValue>>,
    ) -> IngestionResult<(usize, usize, Option<KeyValue>)> {
        let item_decomp = decomposer::decompose(item);
        let mut total_gen: usize = 0;
        let mut total_exec: usize = 0;

        // Extract parent scalar key fields to inject into children.
        // When {"user_id": "alice", "posts": [...]} decomposes, each post
        // needs user_id so it can be used as a key field for deduplication.
        let parent_key_fields: Vec<(String, Value)> = item_decomp
            .parent
            .as_object()
            .map(|obj| {
                obj.iter()
                    .filter(|(_, v)| v.is_string() || v.is_number())
                    .map(|(k, v)| (format!("parent_{}", k), v.clone()))
                    .collect()
            })
            .unwrap_or_default();

        // Recursively process each child group's items and collect references.
        let mut child_references: HashMap<String, Vec<Value>> = HashMap::new();

        // Skip children if depth limit reached
        if depth >= MAX_DECOMPOSITION_DEPTH {
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Decomposition depth limit ({}) reached during ingestion for structure hash '{}' — skipping children",
                MAX_DECOMPOSITION_DEPTH,
                structure_hash
            );
        } else {
            for child_group in &item_decomp.children {
                let mut refs_for_field = Vec::new();

                for child_item in &child_group.items {
                    // Inject parent key fields so children can reference the parent
                    let child_item = if !parent_key_fields.is_empty() {
                        let mut enriched = child_item.clone();
                        if let Some(obj) = enriched.as_object_mut() {
                            for (key, val) in &parent_key_fields {
                                obj.entry(key.clone()).or_insert_with(|| val.clone());
                            }
                        }
                        enriched
                    } else {
                        child_item.clone()
                    };

                    let (gen, exec, child_key_value) = Box::pin(self.ingest_decomposed_item(
                        &child_item,
                        &child_group.structure_hash,
                        schema_cache,
                        node,
                        pub_key,
                        source_file_name.clone(),
                        metadata.clone(),
                        auto_execute,
                        depth + 1,
                        schemas_written_map,
                    ))
                    .await?;
                    total_gen += gen;
                    total_exec += exec;

                    // Build reference matching the indexing system's (schema, key) pattern
                    if let Some(kv) = child_key_value {
                        let child_schema_name = schema_cache
                            .get(&child_group.structure_hash)
                            .map(|c| c.schema_name)
                            .ok_or_else(|| {
                                IngestionError::SchemaCreationError(format!(
                                    "No cached schema for child structure hash '{}' (field '{}')",
                                    child_group.structure_hash, child_group.field_name
                                ))
                            })?;
                        refs_for_field.push(serde_json::json!({
                            "schema": child_schema_name,
                            "key": kv,
                        }));
                    } else {
                        log_feature!(
                        LogFeature::Ingestion,
                        warn,
                        "Child item in field '{}' (structure hash {}) produced no key_value — reference will be missing",
                        child_group.field_name,
                        child_group.structure_hash
                    );
                    }
                }

                child_references.insert(child_group.field_name.clone(), refs_for_field);
            }
        } // end depth guard else

        // Generate and execute mutation for this item's flat parent.
        // Use the structure_hash passed in (hash of full item before decomposition)
        // to look up the cached schema — this matches the key from resolve_schema_for_structure.
        let mut parent = item_decomp.parent;

        // Inject child references into the parent data before mutation generation
        if let Some(parent_obj) = parent.as_object_mut() {
            for (field_name, refs) in &child_references {
                if !refs.is_empty() {
                    parent_obj.insert(field_name.clone(), Value::Array(refs.clone()));
                }
            }
        }

        // Enrich image data at depth 0 (top-level parent is the image)
        let is_image = depth == 0
            && source_file_name
                .as_deref()
                .map(crate::ingestion::is_image_file)
                .unwrap_or(false);
        if is_image {
            if let Some(ref sfn) = source_file_name {
                let dummy_path = std::path::PathBuf::from(sfn);
                crate::ingestion::file_handling::json_processor::enrich_image_json(
                    &mut parent,
                    &dummy_path,
                    Some(sfn.as_str()),
                );
            }
            // Classify photo visibility using AI
            if parent.get("visibility").and_then(|v| v.as_str()).is_none() {
                match crate::ingestion::file_handling::json_processor::classify_visibility(
                    &parent, self,
                )
                .await
                {
                    Ok(visibility) => {
                        if let Value::Object(ref mut map) = parent {
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
        }

        let mut own_key_value: Option<KeyValue> = None;

        if let Some(parent_obj) = parent.as_object() {
            if parent_obj.is_empty() {
                return Ok((total_gen, total_exec, None));
            }

            // If this item's structure differs from the representative (e.g., empty
            // vs. non-empty nested arrays), the structure hash won't be cached yet.
            // Resolve it on the fly so the schema and mutation mappers are available.
            if !schema_cache.contains_key(structure_hash) {
                Box::pin(self.resolve_schema_for_structure(
                    structure_hash,
                    item,
                    schema_cache,
                    node,
                    depth,
                    source_file_name.as_deref(),
                ))
                .await?;
            }

            let cached = schema_cache.get(structure_hash).ok_or_else(|| {
                IngestionError::SchemaCreationError(format!(
                    "No cached schema for structure hash {}",
                    structure_hash
                ))
            })?;

            let schema_name = cached.schema_name;
            let mut mutation_mappers = cached.mutation_mappers;

            // Add identity mappers for Reference fields so generate_mutations includes them
            for (field_name, refs) in &child_references {
                if !refs.is_empty() && !mutation_mappers.contains_key(field_name) {
                    mutation_mappers.insert(field_name.clone(), field_name.clone());
                }
            }

            // Filter mutation mappers to only reference fields that exist in the
            // schema's runtime_fields. The AI may include mappers for fields it
            // dropped from the schema definition, causing write failures.
            let schema_manager = super::get_schema_manager(node).await?;
            if let Ok(Some(schema_meta)) = schema_manager.get_schema_metadata(&schema_name) {
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
                        warn,
                        "Dropped {} mutation mapper(s) for schema '{}' — target fields not in runtime_fields",
                        dropped,
                        schema_name
                    );
                }
            }

            let schema_manager = super::get_schema_manager(node).await?;
            let mutations = super::generate_mutations_for_item(
                parent_obj,
                &schema_name,
                &mutation_mappers,
                &schema_manager,
                pub_key,
                source_file_name,
                metadata,
            )
            .await?;

            // Extract the key_value from the first mutation before execution
            own_key_value = mutations.first().map(|m| m.key_value.clone());

            // Record all (schema_name, key_value) pairs into the accumulator
            for m in &mutations {
                schemas_written_map
                    .entry(m.schema_name.clone())
                    .or_default()
                    .push(m.key_value.clone());
            }

            let gen_count = mutations.len();
            total_gen += gen_count;

            if auto_execute && !mutations.is_empty() {
                let exec_count = node
                    .mutate_batch(mutations)
                    .await
                    .map(|ids| ids.len())
                    .map_err(|e| {
                        IngestionError::SchemaSystemError(
                            fold_db::schema::SchemaError::InvalidData(e.to_string()),
                        )
                    })?;
                total_exec += exec_count;
            }
        }

        Ok((total_gen, total_exec, own_key_value))
    }
}
