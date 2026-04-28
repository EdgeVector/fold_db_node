use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::schema::types::operations::{MutationType, Operation};
use fold_db::schema::types::{KeyValue, Mutation};
use serde_json::Value;
use std::collections::HashMap;

use super::OperationProcessor;

impl OperationProcessor {
    /// Execute a mutation with access control.
    /// Builds AccessContext from caller's public key and checks write permissions.
    pub async fn execute_mutation_op_with_access(
        &self,
        mutation: Mutation,
        caller_pub_key: &str,
    ) -> FoldDbResult<String> {
        let access_context = self.build_access_context(caller_pub_key).await?;

        let db = self.get_db()?;
        let mut ids = db
            .mutation_manager()
            .write_mutations_with_access(vec![mutation], &access_context, None)
            .await
            .map_err(Self::mutation_write_error)?;

        ids.pop()
            .ok_or_else(|| Self::mutation_write_error("Batch mutation returned no IDs"))
    }
}

impl OperationProcessor {
    /// Map a mutation write error to FoldDbError with logging.
    fn mutation_write_error(e: impl std::fmt::Display) -> FoldDbError {
        tracing::error!(
            target: "fold_node::mutation",
            "Mutation execution failed: {}",
            e
        );
        FoldDbError::Config(format!("Mutation execution failed: {}", e))
    }

    /// Executes a mutation operation from a Mutation struct.
    pub async fn execute_mutation_op(&self, mutation: Mutation) -> FoldDbResult<String> {
        tracing::info!(
            target: "fold_node::mutation",
            "Executing mutation for schema: {}",
            mutation.schema_name
        );

        let db = self.get_db()?;
        let mut ids = db
            .mutation_manager()
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
        let db = self.get_db()?;
        let mutation_ids = db
            .mutation_manager()
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
