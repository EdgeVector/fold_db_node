use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use fold_db::db_operations::native_index::{cosine_similarity, Embedder, FastEmbedModel};
use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::schema::types::Schema;
#[cfg(feature = "aws-backend")]
use fold_db::storage::DynamoDbSchemaStore;

#[cfg(feature = "aws-backend")]
pub use fold_db::storage::CloudConfig;

use super::types::{SchemaAddOutcome, SimilarSchemaEntry, SimilarSchemasResponse};

/// Minimum cosine similarity between descriptive names to consider them a semantic match.
const DESCRIPTIVE_NAME_SIMILARITY_THRESHOLD: f32 = 0.8;


/// Storage backend for the schema service
#[derive(Clone)]
pub enum SchemaStorage {
    /// Local sled database (default)
    Sled {
        db: sled::Db,
        schemas_tree: sled::Tree,
    },
    /// Cloud storage (DynamoDB etc) (serverless, no locking needed!)
    #[cfg(feature = "aws-backend")]
    Cloud { store: Arc<DynamoDbSchemaStore> },
}

/// Shared state for the schema service
#[derive(Clone)]
pub struct SchemaServiceState {
    pub(super) schemas: Arc<RwLock<HashMap<String, Schema>>>,
    /// Secondary index: descriptive_name -> schema_name (identity_hash)
    pub(super) descriptive_name_index: Arc<RwLock<HashMap<String, String>>>,
    /// Cached embeddings for descriptive names: descriptive_name -> embedding vector
    descriptive_name_embeddings: Arc<RwLock<HashMap<String, Vec<f32>>>>,
    /// Text embedding model for semantic descriptive name matching
    embedder: Arc<dyn Embedder>,
    pub(super) storage: SchemaStorage,
}

impl SchemaServiceState {
    /// Create a new schema service state with local sled storage
    pub fn new(db_path: String) -> FoldDbResult<Self> {
        let db = sled::open(&db_path).map_err(|e| {
            FoldDbError::Config(format!(
                "Failed to open schema service database at '{}': {}",
                db_path, e
            ))
        })?;

        let schemas_tree = db
            .open_tree("schemas")
            .map_err(|e| FoldDbError::Config(format!("Failed to open schemas tree: {}", e)))?;

        let state = Self {
            schemas: Arc::new(RwLock::new(HashMap::new())),
            descriptive_name_index: Arc::new(RwLock::new(HashMap::new())),
            descriptive_name_embeddings: Arc::new(RwLock::new(HashMap::new())),
            embedder: Arc::new(FastEmbedModel::new()),
            storage: SchemaStorage::Sled { db, schemas_tree },
        };

        // Load schemas synchronously for sled
        state.load_schemas_sync()?;
        state.rebuild_descriptive_name_index();

        Ok(state)
    }

    /// Synchronous version of load_schemas for Sled storage
    fn load_schemas_sync(&self) -> FoldDbResult<()> {
        let mut schemas = self
            .schemas
            .write()
            .map_err(|_| FoldDbError::Config("Failed to acquire schemas write lock".to_string()))?;

        schemas.clear();

        match &self.storage {
            SchemaStorage::Sled { schemas_tree, .. } => {
                let mut count = 0;
                for result in schemas_tree.iter() {
                    let (key, value) = result.map_err(|e| {
                        FoldDbError::Config(format!("Failed to iterate over schemas tree: {}", e))
                    })?;

                    let name = String::from_utf8(key.to_vec()).map_err(|e| {
                        FoldDbError::Config(format!("Failed to decode schema name from key: {}", e))
                    })?;

                    let schema: Schema = serde_json::from_slice(&value).map_err(|e| {
                        FoldDbError::Config(format!(
                            "Failed to parse schema '{}' from database: {}",
                            name, e
                        ))
                    })?;

                    schemas.insert(name, schema);
                    count += 1;
                }

                log_feature!(
                    LogFeature::Schema,
                    info,
                    "Schema service loaded {} schemas from sled",
                    count
                );
            }
            #[cfg(feature = "aws-backend")]
            _ => {
                return Err(FoldDbError::Config(
                    "load_schemas_sync called on non-Sled storage".to_string(),
                ));
            }
        }

        Ok(())
    }

    /// Create a new schema service state with Cloud storage
    /// No locking needed - identity hashes ensure idempotent writes!
    #[cfg(feature = "aws-backend")]
    pub async fn new_with_cloud(config: CloudConfig) -> FoldDbResult<Self> {
        log_feature!(
            LogFeature::Schema,
            info,
            "Initializing schema service with DynamoDB in region: {}",
            config.region
        );

        let store = DynamoDbSchemaStore::new(config).await?;

        let state = Self {
            schemas: Arc::new(RwLock::new(HashMap::new())),
            descriptive_name_index: Arc::new(RwLock::new(HashMap::new())),
            descriptive_name_embeddings: Arc::new(RwLock::new(HashMap::new())),
            embedder: Arc::new(FastEmbedModel::new()),
            storage: SchemaStorage::Cloud {
                store: Arc::new(store),
            },
        };

        // Load schemas on initialization
        state.load_schemas().await?;
        state.rebuild_descriptive_name_index();

        log_feature!(
            LogFeature::Schema,
            info,
            "Schema service initialized with DynamoDB, loaded {} schemas",
            state.schemas.read().map(|s| s.len()).unwrap_or(0)
        );

        Ok(state)
    }

    /// Load all schemas from storage (works for both Sled and DynamoDB)
    pub async fn load_schemas(&self) -> FoldDbResult<()> {
        match &self.storage {
            SchemaStorage::Sled { schemas_tree, .. } => {
                let mut schemas = self.schemas.write().map_err(|_| {
                    FoldDbError::Config("Failed to acquire schemas write lock".to_string())
                })?;

                schemas.clear();
                let mut count = 0;
                for result in schemas_tree.iter() {
                    let (key, value) = result.map_err(|e| {
                        FoldDbError::Config(format!("Failed to iterate over schemas tree: {}", e))
                    })?;

                    let name = String::from_utf8(key.to_vec()).map_err(|e| {
                        FoldDbError::Config(format!("Failed to decode schema name from key: {}", e))
                    })?;

                    let schema: Schema = serde_json::from_slice(&value).map_err(|e| {
                        FoldDbError::Config(format!(
                            "Failed to parse schema '{}' from database: {}",
                            name, e
                        ))
                    })?;

                    log_feature!(
                        LogFeature::Schema,
                        info,
                        "Loaded schema '{}' from sled database",
                        name
                    );

                    schemas.insert(name, schema);
                    count += 1;
                }

                log_feature!(
                    LogFeature::Schema,
                    info,
                    "Schema service loaded {} schemas from sled",
                    count
                );
            }
            #[cfg(feature = "aws-backend")]
            SchemaStorage::Cloud { store } => {
                let all_schemas = store.get_all_schemas().await?;
                let count = all_schemas.len();

                let mut schemas = self.schemas.write().map_err(|_| {
                    FoldDbError::Config("Failed to acquire schemas write lock".to_string())
                })?;

                schemas.clear();

                for schema in all_schemas {
                    log_feature!(
                        LogFeature::Schema,
                        info,
                        "Loaded schema '{}' from DynamoDB",
                        schema.name
                    );
                    schemas.insert(schema.name.clone(), schema);
                }

                log_feature!(
                    LogFeature::Schema,
                    info,
                    "Schema service loaded {} schemas from DynamoDB",
                    count
                );
            }
        }

        Ok(())
    }

    /// Rebuild the descriptive_name -> schema_name index and embeddings cache.
    fn rebuild_descriptive_name_index(&self) {
        let schemas = match self.schemas.read() {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut index = match self.descriptive_name_index.write() {
            Ok(i) => i,
            Err(_) => return,
        };
        let mut embeddings = match self.descriptive_name_embeddings.write() {
            Ok(e) => e,
            Err(_) => return,
        };
        index.clear();
        embeddings.clear();
        for (name, schema) in schemas.iter() {
            if let Some(ref desc) = schema.descriptive_name {
                index.insert(desc.clone(), name.clone());
                match self.embedder.embed_text(desc) {
                    Ok(vec) => { embeddings.insert(desc.clone(), vec); }
                    Err(e) => {
                        log_feature!(
                            LogFeature::Schema,
                            warn,
                            "Failed to embed descriptive_name '{}': {}",
                            desc,
                            e
                        );
                    }
                }
            }
        }
    }

    /// Create a schema service state with a custom embedder (for testing).
    #[cfg(any(test, feature = "test-utils"))]
    pub fn new_with_embedder(db_path: String, embedder: Arc<dyn Embedder>) -> FoldDbResult<Self> {
        let db = sled::open(&db_path).map_err(|e| {
            FoldDbError::Config(format!(
                "Failed to open schema service database at '{}': {}",
                db_path, e
            ))
        })?;

        let schemas_tree = db
            .open_tree("schemas")
            .map_err(|e| FoldDbError::Config(format!("Failed to open schemas tree: {}", e)))?;

        let state = Self {
            schemas: Arc::new(RwLock::new(HashMap::new())),
            descriptive_name_index: Arc::new(RwLock::new(HashMap::new())),
            descriptive_name_embeddings: Arc::new(RwLock::new(HashMap::new())),
            embedder,
            storage: SchemaStorage::Sled { db, schemas_tree },
        };

        state.load_schemas_sync()?;
        state.rebuild_descriptive_name_index();

        Ok(state)
    }

    /// Find an existing descriptive_name that matches the given name.
    /// First tries exact match, then falls back to semantic (embedding) similarity.
    /// Returns (matched_descriptive_name, schema_identity_hash, is_exact_match).
    fn find_matching_descriptive_name(
        &self,
        desc_name: &str,
    ) -> FoldDbResult<(Option<String>, Option<String>, bool)> {
        // 1. Exact match
        let index = self.descriptive_name_index.read().map_err(|_| {
            FoldDbError::Config("Failed to acquire descriptive_name_index read lock".to_string())
        })?;
        if let Some(hash) = index.get(desc_name) {
            return Ok((Some(desc_name.to_string()), Some(hash.clone()), true));
        }
        drop(index);

        // 2. Semantic similarity via embeddings
        let query_embedding = match self.embedder.embed_text(desc_name) {
            Ok(vec) => vec,
            Err(e) => {
                log_feature!(
                    LogFeature::Schema,
                    warn,
                    "Failed to embed descriptive_name '{}' for similarity search: {}",
                    desc_name,
                    e
                );
                return Ok((None, None, false));
            }
        };

        let embeddings = self.descriptive_name_embeddings.read().map_err(|_| {
            FoldDbError::Config("Failed to acquire descriptive_name_embeddings read lock".to_string())
        })?;

        let mut best_match: Option<(&str, f32)> = None;
        for (existing_desc, existing_vec) in embeddings.iter() {
            let sim = cosine_similarity(&query_embedding, existing_vec);
            if sim >= DESCRIPTIVE_NAME_SIMILARITY_THRESHOLD
                && best_match.is_none_or(|(_, best_sim)| sim > best_sim)
            {
                best_match = Some((existing_desc.as_str(), sim));
            }
        }

        if let Some((matched_desc, similarity)) = best_match {
            log_feature!(
                LogFeature::Schema,
                info,
                "Semantic descriptive_name match: '{}' ≈ '{}' (similarity: {:.3})",
                desc_name,
                matched_desc,
                similarity
            );
            let index = self.descriptive_name_index.read().map_err(|_| {
                FoldDbError::Config("Failed to acquire descriptive_name_index read lock".to_string())
            })?;
            let hash = index.get(matched_desc).cloned();
            return Ok((Some(matched_desc.to_string()), hash, false));
        }

        Ok((None, None, false))
    }

    pub async fn add_schema(
        &self,
        mut schema: Schema,
        mutation_mappers: HashMap<String, String>,
    ) -> FoldDbResult<SchemaAddOutcome> {
        // Deduplicate fields before computing identity hash
        schema.dedup_fields();

        // Ensure identity_hash is computed
        if schema.identity_hash.is_none() {
            schema.compute_identity_hash();
        }

        // Get the original schema name before we modify it
        let original_schema_name = schema.name.clone();

        // Use identity_hash as the schema identifier
        let identity_hash = schema
            .get_identity_hash()
            .ok_or_else(|| {
                FoldDbError::Config("Schema must have identity_hash computed".to_string())
            })?
            .clone();

        log_feature!(
            LogFeature::Schema,
            info,
            "Schema '{}' identity_hash: {}",
            original_schema_name,
            identity_hash
        );

        // Use identity_hash as unique identifier (SHA256 of sorted field names)
        let schema_name = identity_hash.clone();

        // Check if this exact schema already exists (same identity hash = same fields)
        {
            let schemas = self.schemas.read().map_err(|_| {
                FoldDbError::Config("Failed to acquire schemas read lock".to_string())
            })?;

            if let Some(existing_schema) = schemas.get(&schema_name) {
                // If this schema has been superseded by expansion, return the
                // current active schema instead of the old superseded one.
                if let Some(ref desc_name) = existing_schema.descriptive_name {
                    let index = self.descriptive_name_index.read().map_err(|_| {
                        FoldDbError::Config("Failed to acquire descriptive_name_index read lock".to_string())
                    })?;
                    if let Some(current_hash) = index.get(desc_name) {
                        if *current_hash != schema_name {
                            // This schema was superseded — return the current active one
                            if let Some(active_schema) = schemas.get(current_hash) {
                                log_feature!(
                                    LogFeature::Schema,
                                    info,
                                    "Schema '{}' was superseded by '{}' — returning active schema",
                                    schema_name,
                                    current_hash
                                );
                                return Ok(SchemaAddOutcome::AlreadyExists(active_schema.clone()));
                            }
                        }
                    }
                }

                log_feature!(
                    LogFeature::Schema,
                    info,
                    "Schema '{}' already exists - returning existing schema",
                    schema_name
                );

                return Ok(SchemaAddOutcome::AlreadyExists(existing_schema.clone()));
            }
        }

        // Check for schema expansion: if the new schema has a descriptive_name that
        // matches an existing schema (exact or semantic), merge fields (expand, never shrink).
        if let Some(incoming_desc_name) = schema.descriptive_name.clone() {
            let (matched_desc, existing_schema_name, _) = self.find_matching_descriptive_name(&incoming_desc_name)?;

            if let Some(old_name) = existing_schema_name {
                // If matched via semantic similarity, adopt the existing descriptive_name
                // so the index stays consistent.
                if let Some(ref canonical_desc) = matched_desc {
                    if *canonical_desc != incoming_desc_name {
                        log_feature!(
                            LogFeature::Schema,
                            info,
                            "Semantic match: incoming '{}' matched existing '{}'",
                            incoming_desc_name,
                            canonical_desc
                        );
                        schema.descriptive_name = Some(canonical_desc.clone());
                    }
                }
                // Use the (possibly canonical) descriptive_name for the rest of expansion
                let desc_name = schema.descriptive_name.clone().unwrap_or(incoming_desc_name);
                // We already checked exact-hash match above, so the old schema
                // has a different (smaller) field set. Merge fields as a superset.
                let old_schema = {
                    let schemas = self.schemas.read().map_err(|_| {
                        FoldDbError::Config("Failed to acquire schemas read lock".to_string())
                    })?;
                    schemas.get(&old_name).cloned()
                };

                if let Some(existing) = old_schema {
                    let existing_fields = existing.fields.clone().unwrap_or_default();
                    let existing_set: HashSet<String> =
                        existing_fields.iter().cloned().collect();
                    let new_field_set: HashSet<String> = schema
                        .fields
                        .as_ref()
                        .map(|nf| nf.iter().cloned().collect())
                        .unwrap_or_default();

                    // If the new schema's fields are a subset of (or equal to) the
                    // existing schema, just reuse the existing schema — no expansion needed.
                    if new_field_set.is_subset(&existing_set) {
                        log_feature!(
                            LogFeature::Schema,
                            info,
                            "New schema is a subset of existing '{}' (descriptive_name='{}') — reusing existing",
                            old_name,
                            desc_name
                        );
                        return Ok(SchemaAddOutcome::AlreadyExists(existing));
                    }

                    // Similar descriptive name → expand to superset of both schemas.
                    log_feature!(
                        LogFeature::Schema,
                        info,
                        "Expanding schema (descriptive_name='{}') — merging fields from old hash '{}'",
                        desc_name,
                        old_name
                    );

                    let new_fields_to_add: Vec<String> = new_field_set
                        .difference(&existing_set)
                        .cloned()
                        .collect();
                    let mut merged_fields = existing_fields.clone();
                    merged_fields.extend(new_fields_to_add);
                    schema.fields = Some(merged_fields);

                    // Set field_mappers on the expanded schema for shared fields,
                    // pointing to the old schema's fields (which own the molecules).
                    // New fields get no mapper — they'll get fresh molecules.
                    use fold_db::schema::types::declarative_schemas::FieldMapper;
                    let mut mappers: HashMap<String, FieldMapper> = schema
                        .field_mappers()
                        .cloned()
                        .unwrap_or_default();
                    for field in &existing_fields {
                        mappers.entry(field.clone()).or_insert_with(|| {
                            FieldMapper::new(old_name.clone(), field.clone())
                        });
                    }
                    schema.field_mappers = Some(mappers);

                    // Don't carry over field_molecule_uuids — the node's
                    // apply_field_mappers will resolve them from the old schema.
                    schema.field_molecule_uuids = None;

                    // Merge field_classifications (keep existing, add new)
                    for (field, classifications) in &existing.field_classifications {
                        schema
                            .field_classifications
                            .entry(field.clone())
                            .or_insert_with(|| classifications.clone());
                    }

                    // Merge ref_fields (keep existing references)
                    for (field, target) in &existing.ref_fields {
                        schema
                            .ref_fields
                            .entry(field.clone())
                            .or_insert_with(|| target.clone());
                    }

                    // Recompute identity hash with merged fields
                    schema.compute_identity_hash();
                    let new_hash = schema
                        .get_identity_hash()
                        .ok_or_else(|| {
                            FoldDbError::Config("Failed to compute merged identity_hash".to_string())
                        })?
                        .clone();
                    schema.name = new_hash.clone();

                    // Persist expanded schema (old schema stays in storage)
                    self.persist_schema(&schema, &mutation_mappers).await?;

                    // Update in-memory cache: keep old, insert new
                    {
                        let mut schemas = self.schemas.write().map_err(|_| {
                            FoldDbError::Config("Failed to acquire schemas write lock".to_string())
                        })?;
                        schemas.insert(new_hash.clone(), schema.clone());
                    }

                    // Update descriptive_name index to point to expanded schema
                    {
                        let mut index = self.descriptive_name_index.write().map_err(|_| {
                            FoldDbError::Config("Failed to acquire descriptive_name_index write lock".to_string())
                        })?;
                        index.insert(desc_name.clone(), new_hash);
                    }

                    log_feature!(
                        LogFeature::Schema,
                        info,
                        "Schema expanded: old='{}' (blocked) -> new='{}' (descriptive_name='{}')",
                        old_name,
                        schema.name,
                        desc_name
                    );

                    return Ok(SchemaAddOutcome::Expanded(old_name, schema, mutation_mappers));
                }
            }
        }

        // Fallback: if descriptive_name matching didn't find a match, check for
        // high field overlap with any existing schema. If >50% of fields overlap,
        // treat it as the same concept and expand (the AI may generate inconsistent
        // descriptive names for the same data type).
        let overlap_match: Option<(String, Schema)> = {
            let new_fields: HashSet<String> = schema
                .fields
                .as_ref()
                .map(|f| f.iter().cloned().collect())
                .unwrap_or_default();

            if new_fields.is_empty() {
                None
            } else {
                let schemas = self.schemas.read().map_err(|_| {
                    FoldDbError::Config("Failed to acquire schemas read lock".to_string())
                })?;

                let mut best: Option<(String, usize, Schema)> = None;
                for (existing_name, existing_schema) in schemas.iter() {
                    let existing_fields: HashSet<String> = existing_schema
                        .fields
                        .as_ref()
                        .map(|f| f.iter().cloned().collect())
                        .unwrap_or_default();
                    if existing_fields.is_empty() {
                        continue;
                    }
                    let overlap = new_fields.intersection(&existing_fields).count();
                    let min_size = new_fields.len().min(existing_fields.len());
                    // >50% of the smaller schema's fields overlap
                    if min_size > 0
                        && overlap * 2 > min_size
                        && best.as_ref().is_none_or(|(_, b, _)| overlap > *b)
                    {
                        best = Some((
                            existing_name.clone(),
                            overlap,
                            existing_schema.clone(),
                        ));
                    }
                }
                // Lock is dropped here when `schemas` goes out of scope
                best.map(|(name, _overlap, s)| (name, s))
            }
        };

        if let Some((old_name, existing)) = overlap_match {
            let new_fields: HashSet<String> = schema
                .fields
                .as_ref()
                .map(|f| f.iter().cloned().collect())
                .unwrap_or_default();
            let existing_fields = existing.fields.clone().unwrap_or_default();
            let existing_set: HashSet<String> = existing_fields.iter().cloned().collect();

            log_feature!(
                LogFeature::Schema,
                info,
                "Field overlap fallback: {} fields overlap with '{}' — expanding",
                new_fields.intersection(&existing_set).count(),
                old_name
            );

            // If new is a subset, return existing
            if new_fields.is_subset(&existing_set) {
                return Ok(SchemaAddOutcome::AlreadyExists(existing));
            }

            // Adopt the existing descriptive_name for consistency
            if let Some(ref desc) = existing.descriptive_name {
                schema.descriptive_name = Some(desc.clone());
            }

            // Merge to superset
            let new_only: Vec<String> =
                new_fields.difference(&existing_set).cloned().collect();
            let mut merged = existing_fields.clone();
            merged.extend(new_only);
            schema.fields = Some(merged);

            // field_mappers for shared fields
            use fold_db::schema::types::declarative_schemas::FieldMapper;
            let mut mappers: HashMap<String, FieldMapper> =
                schema.field_mappers().cloned().unwrap_or_default();
            for field in &existing_fields {
                mappers.entry(field.clone()).or_insert_with(|| {
                    FieldMapper::new(old_name.clone(), field.clone())
                });
            }
            schema.field_mappers = Some(mappers);
            schema.field_molecule_uuids = None;

            // Recompute identity hash for merged schema
            schema.compute_identity_hash();
            let new_hash = schema.get_identity_hash().cloned().unwrap_or_default();
            schema.name = new_hash.clone();

            self.persist_schema(&schema, &mutation_mappers).await?;

            {
                let mut schemas = self.schemas.write().map_err(|_| {
                    FoldDbError::Config(
                        "Failed to acquire schemas write lock".to_string(),
                    )
                })?;
                schemas.insert(new_hash.clone(), schema.clone());
            }

            if let Some(ref desc_name) = schema.descriptive_name {
                let mut index = self.descriptive_name_index.write().map_err(|_| {
                    FoldDbError::Config(
                        "Failed to acquire descriptive_name_index write lock".to_string(),
                    )
                })?;
                index.insert(desc_name.clone(), new_hash);
            }

            return Ok(SchemaAddOutcome::Expanded(old_name, schema, mutation_mappers));
        }

        schema.name = schema_name.clone();

        // Persist to storage backend
        self.persist_schema(&schema, &mutation_mappers).await?;

        // Insert into in-memory cache and update descriptive_name index
        {
            let mut schemas = self.schemas.write().map_err(|_| {
                FoldDbError::Config("Failed to acquire schemas write lock".to_string())
            })?;
            schemas.insert(schema_name.clone(), schema.clone());
        }

        if let Some(ref desc_name) = schema.descriptive_name {
            let mut index = self.descriptive_name_index.write().map_err(|_| {
                FoldDbError::Config("Failed to acquire descriptive_name_index write lock".to_string())
            })?;
            index.insert(desc_name.clone(), schema_name.clone());
            drop(index);

            // Cache embedding for new descriptive_name
            if let Ok(vec) = self.embedder.embed_text(desc_name) {
                if let Ok(mut embeddings) = self.descriptive_name_embeddings.write() {
                    embeddings.insert(desc_name.clone(), vec);
                }
            }
        }

        log_feature!(
            LogFeature::Schema,
            info,
            "Schema '{}' successfully added to registry",
            schema_name
        );

        Ok(SchemaAddOutcome::Added(schema, mutation_mappers))
    }

    /// Persist a schema to the storage backend.
    #[allow(unused_variables)]
    async fn persist_schema(
        &self,
        schema: &Schema,
        mutation_mappers: &HashMap<String, String>,
    ) -> FoldDbResult<()> {
        match &self.storage {
            SchemaStorage::Sled { db, schemas_tree } => {
                let serialized = serde_json::to_vec(schema).map_err(|e| {
                    FoldDbError::Serialization(format!(
                        "Failed to serialize schema '{}': {}", schema.name, e
                    ))
                })?;
                schemas_tree
                    .insert(schema.name.as_bytes(), serialized)
                    .map_err(|e| {
                        FoldDbError::Config(format!(
                            "Failed to insert schema '{}' into sled: {}", schema.name, e
                        ))
                    })?;
                db.flush().map_err(|e| {
                    FoldDbError::Config(format!("Failed to flush sled: {}", e))
                })?;
                log_feature!(LogFeature::Schema, info, "Schema '{}' persisted to sled", schema.name);
            }
            #[cfg(feature = "aws-backend")]
            SchemaStorage::Cloud { store } => {
                store.put_schema(schema, mutation_mappers).await?;
                log_feature!(LogFeature::Schema, info, "Schema '{}' persisted to DynamoDB", schema.name);
            }
        }
        Ok(())
    }

    /// Get all schema names (public accessor for Lambda integration)
    pub fn get_schema_names(&self) -> FoldDbResult<Vec<String>> {
        let schemas = self
            .schemas
            .read()
            .map_err(|_| FoldDbError::Config("Failed to acquire schemas read lock".to_string()))?;
        Ok(schemas.keys().cloned().collect())
    }

    /// Get all schemas (public accessor for Lambda integration)
    pub fn get_all_schemas_cached(&self) -> FoldDbResult<Vec<Schema>> {
        let schemas = self
            .schemas
            .read()
            .map_err(|_| FoldDbError::Config("Failed to acquire schemas read lock".to_string()))?;
        Ok(schemas.values().cloned().collect())
    }

    /// Get a schema by name (public accessor for Lambda integration)
    pub fn get_schema_by_name(&self, name: &str) -> FoldDbResult<Option<Schema>> {
        let schemas = self
            .schemas
            .read()
            .map_err(|_| FoldDbError::Config("Failed to acquire schemas read lock".to_string()))?;
        Ok(schemas.get(name).cloned())
    }

    /// Get schema count (public accessor for Lambda integration)
    pub fn get_schema_count(&self) -> usize {
        self.schemas.read().map(|s| s.len()).unwrap_or(0)
    }

    /// Find schemas similar to the given schema using Jaccard index on field name sets
    pub fn find_similar_schemas(
        &self,
        name: &str,
        threshold: f64,
    ) -> FoldDbResult<SimilarSchemasResponse> {
        let schemas = self
            .schemas
            .read()
            .map_err(|_| FoldDbError::Config("Failed to acquire schemas read lock".to_string()))?;

        let target = schemas.get(name).ok_or_else(|| {
            FoldDbError::Config(format!("Schema '{}' not found", name))
        })?;

        let target_fields = collect_field_names(target);

        let mut similar: Vec<SimilarSchemaEntry> = schemas
            .iter()
            .filter(|(k, _)| k.as_str() != name)
            .filter_map(|(_, schema)| {
                let other_fields = collect_field_names(schema);
                let similarity = jaccard_index(&target_fields, &other_fields);
                if similarity >= threshold {
                    Some(SimilarSchemaEntry {
                        schema: schema.clone(),
                        similarity,
                    })
                } else {
                    None
                }
            })
            .collect();

        similar.sort_by(|a, b| b.similarity.partial_cmp(&a.similarity).unwrap_or(std::cmp::Ordering::Equal));

        Ok(SimilarSchemasResponse {
            query_schema: name.to_string(),
            threshold,
            similar_schemas: similar,
        })
    }
}

/// Collect all field names from a schema (union of fields and transform_fields keys)
fn collect_field_names(schema: &Schema) -> HashSet<String> {
    let mut names = HashSet::new();
    if let Some(ref fields) = schema.fields {
        for f in fields {
            names.insert(f.clone());
        }
    }
    if let Some(ref tf) = schema.transform_fields {
        for key in tf.keys() {
            names.insert(key.clone());
        }
    }
    names
}

/// Compute Jaccard index: |A ∩ B| / |A ∪ B|
pub(crate) fn jaccard_index(a: &HashSet<String>, b: &HashSet<String>) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let intersection = a.intersection(b).count();
    let union = a.union(b).count();
    intersection as f64 / union as f64
}
