use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::schema::types::operations::{MutationType, Operation};
use fold_db::schema::types::{KeyValue, Mutation};
use serde_json::Value;
use std::collections::HashMap;

use super::OperationProcessor;

impl OperationProcessor {
    /// Map a mutation write error to FoldDbError with logging.
    fn mutation_write_error(e: impl std::fmt::Display) -> FoldDbError {
        log_feature!(
            LogFeature::Mutation,
            error,
            "Mutation execution failed: {}",
            e
        );
        FoldDbError::Config(format!("Mutation execution failed: {}", e))
    }

    /// Executes a mutation operation from a Mutation struct.
    pub async fn execute_mutation_op(&self, mutation: Mutation) -> FoldDbResult<String> {
        log_feature!(
            LogFeature::Mutation,
            info,
            "Executing mutation for schema: {}",
            mutation.schema_name
        );

        let mut db = self.get_db().await?;
        let mut ids = db
            .mutation_manager
            .write_mutations_batch_async(vec![mutation])
            .await
            .map_err(Self::mutation_write_error)?;

        ids.pop()
            .ok_or_else(|| Self::mutation_write_error("Batch mutation returned no IDs"))
    }

    /// Executes a mutation operation (legacy wrapper).
    pub async fn execute_mutation(
        &self,
        schema: String,
        fields_and_values: HashMap<String, Value>,
        key_value: KeyValue,
        mutation_type: MutationType,
    ) -> FoldDbResult<String> {
        // Delete mutations are allowed to have empty fields_and_values
        if fields_and_values.is_empty() && mutation_type != MutationType::Delete {
            return Err(FoldDbError::Config("No fields to mutate".to_string()));
        }

        let mutation = Mutation::new(
            schema,
            fields_and_values,
            key_value,
            String::new(),
            mutation_type,
        );

        self.execute_mutation_op(mutation).await
    }

    /// Executes multiple mutations in a batch from Mutation structs.
    pub async fn execute_mutations_batch_ops(
        &self,
        mutations: Vec<Mutation>,
    ) -> FoldDbResult<Vec<String>> {
        let mut db = self.get_db().await?;
        let mutation_ids = db
            .mutation_manager
            .write_mutations_batch_async(mutations)
            .await
            .map_err(Self::mutation_write_error)?;
        Ok(mutation_ids)
    }

    /// Executes multiple mutations in a batch for improved performance (from JSON).
    pub async fn execute_mutations_batch(
        &self,
        mutations_data: Vec<Value>,
    ) -> FoldDbResult<Vec<String>> {
        if mutations_data.is_empty() {
            return Ok(Vec::new());
        }

        let mut mutations = Vec::new();

        // Parse each mutation from the input data
        for mutation_data in mutations_data {
            let (schema, fields_and_values, key_value, mutation_type, source_file_name) =
                match serde_json::from_value::<Operation>(mutation_data) {
                    Ok(Operation::Mutation {
                        schema,
                        fields_and_values,
                        key_value,
                        mutation_type,
                        source_file_name,
                    }) => (
                        schema,
                        fields_and_values,
                        key_value,
                        mutation_type,
                        source_file_name,
                    ),
                    Err(e) => {
                        return Err(FoldDbError::Config(format!(
                            "Failed to parse mutation: {}",
                            e
                        )));
                    }
                };

            // Delete mutations are allowed to have empty fields_and_values
            if fields_and_values.is_empty() && mutation_type != MutationType::Delete {
                return Err(FoldDbError::Config("No fields to mutate".to_string()));
            }

            let mut mutation = Mutation::new(
                schema,
                fields_and_values,
                key_value,
                String::new(),
                mutation_type,
            );

            // Add source_file_name if provided
            if let Some(filename) = source_file_name {
                mutation = mutation.with_source_file_name(filename);
            }

            mutations.push(mutation);
        }

        self.execute_mutations_batch_ops(mutations).await
    }
}
