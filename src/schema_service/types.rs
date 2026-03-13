use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use fold_db::schema::types::Schema;

/// Response containing a list of available schema names
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchemasListResponse {
    pub schemas: Vec<String>,
}

/// Response containing all available schemas with their definitions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AvailableSchemasResponse {
    pub schemas: Vec<Schema>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SchemaAddOutcome {
    Added(Schema, HashMap<String, String>), // Schema and mutation_mappers
    AlreadyExists(Schema, HashMap<String, String>), // Exact same identity hash + mappers from canonicalization
    /// Existing schema was expanded with new fields (old schema name, expanded schema, mappers)
    Expanded(String, Schema, HashMap<String, String>),
}

/// Error response structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub error: String,
}

/// Request structure for adding a schema with mutation mappers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddSchemaRequest {
    pub schema: Schema,
    pub mutation_mappers: HashMap<String, String>,
}

/// Response structure for adding a schema with mutation mappers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddSchemaResponse {
    pub schema: Schema,
    pub mutation_mappers: HashMap<String, String>,
    /// When a schema expansion occurred, this contains the old schema name
    /// that was replaced. The node should remove the old schema and load the new one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replaced_schema: Option<String>,
}

/// Reload response structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReloadResponse {
    pub success: bool,
    pub schemas_loaded: usize,
}

/// Health check response structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthResponse {
    pub status: String,
}

/// A schema entry with its similarity score
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarSchemaEntry {
    pub schema: Schema,
    pub similarity: f64,
}

/// Response for the find-similar-schemas endpoint
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimilarSchemasResponse {
    pub query_schema: String,
    pub threshold: f64,
    pub similar_schemas: Vec<SimilarSchemaEntry>,
}

/// Request for resetting the schema service database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetRequest {
    pub confirm: bool,
}

/// Response for resetting the schema service database
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResetResponse {
    pub success: bool,
    pub message: String,
}
