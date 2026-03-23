use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::schema::types::field_value_type::FieldValueType;
use fold_db::schema::types::key_config::KeyConfig;
use fold_db::schema::types::operations::Query;
use fold_db::schema::types::schema::DeclarativeSchemaType as SchemaType;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::OperationProcessor;

/// Local view state — views are managed by the schema_service, not SchemaCore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ViewState {
    Pending,
    Approved,
    Blocked,
}

/// Local transform view definition — views are managed by the schema_service, not SchemaCore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransformView {
    pub name: String,
    pub schema_type: SchemaType,
    pub key_config: Option<KeyConfig>,
    pub input_queries: Vec<Query>,
    pub wasm_transform: Option<Vec<u8>>,
    pub output_fields: HashMap<String, FieldValueType>,
}

impl TransformView {
    pub fn new(
        name: String,
        schema_type: SchemaType,
        key_config: Option<KeyConfig>,
        input_queries: Vec<Query>,
        wasm_transform: Option<Vec<u8>>,
        output_fields: HashMap<String, FieldValueType>,
    ) -> Self {
        Self {
            name,
            schema_type,
            key_config,
            input_queries,
            wasm_transform,
            output_fields,
        }
    }
}

impl OperationProcessor {
    /// List all views with their states.
    pub async fn list_views(&self) -> FoldDbResult<Vec<(TransformView, ViewState)>> {
        Err(FoldDbError::Config(
            "Views are managed by the schema_service, not the local node".to_string(),
        ))
    }

    /// Get a specific view by name.
    pub async fn get_view(&self, _name: &str) -> FoldDbResult<Option<TransformView>> {
        Err(FoldDbError::Config(
            "Views are managed by the schema_service, not the local node".to_string(),
        ))
    }

    /// Register a new transform view.
    pub async fn create_view(&self, _view: TransformView) -> FoldDbResult<()> {
        Err(FoldDbError::Config(
            "Views are managed by the schema_service, not the local node".to_string(),
        ))
    }

    /// Approve a view for queries and mutations.
    pub async fn approve_view(&self, _name: &str) -> FoldDbResult<()> {
        Err(FoldDbError::Config(
            "Views are managed by the schema_service, not the local node".to_string(),
        ))
    }

    /// Block a view from queries and mutations.
    pub async fn block_view(&self, _name: &str) -> FoldDbResult<()> {
        Err(FoldDbError::Config(
            "Views are managed by the schema_service, not the local node".to_string(),
        ))
    }

    /// Delete (remove) a view and clean up storage.
    pub async fn delete_view(&self, _name: &str) -> FoldDbResult<()> {
        Err(FoldDbError::Config(
            "Views are managed by the schema_service, not the local node".to_string(),
        ))
    }

    /// Load a view from the global schema service, including all transitive
    /// dependencies (source schemas and source views).
    pub async fn load_view(
        &self,
        name: &str,
    ) -> FoldDbResult<crate::fold_node::node::ViewLoadResult> {
        self.node.load_view_from_service(name).await
    }
}
