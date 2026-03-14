//! Native index search, interpretation, and alternative query suggestion.

use super::super::types::{QueryPlan, ToolCallRecord};
use fold_db::schema::types::Query;
use serde_json::Value;

use super::LlmQueryService;

/// Expand `~` or `~/...` to the user's home directory.
fn expand_home_path(path: &str) -> std::path::PathBuf {
    if path.starts_with("~/") {
        dirs::home_dir()
            .map(|h| h.join(&path[2..]))
            .unwrap_or_else(|| std::path::PathBuf::from(path))
    } else if path == "~" {
        dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from(path))
    } else {
        std::path::PathBuf::from(path)
    }
}

/// Update agent progress if a tracker is available. Best-effort — errors are silently ignored.
async fn update_agent_progress(
    tracker: Option<&crate::ingestion::ProgressTracker>,
    job_id: &str,
    pct: u8,
    message: String,
) {
    if let Some(tracker) = tracker {
        if let Ok(Some(mut job)) = tracker.load(job_id).await {
            job.update_progress(pct, message);
            let _ = tracker.save(&job).await;
        }
    }
}

impl LlmQueryService {
    /// Generate query terms for native index search based on a natural language query
    pub async fn generate_native_index_query_terms(
        &self,
        user_query: &str,
        schemas: &[fold_db::schema::SchemaWithState],
    ) -> Result<Vec<String>, String> {
        let prompt = self.build_native_index_query_terms_prompt(user_query, schemas);
        let response = self.call_llm(&prompt).await?;
        self.parse_query_terms_response(&response)
    }

    /// Search the native index and return results (without AI interpretation)
    ///
    /// This is the first step of the AI-native index query workflow.
    /// Call `interpret_native_index_results` separately to get AI interpretation.
    pub async fn search_native_index(
        &self,
        user_query: &str,
        schemas: &[fold_db::schema::SchemaWithState],
        db_ops: &fold_db::db_operations::DbOperations,
    ) -> Result<Vec<fold_db::db_operations::IndexResult>, String> {
        // Step 1: Generate native index search terms using AI
        let search_terms = self
            .generate_native_index_search_terms(user_query, schemas)
            .await?;

        // Step 2: Execute native index searches for each term
        let mut all_results = Vec::new();
        if let Some(native_index_mgr) = db_ops.native_index_manager() {
            for term in &search_terms {
                match native_index_mgr
                    .search_all_classifications(term)
                    .await
                {
                    Ok(mut results) => {
                        log::debug!(
                            "LLM Query: Term '{}' returned {} results",
                            term,
                            results.len()
                        );
                        all_results.append(&mut results);
                    }
                    Err(e) => {
                        log::warn!("Native index search failed for term '{}': {}", term, e);
                    }
                }
            }
        }

        log::info!(
            "LLM Query: Found {} results from native index",
            all_results.len()
        );

        Ok(all_results)
    }

    /// Generate native index search terms specifically for search execution
    async fn generate_native_index_search_terms(
        &self,
        user_query: &str,
        schemas: &[fold_db::schema::SchemaWithState],
    ) -> Result<Vec<String>, String> {
        let prompt = self.build_native_index_search_prompt(user_query, schemas);
        let response = self.call_llm(&prompt).await?;
        self.parse_query_terms_response(&response)
    }

    /// Interpret native index search results using AI
    ///
    /// This method takes search results (potentially hydrated with actual values)
    /// and sends them to the AI for interpretation and summarization.
    pub async fn interpret_native_index_results(
        &self,
        original_query: &str,
        results: &[fold_db::db_operations::IndexResult],
    ) -> Result<String, String> {
        log::info!(
            "LLM Query: Sending {} results to AI for interpretation",
            results.len()
        );
        if results.is_empty() {
            log::warn!("LLM Query: No results to send to AI");
        } else {
            log::debug!(
                "LLM Query: Sample result - schema={}, field={}, key_value={:?}",
                results[0].schema_name,
                results[0].field,
                results[0].key_value
            );
        }
        let prompt = self.build_native_index_interpretation_prompt(original_query, results);
        self.call_llm(&prompt).await
    }

    /// Suggest alternative query strategies when results are empty
    pub async fn suggest_alternative_query(
        &self,
        original_user_query: &str,
        failed_query: &Query,
        schemas: &[fold_db::schema::SchemaWithState],
        previous_attempts: &[String],
    ) -> Result<Option<QueryPlan>, String> {
        let prompt = self.build_alternative_query_prompt(
            original_user_query,
            failed_query,
            schemas,
            previous_attempts,
        );
        let response = self.call_llm(&prompt).await?;
        self.parse_alternative_query(&response)
    }

    /// Execute a tool call and return the result
    pub(super) async fn execute_tool(
        &self,
        tool: &str,
        params: &Value,
        node: &crate::fold_node::node::FoldNode,
        progress_tracker: Option<&crate::ingestion::ProgressTracker>,
    ) -> Result<Value, String> {
        let processor = crate::fold_node::OperationProcessor::new(node.clone());

        match tool {
            "query" => {
                let schema_name = params
                    .get("schema_name")
                    .and_then(|s| s.as_str())
                    .ok_or("query tool requires 'schema_name' parameter")?;

                let mut fields: Vec<String> = params
                    .get("fields")
                    .and_then(|f| f.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();

                // When the agent omits fields, default to all fields from the schema
                if fields.is_empty() {
                    if let Ok(Some(schema_with_state)) = processor.get_schema(schema_name).await {
                        fields = schema_with_state.schema.runtime_fields.keys().cloned().collect();
                    }
                }

                let filter = params.get("filter").cloned();
                let sort_order = params.get("sort_order").cloned();

                let query = Query {
                    schema_name: schema_name.to_string(),
                    fields,
                    filter: filter.and_then(|f| serde_json::from_value(f).ok()),
                    as_of: None,
                    rehydrate_depth: Some(1),
                    sort_order: sort_order.and_then(|s| serde_json::from_value(s).ok()),
                };

                let results = processor
                    .execute_query_json(query)
                    .await
                    .map_err(|e| format!("Query execution failed: {}", e))?;

                Ok(Value::Array(results))
            }

            "list_schemas" => {
                let schemas = processor
                    .list_schemas()
                    .await
                    .map_err(|e| format!("Failed to list schemas: {}", e))?;

                serde_json::to_value(&schemas)
                    .map_err(|e| format!("Failed to serialize schemas: {}", e))
            }

            "get_schema" => {
                let name = params
                    .get("name")
                    .and_then(|n| n.as_str())
                    .ok_or("get_schema tool requires 'name' parameter")?;

                let schema = processor
                    .get_schema(name)
                    .await
                    .map_err(|e| format!("Failed to get schema: {}", e))?;

                match schema {
                    Some(s) => serde_json::to_value(&s)
                        .map_err(|e| format!("Failed to serialize schema: {}", e)),
                    None => Ok(Value::Null),
                }
            }

            "search" => {
                let terms = params
                    .get("terms")
                    .and_then(|t| t.as_str())
                    .ok_or("search tool requires 'terms' parameter")?;

                let results = processor
                    .native_index_search(terms)
                    .await
                    .map_err(|e| format!("Search failed: {}", e))?;

                serde_json::to_value(&results)
                    .map_err(|e| format!("Failed to serialize search results: {}", e))
            }

            "scan_folder" => {
                let path = params
                    .get("path")
                    .and_then(|p| p.as_str())
                    .ok_or("scan_folder tool requires 'path' parameter")?;
                let max_files = params
                    .get("max_files")
                    .and_then(|m| m.as_u64())
                    .unwrap_or(100) as usize;

                let expanded = expand_home_path(path);
                let folder_path = expanded.as_path();
                let scan_result = processor
                    .smart_folder_scan(folder_path, 10, max_files)
                    .await
                    .map_err(|e| format!("Folder scan failed: {}", e))?;

                serde_json::to_value(&scan_result)
                    .map_err(|e| format!("Failed to serialize scan results: {}", e))
            }

            "ingest_files" => {
                let folder_path_raw = params
                    .get("folder_path")
                    .and_then(|p| p.as_str())
                    .ok_or("ingest_files tool requires 'folder_path' parameter")?;
                let files = params
                    .get("files")
                    .and_then(|f| f.as_array())
                    .ok_or("ingest_files tool requires 'files' parameter (array of relative paths)")?;

                let base_expanded = expand_home_path(folder_path_raw);
                let base = base_expanded.as_path();
                let file_list: Vec<&str> = files
                    .iter()
                    .filter_map(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .collect();
                let total = file_list.len();

                // Create a batch-level progress entry so the frontend can poll it
                let batch_progress_id = format!("agent-ingest-{}", uuid::Uuid::new_v4());
                if let Some(tracker) = progress_tracker {
                    let progress_service =
                        crate::ingestion::progress::ProgressService::new(tracker.clone());
                    progress_service
                        .start_progress(batch_progress_id.clone(), "agent".to_string())
                        .await;
                    progress_service
                        .update_progress_with_percentage(
                            &batch_progress_id,
                            crate::ingestion::progress::IngestionStep::ExecutingMutations,
                            format!("Ingesting 0/{} files...", total),
                            5,
                        )
                        .await;
                }

                let pub_key = node.get_node_public_key().to_string();
                let mut results = Vec::new();
                for (idx, relative) in file_list.iter().enumerate() {
                    let full_path = base.join(relative);

                    // Update batch progress
                    if let Some(tracker) = progress_tracker {
                        let pct = ((idx as f64 / total as f64) * 90.0 + 5.0) as u8;
                        let progress_service =
                            crate::ingestion::progress::ProgressService::new(tracker.clone());
                        progress_service
                            .update_progress_with_percentage(
                                &batch_progress_id,
                                crate::ingestion::progress::IngestionStep::ExecutingMutations,
                                format!(
                                    "Ingesting {}/{} files: {}",
                                    idx + 1,
                                    total,
                                    relative
                                ),
                                pct,
                            )
                            .await;
                    }

                    // Skip files that have already been ingested (dedup by content hash)
                    if let Ok(hash) = crate::ingestion::smart_folder_scanner::compute_file_hash(&full_path) {
                        if node.is_file_ingested(&pub_key, &hash).await.is_some() {
                            log::info!("Agent ingest_files: skipping already-ingested file: {}", relative);
                            results.push(serde_json::json!({
                                "file": relative,
                                "success": true,
                                "skipped": true,
                                "reason": "already ingested",
                            }));
                            continue;
                        }
                    }

                    match processor
                        .ingest_single_file_with_tracker(
                            &full_path,
                            true,
                            progress_tracker.cloned(),
                        )
                        .await
                    {
                        Ok(response) => {
                            results.push(serde_json::json!({
                                "file": relative,
                                "success": response.success,
                                "schema_used": response.schema_used,
                                "new_schema_created": response.new_schema_created,
                                "mutations_generated": response.mutations_generated,
                                "mutations_executed": response.mutations_executed,
                            }));
                        }
                        Err(e) => {
                            results.push(serde_json::json!({
                                "file": relative,
                                "success": false,
                                "error": e.to_string(),
                            }));
                        }
                    }
                }

                // Mark batch progress as complete
                if let Some(tracker) = progress_tracker {
                    let progress_service =
                        crate::ingestion::progress::ProgressService::new(tracker.clone());
                    let succeeded = results.iter().filter(|r| r["success"] == true).count();
                    progress_service
                        .complete_progress(
                            &batch_progress_id,
                            crate::ingestion::progress::IngestionResults {
                                schema_name: String::new(),
                                new_schema_created: false,
                                mutations_generated: total,
                                mutations_executed: succeeded,
                                schemas_written: vec![],
                            },
                        )
                        .await;
                }

                let succeeded = results.iter().filter(|r| r["success"] == true).count();
                Ok(serde_json::json!({
                    "total": results.len(),
                    "succeeded": succeeded,
                    "failed": results.len() - succeeded,
                    "results": results,
                }))
            }

            _ => Err(format!("Unknown tool: {}", tool)),
        }
    }

    /// Run an autonomous agent query that can use tools to accomplish tasks
    ///
    /// The agent will iteratively:
    /// 1. Send the conversation to the LLM
    /// 2. Parse the response for tool calls or final answer
    /// 3. Execute tool calls and add results to conversation
    /// 4. Repeat until a final answer is given or max_iterations reached
    #[allow(clippy::too_many_arguments)]
    pub async fn run_agent_query(
        &self,
        user_query: &str,
        schemas: &[fold_db::schema::SchemaWithState],
        node: &crate::fold_node::node::FoldNode,
        _user_hash: &str,
        max_iterations: usize,
        prior_history: &[super::super::types::Message],
        progress_tracker: Option<&crate::ingestion::ProgressTracker>,
    ) -> Result<(String, Vec<ToolCallRecord>), String> {
        let mut tool_calls: Vec<ToolCallRecord> = Vec::new();

        // Build prior conversation history into a context string
        let mut conversation_context = String::new();
        if !prior_history.is_empty() {
            conversation_context.push_str("## Previous Conversation\n");
            for msg in prior_history {
                conversation_context.push_str(&format!(
                    "\n{}: {}\n",
                    msg.role, msg.content
                ));
            }
            conversation_context.push_str("\n## Current Turn\n");
        }

        // Build the initial system prompt with tool definitions
        let system_prompt = self.build_agent_system_prompt(schemas);
        let today = chrono::Local::now().format("%A, %B %-d, %Y").to_string();

        log::info!(
            "Agent: Starting query with max {} iterations, {} prior messages: {}",
            max_iterations,
            prior_history.len(),
            user_query
        );

        // Create an agent progress job so the frontend can track what's happening
        let agent_job_id = format!("agent-{}", uuid::Uuid::new_v4());
        if let Some(tracker) = progress_tracker {
            let user_id = fold_db::logging::core::get_current_user_id()
                .unwrap_or_else(|| "unknown".to_string());
            let mut job = fold_db::progress::Job::new(
                agent_job_id.clone(),
                fold_db::progress::JobType::Other("agent".to_string()),
            )
            .with_user(user_id);
            job.update_progress(5, "Thinking...".to_string());
            let _ = tracker.save(&job).await;
        }

        for iteration in 0..max_iterations {
            // Build the full prompt with conversation history
            // Repeat the current date at the end so it's fresh context when generating the answer
            let full_prompt = format!(
                "{}\n\n{}\n\nUser Query: {}\n\nReminder: Today is {}. Dates before today are in the past. Dates after today are in the future.\n\nRespond with a JSON object. Either:\n- {{\"tool\": \"tool_name\", \"params\": {{...}}}} to use a tool\n- {{\"answer\": \"your final response\"}} when you have the answer",
                system_prompt,
                conversation_context,
                user_query,
                today
            );

            log::debug!("Agent: Iteration {} - calling LLM", iteration + 1);

            let pct = 5 + (iteration * 90 / max_iterations.max(1)).min(90) as u8;
            update_agent_progress(progress_tracker, &agent_job_id, pct, format!("Thinking... (step {})", iteration + 1)).await;

            let response = self.call_llm(&full_prompt).await?;

            log::debug!("Agent: LLM response: {}", &response[..response.len().min(200)]);

            // Parse the response
            let action = self.parse_agent_response(&response)?;

            match action {
                super::super::types::AgentAction::Answer(answer) => {
                    log::info!(
                        "Agent: Completed after {} iterations with {} tool calls",
                        iteration + 1,
                        tool_calls.len()
                    );
                    // Mark agent job complete
                    if let Some(tracker) = progress_tracker {
                        if let Ok(Some(mut job)) = tracker.load(&agent_job_id).await {
                            job.complete(None);
                            let _ = tracker.save(&job).await;
                        }
                    }
                    return Ok((answer, tool_calls));
                }
                super::super::types::AgentAction::ToolCall { tool, params } => {
                    log::info!("Agent: Calling tool '{}' with params: {}", tool, params);

                    // Update progress: executing tool
                    let tool_pct = 10 + (iteration * 90 / max_iterations.max(1)).min(85) as u8;
                    let tool_label = match tool.as_str() {
                        "ingest_files" => "Ingesting files...",
                        "query" => "Querying database...",
                        "scan_folder" => "Scanning folder...",
                        "list_schemas" => "Listing schemas...",
                        _ => "Executing tool...",
                    };
                    update_agent_progress(progress_tracker, &agent_job_id, tool_pct, format!("{} ({})", tool_label, tool)).await;

                    // Execute the tool, capturing errors as results so the agent can retry
                    let result = match self.execute_tool(&tool, &params, node, progress_tracker).await {
                        Ok(val) => val,
                        Err(e) => {
                            log::warn!("Agent: Tool '{}' failed: {}", tool, e);
                            serde_json::json!({ "error": e })
                        }
                    };

                    log::debug!("Agent: Tool '{}' returned: {}", tool, &result.to_string()[..result.to_string().len().min(200)]);

                    // Record the tool call
                    tool_calls.push(ToolCallRecord {
                        tool: tool.clone(),
                        params: params.clone(),
                        result: result.clone(),
                    });

                    // Add to conversation context
                    conversation_context.push_str(&format!(
                        "\n\nTool call: {}\nParameters: {}\nResult: {}\n",
                        tool,
                        serde_json::to_string_pretty(&params).unwrap_or_default(),
                        serde_json::to_string_pretty(&result).unwrap_or_default()
                    ));
                }
            }
        }

        // Mark agent job as failed on max iterations
        if let Some(tracker) = progress_tracker {
            if let Ok(Some(mut job)) = tracker.load(&agent_job_id).await {
                job.fail("Reached maximum iterations without a final answer".to_string());
                let _ = tracker.save(&job).await;
            }
        }

        Err(format!(
            "Agent reached maximum iterations ({}) without providing a final answer",
            max_iterations
        ))
    }
}
