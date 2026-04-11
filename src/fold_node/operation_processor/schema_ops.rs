use fold_db::error::FoldDbResult;
use fold_db::schema::SchemaWithState;

use super::OperationProcessor;

impl OperationProcessor {
    /// List active schemas with their states (excludes Blocked/superseded schemas).
    pub async fn list_schemas(&self) -> FoldDbResult<Vec<SchemaWithState>> {
        let db = self.get_db()?;
        Ok(db.schema_manager.get_active_schemas_with_states()?)
    }

    /// Get a specific schema by name with its state.
    pub async fn get_schema(&self, name: &str) -> FoldDbResult<Option<SchemaWithState>> {
        let db = self.get_db()?;
        let mgr = &db.schema_manager;
        match mgr.get_schema_metadata(name)? {
            Some(schema) => {
                let states = mgr.get_schema_states()?;
                let state = states.get(name).copied().unwrap_or_default();
                Ok(Some(SchemaWithState::new(schema, state)))
            }
            None => Ok(None),
        }
    }

    /// Approve a schema and apply classification-based default access policies.
    pub async fn approve_schema(&self, schema_name: &str) -> FoldDbResult<()> {
        let db = self.get_db()?;
        db.schema_manager.approve(schema_name).await?;
        drop(db); // Release lock before apply_classification_defaults acquires it

        // Auto-apply access policies based on data classification
        match self.apply_classification_defaults(schema_name).await {
            Ok(applied) => {
                if applied > 0 {
                    fold_db::log_feature!(
                        fold_db::logging::features::LogFeature::Schema,
                        info,
                        "Applied access policies to {} fields in '{}'",
                        applied,
                        schema_name
                    );
                }
            }
            Err(e) => {
                fold_db::log_feature!(
                    fold_db::logging::features::LogFeature::Schema,
                    warn,
                    "Failed to apply classification defaults to '{}': {}",
                    schema_name,
                    e
                );
            }
        }
        Ok(())
    }

    /// Block a schema.
    pub async fn block_schema(&self, schema_name: &str) -> FoldDbResult<()> {
        let db = self.get_db()?;
        Ok(db.schema_manager.block_schema(schema_name).await?)
    }

    /// Load schemas from the schema service.
    /// Returns (available_count, loaded_count, failed_schemas).
    pub async fn load_schemas(&self) -> FoldDbResult<(usize, usize, Vec<String>)> {
        let schemas = self.node.fetch_available_schemas().await?;
        let schema_count = schemas.len();
        let mut loaded_count = 0;
        let mut failed_schemas = Vec::new();

        for schema in schemas {
            let schema_name = schema.name.clone();
            let result = {
                let db = self.get_db()?;
                db.schema_manager.load_schema_internal(schema).await
            };

            match result {
                Ok(_) => loaded_count += 1,
                Err(e) => {
                    log::error!("Failed to load schema {}: {}", schema_name, e);
                    failed_schemas.push(schema_name);
                }
            }
        }

        Ok((schema_count, loaded_count, failed_schemas))
    }
}
