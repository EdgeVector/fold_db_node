//! LLM prompt builders for query analysis, summarization, chat, and native index.
//!
//! Static instruction text is sourced from `fold_db::llm_registry::prompts::query`.

use super::super::types::Message;
use super::LlmQueryService;
use fold_db::llm_registry::prompts::query as qp;
use fold_db::schema::SchemaWithState;
use serde_json::Value;

/// Format a schema header line for LLM prompts.
/// Shows descriptive name with ID when available, falls back to just the name.
fn schema_prompt_label(schema: &SchemaWithState) -> String {
    match &schema.schema.descriptive_name {
        Some(dn) => format!("{} (ID: `{}`)", dn, schema.schema.name),
        None => schema.schema.name.clone(),
    }
}

impl LlmQueryService {
    /// Build the analysis prompt
    pub(super) fn build_analysis_prompt(
        &self,
        user_query: &str,
        schemas: &[SchemaWithState],
    ) -> String {
        let mut prompt = String::from(qp::QUERY_ANALYSIS_PREAMBLE);

        prompt.push_str("Available Schemas:\n");
        for schema in schemas {
            prompt.push_str(&format!(
                "- {} (Type: {:?}, State: {:?})\n",
                schema_prompt_label(schema),
                schema.schema.schema_type,
                schema.state
            ));

            if let Some(ref key) = schema.schema.key {
                if let Some(ref hash_field) = key.hash_field {
                    prompt.push_str(&format!("  Hash Key: {} (filters: HashKey, HashPattern, HashRangeKey, HashRangePrefix operate on this field)\n", hash_field));
                }
                if let Some(ref range_field) = key.range_field {
                    prompt.push_str(&format!("  Range Key: {} (filters: RangePrefix, RangePattern, RangeRange, HashRangeKey, HashRangePrefix operate on this field)\n", range_field));
                }
            }

            prompt.push_str("  Fields: ");
            let field_names: Vec<String> = schema.schema.runtime_fields.keys().cloned().collect();
            prompt.push_str(&field_names.join(", "));
            prompt.push('\n');
        }

        prompt.push_str(&format!("\nUser Query: {}\n\n", user_query));

        prompt.push_str(
            "Determine:\n\
            1. Which schema(s) to query\n\
            2. What fields to retrieve\n\
            3. What filters to apply (if any)\n\
            4. If an index is needed (consider element count > 10,000 as threshold)\n\n",
        );
        prompt.push_str(qp::FILTER_TYPES_INSTRUCTION);
        prompt.push_str("\n\n");
        prompt.push_str(qp::FILTER_SELECTION_RULES);
        prompt.push_str("\n\n");
        prompt.push_str(
            "EXAMPLES:\n\
            - Search for word \"ai\" in BlogPostWordIndex (hash_field=word): {\"HashKey\": \"ai\"} ✓ CORRECT\n\
            - Search for author \"Jennifer Liu\" in schema with hash_field=author: {\"HashKey\": \"Jennifer Liu\"} ✓ CORRECT\n\
            - Get blog post by ID in BlogPost (range_field=post_id): {\"RangePrefix\": \"post-123\"} ✓ CORRECT\n\
            - Get most recent posts: null filter + sort_order \"desc\" ✓ CORRECT\n\
            - Get posts in date range (range_field=publish_date): {\"RangeRange\": {\"start\": \"2025-09-01\", \"end\": \"2025-09-30\"}} ✓ CORRECT\n\n"
        );
        prompt.push_str(qp::QUERY_RESPONSE_FORMAT);

        prompt
    }

    /// Build the summarization prompt
    pub(super) fn build_summarization_prompt(
        &self,
        original_query: &str,
        results: &[Value],
    ) -> String {
        let results_preview = if results.len() > 1000 {
            &results[..1000]
        } else {
            results
        };

        let results_str = serde_json::to_string_pretty(results_preview)
            .unwrap_or_else(|_| "Failed to serialize results".to_string());

        format!(
            "{}\
            Original Query: {}\n\
            Results ({} total): {}\n\n\
            Provide:\n\
            1. High-level summary\n\
            2. Key insights\n\
            3. Notable patterns or anomalies\n\n\
            Keep the summary concise and informative.",
            qp::SUMMARIZATION_PREAMBLE,
            original_query,
            results.len(),
            results_str
        )
    }

    /// Build the chat prompt for follow-up questions
    pub(super) fn build_chat_prompt(
        &self,
        original_query: &str,
        results: &[Value],
        conversation_history: &[Message],
        question: &str,
    ) -> String {
        let results_preview = if results.len() > 1000 {
            &results[..1000]
        } else {
            results
        };

        let results_str = serde_json::to_string_pretty(results_preview)
            .unwrap_or_else(|_| "Failed to serialize results".to_string());

        let mut prompt = String::from(qp::CHAT_PREAMBLE);

        prompt.push_str(&format!("Original Query: {}\n", original_query));
        prompt.push_str(&format!(
            "Results ({} total): {}\n\n",
            results.len(),
            results_str
        ));

        if !conversation_history.is_empty() {
            prompt.push_str("Conversation History:\n");
            for msg in conversation_history {
                prompt.push_str(&format!("{}: {}\n", msg.role, msg.content));
            }
            prompt.push('\n');
        }

        prompt.push_str(&format!("User Question: {}\n\n", question));
        prompt.push_str("Provide a clear, concise answer based on the data.");

        prompt
    }

    /// Build prompt to analyze if a followup needs a new query
    pub(super) fn build_followup_analysis_prompt(
        &self,
        original_query: &str,
        results: &[Value],
        question: &str,
        schemas: &[fold_db::schema::SchemaWithState],
    ) -> String {
        let results_preview = if results.len() > 100 {
            &results[..100]
        } else {
            results
        };

        let results_str = serde_json::to_string_pretty(results_preview)
            .unwrap_or_else(|_| "Failed to serialize results".to_string());

        let mut prompt = String::from(qp::FOLLOWUP_ANALYSIS_PREAMBLE);

        prompt.push_str(&format!("Original Query: {}\n", original_query));
        prompt.push_str(&format!(
            "Existing Results ({} total): {}\n\n",
            results.len(),
            results_str
        ));
        prompt.push_str(&format!("Follow-up Question: {}\n\n", question));

        prompt.push_str("Available Schemas:\n");
        for schema in schemas {
            prompt.push_str(&format!(
                "- {} (Type: {:?})\n",
                schema_prompt_label(schema),
                schema.schema.schema_type
            ));

            if let Some(ref key) = schema.schema.key {
                if let Some(ref hash_field) = key.hash_field {
                    prompt.push_str(&format!("  Hash Key: {} (filters: HashKey, HashPattern, HashRangeKey, HashRangePrefix operate on this field)\n", hash_field));
                }
                if let Some(ref range_field) = key.range_field {
                    prompt.push_str(&format!("  Range Key: {} (filters: RangePrefix, RangePattern, RangeRange, HashRangeKey, HashRangePrefix operate on this field)\n", range_field));
                }
            }

            prompt.push_str("  Fields: ");
            let field_names: Vec<String> = schema.schema.runtime_fields.keys().cloned().collect();
            prompt.push_str(&field_names.join(", "));
            prompt.push('\n');
        }

        prompt.push_str("\nDetermine if:\n");
        prompt.push_str("1. The question can be FULLY answered from the existing results (needs_query: false)\n");
        prompt.push_str(
            "2. The question needs NEW data that requires a query (needs_query: true)\n\n",
        );

        prompt.push_str("If a new query is needed, provide:\n");
        prompt.push_str("- query: The Query object to execute (same format as before)\n");
        prompt.push_str("- reasoning: Why a new query is needed\n\n");

        prompt.push_str(qp::FILTER_TYPES_INSTRUCTION);
        prompt.push_str("\n\n");
        prompt.push_str(qp::FILTER_SELECTION_RULES);
        prompt.push_str("\n\n");
        prompt.push_str(qp::FOLLOWUP_RESPONSE_FORMAT);

        prompt
    }

    /// Build prompt to generate native index query terms
    pub(super) fn build_native_index_query_terms_prompt(
        &self,
        user_query: &str,
        schemas: &[fold_db::schema::SchemaWithState],
    ) -> String {
        let mut prompt = String::from(qp::NATIVE_INDEX_QUERY_TERMS_PREAMBLE);

        prompt.push_str("Available Schemas:\n");
        for schema in schemas {
            prompt.push_str(&format!(
                "- {} (Type: {:?}, State: {:?})\n",
                schema_prompt_label(schema),
                schema.schema.schema_type,
                schema.state
            ));

            if let Some(ref key) = schema.schema.key {
                if let Some(ref hash_field) = key.hash_field {
                    prompt.push_str(&format!(
                        "  Hash Key: {} (indexed for fast lookup)\n",
                        hash_field
                    ));
                }
                if let Some(ref range_field) = key.range_field {
                    prompt.push_str(&format!(
                        "  Range Key: {} (indexed for fast lookup)\n",
                        range_field
                    ));
                }
            }

            prompt.push_str("  Fields: ");
            let field_names: Vec<String> = schema.schema.runtime_fields.keys().cloned().collect();
            prompt.push_str(&field_names.join(", "));
            prompt.push('\n');
        }

        prompt.push_str(&format!("\nUser Query: {}\n\n", user_query));
        prompt.push_str("Generate 3-8 relevant search terms that would help find records matching this query.\n\n");
        prompt.push_str(qp::NATIVE_INDEX_SEARCH_GUIDELINES);

        prompt
    }

    /// Build prompt for native index search term generation
    pub(super) fn build_native_index_search_prompt(
        &self,
        user_query: &str,
        schemas: &[fold_db::schema::SchemaWithState],
    ) -> String {
        let mut prompt = String::from(
            "You are generating search terms for a native word index system. Based on the user's natural language query, \
            generate 3-6 specific search terms that will be used to search the native index.\n\n"
        );

        prompt.push_str("Available Schemas:\n");
        for schema in schemas {
            prompt.push_str(&format!(
                "- {} (Type: {:?}, State: {:?})\n",
                schema_prompt_label(schema),
                schema.schema.schema_type,
                schema.state
            ));

            if let Some(ref key) = schema.schema.key {
                if let Some(ref hash_field) = key.hash_field {
                    prompt.push_str(&format!(
                        "  Hash Key: {} (indexed for fast lookup)\n",
                        hash_field
                    ));
                }
                if let Some(ref range_field) = key.range_field {
                    prompt.push_str(&format!(
                        "  Range Key: {} (indexed for fast lookup)\n",
                        range_field
                    ));
                }
            }

            prompt.push_str("  Fields: ");
            let field_names: Vec<String> = schema.schema.runtime_fields.keys().cloned().collect();
            prompt.push_str(&field_names.join(", "));
            prompt.push('\n');
        }

        prompt.push_str(&format!("\nUser Query: {}\n\n", user_query));
        prompt.push_str("Generate 3-6 specific search terms that will be used to search the native word index.\n\n");
        prompt.push_str(qp::NATIVE_INDEX_SEARCH_GUIDELINES);

        prompt
    }

    /// Build prompt for interpreting native index results
    pub(super) fn build_native_index_interpretation_prompt(
        &self,
        original_query: &str,
        results: &[fold_db::db_operations::IndexResult],
    ) -> String {
        let results_preview = if results.len() > 50 {
            &results[..50]
        } else {
            results
        };

        let results_str = serde_json::to_string_pretty(results_preview)
            .unwrap_or_else(|_| "Failed to serialize results".to_string());

        format!(
            "{}\
            Original User Query: {}\n\
            Search Results ({} total, showing first {}):\n{}\n\n\
            Provide:\n\
            1. A summary of what was found\n\
            2. Key insights from the results\n\
            3. Notable patterns or interesting findings\n\
            4. If no results were found, suggest alternative search terms\n\n\
            Keep the response concise, informative, and helpful to the user.",
            qp::NATIVE_INDEX_INTERPRETATION_PREAMBLE,
            original_query,
            results.len(),
            results_preview.len(),
            results_str
        )
    }

    /// Build prompt to suggest alternative query strategies
    pub(super) fn build_alternative_query_prompt(
        &self,
        original_user_query: &str,
        failed_query: &fold_db::schema::types::Query,
        schemas: &[fold_db::schema::SchemaWithState],
        previous_attempts: &[String],
    ) -> String {
        let mut prompt = String::from(qp::ALTERNATIVE_QUERY_PREAMBLE);

        prompt.push_str(&format!(
            "User's Original Question: {}\n\n",
            original_user_query
        ));

        prompt.push_str("Failed Query:\n");
        prompt.push_str(&format!("  Schema: {}\n", failed_query.schema_name));
        prompt.push_str(&format!("  Fields: {:?}\n", failed_query.fields));
        prompt.push_str(&format!("  Filter: {:?}\n\n", failed_query.filter));

        if !previous_attempts.is_empty() {
            prompt.push_str("Previous Failed Attempts:\n");
            for (i, attempt) in previous_attempts.iter().enumerate() {
                prompt.push_str(&format!("{}. {}\n", i + 1, attempt));
            }
            prompt.push('\n');
        }

        prompt.push_str("Available Schemas:\n");
        for schema in schemas {
            prompt.push_str(&format!(
                "- {} (Type: {:?}, State: {:?})\n",
                schema_prompt_label(schema),
                schema.schema.schema_type,
                schema.state
            ));

            if let Some(ref key) = schema.schema.key {
                if let Some(ref hash_field) = key.hash_field {
                    prompt.push_str(&format!("  Hash Key: {} (filters: HashKey, HashPattern, HashRangeKey, HashRangePrefix operate on this field)\n", hash_field));
                }
                if let Some(ref range_field) = key.range_field {
                    prompt.push_str(&format!("  Range Key: {} (filters: RangePrefix, RangePattern, RangeRange, HashRangeKey, HashRangePrefix operate on this field)\n", range_field));
                }
            }

            prompt.push_str("  Fields: ");
            let field_names: Vec<String> = schema.schema.runtime_fields.keys().cloned().collect();
            prompt.push_str(&field_names.join(", "));
            prompt.push('\n');
        }

        prompt.push_str("\nSuggest ONE alternative approach:\n");
        prompt.push_str("1. Try a different schema that might have the data\n");
        prompt.push_str(
            "2. Broaden the filter (e.g., remove date constraints, use pattern matching)\n",
        );
        prompt.push_str("3. Try a different filter type (e.g., null filter for all records)\n");
        prompt.push_str("4. Search in related/index schemas\n\n");

        prompt
            .push_str("If you believe there are NO reasonable alternatives left, respond with:\n");
        prompt.push_str(
            "{\"has_alternative\": false, \"query\": null, \"reasoning\": \"explanation\"}\n\n",
        );

        prompt.push_str("Otherwise, respond with:\n");
        prompt.push_str("{\n");
        prompt.push_str("  \"has_alternative\": true,\n");
        prompt.push_str(
            "  \"query\": {\"schema_name\": \"...\", \"fields\": [...], \"filter\": ..., \"sort_order\": \"asc\" or \"desc\" or null},\n",
        );
        prompt.push_str("  \"reasoning\": \"why this approach might work\"\n");
        prompt.push_str("}\n\n");

        prompt.push_str(qp::FILTER_TYPES_INSTRUCTION);
        prompt.push_str("\n\n");
        prompt.push_str(
            "CRITICAL: Prefer key-based filters over Value filter.\n\
            Check Hash Key and Range Key fields to determine correct filter.\n\
            If search matches a key field, use key filter (HashKey/RangePrefix), NOT Value filter.\n\n\
            IMPORTANT: Return ONLY the JSON object."
        );

        prompt
    }
}
