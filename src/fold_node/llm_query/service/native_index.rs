//! Native index search, interpretation, and alternative query suggestion.

use super::super::conversation_store::AI_CONVERSATIONS_SCHEMA;
use super::super::types::{AgentOutcome, QueryPlan, ToolCallRecord};
use fold_db::schema::types::field_value_type::FieldValueType;
use fold_db::schema::types::key_config::KeyConfig;
use fold_db::schema::types::operations::Query;
use fold_db::schema::types::schema::DeclarativeSchemaType as SchemaType;
use fold_db::view::types::TransformView;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

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

/// Strip native-index hits that mirror turns from the *current* agent
/// session. Each agent_query turn is persisted to `ai_conversations`
/// keyed by `(session_id, timestamp)` and auto-embedded by fold_db's
/// `NativeIndexManager`, so without this filter the agent's just-asked
/// queries dominate later searches in the same session — but the LLM
/// already has those turns in its conversation context, so re-surfacing
/// them is pure noise that crowds out real user data (recursive pollution
/// → "you have no data").
///
/// We only drop hits whose hash key matches `current_session_id`. Hits
/// from earlier sessions remain visible: they are *not* in the LLM's
/// context window, so the agent still needs the index to recall them
/// (e.g. "didn't I ask you about Tokyo last week?"). Hits from any other
/// schema are always preserved.
fn drop_current_session_hits(
    results: &mut Vec<fold_db::db_operations::IndexResult>,
    current_session_id: &str,
) {
    results.retain(|r| {
        if r.schema_name != AI_CONVERSATIONS_SCHEMA {
            return true;
        }
        // Drop only hits keyed to *this* session. ai_conversations is
        // HashRange(session_id, timestamp), so every fragment hit on
        // any field of the record carries the session_id in
        // `key_value.hash`.
        r.key_value.hash.as_deref() != Some(current_session_id)
    });
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
    ///
    /// Plumbing schemas (`Mention`, `MentionBySource`, `ExtractionStatus`,
    /// `IngestionError`, `TriggerFiring`, `ExtractionRule`) are filtered
    /// out via
    /// [`crate::fold_node::operation_processor::is_internal_index_schema`]
    /// — the LLM-facing query path should never surface
    /// fingerprint-cross-reference rows.
    ///
    /// `ai_conversations` is *not* filtered here: per-term
    /// [`drop_current_session_hits`] already strips current-session turns,
    /// and prior-session conversations should remain searchable so the
    /// agent can recall earlier turns ("didn't I ask you about Tokyo last
    /// week?"). The agent's `search` tool (in [`Self::execute_tool`]) does
    /// drop all `ai_conversations` because it surfaces results as a
    /// "data type" inventory, where self-references are pure noise.
    pub async fn search_native_index(
        &self,
        user_query: &str,
        schemas: &[fold_db::schema::SchemaWithState],
        db_ops: &fold_db::db_operations::DbOperations,
        current_session_id: &str,
    ) -> Result<Vec<fold_db::db_operations::IndexResult>, String> {
        // Step 1: Generate native index search terms using AI
        let search_terms = self
            .generate_native_index_search_terms(user_query, schemas)
            .await?;

        // Build a canonical→descriptive name map so the internal-schema
        // filter can match either form (fingerprint schemas have hashed
        // canonical names but stable descriptive names).
        let display_names: std::collections::HashMap<&str, &str> = schemas
            .iter()
            .filter_map(|s| {
                s.schema
                    .descriptive_name
                    .as_deref()
                    .map(|dn| (s.schema.name.as_str(), dn))
            })
            .collect();

        // Step 2: Execute native index searches for each term. See
        // `drop_current_session_hits` for why current-session turns are
        // stripped while prior sessions stay visible.
        let mut all_results = Vec::new();
        if let Some(native_index_mgr) = db_ops.native_index_manager() {
            for term in &search_terms {
                match native_index_mgr.search_all_classifications(term).await {
                    Ok(mut results) => {
                        let raw_count = results.len();
                        drop_current_session_hits(&mut results, current_session_id);
                        tracing::debug!(
                            "LLM Query: Term '{}' returned {} results ({} after dropping current-session ai_conversations)",
                            term,
                            raw_count,
                            results.len()
                        );
                        all_results.append(&mut results);
                    }
                    Err(e) => {
                        tracing::warn!("Native index search failed for term '{}': {}", term, e);
                    }
                }
            }
        }

        let pre_filter = all_results.len();
        all_results.retain(|r| {
            // Keep ai_conversations — drop_current_session_hits already
            // handled the noise case.
            if r.schema_name == AI_CONVERSATIONS_SCHEMA {
                return true;
            }
            let descriptive = display_names.get(r.schema_name.as_str()).copied();
            !crate::fold_node::operation_processor::is_internal_index_schema(
                &r.schema_name,
                descriptive,
            )
        });

        tracing::info!(
            "LLM Query: Found {} results from native index ({} after filtering internal schemas)",
            pre_filter,
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
        tracing::info!(
            "LLM Query: Sending {} results to AI for interpretation",
            results.len()
        );
        if results.is_empty() {
            tracing::warn!("LLM Query: No results to send to AI");
        } else {
            tracing::debug!(
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

    /// Execute a tool call and return the result.
    ///
    /// `current_session_id` is the agent loop's session id; it scopes
    /// the `search` tool's filter so the agent never re-surfaces its
    /// own current-session turns (which the LLM already has in context).
    pub(super) async fn execute_tool(
        &self,
        tool: &str,
        params: &Value,
        node: &crate::fold_node::node::FoldNode,
        progress_tracker: Option<&crate::ingestion::ProgressTracker>,
        current_session_id: &str,
    ) -> Result<Value, String> {
        let processor = crate::fold_node::OperationProcessor::from_ref(node);

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
                        fields = schema_with_state
                            .schema
                            .runtime_fields
                            .keys()
                            .cloned()
                            .collect();
                    }
                }

                let filter = params.get("filter").cloned();
                let sort_order = params.get("sort_order").cloned();
                let value_filters = params.get("value_filters").cloned();
                let limit = params.get("limit").and_then(|l| l.as_u64()).unwrap_or(50) as usize;

                let query = Query {
                    schema_name: schema_name.to_string(),
                    fields,
                    filter: filter.and_then(|f| serde_json::from_value(f).ok()),
                    as_of: None,
                    rehydrate_depth: Some(1),
                    sort_order: sort_order.and_then(|s| serde_json::from_value(s).ok()),
                    value_filters: value_filters.and_then(|v| serde_json::from_value(v).ok()),
                };

                let results = processor
                    .execute_query_json(query)
                    .await
                    .map_err(|e| format!("Query execution failed: {}", e))?;

                let total_count = results.len();
                let mut records: Vec<Value> = results.into_iter().take(limit).collect();

                // Safety: cap the serialized size at ~100K chars (~25K tokens)
                // to prevent blowing the conversation context window.
                const MAX_RESULT_CHARS: usize = 100_000;
                let mut serialized = serde_json::to_string(&records).unwrap_or_default();
                while serialized.len() > MAX_RESULT_CHARS && records.len() > 1 {
                    records.pop();
                    serialized = serde_json::to_string(&records).unwrap_or_default();
                }

                let shown = records.len();
                let mut result = serde_json::json!({
                    "records": records,
                    "total_count": total_count,
                    "returned_count": shown,
                });
                if total_count > shown {
                    result["truncated"] = serde_json::json!(true);
                    result["message"] = serde_json::json!(format!(
                        "Showing {} of {} results (trimmed to fit context). Use 'limit' with smaller values, request fewer fields, or use 'filter' to narrow results.",
                        shown, total_count
                    ));
                }
                Ok(result)
            }

            "list_schemas" => {
                let schemas = processor
                    .list_schemas()
                    .await
                    .map_err(|e| format!("Failed to list schemas: {}", e))?;

                // Annotate each schema with `record_count` so the agent can
                // answer "how much data is in my database?" in a single call
                // instead of issuing an unfiltered query per schema.
                let mut entries: Vec<Value> = Vec::with_capacity(schemas.len());
                for schema in &schemas {
                    let mut entry = serde_json::to_value(schema)
                        .map_err(|e| format!("Failed to serialize schema: {}", e))?;
                    let count = processor
                        .count_schema_records(&schema.schema.name)
                        .await
                        .unwrap_or(0);
                    if let Some(obj) = entry.as_object_mut() {
                        obj.insert("record_count".to_string(), serde_json::json!(count));
                    }
                    entries.push(entry);
                }
                Ok(Value::Array(entries))
            }

            "count_records" => {
                let schema_name = params
                    .get("schema_name")
                    .and_then(|s| s.as_str())
                    .ok_or("count_records tool requires 'schema_name' parameter")?;

                let count = processor
                    .count_schema_records(schema_name)
                    .await
                    .map_err(|e| format!("count_schema_records failed: {}", e))?;

                Ok(serde_json::json!({
                    "schema_name": schema_name,
                    "record_count": count,
                }))
            }

            "list_orgs" => {
                let pool = crate::handlers::org::get_sled_pool(node)
                    .await
                    .map_err(|e| format!("Failed to get database: {}", e))?;
                let orgs = fold_db::org::operations::list_orgs(&pool)
                    .map_err(|e| format!("Failed to list orgs: {}", e))?;
                serde_json::to_value(&orgs).map_err(|e| format!("Failed to serialize orgs: {}", e))
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

                // include_internal=false strips Mention / MentionBySource /
                // ExtractionStatus / IngestionError / TriggerFiring /
                // ai_conversations / ExtractionRule. The agent's `search`
                // tool surfaces results as a "what data do I have?"
                // inventory, where bookkeeping schemas (and the agent's own
                // past turns) are pure noise.
                let mut results = processor
                    .native_index_search(terms, false)
                    .await
                    .map_err(|e| format!("Search failed: {}", e))?;

                // Defensive: ai_conversations is already in the
                // include_internal filter list, so this is a no-op today —
                // kept so the intent survives if the filter list ever
                // changes.
                drop_current_session_hits(&mut results, current_session_id);

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
                let files = params.get("files").and_then(|f| f.as_array()).ok_or(
                    "ingest_files tool requires 'files' parameter (array of relative paths)",
                )?;
                let org_hash = params
                    .get("org_hash")
                    .and_then(|h| h.as_str())
                    .map(|s| s.to_string());

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
                                format!("Ingesting {}/{} files: {}", idx + 1, total, relative),
                                pct,
                            )
                            .await;
                    }

                    // Skip files that have already been ingested (dedup by content hash)
                    if let Ok(hash) =
                        crate::ingestion::smart_folder::scanner::compute_file_hash(&full_path)
                    {
                        if node.is_file_ingested(&pub_key, &hash).await.is_some() {
                            tracing::info!(
                                "Agent ingest_files: skipping already-ingested file: {}",
                                relative
                            );
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
                            org_hash.clone(),
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

            "create_view" => {
                let name = params
                    .get("name")
                    .and_then(|n| n.as_str())
                    .ok_or("create_view tool requires 'name' parameter")?;

                let schema_type_str = params
                    .get("schema_type")
                    .and_then(|s| s.as_str())
                    .ok_or("create_view tool requires 'schema_type' parameter")?;

                let schema_type: SchemaType =
                    serde_json::from_value(Value::String(schema_type_str.to_string()))
                        .map_err(|e| format!("Invalid schema_type '{}': {}", schema_type_str, e))?;

                let key_config: Option<KeyConfig> = params
                    .get("key_config")
                    .and_then(|k| {
                        if k.is_null() {
                            None
                        } else {
                            Some(serde_json::from_value(k.clone()))
                        }
                    })
                    .transpose()
                    .map_err(|e| format!("Invalid key_config: {}", e))?;

                let input_queries_val = params
                    .get("input_queries")
                    .and_then(|q| q.as_array())
                    .ok_or("create_view tool requires 'input_queries' parameter (array)")?;

                let input_queries: Vec<Query> = input_queries_val
                    .iter()
                    .map(|q| serde_json::from_value(q.clone()))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| format!("Invalid input_queries: {}", e))?;

                let output_fields_val = params
                    .get("output_fields")
                    .and_then(|o| o.as_object())
                    .ok_or("create_view tool requires 'output_fields' parameter (object)")?;

                let output_fields: HashMap<String, FieldValueType> = output_fields_val
                    .iter()
                    .map(|(k, v)| {
                        let fvt: FieldValueType = serde_json::from_value(v.clone())
                            .map_err(|e| format!("Invalid field type for '{}': {}", k, e))?;
                        Ok((k.clone(), fvt))
                    })
                    .collect::<Result<HashMap<_, _>, String>>()?;

                let rust_transform = params
                    .get("rust_transform")
                    .and_then(|r| r.as_str())
                    .ok_or("create_view tool requires 'rust_transform' parameter")?;

                // Register the transform with schema_service — it compiles, validates,
                // and persists the WASM. fold_db_node no longer compiles locally; trust
                // and validation live in schema_service (see
                // `preferences/fold_db_vs_fold_db_node_boundary` and
                // `projects/trigger-feature` Phase 2c).
                //
                // As of the Transform Worker Split (schema_service PRs #28 /
                // #30), the deployed Lambda may enqueue a `cargo build` to an
                // out-of-process worker and return 202 + `job_id`. The SDK's
                // `register_transform` wrapper hides that behind a
                // sync-looking call: it polls until the worker commits, then
                // returns the final `TransformRecord`. Dev/local schema
                // services still serve the synchronous path unchanged — both
                // routes land in the same wrapper.
                let schema_service_url = node.schema_service_url().ok_or(
                    "create_view requires a configured schema_service_url — start the node with --local-schema or point at a real schema service",
                )?;
                if crate::fold_node::FoldNode::is_test_schema_service(&schema_service_url) {
                    return Err(
                        "create_view cannot run against a test/mock schema service — it needs a real service to compile the rust_transform".to_string(),
                    );
                }

                // 1 billion wasmtime fuel units — roughly hundreds of ms of
                // guest work on a modern CPU, enough for the row-at-a-time
                // transforms the LLM emits today. Follow-up: expose a
                // `max_gas` parameter on the create_view tool so the LLM can
                // override per-transform; this constant is the fallback.
                // Required by schema_service since MDT-E (PR #25,
                // 2026-04-22) and enforced on every device per the
                // WasmTransformSpec contract in fold_db.
                const CREATE_VIEW_DEFAULT_MAX_GAS: u64 = 1_000_000_000;

                let register_req = schema_service_core::types::RegisterTransformRequest {
                    name: name.to_string(),
                    version: "1.0.0".to_string(),
                    description: None,
                    input_queries: input_queries.clone(),
                    output_fields: output_fields.clone(),
                    source_url: None,
                    rust_source: rust_transform.to_string(),
                    max_gas: CREATE_VIEW_DEFAULT_MAX_GAS,
                };

                tracing::info!(
                    "create_view: submitting rust_transform for view '{}' to schema service at {}",
                    name,
                    &schema_service_url
                );
                let schema_client =
                    schema_service_client::SchemaServiceClient::new(&schema_service_url);
                let record = schema_client
                    .register_transform(&register_req)
                    .await
                    .map_err(|e| {
                        format!(
                            "Schema service rejected transform registration for '{}': {}",
                            name, e
                        )
                    })?;
                let hash = record.hash.clone();

                // `register_transform` returns metadata only; the actual WASM
                // blob lives under a separate endpoint (content-addressed so
                // multiple view definitions can share the same bytes).
                // `schema_service_client` doesn't wrap this yet — a raw GET
                // is adequate since the base URL is already validated.
                let base = schema_service_url.trim_end_matches('/');
                // trace-egress: propagate (schema service; .send() wrapped with inject_w3c below)
                let http = reqwest::Client::new();
                let request = observability::propagation::inject_w3c(
                    http.get(format!("{}/v1/transform/{}/wasm", base, hash)),
                );
                let wasm_resp = request.send().await.map_err(|e| {
                    format!("Failed to fetch compiled WASM for hash {}: {}", hash, e)
                })?;
                if !wasm_resp.status().is_success() {
                    return Err(format!(
                        "Schema service returned {} fetching compiled WASM for hash {}",
                        wasm_resp.status(),
                        hash
                    ));
                }
                let wasm_bytes = wasm_resp
                    .bytes()
                    .await
                    .map_err(|e| format!("Failed to read WASM bytes: {}", e))?
                    .to_vec();
                tracing::info!(
                    "create_view: schema service compiled {} bytes of WASM for view '{}' (hash {})",
                    wasm_bytes.len(),
                    name,
                    hash
                );

                // MDT-E: TransformView now carries WasmTransformSpec (bytes +
                // per-invocation fuel ceiling) instead of raw bytes. Pair the
                // compiled bytes with the same max_gas the schema service just
                // validated. gas_model is left as None — the fit harness runs
                // on the service side and its output is carried via the
                // transform registry record, not re-derived here.
                let wasm_transform_spec = fold_db::view::types::WasmTransformSpec {
                    bytes: wasm_bytes,
                    max_gas: CREATE_VIEW_DEFAULT_MAX_GAS,
                    gas_model: None,
                };
                let view = TransformView::new(
                    name.to_string(),
                    schema_type,
                    key_config,
                    input_queries,
                    Some(wasm_transform_spec),
                    output_fields,
                );

                processor
                    .create_view(view)
                    .await
                    .map_err(|e| format!("Failed to create view: {}", e))?;

                Ok(serde_json::json!({
                    "success": true,
                    "message": format!("View '{}' created successfully with WASM transform", name),
                    "view_name": name,
                    "transform_hash": hash,
                }))
            }

            "discovery_opt_in" => {
                let schema_name = params
                    .get("schema_name")
                    .and_then(|v| v.as_str())
                    .ok_or("discovery_opt_in requires 'schema_name'")?
                    .to_string();
                let category = params
                    .get("category")
                    .and_then(|v| v.as_str())
                    .ok_or("discovery_opt_in requires 'category'")?
                    .to_string();
                let include_preview = params
                    .get("include_preview")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                let mut opt_in_config =
                    crate::discovery::config::DiscoveryOptIn::new(schema_name.clone(), category);

                if include_preview {
                    opt_in_config = opt_in_config.with_preview(100, Vec::new());
                }

                // Parse field_privacy map if provided
                let mut field_classes: HashMap<String, String> = HashMap::new();
                if let Some(fp_val) = params.get("field_privacy") {
                    if let Some(fp_obj) = fp_val.as_object() {
                        let mut privacy_map = HashMap::new();
                        for (field, class_val) in fp_obj {
                            if let Some(class_str) = class_val.as_str() {
                                // Unknown strings (including the historical
                                // "PublishIfAnonymous") collapse to AlwaysPublish
                                // since the anonymity gate no longer exists —
                                // there is no "publish only if content is
                                // anonymous" middle ground anymore. See
                                // `preferences/no-discovery-anonymity-gating`.
                                let class = match class_str {
                                    "NeverPublish" => crate::discovery::field_privacy::FieldPrivacyClass::NeverPublish,
                                    _ => crate::discovery::field_privacy::FieldPrivacyClass::AlwaysPublish,
                                };
                                field_classes.insert(field.clone(), class_str.to_string());
                                privacy_map.insert(field.clone(), class);
                            }
                        }
                        opt_in_config = opt_in_config.with_field_privacy(privacy_map);
                    }
                }

                let db = node
                    .get_fold_db()
                    .map_err(|e| format!("Failed to access database: {}", e))?;
                let store = db.get_db_ops().raw_metadata_store();

                crate::discovery::config::save_opt_in(&*store, &opt_in_config)
                    .await
                    .map_err(|e| format!("Failed to save discovery opt-in: {}", e))?;

                Ok(serde_json::json!({
                    "success": true,
                    "message": format!("Schema '{}' opted into discovery", schema_name),
                    "field_classes": field_classes,
                }))
            }

            "discovery_opt_out" => {
                let schema_name = params
                    .get("schema_name")
                    .and_then(|v| v.as_str())
                    .ok_or("discovery_opt_out requires 'schema_name'")?
                    .to_string();

                let db = node
                    .get_fold_db()
                    .map_err(|e| format!("Failed to access database: {}", e))?;
                let store = db.get_db_ops().raw_metadata_store();

                crate::discovery::config::remove_opt_in(&*store, &schema_name)
                    .await
                    .map_err(|e| format!("Failed to remove discovery opt-in: {}", e))?;

                Ok(serde_json::json!({
                    "success": true,
                    "message": format!("Schema '{}' removed from discovery", schema_name),
                }))
            }

            "discovery_status" => {
                let db = node
                    .get_fold_db()
                    .map_err(|e| format!("Failed to access database: {}", e))?;
                let store = db.get_db_ops().raw_metadata_store();

                let configs = crate::discovery::config::list_opt_ins(&*store)
                    .await
                    .map_err(|e| format!("Failed to list discovery opt-ins: {}", e))?;

                let entries: Vec<serde_json::Value> = configs
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "schema_name": c.schema_name,
                            "category": c.category,
                            "include_preview": c.include_preview,
                            "field_privacy": c.field_privacy,
                            "opted_in_at": c.opted_in_at.to_rfc3339(),
                        })
                    })
                    .collect();

                Ok(serde_json::json!({
                    "success": true,
                    "schemas": entries,
                    "total": entries.len(),
                }))
            }

            "web_search" => {
                let query = params
                    .get("query")
                    .and_then(|q| q.as_str())
                    .ok_or("web_search tool requires 'query' parameter")?;
                let count = params.get("count").and_then(|c| c.as_u64()).unwrap_or(5) as usize;

                tracing::info!("Agent web_search: query='{}', count={}", query, count);
                let results = super::web_tools::web_search(self.config_dir(), query, count).await?;

                Ok(serde_json::json!({
                    "results": results,
                    "total": results.len(),
                    "query": query,
                }))
            }

            "fetch_url" => {
                let url = params
                    .get("url")
                    .and_then(|u| u.as_str())
                    .ok_or("fetch_url tool requires 'url' parameter")?;

                tracing::info!("Agent fetch_url: url='{}'", url);
                let content = super::web_tools::fetch_url(url).await?;

                Ok(serde_json::json!({
                    "url": url,
                    "content": content,
                    "length": content.len(),
                }))
            }

            "ingest_json" => {
                let data = params
                    .get("data")
                    .ok_or("ingest_json tool requires 'data' parameter (JSON object or array)")?
                    .clone();

                // Validate: must be object or array
                if !data.is_object() && !data.is_array() {
                    return Err(
                        "ingest_json 'data' must be a JSON object or array of objects".to_string(),
                    );
                }
                if let Some(arr) = data.as_array() {
                    if arr.is_empty() {
                        return Err("ingest_json 'data' array must not be empty".to_string());
                    }
                }

                let source_context = params
                    .get("source_context")
                    .and_then(|s| s.as_str())
                    .map(|s| s.to_string());

                tracing::info!(
                    "Agent ingest_json: source_context={:?}, data_type={}",
                    source_context,
                    if data.is_array() { "array" } else { "object" }
                );

                let progress_id = format!("agent-ingest-json-{}", uuid::Uuid::new_v4());
                let pub_key = node.get_node_public_key().to_string();

                let request = crate::ingestion::IngestionRequest {
                    data,
                    auto_execute: true,
                    pub_key,
                    source_file_name: source_context.map(|ctx| format!("{}.json", ctx)),
                    progress_id: Some(progress_id.clone()),
                    file_hash: None,
                    source_folder: None,
                    image_descriptive_name: None,
                    org_hash: None,
                    image_bytes: None,
                    forced_schema_descriptive_name: None,
                };

                let config_dir = node
                    .config
                    .config_dir
                    .clone()
                    .ok_or_else(|| "NodeConfig.config_dir not set".to_string())?;
                let service =
                    crate::ingestion::ingestion_service::IngestionService::from_config_dir(
                        &config_dir,
                    )
                    .map_err(|e| format!("Failed to create ingestion service: {}", e))?;

                let tracker = match progress_tracker {
                    Some(t) => t.clone(),
                    None => crate::ingestion::create_progress_tracker().await,
                };
                let progress_service = crate::ingestion::progress::ProgressService::new(tracker);
                progress_service
                    .start_progress(progress_id.clone(), "agent".to_string())
                    .await;

                let response = service
                    .process_json_with_node_and_progress(
                        request,
                        node,
                        &progress_service,
                        progress_id,
                    )
                    .await
                    .map_err(|e| format!("JSON ingestion failed: {}", e))?;

                // Look up descriptive_name so the agent sees the human-readable schema name
                let descriptive_name = if let Some(ref schema_name) = response.schema_used {
                    processor
                        .get_schema(schema_name)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|s| s.schema.descriptive_name.clone())
                } else {
                    None
                };

                Ok(serde_json::json!({
                    "success": response.success,
                    "schema_name": descriptive_name.as_deref().unwrap_or_else(|| response.schema_used.as_deref().unwrap_or("unknown")),
                    "schema_id": response.schema_used,
                    "new_schema_created": response.new_schema_created,
                    "mutations_generated": response.mutations_generated,
                    "mutations_executed": response.mutations_executed,
                    "errors": response.errors,
                }))
            }

            "update_record" => {
                let schema_name = params
                    .get("schema_name")
                    .and_then(|s| s.as_str())
                    .ok_or("update_record tool requires 'schema_name' parameter")?;

                let key_obj = params.get("key").and_then(|k| k.as_object()).ok_or(
                    "update_record tool requires 'key' parameter (object with hash_key/range_key)",
                )?;

                let hash_key = key_obj
                    .get("hash_key")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                let range_key = key_obj
                    .get("range_key")
                    .and_then(|v| v.as_str())
                    .map(String::from);

                if hash_key.is_none() && range_key.is_none() {
                    return Err(
                        "update_record 'key' must include at least one of 'hash_key' or 'range_key'"
                            .to_string(),
                    );
                }

                let fields_obj = params
                    .get("fields")
                    .and_then(|f| f.as_object())
                    .ok_or("update_record tool requires 'fields' parameter (object)")?;

                if fields_obj.is_empty() {
                    return Err("update_record 'fields' must not be empty".to_string());
                }

                let fields_and_values: HashMap<String, Value> = fields_obj
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();

                let key_value = fold_db::schema::types::KeyValue::new(hash_key, range_key);

                tracing::info!(
                    "Agent update_record: schema={}, key={:?}, fields={:?}",
                    schema_name,
                    key_value,
                    fields_and_values.keys().collect::<Vec<_>>()
                );

                let mutation_id = processor
                    .execute_mutation(
                        schema_name.to_string(),
                        fields_and_values,
                        key_value,
                        fold_db::schema::types::operations::MutationType::Update,
                    )
                    .await
                    .map_err(|e| format!("Update failed: {}", e))?;

                Ok(serde_json::json!({
                    "success": true,
                    "mutation_id": mutation_id,
                    "message": format!("Record updated in schema '{}'", schema_name),
                }))
            }

            "set_field_policy" => Err("Access control has been removed from fold_db".to_string()),

            "get_field_policies" => Err("Access control has been removed from fold_db".to_string()),

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
    // 120s accommodates web-tool-heavy queries with multiple LLM round-trips.
    const AGENT_QUERY_TIMEOUT: Duration = Duration::from_secs(120);

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
        current_session_id: &str,
    ) -> Result<AgentOutcome, String> {
        // Create an agent progress job so the frontend can track what's happening
        let agent_job_id = format!("agent-{}", uuid::Uuid::new_v4());
        if let Some(tracker) = progress_tracker {
            let user_id = fold_db::user_context::get_current_user_id()
                .unwrap_or_else(|| "unknown".to_string());
            let mut job = fold_db::progress::Job::new(
                agent_job_id.clone(),
                fold_db::progress::JobType::Other("agent".to_string()),
            )
            .with_user(user_id);
            job.update_progress(5, "Thinking...".to_string());
            let _ = tracker.save(&job).await;
        }

        match tokio::time::timeout(
            Self::AGENT_QUERY_TIMEOUT,
            self.run_agent_query_inner(
                user_query,
                schemas,
                node,
                max_iterations,
                prior_history,
                progress_tracker,
                &agent_job_id,
                current_session_id,
            ),
        )
        .await
        {
            Ok(result) => result,
            Err(_elapsed) => {
                tracing::error!(
                    "Agent: query timed out after {}s",
                    Self::AGENT_QUERY_TIMEOUT.as_secs()
                );
                if let Some(tracker) = progress_tracker {
                    if let Ok(Some(mut job)) = tracker.load(&agent_job_id).await {
                        job.fail("Agent query timed out".to_string());
                        let _ = tracker.save(&job).await;
                    }
                }
                Err(format!(
                    "Agent query timed out after {} seconds",
                    Self::AGENT_QUERY_TIMEOUT.as_secs()
                ))
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_agent_query_inner(
        &self,
        user_query: &str,
        schemas: &[fold_db::schema::SchemaWithState],
        node: &crate::fold_node::node::FoldNode,
        max_iterations: usize,
        prior_history: &[super::super::types::Message],
        progress_tracker: Option<&crate::ingestion::ProgressTracker>,
        agent_job_id: &str,
        current_session_id: &str,
    ) -> Result<AgentOutcome, String> {
        let mut tool_calls: Vec<ToolCallRecord> = Vec::new();

        // Build prior conversation history into a context string
        let mut conversation_context = String::new();
        if !prior_history.is_empty() {
            conversation_context.push_str("## Previous Conversation\n");
            for msg in prior_history {
                conversation_context.push_str(&format!("\n{}: {}\n", msg.role, msg.content));
            }
            conversation_context.push_str("\n## Current Turn\n");
        }

        // Load org memberships for context
        let orgs = {
            let db_guard = node.get_fold_db().ok();
            db_guard
                .and_then(|g| g.sled_pool().cloned())
                .map(|pool| fold_db::org::operations::list_orgs(&pool).unwrap_or_default())
                .unwrap_or_default()
        };

        // Build the initial system prompt with tool definitions and org context
        let system_prompt = self.build_agent_system_prompt_with_orgs(schemas, &orgs);
        let today = chrono::Local::now().format("%A, %B %-d, %Y").to_string();

        tracing::info!(
            "Agent: Starting query with max {} iterations, {} prior messages: {}",
            max_iterations,
            prior_history.len(),
            user_query
        );

        // Track consecutive empty-response failures so we can abort early
        // instead of burning all iterations on a broken LLM backend.
        let mut consecutive_empty_errors: u32 = 0;
        const MAX_CONSECUTIVE_EMPTY: u32 = 2;

        // Last non-empty raw LLM response — included in the partial-progress
        // message when we hit `max_iterations` so the caller has *something*
        // to show the user instead of "Internal error".
        let mut last_response: Option<String> = None;

        for iteration in 0..max_iterations {
            // Build the full prompt with conversation history
            // Repeat the current date at the end so it's fresh context when generating the answer
            let full_prompt = format!(
                "{}\n\n{}\n\nUser Query: {}\n\nReminder: Today is {}. Dates before today are in the past. Dates after today are in the future.\n\nIf the user asked a \"how many / count / total\" question, do NOT call `query` with `fields:[\"count\"]` — that returns records, not aggregates. Use `count_records` for one schema, or `list_schemas` to read `record_count` for every schema in one call.\n\nRespond with a JSON object. Either:\n- {{\"tool\": \"tool_name\", \"params\": {{...}}}} to use a tool\n- {{\"answer\": \"your final response\"}} when you have the answer",
                system_prompt,
                conversation_context,
                user_query,
                today
            );

            tracing::debug!("Agent: Iteration {} - calling LLM", iteration + 1);

            let pct = 5 + (iteration * 90 / max_iterations.max(1)).min(90) as u8;
            update_agent_progress(
                progress_tracker,
                agent_job_id,
                pct,
                format!("Thinking... (step {})", iteration + 1),
            )
            .await;

            let response = self.call_llm(&full_prompt).await?;

            tracing::debug!(
                "Agent: LLM response: {}",
                &response[..response.len().min(200)]
            );

            if !response.trim().is_empty() {
                last_response = Some(response.clone());
            }

            // Parse the response — empty responses now return Err
            let action = match self.parse_agent_response(&response) {
                Ok(action) => {
                    consecutive_empty_errors = 0;
                    action
                }
                Err(e) if e.contains("empty response") => {
                    consecutive_empty_errors += 1;
                    tracing::warn!(
                        "Agent: empty LLM response on iteration {} ({}/{})",
                        iteration + 1,
                        consecutive_empty_errors,
                        MAX_CONSECUTIVE_EMPTY
                    );
                    if consecutive_empty_errors >= MAX_CONSECUTIVE_EMPTY {
                        if let Some(tracker) = progress_tracker {
                            if let Ok(Some(mut job)) = tracker.load(agent_job_id).await {
                                job.fail("LLM backend returning empty responses".to_string());
                                let _ = tracker.save(&job).await;
                            }
                        }
                        return Err(
                            "LLM backend returned empty responses on consecutive attempts"
                                .to_string(),
                        );
                    }
                    // Add a note to context so the LLM knows the previous attempt failed
                    conversation_context
                        .push_str("\n\n[System: previous response was empty, please try again]\n");
                    continue;
                }
                Err(e) => return Err(e),
            };

            match action {
                super::super::types::AgentAction::Answer(answer) => {
                    // Reject empty answers — the agent must provide a substantive response
                    if answer.trim().is_empty() {
                        tracing::warn!(
                            "Agent: LLM returned empty answer on iteration {}",
                            iteration + 1
                        );
                        conversation_context.push_str(
                            "\n\n[System: your answer was empty, please provide a substantive response]\n",
                        );
                        continue;
                    }

                    tracing::info!(
                        "Agent: Completed after {} iterations with {} tool calls",
                        iteration + 1,
                        tool_calls.len()
                    );
                    // Mark agent job complete
                    if let Some(tracker) = progress_tracker {
                        if let Ok(Some(mut job)) = tracker.load(agent_job_id).await {
                            job.complete(None);
                            let _ = tracker.save(&job).await;
                        }
                    }
                    return Ok(AgentOutcome {
                        answer,
                        tool_calls,
                        stopped_reason: None,
                    });
                }
                super::super::types::AgentAction::ToolCall { tool, params } => {
                    tracing::info!("Agent: Calling tool '{}' with params: {}", tool, params);

                    // Update progress: executing tool
                    let tool_pct = 10 + (iteration * 90 / max_iterations.max(1)).min(85) as u8;
                    let tool_label = match tool.as_str() {
                        "ingest_files" => "Ingesting files...",
                        "ingest_json" => "Ingesting JSON data...",
                        "query" => "Querying database...",
                        "scan_folder" => "Scanning folder...",
                        "list_schemas" => "Listing schemas...",
                        "count_records" => "Counting records...",
                        "list_orgs" => "Listing organizations...",
                        "create_view" => "Registering WASM view with schema service...",
                        "update_record" => "Updating record...",
                        "web_search" => "Searching the web...",
                        "fetch_url" => "Fetching URL...",
                        _ => "Executing tool...",
                    };
                    update_agent_progress(
                        progress_tracker,
                        agent_job_id,
                        tool_pct,
                        format!("{} ({})", tool_label, tool),
                    )
                    .await;

                    // Execute the tool, capturing errors as results so the agent can retry
                    let result = match self
                        .execute_tool(&tool, &params, node, progress_tracker, current_session_id)
                        .await
                    {
                        Ok(val) => val,
                        Err(e) => {
                            tracing::warn!("Agent: Tool '{}' failed: {}", tool, e);
                            serde_json::json!({ "error": e })
                        }
                    };

                    tracing::debug!(
                        "Agent: Tool '{}' returned: {}",
                        tool,
                        &result.to_string()[..result.to_string().len().min(200)]
                    );

                    // Record the tool call
                    tool_calls.push(ToolCallRecord {
                        tool: tool.clone(),
                        params: params.clone(),
                        result: result.clone(),
                    });

                    // Add to conversation context with token budget guard.
                    // Rough estimate: 1 token ≈ 4 chars. Cap any single tool
                    // result at ~30K tokens (120K chars) to stay within model limits.
                    const MAX_RESULT_CHARS: usize = 120_000;
                    let result_str = serde_json::to_string_pretty(&result).unwrap_or_default();
                    let result_display = if result_str.len() > MAX_RESULT_CHARS {
                        format!(
                            "{}...\n\n[TRUNCATED: result was {} chars (~{} tokens). Use 'limit' param or request fewer/smaller fields to get complete results.]",
                            &result_str[..MAX_RESULT_CHARS],
                            result_str.len(),
                            result_str.len() / 4
                        )
                    } else {
                        result_str
                    };
                    conversation_context.push_str(&format!(
                        "\n\nTool call: {}\nParameters: {}\nResult: {}\n",
                        tool,
                        serde_json::to_string_pretty(&params).unwrap_or_default(),
                        result_display
                    ));
                }
            }
        }

        // Hitting `max_iterations` is no longer an error — we surface a
        // structured partial-progress outcome (`stopped_reason =
        // max_iterations`) so the caller can show the user something useful
        // instead of "Internal error". Reproducer that motivated this:
        // "How many reminders do I have?" against Ollama llama3.1:8b would
        // loop on `query` with `fields:["count"]` and bottom out in HTTP 500.
        tracing::warn!(
            "Agent: reached max_iterations ({}) with {} tool calls — returning partial outcome",
            max_iterations,
            tool_calls.len()
        );
        if let Some(tracker) = progress_tracker {
            if let Ok(Some(mut job)) = tracker.load(agent_job_id).await {
                job.fail("Reached maximum iterations without a final answer".to_string());
                let _ = tracker.save(&job).await;
            }
        }

        let last_partial = last_response
            .as_deref()
            .map(|r| r.trim())
            .filter(|r| !r.is_empty())
            .map(|r| {
                let truncated: String = r.chars().take(500).collect();
                if r.len() > truncated.len() {
                    format!(" Last partial reasoning: {}…", truncated)
                } else {
                    format!(" Last partial reasoning: {}", truncated)
                }
            })
            .unwrap_or_default();

        let answer = format!(
            "Sorry, I couldn't reach a final answer in {} steps.{}",
            max_iterations, last_partial
        );

        Ok(AgentOutcome {
            answer,
            tool_calls,
            stopped_reason: Some("max_iterations".to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_db::db_operations::IndexResult;
    use fold_db::schema::types::key_value::KeyValue;

    fn make_result(schema: &str, hash_key: Option<&str>) -> IndexResult {
        IndexResult {
            schema_name: schema.to_string(),
            schema_display_name: None,
            field: "body".to_string(),
            key_value: KeyValue::new(hash_key.map(String::from), None),
            value: serde_json::Value::Null,
            metadata: None,
            molecule_versions: None,
        }
    }

    #[test]
    fn drops_only_current_session_ai_conversations() {
        let mut results = vec![
            make_result("journal", Some("anything")),
            make_result(AI_CONVERSATIONS_SCHEMA, Some("session-current")),
            make_result(AI_CONVERSATIONS_SCHEMA, Some("session-old")),
            make_result("notes", Some("anything")),
            make_result(AI_CONVERSATIONS_SCHEMA, Some("session-current")),
        ];

        drop_current_session_hits(&mut results, "session-current");

        let kept: Vec<(&str, Option<&str>)> = results
            .iter()
            .map(|r| (r.schema_name.as_str(), r.key_value.hash.as_deref()))
            .collect();
        assert_eq!(
            kept,
            vec![
                ("journal", Some("anything")),
                // session-old ai_conversations row stays — it's not in
                // the LLM's context window, so the agent legitimately
                // needs the index to recall it.
                (AI_CONVERSATIONS_SCHEMA, Some("session-old")),
                ("notes", Some("anything")),
            ],
            "only current-session ai_conversations rows should be removed; \
             prior sessions and other schemas preserved in order"
        );
    }

    #[test]
    fn keeps_ai_conversations_with_no_session_id() {
        // A defensive case: if a row somehow lands without a hash key,
        // we shouldn't drop it on a session_id == "" match.
        let mut results = vec![make_result(AI_CONVERSATIONS_SCHEMA, None)];
        drop_current_session_hits(&mut results, "");
        assert_eq!(
            results.len(),
            1,
            "None hash must not match empty session id"
        );
    }

    #[test]
    fn empty_input_stays_empty() {
        let mut results: Vec<IndexResult> = Vec::new();
        drop_current_session_hits(&mut results, "session-current");
        assert!(results.is_empty());
    }

    #[test]
    fn keeps_all_when_no_match() {
        let mut results = vec![
            make_result("journal", Some("a")),
            make_result("notes", Some("b")),
        ];
        drop_current_session_hits(&mut results, "session-current");
        assert_eq!(results.len(), 2);
    }

    // ── Agent-loop tests with a scripted mock backend ──────────────────
    //
    // These tests exercise `run_agent_query` end-to-end without an LLM
    // provider. They cover:
    //   1. `max_iterations` returns Ok with `stopped_reason` instead of an
    //      Err — the regression that produced HTTP 500 "Internal error" on
    //      the dogfood "How many reminders do I have?" repro.
    //   2. A single `list_schemas` tool call is enough to satisfy a count
    //      question, validating the prompt + tool plumbing for fix (B).

    use crate::fold_node::config::NodeConfig;
    use crate::fold_node::FoldNode;
    use crate::ingestion::ai::client::AiBackend;
    use crate::ingestion::IngestionResult;
    use async_trait::async_trait;
    use std::sync::Arc;
    use std::sync::Mutex;

    /// Backend that returns a queue of pre-scripted responses. After the
    /// queue is exhausted, it keeps returning the last response (so a
    /// max-iterations test can pin the agent on a tool call indefinitely).
    struct ScriptedBackend {
        responses: Mutex<Vec<String>>,
    }

    impl ScriptedBackend {
        fn new(responses: Vec<&str>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses.into_iter().map(String::from).collect()),
            })
        }
    }

    #[async_trait]
    impl AiBackend for ScriptedBackend {
        async fn call(&self, _prompt: &str) -> IngestionResult<String> {
            let mut q = self.responses.lock().unwrap();
            // Pop from the front; if empty, repeat the most recent response.
            // Repeating lets `max_iterations` tests sit on a perpetual tool
            // call without exhausting the script.
            let next = if q.is_empty() {
                "{\"tool\":\"list_schemas\",\"params\":{}}".to_string()
            } else {
                q.remove(0)
            };
            Ok(next)
        }
    }

    async fn setup_node_for_agent() -> (FoldNode, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let config = NodeConfig::new(temp.path().to_path_buf())
            .with_schema_service_url("test://mock")
            .with_seed_identity(crate::identity::identity_from_keypair(&keypair));
        let node = FoldNode::new(config).await.unwrap();
        (node, temp)
    }

    #[tokio::test]
    async fn max_iterations_returns_ok_with_stopped_reason() {
        // Scripted backend that always emits a tool call → never an answer.
        // This forces the agent loop to bottom out on `max_iterations`.
        let backend = ScriptedBackend::new(vec![]); // queue empty → repeats default tool call
        let service = LlmQueryService::with_backend(backend, std::path::PathBuf::new());

        let (node, _temp) = setup_node_for_agent().await;

        let outcome = service
            .run_agent_query(
                "How many reminders do I have?",
                &[],
                &node,
                "test-user",
                3,
                &[],
                None,
                "test-session",
            )
            .await
            .expect("max_iterations must yield Ok, not Err");

        assert_eq!(
            outcome.stopped_reason.as_deref(),
            Some("max_iterations"),
            "stopped_reason should signal max_iterations was hit"
        );
        assert!(
            !outcome.answer.trim().is_empty(),
            "answer must be non-empty so the UI has something to render"
        );
        assert!(
            outcome.answer.contains("couldn't reach a final answer in 3 steps"),
            "partial-progress message should report the iteration cap; got: {}",
            outcome.answer
        );
        assert_eq!(
            outcome.tool_calls.len(),
            3,
            "agent should have made one tool call per iteration before stopping"
        );
        assert!(
            outcome.tool_calls.iter().all(|tc| tc.tool == "list_schemas"),
            "scripted backend pinned the agent on list_schemas; got: {:?}",
            outcome.tool_calls.iter().map(|tc| &tc.tool).collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn count_question_resolves_via_list_schemas_in_one_tool_call() {
        // Mock the LLM the way fix (B) intends the prompt to steer it:
        // (1) call list_schemas, (2) answer with the count.
        let backend = ScriptedBackend::new(vec![
            "{\"tool\":\"list_schemas\",\"params\":{}}",
            "{\"answer\":\"You have 0 reminders in your database.\"}",
        ]);
        let service = LlmQueryService::with_backend(backend, std::path::PathBuf::new());

        let (node, _temp) = setup_node_for_agent().await;

        let outcome = service
            .run_agent_query(
                "How many reminders do I have?",
                &[],
                &node,
                "test-user",
                4,
                &[],
                None,
                "test-session",
            )
            .await
            .expect("agent should produce a normal final answer");

        assert!(
            outcome.stopped_reason.is_none(),
            "normal completion must not set stopped_reason; got {:?}",
            outcome.stopped_reason
        );
        assert_eq!(outcome.tool_calls.len(), 1, "one list_schemas call is enough");
        assert_eq!(outcome.tool_calls[0].tool, "list_schemas");
        assert!(
            outcome.answer.contains("0 reminders"),
            "answer should report the count derived from list_schemas; got: {}",
            outcome.answer
        );
    }
}
