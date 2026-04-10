//! Shared Mutation Handlers
//!
//! Framework-agnostic handlers for mutation operations.
//! These can be called by both HTTP server routes and Lambda handlers.

use crate::fold_node::node::FoldNode;
use crate::fold_node::OperationProcessor;
use crate::handlers::handler_response;
use crate::handlers::response::{ApiResponse, HandlerResult, IntoTypedHandlerError};
use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::{Mutation, MutationType};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// Default timeout for waiting on background tasks after batch mutations.
const DEFAULT_BACKGROUND_TASK_TIMEOUT: Duration = Duration::from_secs(5);

handler_response! {
    /// Response for mutation execution
    pub struct MutationResponse {
        /// The mutation IDs that were executed
        pub mutation_ids: Vec<String>,
        /// Number of mutations executed
        pub count: usize,
    }
}

handler_response! {
    /// Single mutation response (for backward compatibility with existing API)
    pub struct SingleMutationResponse {
        /// The mutation ID
        pub mutation_id: String,
        /// Success flag
        pub success: bool,
    }
}

/// Execute a single mutation
pub async fn execute_mutation(
    mutation: Mutation,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<MutationResponse> {
    execute_mutations_batch(vec![mutation], user_hash, node).await
}

/// Execute a single mutation from components (used by HTTP server).
/// Access control: checks write permissions via the caller's trust distances.
pub async fn execute_mutation_from_components(
    schema: String,
    fields_and_values: HashMap<String, Value>,
    key_value: KeyValue,
    mutation_type: MutationType,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<SingleMutationResponse> {
    let processor = OperationProcessor::new(node.clone());
    let caller_pub_key = node.get_node_public_key().to_string();

    let mutation = Mutation::new(
        schema,
        fields_and_values,
        key_value,
        caller_pub_key.clone(),
        mutation_type,
    );
    let mutation_id = processor
        .execute_mutation_op_with_access(mutation, &caller_pub_key)
        .await
        .typed_handler_err()?;

    Ok(ApiResponse::success_with_user(
        SingleMutationResponse {
            mutation_id,
            success: true,
        },
        user_hash,
    ))
}

/// Execute multiple mutations in a batch
pub async fn execute_mutations_batch(
    mutations: Vec<Mutation>,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<MutationResponse> {
    let count = mutations.len();

    let mutation_ids = node.mutate_batch(mutations).await.typed_handler_err()?;

    // Wait for background tasks (indexing) to complete
    node.wait_for_background_tasks(DEFAULT_BACKGROUND_TASK_TIMEOUT)
        .await;

    Ok(ApiResponse::success_with_user(
        MutationResponse {
            mutation_ids,
            count,
        },
        user_hash,
    ))
}

/// Execute mutations batch from JSON values (used by HTTP server)
pub async fn execute_mutations_batch_from_json(
    mutations_data: Vec<Value>,
    user_hash: &str,
    node: &FoldNode,
) -> HandlerResult<MutationResponse> {
    let processor = OperationProcessor::new(node.clone());
    let count = mutations_data.len();

    let mutation_ids = processor
        .execute_mutations_batch(mutations_data)
        .await
        .typed_handler_err()?;

    Ok(ApiResponse::success_with_user(
        MutationResponse {
            mutation_ids,
            count,
        },
        user_hash,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mutation_response_serialization() {
        let response = MutationResponse {
            mutation_ids: vec!["id1".to_string()],
            count: 1,
        };
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("mutation_ids"));
        assert!(json.contains("count"));
    }
}
