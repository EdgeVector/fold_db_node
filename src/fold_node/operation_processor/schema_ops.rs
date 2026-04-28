use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::schema::types::field::Field;
use fold_db::schema::{SchemaError, SchemaWithState};

use super::OperationProcessor;

impl OperationProcessor {
    /// List active schemas with their states (excludes Blocked/superseded schemas).
    pub async fn list_schemas(&self) -> FoldDbResult<Vec<SchemaWithState>> {
        let db = self.get_db()?;
        Ok(db.schema_manager().get_active_schemas_with_states()?)
    }

    /// Get a specific schema by name with its state.
    pub async fn get_schema(&self, name: &str) -> FoldDbResult<Option<SchemaWithState>> {
        let db = self.get_db()?;
        let mgr = &db.schema_manager();
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
        db.schema_manager().approve(schema_name).await?;
        drop(db); // Release lock before apply_classification_defaults acquires it

        // Auto-apply access policies based on data classification
        match self.apply_classification_defaults(schema_name).await {
            Ok(applied) => {
                if applied > 0 {
                    tracing::info!(
                    target: "fold_node::schema",
                                "Applied access policies to {} fields in '{}'",
                                applied,
                                schema_name
                            );
                }
            }
            Err(e) => {
                tracing::warn!(
                target: "fold_node::schema",
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
        Ok(db.schema_manager().block_schema(schema_name).await?)
    }

    /// Tag an existing schema with an org_hash, or clear it with `None`.
    ///
    /// When set, subsequent writes to this schema partition onto the org sync
    /// log (key prefix `{org_hash}:…`) instead of the personal one. Also sets
    /// `trust_domain` to `org:{org_hash}` so field access policies inherit the
    /// org domain by default (symmetric with `populate_runtime_fields`).
    ///
    /// Does NOT rewrite molecules already stored under the personal prefix —
    /// only affects writes performed after the tag lands.
    pub async fn set_schema_org_hash(
        &self,
        schema_name: &str,
        org_hash: Option<String>,
    ) -> FoldDbResult<()> {
        let db = self.get_db()?;
        let mgr = db.schema_manager();
        let mut schema = mgr
            .get_schema_metadata(schema_name)?
            .ok_or_else(|| FoldDbError::Schema(SchemaError::NotFound(schema_name.to_string())))?;

        schema.org_hash = org_hash.clone();
        schema.trust_domain = org_hash.as_ref().map(|h| format!("org:{}", h));

        // Mirror populate_runtime_fields's propagation so in-memory writes use
        // the new prefix too (field-level storage_key() reads FieldCommon.org_hash).
        for field in schema.runtime_fields.values_mut() {
            field.common_mut().set_org_hash(schema.org_hash.clone());
        }

        mgr.update_schema(&schema).await?;
        Ok(())
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
                db.schema_manager().load_schema_internal(schema).await
            };

            match result {
                Ok(_) => loaded_count += 1,
                Err(e) => {
                    tracing::error!("Failed to load schema {}: {}", schema_name, e);
                    failed_schemas.push(schema_name);
                }
            }
        }

        Ok((schema_count, loaded_count, failed_schemas))
    }
}
