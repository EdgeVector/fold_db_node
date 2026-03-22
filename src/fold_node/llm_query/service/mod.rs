//! LLM service for query analysis and summarization.

mod native_index;
mod parsers;
mod prompts;

use super::types::{FollowupAnalysis, Message, QueryPlan};
use crate::ingestion::{
    ai::client::{build_backend, AiBackend},
    config::IngestionConfig,
};
use fold_db::schema::SchemaWithState;
use serde_json::Value;
use std::sync::Arc;

/// Service for LLM-based query analysis and summarization
pub struct LlmQueryService {
    backend: Arc<dyn AiBackend>,
}

impl LlmQueryService {
    /// Create a new LLM query service
    pub fn new(config: IngestionConfig) -> Result<Self, String> {
        let (backend, init_error) = build_backend(&config);
        let backend = backend.ok_or_else(|| {
            init_error.unwrap_or_else(|| "AI backend initialization failed".to_string())
        })?;
        Ok(Self { backend })
    }

    /// Analyze a natural language query and create an execution plan
    pub async fn analyze_query(
        &self,
        user_query: &str,
        schemas: &[SchemaWithState],
    ) -> Result<QueryPlan, String> {
        let prompt = self.build_analysis_prompt(user_query, schemas);

        // Log prompt for debugging (truncated to avoid too much output)
        let prompt_preview = if prompt.len() > 500 {
            format!(
                "{}... [truncated, total {} chars]",
                &prompt[..500],
                prompt.len()
            )
        } else {
            prompt.clone()
        };
        log::debug!("AI Query Prompt Preview: {}", prompt_preview);

        let response = self.call_llm(&prompt).await?;

        let mut query_plan = self.parse_query_plan(&response)?;

        // Canonicalize schema name to ensure strict case match (backend is strict)
        // This handles AI hallucinations where it might output "Myschema" instead of "MySchema"
        let target_schema_lower = query_plan.query.schema_name.to_lowercase();
        for schema_state in schemas {
            if schema_state.schema.name.to_lowercase() == target_schema_lower {
                if query_plan.query.schema_name != schema_state.schema.name {
                    log::info!(
                        "🤖 AI Autocorrect: Normalizing schema name '{}' -> '{}'",
                        query_plan.query.schema_name,
                        schema_state.schema.name
                    );
                    query_plan.query.schema_name = schema_state.schema.name.clone();
                }
                break;
            }
        }

        Ok(query_plan)
    }

    /// Summarize query results
    pub async fn summarize_results(
        &self,
        original_query: &str,
        results: &[Value],
    ) -> Result<String, String> {
        let prompt = self.build_summarization_prompt(original_query, results);
        self.call_llm(&prompt).await
    }

    /// Answer a follow-up question based on context
    pub async fn answer_question(
        &self,
        original_query: &str,
        results: &[Value],
        conversation_history: &[Message],
        question: &str,
    ) -> Result<String, String> {
        let prompt =
            self.build_chat_prompt(original_query, results, conversation_history, question);
        self.call_llm(&prompt).await
    }

    /// Analyze if a follow-up question needs a new query or can be answered from existing results
    pub async fn analyze_followup_question(
        &self,
        original_query: &str,
        results: &[Value],
        question: &str,
        schemas: &[fold_db::schema::SchemaWithState],
    ) -> Result<FollowupAnalysis, String> {
        let prompt =
            self.build_followup_analysis_prompt(original_query, results, question, schemas);
        let response = self.call_llm(&prompt).await?;
        self.parse_followup_analysis(&response)
    }

    /// Build the system prompt with tool definitions for the agent
    fn build_agent_system_prompt(&self, schemas: &[SchemaWithState]) -> String {
        let now = chrono::Local::now();
        let mut prompt = format!(
            "You are a helpful database assistant with access to tools. Use the tools to query and manipulate data to answer the user's question.\n\nCurrent date and time: {}\n\n",
            now.format("%A, %B %-d, %Y at %-I:%M %p")
        );

        prompt.push_str("## Available Tools\n\n");

        prompt.push_str("### query\n");
        prompt.push_str("Query data from a schema.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- schema_name (string, required): Name of the schema to query\n");
        prompt.push_str("- fields (array of strings, optional): Fields to return. If omitted, returns all fields.\n");
        prompt.push_str("- filter (object, optional): Filter to apply. Examples:\n");
        prompt.push_str("  - {\"HashKey\": \"value\"} - exact match on hash key\n");
        prompt.push_str("  - {\"RangePrefix\": \"prefix\"} - prefix match on range key\n");
        prompt.push_str(&format!("  - {{\"RangeRange\": {{\"start\": \"{}\", \"end\": \"9999-12-31\"}}}} - range key between start and end (inclusive). Use today's date as start for upcoming/future items.\n", chrono::Local::now().format("%Y-%m-%d")));
        prompt.push_str("  - {\"SampleN\": 10} - random sample of N records\n");
        prompt.push_str("  - null - no filter (all records)\n");
        prompt.push_str("- sort_order (string, optional): \"asc\" or \"desc\" — sorts results by range key. Use \"desc\" for most recent/latest queries.\n");
        prompt.push_str("When the user asks for \"upcoming\", \"future\", or \"after today\" items and a schema has a date-based range key, use RangeRange with today's date as start.\n");
        prompt.push_str("When the user asks for \"most recent\", \"latest\", or \"newest\" items, use null filter with sort_order \"desc\".\n");
        prompt.push_str("Example: {\"tool\": \"query\", \"params\": {\"schema_name\": \"Tweet\", \"fields\": [\"content\", \"author\"], \"filter\": null, \"sort_order\": \"desc\"}}\n\n");

        prompt.push_str("### list_schemas\n");
        prompt.push_str("List all available schemas.\n");
        prompt.push_str("Parameters: none\n");
        prompt.push_str("Example: {\"tool\": \"list_schemas\", \"params\": {}}\n\n");

        prompt.push_str("### get_schema\n");
        prompt.push_str("Get details of a specific schema including its fields and key configuration.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- name (string, required): Name of the schema\n");
        prompt.push_str("Example: {\"tool\": \"get_schema\", \"params\": {\"name\": \"Tweet\"}}\n\n");

        prompt.push_str("### search\n");
        prompt.push_str("**PREFERRED for content discovery.** Full-text search across all indexed fields (tags, subjects, names, descriptions, etc.).\n");
        prompt.push_str("Use this whenever the user asks about finding, searching, or checking if data exists.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- terms (string, required): Search keywords (e.g. \"lake\", \"birthday\", \"Leonardo da Vinci\")\n");
        prompt.push_str("Returns matching records with schema_name, field, key_value, and matched content.\n");
        prompt.push_str("After getting results, use the **query** tool with the returned schema_name and key to fetch full records.\n");
        prompt.push_str("Example: {\"tool\": \"search\", \"params\": {\"terms\": \"lake\"}}\n\n");

        prompt.push_str("### scan_folder\n");
        prompt.push_str("Scan a filesystem folder to discover files suitable for ingestion. Uses AI to classify files as personal data, media, config, etc.\n");
        prompt.push_str("Automatically skips config files, binaries, code projects, and other non-personal data.\n");
        prompt.push_str("Use this when the user wants to add/import/ingest data from a folder.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- path (string, required): Folder path to scan (e.g. \"sample_data\", \"/Users/tom/Documents\", \"~/Documents\")\n");
        prompt.push_str("- max_files (number, optional): Maximum files to scan (default: 100)\n");
        prompt.push_str("Returns: recommended_files (files to ingest), skipped_files, summary by category, total_estimated_cost.\n");
        prompt.push_str("After scanning, show the user what was found and ask if they want to proceed with ingestion.\n");
        prompt.push_str("Example: {\"tool\": \"scan_folder\", \"params\": {\"path\": \"sample_data\"}}\n\n");

        prompt.push_str("### ingest_files\n");
        prompt.push_str("Ingest files from a previously scanned folder into the database. Each file is processed by AI to extract schema and data.\n");
        prompt.push_str("Only call this AFTER scan_folder and AFTER the user confirms they want to proceed.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- folder_path (string, required): The same folder path used in scan_folder\n");
        prompt.push_str("- files (array of strings, required): Relative file paths from the scan results (use the 'path' field from recommended_files)\n");
        prompt.push_str("Returns: total files processed, succeeded count, failed count, per-file results with schema_used.\n");
        prompt.push_str("Example: {\"tool\": \"ingest_files\", \"params\": {\"folder_path\": \"sample_data\", \"files\": [\"contacts/address_book.json\", \"journal/2025-01-15.txt\"]}}\n\n");

        prompt.push_str("### create_view\n");
        prompt.push_str("Create a transform view with a compiled Rust WASM transform. Every view MUST have a WASM transform — identity views are not allowed through this tool.\n");
        prompt.push_str("Before calling this tool, ALWAYS use get_schema first to inspect the source schema(s) and understand their fields.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- name (string, required): Unique view name (e.g. \"EnrichedPosts\")\n");
        prompt.push_str("- schema_type (string, required): \"Single\", \"Range\", \"Hash\", or \"HashRange\"\n");
        prompt.push_str("- key_config (object, optional): {\"hash_field\": \"field_name\", \"range_field\": \"field_name\"} — required for Hash/Range/HashRange types\n");
        prompt.push_str("- input_queries (array, required): [{\"schema_name\": \"Name\", \"fields\": [\"field1\", \"field2\"]}]\n");
        prompt.push_str("- output_fields (object, required): {\"field_name\": \"type\"} where type is Any, String, Integer, Boolean, Date, Bytes, Reference, List, or Map\n");
        prompt.push_str("- rust_transform (string, required): A complete Rust function definition. See template below.\n\n");

        prompt.push_str("#### Rust Transform Template\n");
        prompt.push_str("The transform receives query results as JSON and must return output fields as JSON.\n");
        prompt.push_str("```rust\n");
        prompt.push_str("fn transform_impl(input: Value) -> Value {\n");
        prompt.push_str("    // input structure:\n");
        prompt.push_str("    // {\"inputs\": {\"SchemaName\": [{\"key\": {..}, \"fields\": {\"field\": value, ..}}, ..]}}\n");
        prompt.push_str("    //\n");
        prompt.push_str("    // Must return:\n");
        prompt.push_str("    // {\"fields\": {\"output_field\": value, ...}}\n");
        prompt.push_str("    // For multi-record output, return:\n");
        prompt.push_str("    // {\"records\": [{\"key\": {..}, \"fields\": {\"output_field\": value}}, ...]}\n");
        prompt.push_str("    \n");
        prompt.push_str("    let inputs = &input[\"inputs\"];\n");
        prompt.push_str("    // ... your transform logic ...\n");
        prompt.push_str("    serde_json::json!({ \"fields\": { /* ... */ } })\n");
        prompt.push_str("}\n");
        prompt.push_str("```\n\n");

        prompt.push_str("#### Examples\n\n");
        prompt.push_str("**Word count view** (count words in a text field):\n");
        prompt.push_str("```json\n");
        prompt.push_str("{\"tool\": \"create_view\", \"params\": {\n");
        prompt.push_str("  \"name\": \"PostWordCounts\",\n");
        prompt.push_str("  \"schema_type\": \"Single\",\n");
        prompt.push_str("  \"input_queries\": [{\"schema_name\": \"BlogPost\", \"fields\": [\"content\"]}],\n");
        prompt.push_str("  \"output_fields\": {\"word_count\": \"Integer\", \"content_preview\": \"String\"},\n");
        prompt.push_str("  \"rust_transform\": \"fn transform_impl(input: Value) -> Value {\\n    let inputs = &input[\\\"inputs\\\"];\\n    let posts = inputs[\\\"BlogPost\\\"].as_array().unwrap_or(&vec![]);\\n    let records: Vec<Value> = posts.iter().map(|post| {\\n        let content = post[\\\"fields\\\"][\\\"content\\\"].as_str().unwrap_or(\\\"\\\");\\n        let word_count = content.split_whitespace().count();\\n        let preview = content.chars().take(100).collect::<String>();\\n        serde_json::json!({\\n            \\\"key\\\": post[\\\"key\\\"],\\n            \\\"fields\\\": {\\n                \\\"word_count\\\": word_count,\\n                \\\"content_preview\\\": preview\\n            }\\n        })\\n    }).collect();\\n    serde_json::json!({ \\\"records\\\": records })\\n}\"\n");
        prompt.push_str("}}\n");
        prompt.push_str("```\n\n");

        prompt.push_str("**Concatenation view** (merge fields from multiple schemas):\n");
        prompt.push_str("```json\n");
        prompt.push_str("{\"tool\": \"create_view\", \"params\": {\n");
        prompt.push_str("  \"name\": \"AuthoredPosts\",\n");
        prompt.push_str("  \"schema_type\": \"Single\",\n");
        prompt.push_str("  \"input_queries\": [\n");
        prompt.push_str("    {\"schema_name\": \"BlogPost\", \"fields\": [\"title\", \"author_id\"]},\n");
        prompt.push_str("    {\"schema_name\": \"Author\", \"fields\": [\"name\"]}\n");
        prompt.push_str("  ],\n");
        prompt.push_str("  \"output_fields\": {\"title\": \"String\", \"author_name\": \"String\"},\n");
        prompt.push_str("  \"rust_transform\": \"fn transform_impl(input: Value) -> Value {\\n    let inputs = &input[\\\"inputs\\\"];\\n    let posts = inputs[\\\"BlogPost\\\"].as_array().unwrap_or(&vec![]);\\n    let authors = inputs[\\\"Author\\\"].as_array().unwrap_or(&vec![]);\\n    let first_author = authors.first().map(|a| a[\\\"fields\\\"][\\\"name\\\"].as_str().unwrap_or(\\\"Unknown\\\")).unwrap_or(\\\"Unknown\\\");\\n    let records: Vec<Value> = posts.iter().map(|post| {\\n        serde_json::json!({\\n            \\\"key\\\": post[\\\"key\\\"],\\n            \\\"fields\\\": {\\n                \\\"title\\\": post[\\\"fields\\\"][\\\"title\\\"],\\n                \\\"author_name\\\": first_author\\n            }\\n        })\\n    }).collect();\\n    serde_json::json!({ \\\"records\\\": records })\\n}\"\n");
        prompt.push_str("}}\n");
        prompt.push_str("```\n\n");

        prompt.push_str("## Available Schemas\n\n");
        for schema in schemas {
            prompt.push_str(&format!(
                "- **{}** (Type: {:?}, State: {:?})\n",
                schema.schema.name, schema.schema.schema_type, schema.state
            ));

            if let Some(ref key) = schema.schema.key {
                if let Some(ref hash_field) = key.hash_field {
                    prompt.push_str(&format!("  - Hash Key: {}\n", hash_field));
                }
                if let Some(ref range_field) = key.range_field {
                    prompt.push_str(&format!("  - Range Key: {}\n", range_field));
                }
            }

            prompt.push_str("  - Fields: ");
            let field_names: Vec<String> = schema.schema.runtime_fields.keys().cloned().collect();
            prompt.push_str(&field_names.join(", "));
            prompt.push('\n');
        }

        prompt.push_str("\n## Instructions\n\n");
        prompt.push_str("1. Analyze the user's request\n");
        prompt.push_str("2. **For content discovery questions** (\"do I have\", \"find\", \"show me\", \"any photos of\", \"search for\"), ");
        prompt.push_str("ALWAYS use the **search** tool first with relevant keywords. ");
        prompt.push_str("This searches the full-text index and will find records by tags, subjects, descriptions, names, and other indexed content. ");
        prompt.push_str("After getting search results, use the **query** tool with the returned schema and key to fetch full records.\n");
        prompt.push_str("3. Use other tools to gather additional information as needed\n");
        prompt.push_str("4. When you have enough information to answer, provide your final response\n");
        prompt.push_str("5. Use the current date/time above to determine temporal context. Events with dates before today are in the PAST. Events with dates after today are in the FUTURE. Label them accordingly (e.g. \"upcoming\" vs \"past\").\n\n");
        prompt.push_str("## Reference Fields\n\n");
        prompt.push_str("Some fields are References to records in other schemas. Query results automatically resolve references one level deep.\n");
        prompt.push_str("If a field value is an array of objects with \"schema\" and \"key\" properties, those are references to child records.\n");
        prompt.push_str("The referenced data will be included inline when available. If you need deeper data (references within references), ");
        prompt.push_str("use get_schema to find the child schema's fields, then use query to fetch the child schema's data directly.\n\n");
        prompt.push_str("IMPORTANT: Always respond with valid JSON. Either:\n");
        prompt.push_str("- {\"tool\": \"tool_name\", \"params\": {...}} to call a tool\n");
        prompt.push_str("- {\"answer\": \"your response\"} to provide the final answer\n");

        prompt
    }

    /// Call the LLM service
    pub(super) async fn call_llm(&self, prompt: &str) -> Result<String, String> {
        self.backend.call(prompt).await.map_err(|e| format!("AI backend error: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_db::schema::types::{
        DeclarativeSchemaDefinition, KeyConfig,
    };
    use fold_db::schema::{SchemaState, SchemaWithState};

    fn create_test_hash_range_schema() -> SchemaWithState {
        let mut schema = DeclarativeSchemaDefinition::new(
            "BlogPostAuthorIndex".to_string(),
            fold_db::schema::types::schema::DeclarativeSchemaType::HashRange,
            Some(KeyConfig {
                hash_field: Some("author".to_string()),
                range_field: Some("publish_date".to_string()),
            }),
            None, // fields
            None, // transform_fields
            None, // field_mappers
        );

        schema.descriptive_name = Some("Blog Post Author Index".to_string());
        schema.field_classifications.insert("author".to_string(), vec!["word".to_string()]);
        schema.field_classifications.insert("publish_date".to_string(), vec!["word".to_string()]);

        SchemaWithState {
            schema,
            state: SchemaState::Approved,
        }
    }

    #[test]
    fn test_prompt_includes_hash_and_range_keys() {
        let mut config = crate::ingestion::config::IngestionConfig::default();
        config.provider = crate::ingestion::config::AIProvider::Ollama;

        let service = LlmQueryService::new(config).expect("Failed to create service");
        let schemas = vec![create_test_hash_range_schema()];

        let prompt = service.build_analysis_prompt("Find posts by Jennifer Liu", &schemas);

        // Verify prompt includes hash key information
        assert!(
            prompt.contains("Hash Key: author"),
            "Prompt should include Hash Key field"
        );
        assert!(
            prompt.contains("Range Key: publish_date"),
            "Prompt should include Range Key field"
        );

        // Verify prompt includes filter guidance
        assert!(
            prompt.contains("HashKey"),
            "Prompt should mention HashKey filter"
        );
        assert!(
            prompt.contains("CRITICAL"),
            "Prompt should include critical filter selection guidance"
        );
        assert!(
            prompt.contains("Jennifer Liu"),
            "Prompt should include the example with Jennifer Liu"
        );
    }

    #[test]
    fn test_prompt_shows_correct_vs_incorrect_examples() {
        let mut config = crate::ingestion::config::IngestionConfig::default();
        config.provider = crate::ingestion::config::AIProvider::Ollama;

        let service = LlmQueryService::new(config).expect("Failed to create service");
        let schemas = vec![create_test_hash_range_schema()];

        let prompt = service.build_analysis_prompt("Test query", &schemas);

        // Verify prompt includes correct examples
        assert!(
            prompt.contains("✓ CORRECT"),
            "Prompt should show correct examples"
        );
    }
}
