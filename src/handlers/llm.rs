//! Shared LLM Query Handlers
//!
//! Framework-agnostic handlers for LLM query operations.
//! These can be called by both HTTP server routes and Lambda handlers.

use crate::fold_node::llm_query::{conversation_store, types::*, LlmQueryService, SessionManager};
use crate::fold_node::node::FoldNode;
use crate::handlers::response::{
    get_db_guard, ApiResponse, HandlerError, HandlerResult, IntoHandlerError, IntoTypedHandlerError,
};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::schema::SchemaWithState;
use serde_json::{json, Value};

use super::llm_hydration::hydrate_index_results;
pub use super::llm_types::*;

// ============================================================================
// Shared Helpers
// ============================================================================

/// Retrieve a session context and its query results, or return an appropriate error.
fn get_session_with_results(
    session_manager: &SessionManager,
    session_id: &str,
) -> Result<(SessionContext, Vec<Value>), HandlerError> {
    let context = match session_manager.get_session(session_id) {
        Ok(Some(ctx)) => ctx,
        Ok(None) => return Err(HandlerError::NotFound("Session not found".to_string())),
        Err(e) => {
            return Err(HandlerError::Internal(format!(
                "Failed to get session: {}",
                e
            )))
        }
    };

    let results = context.query_results.clone().ok_or_else(|| {
        HandlerError::BadRequest("No query results available in session".to_string())
    })?;

    Ok((context, results))
}

/// Log a warning if a session update fails. Session tracking is best-effort —
/// the primary operation has already succeeded, so we warn rather than fail.
fn warn_session_err(result: Result<(), String>, action: &str) {
    if let Err(e) = result {
        log_feature!(
            LogFeature::Query,
            warn,
            "Session update failed ({}): {}",
            action,
            e
        );
    }
}

/// Fetch all schemas with states from the database.
async fn get_schemas(node: &FoldNode) -> Result<Vec<SchemaWithState>, HandlerError> {
    let db_guard = get_db_guard(node)?;
    db_guard
        .schema_manager()
        .get_schemas_with_states()
        .typed_handler_err()
}

/// Canned answer returned by the agent when the node has no user-authored data.
/// Mirrored in tests; changes here are user-visible.
const EMPTY_STORE_AGENT_ANSWER: &str = "You haven't ingested any data yet. To get started:\n\n\
    • Import from Apple (Notes, Photos, Calendar, Contacts, Reminders) in Settings → Apple Import\n\
    • Upload files via the File Upload tab\n\
    • Connect to an organization to see shared data\n\n\
    Once data is ingested, ask me again and I'll query it for you.";

/// Returns true when no approved schema is user-authored.
///
/// Every fresh node has the 12 Phase-1 built-ins (Fingerprint, Mention, Persona, …)
/// pre-approved, but these are platform plumbing, not ingested content. If that's
/// all we see, running the full tool loop is ~30s of wasted tokens against empty
/// molecules (alpha dogfood papercut c600e). Callers should short-circuit to
/// `EMPTY_STORE_AGENT_ANSWER`.
///
/// Safe fallback: a schema missing `descriptive_name` is treated as user-authored,
/// so we run the agent rather than incorrectly short-circuiting.
fn is_empty_user_store(schemas: &[SchemaWithState]) -> bool {
    use crate::schema_service::builtin_schemas::PHASE_1_DESCRIPTIVE_NAMES;
    use fold_db::schema::SchemaState;
    schemas
        .iter()
        .filter(|s| s.state == SchemaState::Approved)
        .all(|s| {
            s.schema
                .descriptive_name
                .as_deref()
                .map(|d| PHASE_1_DESCRIPTIVE_NAMES.contains(&d))
                .unwrap_or(false)
        })
}

// ============================================================================
// Handler Functions
// ============================================================================

/// Handle chat action - ask a follow-up question about query results
///
/// # Arguments
/// * `request` - The chat request
/// * `user_hash` - User identifier for isolation
/// * `service` - LLM query service
/// * `session_manager` - Session manager for tracking conversation state
/// * `node` - FoldDB node instance
///
/// # Returns
/// * `HandlerResult<ChatHandlerResponse>` - Chat response with answer
pub async fn chat(
    request: ChatRequest,
    user_hash: &str,
    service: &LlmQueryService,
    session_manager: &SessionManager,
    node: &FoldNode,
) -> HandlerResult<ChatHandlerResponse> {
    log_feature!(
        LogFeature::Query,
        info,
        "AI Query Chat: received for session: {:?}, user: {}",
        request.session_id,
        user_hash
    );

    let session_id = &request.session_id;
    let question = &request.question;

    let (context, results) = get_session_with_results(session_manager, session_id)?;
    let schemas = get_schemas(node).await?;

    // Analyze if question needs a new query
    let analysis = service
        .analyze_followup_question(&context.original_query, &results, question, &schemas)
        .await
        .handler_err("analyze followup")?;

    // Answer the question using existing context
    let answer = service
        .answer_question(
            &context.original_query,
            &results,
            &context.conversation_history,
            question,
        )
        .await
        .handler_err("answer question")?;

    // Update session with conversation
    warn_session_err(
        session_manager.add_message(session_id, "user".to_string(), question.clone()),
        "add user message",
    );

    let assistant_message = if analysis.needs_query {
        format!("[Analyzed context: {}]\n\n{}", analysis.reasoning, answer)
    } else {
        answer.clone()
    };

    warn_session_err(
        session_manager.add_message(
            session_id,
            "assistant".to_string(),
            assistant_message.clone(),
        ),
        "add assistant message",
    );

    // Persist chat turn to FoldDB
    conversation_store::save_chat_turn(
        node,
        session_id.clone(),
        question.clone(),
        assistant_message.clone(),
    )
    .await;

    Ok(ApiResponse::success_with_user(
        ChatHandlerResponse {
            answer: assistant_message,
            context_used: true,
        },
        user_hash,
    ))
}

/// Analyze if a follow-up question can be answered from existing context
///
/// # Arguments
/// * `request` - The chat request containing the question
/// * `user_hash` - User identifier for isolation
/// * `service` - LLM query service
/// * `session_manager` - Session manager for tracking conversation state
/// * `node` - FoldDB node instance
///
/// # Returns
/// * `HandlerResult<AnalyzeFollowupHandlerResponse>` - Analysis of whether new query is needed
pub async fn analyze_followup(
    request: ChatRequest,
    user_hash: &str,
    service: &LlmQueryService,
    session_manager: &SessionManager,
    node: &FoldNode,
) -> HandlerResult<AnalyzeFollowupHandlerResponse> {
    log_feature!(
        LogFeature::Query,
        info,
        "AI Query Analyze Followup: received for session: {:?}, user: {}",
        request.session_id,
        user_hash
    );

    let session_id = &request.session_id;
    let question = &request.question;

    let (context, results) = get_session_with_results(session_manager, session_id)?;
    let schemas = get_schemas(node).await?;

    // Analyze followup question
    let analysis = service
        .analyze_followup_question(&context.original_query, &results, question, &schemas)
        .await
        .handler_err("analyze followup")?;

    Ok(ApiResponse::success_with_user(
        AnalyzeFollowupHandlerResponse {
            needs_query: analysis.needs_query,
            query: analysis.query,
            reasoning: analysis.reasoning,
        },
        user_hash,
    ))
}

/// Execute an AI-native index query workflow
///
/// This handler implements a three-step process:
/// 1. Search the native index for matching entries
/// 2. Hydrate results by fetching actual field values from records
/// 3. Send hydrated results to AI for interpretation
///
/// # Arguments
/// * `request` - The run query request
/// * `user_hash` - User identifier for isolation
/// * `service` - LLM query service
/// * `session_manager` - Session manager for tracking conversation state
/// * `node` - FoldDB node instance
///
/// # Returns
/// * `HandlerResult<AiNativeIndexHandlerResponse>` - AI interpretation and raw results
pub async fn ai_native_index_query(
    request: RunQueryRequest,
    user_hash: &str,
    service: &LlmQueryService,
    session_manager: &SessionManager,
    node: &FoldNode,
) -> HandlerResult<AiNativeIndexHandlerResponse> {
    log_feature!(
        LogFeature::Query,
        info,
        "AI Native Index Query: received for session: {:?}, user: {}",
        request.session_id,
        user_hash
    );

    // Create or get session
    let session_id = session_manager
        .create_or_get_session(request.session_id.clone(), request.query.clone())
        .handler_err("create session")?;

    // Get FoldDb for both schema access and hydration queries
    let db_guard = get_db_guard(node)?;

    // Get available schemas
    let schemas: Vec<SchemaWithState> = db_guard
        .schema_manager()
        .get_schemas_with_states()
        .typed_handler_err()?;

    let db_ops = db_guard.get_db_ops();

    // Step 1: Search the native index
    let search_results = service
        .search_native_index(&request.query, &schemas, &db_ops)
        .await
        .handler_err("search native index")?;

    log_feature!(
        LogFeature::Query,
        info,
        "AI Native Index Query: found {} results, hydrating...",
        search_results.len()
    );

    // Step 2: Hydrate results by fetching actual field values.
    // Loopback owner context — see trust-boundary note in CLAUDE.md.
    let owner_ctx = fold_db::access::AccessContext::owner(node.get_node_public_key().to_string());
    let hydrated_results = hydrate_index_results(search_results, &db_guard, &owner_ctx).await;

    log_feature!(
        LogFeature::Query,
        info,
        "AI Native Index Query: hydration complete, {} results ready for AI interpretation",
        hydrated_results.len()
    );

    // Step 3: Send hydrated results to AI for interpretation
    let ai_interpretation = service
        .interpret_native_index_results(&request.query, &hydrated_results)
        .await
        .handler_err("interpret results")?;

    // Store results in session for context tracking
    let results_as_json: Vec<Value> = hydrated_results
        .into_iter()
        .map(|result| serde_json::to_value(result).unwrap_or(json!({})))
        .collect();

    warn_session_err(
        session_manager.add_results(&session_id, results_as_json.clone()),
        "store results",
    );
    warn_session_err(
        session_manager.add_message(&session_id, "user".to_string(), request.query.clone()),
        "add user message",
    );
    warn_session_err(
        session_manager.add_message(
            &session_id,
            "assistant".to_string(),
            ai_interpretation.clone(),
        ),
        "add assistant message",
    );

    log_feature!(
        LogFeature::Query,
        info,
        "AI Native Index Query complete for session: {}",
        session_id
    );

    // Persist native-index turn to FoldDB
    conversation_store::save_chat_turn(
        node,
        session_id.clone(),
        request.query.clone(),
        ai_interpretation.clone(),
    )
    .await;

    Ok(ApiResponse::success_with_user(
        AiNativeIndexHandlerResponse {
            ai_interpretation,
            raw_results: results_as_json,
            query: request.query,
            session_id,
        },
        user_hash,
    ))
}

/// Execute an agent query - an autonomous LLM agent that can use tools
///
/// The agent iteratively:
/// 1. Analyzes the user's query
/// 2. Calls tools (query, list_schemas, get_schema, search) as needed
/// 3. Returns a final answer when complete
///
/// # Arguments
/// * `request` - The agent query request
/// * `user_hash` - User identifier for isolation
/// * `service` - LLM query service
/// * `session_manager` - Session manager for tracking conversation state
/// * `node` - FoldDB node instance
///
/// # Returns
/// * `HandlerResult<AgentQueryHandlerResponse>` - Agent response with answer and tool calls
pub async fn agent_query(
    request: AgentQueryHandlerRequest,
    user_hash: &str,
    service: &LlmQueryService,
    session_manager: &SessionManager,
    node: &FoldNode,
    progress_tracker: Option<&crate::ingestion::ProgressTracker>,
) -> HandlerResult<AgentQueryHandlerResponse> {
    log_feature!(
        LogFeature::Query,
        info,
        "Agent Query: received for user: {}, query: {}",
        user_hash,
        &request.query[..request.query.len().min(100)]
    );

    // Create or get session
    let session_id = session_manager
        .create_or_get_session(request.session_id.clone(), request.query.clone())
        .handler_err("create session")?;

    let schemas = get_schemas(node).await?;

    // Short-circuit: nothing ingested yet. Running the tool loop on an empty store
    // wastes tokens + wall-clock (alpha papercut c600e: >30s "Thinking…" on fresh node).
    if is_empty_user_store(&schemas) {
        log_feature!(
            LogFeature::Query,
            info,
            "Agent Query: empty-store short-circuit for session {}",
            session_id
        );
        warn_session_err(
            session_manager.add_message(&session_id, "user".to_string(), request.query.clone()),
            "add user message (empty-store)",
        );
        warn_session_err(
            session_manager.add_message(
                &session_id,
                "assistant".to_string(),
                EMPTY_STORE_AGENT_ANSWER.to_string(),
            ),
            "add assistant message (empty-store)",
        );
        conversation_store::save_conversation_turn(
            node,
            session_id.clone(),
            request.query.clone(),
            EMPTY_STORE_AGENT_ANSWER.to_string(),
            Vec::new(),
        )
        .await;
        return Ok(ApiResponse::success_with_user(
            AgentQueryHandlerResponse {
                answer: EMPTY_STORE_AGENT_ANSWER.to_string(),
                tool_calls: Vec::new(),
                session_id,
            },
            user_hash,
        ));
    }

    // Default max iterations
    let max_iterations = request.max_iterations.unwrap_or(10);

    // Get prior conversation history from session
    let mut prior_history = session_manager
        .get_session(&session_id)
        .ok()
        .flatten()
        .map(|ctx| ctx.conversation_history)
        .unwrap_or_default();

    // Inject structured context (e.g. scan results from the frontend) so the LLM can reference it
    if let Some(context) = &request.context {
        use crate::fold_node::llm_query::types::Message;
        prior_history.push(Message {
            role: "context".to_string(),
            content: format!(
                "Attached data from previous tool results:\n{}",
                serde_json::to_string_pretty(context).unwrap_or_default()
            ),
            timestamp: std::time::SystemTime::now(),
        });
    }

    // Run the agent with prior conversation context
    let (answer, tool_calls) = service
        .run_agent_query(
            &request.query,
            &schemas,
            node,
            user_hash,
            max_iterations,
            &prior_history,
            progress_tracker,
        )
        .await
        .handler_err("run agent query")?;

    // Store conversation in session (user message + tool call summary + assistant answer)
    warn_session_err(
        session_manager.add_message(&session_id, "user".to_string(), request.query.clone()),
        "add user message",
    );

    // Store a summary of tool calls so future turns know what happened
    if !tool_calls.is_empty() {
        let tool_summary: Vec<String> = tool_calls
            .iter()
            .map(|tc| {
                let result_preview = tc.result.to_string();
                let result_short = if result_preview.len() > 500 {
                    format!("{}...[truncated]", &result_preview[..500])
                } else {
                    result_preview
                };
                format!(
                    "Tool: {}\nParams: {}\nResult: {}",
                    tc.tool,
                    serde_json::to_string(&tc.params).unwrap_or_default(),
                    result_short
                )
            })
            .collect();
        warn_session_err(
            session_manager.add_message(
                &session_id,
                "tool_calls".to_string(),
                tool_summary.join("\n---\n"),
            ),
            "add tool calls",
        );
    }

    warn_session_err(
        session_manager.add_message(&session_id, "assistant".to_string(), answer.clone()),
        "add assistant message",
    );

    log_feature!(
        LogFeature::Query,
        info,
        "Agent Query complete for session: {}. Made {} tool calls.",
        session_id,
        tool_calls.len()
    );

    // Persist conversation turn to FoldDB synchronously so failures are visible
    conversation_store::save_conversation_turn(
        node,
        session_id.clone(),
        request.query.clone(),
        answer.clone(),
        tool_calls.clone(),
    )
    .await;

    Ok(ApiResponse::success_with_user(
        AgentQueryHandlerResponse {
            answer,
            tool_calls,
            session_id,
        },
        user_hash,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_db::schema::types::SchemaType;
    use fold_db::schema::{Schema, SchemaState, SchemaWithState};

    fn schema_with(descriptive: Option<&str>) -> Schema {
        let mut s = Schema::new("canonical-name".to_string(), SchemaType::Single, None, None, None, None);
        s.descriptive_name = descriptive.map(str::to_string);
        s
    }

    #[test]
    fn empty_schema_list_is_empty_store() {
        assert!(is_empty_user_store(&[]));
    }

    #[test]
    fn only_builtin_approved_schemas_is_empty_store() {
        let schemas = vec![
            SchemaWithState::new(schema_with(Some("Fingerprint")), SchemaState::Approved),
            SchemaWithState::new(schema_with(Some("Persona")), SchemaState::Approved),
            SchemaWithState::new(schema_with(Some("Identity")), SchemaState::Approved),
        ];
        assert!(is_empty_user_store(&schemas));
    }

    #[test]
    fn one_user_authored_approved_schema_is_not_empty() {
        let schemas = vec![
            SchemaWithState::new(schema_with(Some("Fingerprint")), SchemaState::Approved),
            SchemaWithState::new(schema_with(Some("Recipe")), SchemaState::Approved),
        ];
        assert!(!is_empty_user_store(&schemas));
    }

    #[test]
    fn user_authored_available_still_empty_until_approved() {
        // Available (proposed, not yet approved) schemas do not count — user can't query them.
        let schemas = vec![
            SchemaWithState::new(schema_with(Some("Fingerprint")), SchemaState::Approved),
            SchemaWithState::new(schema_with(Some("Recipe")), SchemaState::Available),
        ];
        assert!(is_empty_user_store(&schemas));
    }

    #[test]
    fn blocked_user_schema_still_empty() {
        let schemas = vec![SchemaWithState::new(
            schema_with(Some("Recipe")),
            SchemaState::Blocked,
        )];
        assert!(is_empty_user_store(&schemas));
    }

    #[test]
    fn schema_without_descriptive_name_treated_as_user_authored() {
        // Safe fallback: when we can't classify, run the agent rather than short-circuit.
        let schemas = vec![SchemaWithState::new(schema_with(None), SchemaState::Approved)];
        assert!(!is_empty_user_store(&schemas));
    }

    #[test]
    fn all_twelve_phase_1_builtins_are_empty_store() {
        use crate::schema_service::builtin_schemas::PHASE_1_DESCRIPTIVE_NAMES;
        let schemas: Vec<SchemaWithState> = PHASE_1_DESCRIPTIVE_NAMES
            .iter()
            .map(|name| SchemaWithState::new(schema_with(Some(name)), SchemaState::Approved))
            .collect();
        assert_eq!(schemas.len(), 12);
        assert!(is_empty_user_store(&schemas));
    }

    #[test]
    fn empty_store_answer_mentions_import_paths() {
        // Guardrail: if someone edits the canned message, keep the three primary paths.
        assert!(EMPTY_STORE_AGENT_ANSWER.contains("Apple"));
        assert!(EMPTY_STORE_AGENT_ANSWER.contains("Upload"));
        assert!(EMPTY_STORE_AGENT_ANSWER.contains("organization"));
    }
}
