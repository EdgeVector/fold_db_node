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

/// Minimum cosine similarity between context-enriched field names to consider them synonyms.
/// Uses "the {field} of the {descriptive_name}" format for embedding context.
/// Set to 0.88 based on empirical testing: artist↔creator=0.93 (true synonym),
/// medium↔artist=0.85 (false positive in "Artwork Collection" context).
const FIELD_SIMILARITY_THRESHOLD: f32 = 0.88;


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
    /// Cached embeddings for context-enriched field names: "desc_name:field_name" -> embedding
    field_embeddings: Arc<RwLock<HashMap<String, Vec<f32>>>>,
    /// Global canonical field registry: canonical_name -> description.
    /// New schema proposals have their field names matched against this list
    /// so that semantically equivalent fields use consistent names across all schemas.
    canonical_fields: Arc<RwLock<HashMap<String, String>>>,
    /// Cached embeddings for canonical field names
    canonical_field_embeddings: Arc<RwLock<HashMap<String, Vec<f32>>>>,
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

        let canonical_fields_tree = db
            .open_tree("canonical_fields")
            .map_err(|e| FoldDbError::Config(format!("Failed to open canonical_fields tree: {}", e)))?;

        let state = Self {
            schemas: Arc::new(RwLock::new(HashMap::new())),
            descriptive_name_index: Arc::new(RwLock::new(HashMap::new())),
            descriptive_name_embeddings: Arc::new(RwLock::new(HashMap::new())),
            field_embeddings: Arc::new(RwLock::new(HashMap::new())),
            canonical_fields: Arc::new(RwLock::new(HashMap::new())),
            canonical_field_embeddings: Arc::new(RwLock::new(HashMap::new())),
            embedder: Arc::new(FastEmbedModel::new()),
            storage: SchemaStorage::Sled { db, schemas_tree },
        };

        // Load schemas synchronously for sled
        state.load_schemas_sync()?;
        state.rebuild_descriptive_name_index();
        state.load_canonical_fields_from_tree(&canonical_fields_tree)?;

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
            field_embeddings: Arc::new(RwLock::new(HashMap::new())),
            canonical_fields: Arc::new(RwLock::new(HashMap::new())),
            canonical_field_embeddings: Arc::new(RwLock::new(HashMap::new())),
            embedder: Arc::new(FastEmbedModel::new()),
            storage: SchemaStorage::Cloud {
                store: Arc::new(store),
            },
        };

        // Load schemas on initialization
        state.load_schemas().await?;
        state.rebuild_descriptive_name_index();
        state.rebuild_canonical_fields_from_schemas();

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

    // --- Canonical field registry ---

    /// Load canonical fields from a sled tree.
    fn load_canonical_fields_from_tree(&self, tree: &sled::Tree) -> FoldDbResult<()> {
        let mut fields = self.canonical_fields.write().map_err(|_| {
            FoldDbError::Config("Failed to acquire canonical_fields write lock".to_string())
        })?;
        let mut embeddings = self.canonical_field_embeddings.write().map_err(|_| {
            FoldDbError::Config("Failed to acquire canonical_field_embeddings write lock".to_string())
        })?;
        fields.clear();
        embeddings.clear();

        for result in tree.iter() {
            let (key, value) = result.map_err(|e| {
                FoldDbError::Config(format!("Failed to iterate canonical_fields: {}", e))
            })?;
            let name = String::from_utf8(key.to_vec()).map_err(|e| {
                FoldDbError::Config(format!("Invalid canonical field key: {}", e))
            })?;
            let desc = String::from_utf8(value.to_vec()).map_err(|e| {
                FoldDbError::Config(format!("Invalid canonical field description: {}", e))
            })?;
            let embed_text = Self::build_embedding_text(&name, &desc);
            if let Ok(vec) = self.embedder.embed_text(&embed_text) {
                embeddings.insert(name.clone(), vec);
            }
            fields.insert(name, desc);
        }

        log_feature!(
            LogFeature::Schema,
            info,
            "Loaded {} canonical fields from sled",
            fields.len()
        );
        Ok(())
    }

    /// Rebuild canonical fields from existing schemas (for cloud storage where
    /// there's no separate canonical_fields tree).
    #[cfg(feature = "aws-backend")]
    fn rebuild_canonical_fields_from_schemas(&self) {
        let schemas = match self.schemas.read() {
            Ok(s) => s,
            Err(_) => return,
        };
        let mut fields = match self.canonical_fields.write() {
            Ok(f) => f,
            Err(_) => return,
        };
        let mut embeddings = match self.canonical_field_embeddings.write() {
            Ok(e) => e,
            Err(_) => return,
        };
        fields.clear();
        embeddings.clear();

        for schema in schemas.values() {
            for field_name in schema.fields.as_deref().unwrap_or(&[]) {
                if !fields.contains_key(field_name) {
                    let desc = Self::build_field_description(field_name, schema);
                    let embed_text = Self::build_embedding_text(field_name, &desc);
                    if let Ok(vec) = self.embedder.embed_text(&embed_text) {
                        embeddings.insert(field_name.clone(), vec);
                    }
                    fields.insert(field_name.clone(), desc);
                }
            }
        }

        log_feature!(
            LogFeature::Schema,
            info,
            "Rebuilt {} canonical fields from schemas",
            fields.len()
        );
    }

    /// Persist a canonical field to sled storage.
    fn persist_canonical_field(&self, name: &str, description: &str) {
        match &self.storage {
            SchemaStorage::Sled { db, .. } => {
                if let Ok(tree) = db.open_tree("canonical_fields") {
                    let _ = tree.insert(name.as_bytes(), description.as_bytes());
                }
            }
            #[cfg(feature = "aws-backend")]
            SchemaStorage::Cloud { .. } => {
                // Cloud storage doesn't have a separate canonical_fields table;
                // canonical fields are rebuilt from schemas on startup.
            }
        }
    }

    /// Build embedding text from a field name and its description.
    /// Embeds "field_name: description" for richer semantic context than bare names.
    fn build_embedding_text(field_name: &str, description: &str) -> String {
        format!("{}: {}", field_name, description)
    }

    /// Build a description for a field from its schema context.
    /// Prefers AI-generated field_descriptions, falls back to field_classifications + descriptive_name.
    fn build_field_description(
        field_name: &str,
        schema: &Schema,
    ) -> String {
        let desc_name = schema.descriptive_name.as_deref().unwrap_or("unknown");

        // Prefer the AI-generated natural language description
        if let Some(desc) = schema.field_descriptions.get(field_name) {
            return format!("{} in {}", desc, desc_name);
        }

        // Fall back to classifications
        let classifications = schema
            .field_classifications
            .get(field_name)
            .map(|c| c.join(", "))
            .unwrap_or_default();

        if classifications.is_empty() {
            format!("field in {}", desc_name)
        } else {
            format!("{} field in {}", classifications, desc_name)
        }
    }

    /// Register new fields from a schema as canonical.
    /// Only adds fields that don't already exist in the registry.
    fn register_canonical_fields(&self, schema: &Schema) {
        let field_names = schema.fields.as_deref().unwrap_or(&[]);

        let mut fields = match self.canonical_fields.write() {
            Ok(f) => f,
            Err(_) => return,
        };
        let mut embeddings = match self.canonical_field_embeddings.write() {
            Ok(e) => e,
            Err(_) => return,
        };

        for field_name in field_names {
            if fields.contains_key(field_name) {
                continue;
            }
            let desc = Self::build_field_description(field_name, schema);
            let embed_text = Self::build_embedding_text(field_name, &desc);
            if let Ok(vec) = self.embedder.embed_text(&embed_text) {
                embeddings.insert(field_name.clone(), vec);
            }
            fields.insert(field_name.clone(), desc.clone());
            // Persist outside lock scope would be cleaner but we hold a write lock
            // on fields — persist uses a separate sled tree so no deadlock risk.
            self.persist_canonical_field(field_name, &desc);
        }
    }

    /// Canonicalize incoming field names against the global canonical field registry.
    /// Returns a rename map: incoming_field -> canonical_field.
    /// Uses the same bidirectional best-match + threshold approach as semantic_field_rename_map.
    /// Embeds "field_name: description" for richer semantic matching.
    fn canonicalize_fields(
        &self,
        incoming_fields: &[String],
        schema: &Schema,
        mutation_mappers: &mut HashMap<String, String>,
    ) -> HashMap<String, String> {
        let canonical = match self.canonical_fields.read() {
            Ok(c) => c,
            Err(_) => return HashMap::new(),
        };
        let embeddings = match self.canonical_field_embeddings.read() {
            Ok(e) => e,
            Err(_) => return HashMap::new(),
        };

        if canonical.is_empty() {
            return HashMap::new();
        }

        let mut rename_map: HashMap<String, String> = HashMap::new();
        let mut claimed: HashSet<String> = HashSet::new();

        for incoming_field in incoming_fields {
            // Don't rename if it already IS a canonical field
            if canonical.contains_key(incoming_field) {
                continue;
            }

            let incoming_desc = Self::build_field_description(incoming_field, schema);
            let incoming_embed_text = Self::build_embedding_text(incoming_field, &incoming_desc);
            let incoming_embedding = match self.embedder.embed_text(&incoming_embed_text) {
                Ok(vec) => vec,
                Err(_) => continue,
            };

            // Find best canonical match
            let mut best: Option<(&str, f32)> = None;
            for (canon_name, canon_vec) in embeddings.iter() {
                let sim = cosine_similarity(&incoming_embedding, canon_vec);
                if sim >= FIELD_SIMILARITY_THRESHOLD
                    && best.is_none_or(|(_, best_sim)| sim > best_sim)
                {
                    best = Some((canon_name.as_str(), sim));
                }
            }

            let Some((matched_canonical, _)) = best else {
                continue;
            };

            // Bidirectional check: is this incoming field the best match
            // for the canonical field too?
            let canon_vec = match embeddings.get(matched_canonical) {
                Some(v) => v,
                None => continue,
            };
            let mut reverse_best: Option<(&str, f32)> = None;
            for candidate in incoming_fields {
                let cand_desc = Self::build_field_description(candidate, schema);
                let cand_embed_text = Self::build_embedding_text(candidate, &cand_desc);
                if let Ok(cand_vec) = self.embedder.embed_text(&cand_embed_text) {
                    let sim = cosine_similarity(canon_vec, &cand_vec);
                    if reverse_best.is_none_or(|(_, best_sim)| sim > best_sim) {
                        reverse_best = Some((candidate.as_str(), sim));
                    }
                }
            }

            let is_mutual = reverse_best.is_some_and(|(best_incoming, _)| best_incoming == incoming_field);
            if is_mutual && !claimed.contains(matched_canonical) {
                log_feature!(
                    LogFeature::Schema,
                    info,
                    "Canonical field rename: '{}' -> '{}'",
                    incoming_field,
                    matched_canonical
                );
                rename_map.insert(incoming_field.clone(), matched_canonical.to_string());
                claimed.insert(matched_canonical.to_string());

                // Update mutation_mappers: incoming data key -> canonical field name
                if let Some(data_key) = mutation_mappers.remove(incoming_field) {
                    mutation_mappers.insert(data_key, matched_canonical.to_string());
                } else {
                    mutation_mappers.insert(incoming_field.clone(), matched_canonical.to_string());
                }
            }
        }

        rename_map
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
        let canonical_fields_tree = db
            .open_tree("canonical_fields")
            .map_err(|e| FoldDbError::Config(format!("Failed to open canonical_fields tree: {}", e)))?;

        let state = Self {
            schemas: Arc::new(RwLock::new(HashMap::new())),
            descriptive_name_index: Arc::new(RwLock::new(HashMap::new())),
            descriptive_name_embeddings: Arc::new(RwLock::new(HashMap::new())),
            field_embeddings: Arc::new(RwLock::new(HashMap::new())),
            canonical_fields: Arc::new(RwLock::new(HashMap::new())),
            canonical_field_embeddings: Arc::new(RwLock::new(HashMap::new())),
            embedder,
            storage: SchemaStorage::Sled { db, schemas_tree },
        };

        state.load_schemas_sync()?;
        state.rebuild_descriptive_name_index();
        state.load_canonical_fields_from_tree(&canonical_fields_tree)?;

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

    /// Check whether two schema names are semantically similar enough to be
    /// considered the same collection. Uses embedding similarity on the
    /// human-readable form of the names (underscores → spaces).
    ///
    /// This acts as a second gate for descriptive_name matching: even if
    /// "Holiday Illustration" ≈ "Famous Paintings" in embedding space, the
    /// schema names `artwork_collection` vs `famous_paintings` should NOT merge.
    fn schema_names_are_similar(&self, incoming: &str, existing: &str) -> bool {
        // Exact match (case-insensitive)
        if incoming.eq_ignore_ascii_case(existing) {
            return true;
        }

        // Convert snake_case to readable form for embedding comparison
        let readable_incoming = incoming.replace('_', " ");
        let readable_existing = existing.replace('_', " ");

        let incoming_emb = match self.embedder.embed_text(&readable_incoming) {
            Ok(v) => v,
            Err(_) => return false,
        };
        let existing_emb = match self.embedder.embed_text(&readable_existing) {
            Ok(v) => v,
            Err(_) => return false,
        };

        let sim = cosine_similarity(&incoming_emb, &existing_emb);
        log_feature!(
            LogFeature::Schema,
            info,
            "Schema name similarity: '{}' vs '{}' = {:.3}",
            incoming,
            existing,
            sim
        );
        // Use a high threshold — schema names are short and precise, so only
        // near-synonyms should match (e.g., "blog_posts" ≈ "blog_articles").
        sim >= 0.85
    }

    /// Get or compute the embedding for a context-enriched field name.
    /// Format: "the {field_name} of the {descriptive_name}"
    fn get_field_embedding(&self, field_name: &str, descriptive_name: &str) -> Option<Vec<f32>> {
        let cache_key = format!("{}:{}", descriptive_name, field_name);

        // Check cache first
        if let Ok(cache) = self.field_embeddings.read() {
            if let Some(vec) = cache.get(&cache_key) {
                return Some(vec.clone());
            }
        }

        // Compute and cache
        let context_text = format!("the {} of the {}", field_name, descriptive_name);
        match self.embedder.embed_text(&context_text) {
            Ok(vec) => {
                if let Ok(mut cache) = self.field_embeddings.write() {
                    cache.insert(cache_key, vec.clone());
                }
                Some(vec)
            }
            Err(e) => {
                log_feature!(
                    LogFeature::Schema,
                    warn,
                    "Failed to embed field '{}' with context '{}': {}",
                    field_name,
                    descriptive_name,
                    e
                );
                None
            }
        }
    }

    /// Find semantic field name matches between incoming and existing schemas.
    ///
    /// For fields in the incoming schema that don't have a literal match in the
    /// existing schema, uses context-enriched embeddings to detect synonyms
    /// (e.g., "creator" ≈ "artist" in an artwork context).
    ///
    /// Returns a map: incoming_field_name → existing_field_name (canonical).
    fn semantic_field_rename_map(
        &self,
        incoming_fields: &[String],
        existing_fields: &[String],
        descriptive_name: &str,
    ) -> HashMap<String, String> {
        let existing_set: HashSet<&String> = existing_fields.iter().collect();
        let mut rename_map: HashMap<String, String> = HashMap::new();
        // Track which existing fields have been claimed to avoid many-to-one mapping
        let mut claimed: HashSet<String> = HashSet::new();

        for incoming_field in incoming_fields {
            // Skip fields that already have a literal match
            if existing_set.contains(incoming_field) {
                continue;
            }

            let incoming_emb = match self.get_field_embedding(incoming_field, descriptive_name) {
                Some(v) => v,
                None => continue,
            };

            let mut best: Option<(&str, f32)> = None;
            for existing_field in existing_fields {
                if claimed.contains(existing_field) {
                    continue;
                }
                let existing_emb =
                    match self.get_field_embedding(existing_field, descriptive_name) {
                        Some(v) => v,
                        None => continue,
                    };
                let sim = cosine_similarity(&incoming_emb, &existing_emb);
                if sim >= FIELD_SIMILARITY_THRESHOLD
                    && best.as_ref().is_none_or(|(_, s)| sim > *s)
                {
                    best = Some((existing_field.as_str(), sim));
                }
            }

            if let Some((matched_field, similarity)) = best {
                // Bidirectional check: verify the existing field's best match among
                // all incoming fields is also this incoming field. This prevents false
                // positives like "medium"→"artist" when "creator"→"artist" is stronger.
                let existing_emb = self
                    .get_field_embedding(matched_field, descriptive_name)
                    .unwrap();
                let mut reverse_best: Option<(&str, f32)> = None;
                for candidate in incoming_fields {
                    if existing_set.contains(candidate) {
                        continue; // skip literal matches
                    }
                    let candidate_emb =
                        match self.get_field_embedding(candidate, descriptive_name) {
                            Some(v) => v,
                            None => continue,
                        };
                    let sim = cosine_similarity(&existing_emb, &candidate_emb);
                    if reverse_best.as_ref().is_none_or(|(_, s)| sim > *s) {
                        reverse_best = Some((candidate.as_str(), sim));
                    }
                }

                let is_mutual = reverse_best
                    .is_some_and(|(best_incoming, _)| best_incoming == incoming_field);

                if is_mutual {
                    log_feature!(
                        LogFeature::Schema,
                        info,
                        "Semantic field rename: '{}' → '{}' (similarity: {:.3}, context: '{}')",
                        incoming_field,
                        matched_field,
                        similarity,
                        descriptive_name
                    );
                    rename_map.insert(incoming_field.clone(), matched_field.to_string());
                    claimed.insert(matched_field.to_string());
                } else {
                    log_feature!(
                        LogFeature::Schema,
                        info,
                        "Rejected non-mutual match: '{}' → '{}' (similarity: {:.3}, but existing field's best match is '{}')",
                        incoming_field,
                        matched_field,
                        similarity,
                        reverse_best.map(|(f, _)| f).unwrap_or("none"),
                    );
                }
            } else {
                log_feature!(
                    LogFeature::Schema,
                    info,
                    "No semantic field match for '{}' in context '{}' — treating as new field",
                    incoming_field,
                    descriptive_name
                );
            }
        }

        rename_map
    }

    /// Apply field renames to a schema: rename fields, update classifications,
    /// mutation_mappers, and ref_fields to use canonical names.
    fn apply_field_renames(
        schema: &mut Schema,
        rename_map: &HashMap<String, String>,
        mutation_mappers: &mut HashMap<String, String>,
    ) {
        if rename_map.is_empty() {
            return;
        }

        // Rename in fields list
        if let Some(ref mut fields) = schema.fields {
            for field in fields.iter_mut() {
                if let Some(canonical) = rename_map.get(field) {
                    *field = canonical.clone();
                }
            }
        }

        // Rename in field_classifications
        for (old_name, canonical) in rename_map {
            if let Some(classifications) = schema.field_classifications.remove(old_name) {
                schema
                    .field_classifications
                    .entry(canonical.clone())
                    .or_insert(classifications);
            }
            // Add mutation_mapper: old_name → canonical so AI mutations still work
            mutation_mappers
                .entry(old_name.clone())
                .or_insert_with(|| canonical.clone());
        }
    }

    /// Expand an incoming schema to be a superset of an existing schema.
    ///
    /// Merges fields, sets field_mappers for shared fields (pointing to the old
    /// schema's molecules), merges classifications and ref_fields, recomputes
    /// identity_hash, persists, and updates caches.
    ///
    /// Returns `SchemaAddOutcome::Expanded` on success, or `AlreadyExists` if
    /// the incoming fields are a subset of the existing.
    async fn expand_schema(
        &self,
        schema: &mut Schema,
        existing: &Schema,
        old_name: &str,
        desc_name: &str,
        mutation_mappers: &HashMap<String, String>,
    ) -> FoldDbResult<SchemaAddOutcome> {
        let existing_fields = existing.fields.clone().unwrap_or_default();
        let existing_set: HashSet<String> = existing_fields.iter().cloned().collect();
        let new_field_set: HashSet<String> = schema
            .fields
            .as_ref()
            .map(|nf| nf.iter().cloned().collect())
            .unwrap_or_default();

        // If the new schema's fields are a subset of the existing, reuse existing
        if new_field_set.is_subset(&existing_set) {
            log_feature!(
                LogFeature::Schema,
                info,
                "New schema is a subset of existing '{}' (descriptive_name='{}') — reusing existing",
                old_name,
                desc_name
            );
            return Ok(SchemaAddOutcome::AlreadyExists(existing.clone(), mutation_mappers.clone()));
        }

        log_feature!(
            LogFeature::Schema,
            info,
            "Expanding schema (descriptive_name='{}') — merging fields from old hash '{}'",
            desc_name,
            old_name
        );

        // Merge to superset: existing fields + new-only fields
        let new_fields_to_add: Vec<String> = new_field_set
            .difference(&existing_set)
            .cloned()
            .collect();
        let mut merged_fields = existing_fields.clone();
        merged_fields.extend(new_fields_to_add);
        schema.fields = Some(merged_fields);

        // Set field_mappers for shared fields (pointing to old schema's molecules)
        use fold_db::schema::types::declarative_schemas::FieldMapper;
        let mut mappers: HashMap<String, FieldMapper> = schema
            .field_mappers()
            .cloned()
            .unwrap_or_default();
        for field in &existing_fields {
            mappers.entry(field.clone()).or_insert_with(|| {
                FieldMapper::new(old_name.to_string(), field.clone())
            });
        }
        schema.field_mappers = Some(mappers);
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

        // Recompute identity hash with merged fields.
        // The expanded schema is a NEW schema — its name is the identity hash
        // (derived from schema name + fields). The old schema keeps its name and
        // gets blocked/superseded. Field mappers point back to the old schema.
        schema.compute_identity_hash();
        let new_hash = schema
            .get_identity_hash()
            .ok_or_else(|| {
                FoldDbError::Config("Failed to compute merged identity_hash".to_string())
            })?
            .clone();
        schema.name = new_hash.clone();
        let expanded_name = schema.name.clone();

        // Persist expanded schema
        self.persist_schema(schema, mutation_mappers).await?;

        // Update in-memory cache
        {
            let mut schemas = self.schemas.write().map_err(|_| {
                FoldDbError::Config("Failed to acquire schemas write lock".to_string())
            })?;
            schemas.insert(expanded_name.clone(), schema.clone());
        }

        // Update descriptive_name index to point to expanded schema
        {
            let mut index = self.descriptive_name_index.write().map_err(|_| {
                FoldDbError::Config("Failed to acquire descriptive_name_index write lock".to_string())
            })?;
            index.insert(desc_name.to_string(), expanded_name);
        }

        // Register new fields as canonical for future schema proposals
        self.register_canonical_fields(schema);

        log_feature!(
            LogFeature::Schema,
            info,
            "Schema expanded: old='{}' (blocked) -> new='{}' (descriptive_name='{}')",
            old_name,
            schema.name,
            desc_name
        );

        Ok(SchemaAddOutcome::Expanded(
            old_name.to_string(),
            schema.clone(),
            mutation_mappers.clone(),
        ))
    }

    pub async fn add_schema(
        &self,
        mut schema: Schema,
        mut mutation_mappers: HashMap<String, String>,
    ) -> FoldDbResult<SchemaAddOutcome> {
        // descriptive_name is required — it's how schemas are identified, displayed,
        // and matched for expansion. A schema without one is a bug in the caller.
        if schema.descriptive_name.as_ref().is_none_or(|dn| dn.trim().is_empty()) {
            return Err(FoldDbError::Config(
                "Schema must have a non-empty descriptive_name".to_string(),
            ));
        }

        // field_descriptions is required — the schema service uses them for
        // semantic field matching (embedding "field_name: description").
        // Without descriptions, field matching degrades to bare name comparison.
        if let Some(ref fields) = schema.fields {
            let missing: Vec<&String> = fields
                .iter()
                .filter(|f| !schema.field_descriptions.contains_key(*f))
                .collect();
            if !missing.is_empty() {
                return Err(FoldDbError::Config(format!(
                    "Schema fields missing descriptions (required for semantic matching): {:?}",
                    missing
                )));
            }
        }

        // Canonicalize field names against the global canonical field registry
        // before any dedup or identity hash computation.
        if let Some(ref fields) = schema.fields {
            let rename_map = self.canonicalize_fields(fields, &schema, &mut mutation_mappers);
            if !rename_map.is_empty() {
                Self::apply_field_renames(&mut schema, &rename_map, &mut mutation_mappers);
            }
        }

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

        // Schema name is ALWAYS the identity_hash (hash of semantic name + fields).
        // This guarantees:
        // - Same semantic name + same fields = same hash = dedup
        // - Same semantic name + different fields = different hash = separate schemas
        // - Different semantic name + same fields = different hash = separate schemas
        // The human-readable name lives in descriptive_name (for display/search).
        let schema_name = identity_hash.clone();

        // Check if this exact schema already exists (same name)
        {
            let schemas = self.schemas.read().map_err(|_| {
                FoldDbError::Config("Failed to acquire schemas read lock".to_string())
            })?;

            if let Some(existing_schema) = schemas.get(&schema_name) {
                // If this schema has been superseded by expansion, redirect to the
                // current active schema for the subset/expansion check.
                let (check_schema, check_name) = if let Some(ref desc_name) = existing_schema.descriptive_name {
                    let index = self.descriptive_name_index.read().map_err(|_| {
                        FoldDbError::Config("Failed to acquire descriptive_name_index read lock".to_string())
                    })?;
                    if let Some(current_hash) = index.get(desc_name) {
                        if *current_hash != schema_name {
                            if let Some(active_schema) = schemas.get(current_hash) {
                                log_feature!(
                                    LogFeature::Schema,
                                    info,
                                    "Schema '{}' was superseded by '{}' — checking active schema",
                                    schema_name,
                                    current_hash
                                );
                                (active_schema.clone(), current_hash.clone())
                            } else {
                                (existing_schema.clone(), schema_name.clone())
                            }
                        } else {
                            (existing_schema.clone(), schema_name.clone())
                        }
                    } else {
                        (existing_schema.clone(), schema_name.clone())
                    }
                } else {
                    (existing_schema.clone(), schema_name.clone())
                };

                // Check if the incoming schema has new fields not in the target schema.
                // If so, fall through to expansion instead of returning AlreadyExists.
                let existing_fields: HashSet<String> = check_schema
                    .fields
                    .as_ref()
                    .map(|f| f.iter().cloned().collect())
                    .unwrap_or_default();
                let incoming_fields: HashSet<String> = schema
                    .fields
                    .as_ref()
                    .map(|f| f.iter().cloned().collect())
                    .unwrap_or_default();
                let has_new_fields = !incoming_fields.is_subset(&existing_fields);

                if has_new_fields {
                    log_feature!(
                        LogFeature::Schema,
                        info,
                        "Schema '{}' (active='{}') has new fields {:?} — expanding",
                        schema_name,
                        check_name,
                        incoming_fields.difference(&existing_fields).collect::<Vec<_>>()
                    );
                    // Fall through to expansion path below
                } else {
                    log_feature!(
                        LogFeature::Schema,
                        info,
                        "Schema '{}' already exists with same fields (active='{}') - returning existing",
                        schema_name,
                        check_name
                    );

                    return Ok(SchemaAddOutcome::AlreadyExists(check_schema, mutation_mappers.clone()));
                }
            }
        }

        // Check for schema expansion: if the new schema has a descriptive_name that
        // matches an existing schema (exact or semantic), merge fields (expand, never shrink).
        if let Some(incoming_desc_name) = schema.descriptive_name.clone() {
            let (matched_desc, existing_schema_name, is_exact_match) = self.find_matching_descriptive_name(&incoming_desc_name)?;

            // For semantic (non-exact) matches, also compare schema names.
            // "holiday_illustrations" and "famous_paintings" have similar descriptive names
            // (both art-related) but are clearly different collections. Only merge when
            // both the descriptive names AND the schema names are semantically close.
            let should_merge = if let Some(ref old_name) = existing_schema_name {
                if is_exact_match {
                    true
                } else {
                    // Compare schema names as a second gate
                    self.schema_names_are_similar(&schema_name, old_name)
                }
            } else {
                false
            };

            if should_merge {
            let old_name = existing_schema_name.unwrap();
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

                    // Semantic field matching: detect synonyms like "creator" ≈ "artist"
                    // and rename incoming fields to canonical names before expansion.
                    let incoming_fields = schema.fields.clone().unwrap_or_default();
                    let rename_map = self.semantic_field_rename_map(
                        &incoming_fields,
                        &existing_fields,
                        &desc_name,
                    );
                    let mut mutation_mappers = mutation_mappers;
                    Self::apply_field_renames(&mut schema, &rename_map, &mut mutation_mappers);

                    // Deduplicate fields after renaming (renamed fields may now
                    // duplicate existing ones)
                    schema.dedup_fields();

                    return self
                        .expand_schema(&mut schema, &existing, &old_name, &desc_name, &mutation_mappers)
                        .await;
                }
            }
        }

        // No field-overlap fallback needed — descriptive_name is required (validated above),
        // so descriptive_name matching handles all expansion cases.

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

        // Register new fields as canonical for future schema proposals
        self.register_canonical_fields(&schema);

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
