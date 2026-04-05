//! LLM service for query analysis and summarization.

mod native_index;
mod parsers;
mod prompts;
pub mod web_tools;

use super::types::{FollowupAnalysis, Message, QueryPlan};
use crate::ingestion::{
    ai::client::{build_backend, AiBackend},
    config::IngestionConfig,
};
use fold_db::llm_registry::prompts::query as qp;
use fold_db::schema::SchemaWithState;
use serde_json::Value;
use std::sync::Arc;

/// Service for LLM-based query analysis and summarization
pub struct LlmQueryService {
    backend: Arc<dyn AiBackend>,
}

impl LlmQueryService {
    /// Create a new LLM query service.
    ///
    /// Uses `config.query_config()` to apply per-use-case provider/model overrides.
    /// If no query overrides are set, inherits from the primary ingestion config.
    pub fn new(config: IngestionConfig) -> Result<Self, String> {
        let effective = config.query_config();
        let (backend, init_error) = build_backend(&effective);
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
            "{}Current date and time: {}\n\n",
            qp::AGENT_SYSTEM_PREAMBLE,
            now.format("%A, %B %-d, %Y at %-I:%M %p")
        );

        prompt.push_str("## Available Tools\n\n");

        prompt.push_str("### query\n");
        prompt.push_str("Query data from a schema.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- schema_name (string, required): Name of the schema to query\n");
        prompt.push_str("- fields (array of strings, optional): Fields to return. If omitted, returns all fields. IMPORTANT: Always specify only the fields you need — large text fields like 'markdown' or 'body' can be very large and cause context overflow.\n");
        prompt.push_str("- filter (object, optional): Filter to apply. Examples:\n");
        prompt.push_str("  - {\"HashKey\": \"value\"} - exact match on hash key\n");
        prompt.push_str("  - {\"RangePrefix\": \"prefix\"} - prefix match on range key\n");
        prompt.push_str(&format!("  - {{\"RangeRange\": {{\"start\": \"{}\", \"end\": \"9999-12-31\"}}}} - range key between start and end (inclusive). Use today's date as start for upcoming/future items.\n", chrono::Local::now().format("%Y-%m-%d")));
        prompt.push_str("  - {\"SampleN\": 10} - random sample of N records\n");
        prompt.push_str("  - null - no filter (all records)\n");
        prompt.push_str("- sort_order (string, optional): \"asc\" or \"desc\" — sorts results by range key. Use \"desc\" for most recent/latest queries.\n");
        prompt.push_str("- value_filters (array, optional): Numeric comparison filters on field values. Applied AFTER key-based filtering. Multiple filters are AND'd. Examples:\n");
        prompt.push_str("  - [{\"LessThan\": {\"field\": \"price\", \"value\": 600}}] - records where price < 600\n");
        prompt.push_str("  - [{\"GreaterThan\": {\"field\": \"score\", \"value\": 90}}] - records where score > 90\n");
        prompt.push_str("  - [{\"Between\": {\"field\": \"price\", \"min\": 200, \"max\": 600}}] - records where 200 <= price <= 600\n");
        prompt.push_str("  - [{\"Equals\": {\"field\": \"rating\", \"value\": 5}}] - records where rating == 5\n");
        prompt.push_str("  - null - no value filtering\n");
        prompt.push_str("When the user asks for \"upcoming\", \"future\", or \"after today\" items and a schema has a date-based range key, use RangeRange with today's date as start.\n");
        prompt.push_str("When the user asks for \"most recent\", \"latest\", or \"newest\" items, use null filter with sort_order \"desc\".\n");
        prompt.push_str("When the user asks for numeric comparisons (e.g., \"flights under $600\", \"scores above 90\"), use value_filters.\n");
        prompt.push_str("Example: {\"tool\": \"query\", \"params\": {\"schema_name\": \"Flight\", \"fields\": [\"airline\", \"price\", \"departure\"], \"filter\": null, \"value_filters\": [{\"LessThan\": {\"field\": \"price\", \"value\": 600}}], \"sort_order\": \"asc\"}}\n\n");

        prompt.push_str("### list_schemas\n");
        prompt.push_str("List all available schemas.\n");
        prompt.push_str("Parameters: none\n");
        prompt.push_str("Example: {\"tool\": \"list_schemas\", \"params\": {}}\n\n");

        prompt.push_str("### get_schema\n");
        prompt.push_str(
            "Get details of a specific schema including its fields and key configuration.\n",
        );
        prompt.push_str("Parameters:\n");
        prompt.push_str("- name (string, required): Name of the schema\n");
        prompt
            .push_str("Example: {\"tool\": \"get_schema\", \"params\": {\"name\": \"Tweet\"}}\n\n");

        prompt.push_str("### search\n");
        prompt.push_str("**PREFERRED for content discovery.** Full-text search across all indexed fields (tags, subjects, names, descriptions, etc.).\n");
        prompt.push_str("Use this whenever the user asks about finding, searching, or checking if data exists.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- terms (string, required): Search keywords (e.g. \"lake\", \"birthday\", \"Leonardo da Vinci\")\n");
        prompt.push_str(
            "Returns matching records with schema_name, field, key_value, and matched content.\n",
        );
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
        prompt.push_str(
            "Example: {\"tool\": \"scan_folder\", \"params\": {\"path\": \"sample_data\"}}\n\n",
        );

        prompt.push_str("### ingest_files\n");
        prompt.push_str("Ingest files from a previously scanned folder into the database. Each file is processed by AI to extract schema and data.\n");
        prompt.push_str(
            "Only call this AFTER scan_folder and AFTER the user confirms they want to proceed.\n",
        );
        prompt.push_str("Parameters:\n");
        prompt.push_str(
            "- folder_path (string, required): The same folder path used in scan_folder\n",
        );
        prompt.push_str("- files (array of strings, required): Relative file paths from the scan results (use the 'path' field from recommended_files)\n");
        prompt.push_str("Returns: total files processed, succeeded count, failed count, per-file results with schema_used.\n");
        prompt.push_str("Example: {\"tool\": \"ingest_files\", \"params\": {\"folder_path\": \"sample_data\", \"files\": [\"contacts/address_book.json\", \"journal/2025-01-15.txt\"]}}\n\n");

        prompt.push_str("### create_view\n");
        prompt.push_str("Create a transform view with a compiled Rust WASM transform. Every view MUST have a WASM transform — identity views are not allowed through this tool.\n");
        prompt.push_str("Before calling this tool, ALWAYS use get_schema first to inspect the source schema(s) and understand their fields.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- name (string, required): Unique view name (e.g. \"EnrichedPosts\")\n");
        prompt.push_str(
            "- schema_type (string, required): \"Single\", \"Range\", \"Hash\", or \"HashRange\"\n",
        );
        prompt.push_str("- key_config (object, optional): {\"hash_field\": \"field_name\", \"range_field\": \"field_name\"} — required for Hash/Range/HashRange types\n");
        prompt.push_str("- input_queries (array, required): [{\"schema_name\": \"Name\", \"fields\": [\"field1\", \"field2\"]}]\n");
        prompt.push_str("- output_fields (object, required): {\"field_name\": \"type\"} where type is Any, String, Integer, Boolean, Date, Bytes, Reference, List, or Map\n");
        prompt.push_str("- rust_transform (string, required): A complete Rust function definition. See template below.\n\n");

        prompt.push_str("#### Rust Transform Template\n");
        prompt.push_str(
            "The transform receives query results as JSON and must return output fields as JSON.\n",
        );
        prompt.push_str("```rust\n");
        prompt.push_str("fn transform_impl(input: Value) -> Value {\n");
        prompt.push_str("    // input structure:\n");
        prompt.push_str("    // {\"inputs\": {\"SchemaName\": [{\"key\": {..}, \"fields\": {\"field\": value, ..}}, ..]}}\n");
        prompt.push_str("    //\n");
        prompt.push_str("    // Must return:\n");
        prompt.push_str("    // {\"fields\": {\"output_field\": value, ...}}\n");
        prompt.push_str("    // For multi-record output, return:\n");
        prompt.push_str(
            "    // {\"records\": [{\"key\": {..}, \"fields\": {\"output_field\": value}}, ...]}\n",
        );
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
        prompt.push_str(
            "  \"input_queries\": [{\"schema_name\": \"BlogPost\", \"fields\": [\"content\"]}],\n",
        );
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
        prompt.push_str(
            "    {\"schema_name\": \"BlogPost\", \"fields\": [\"title\", \"author_id\"]},\n",
        );
        prompt.push_str("    {\"schema_name\": \"Author\", \"fields\": [\"name\"]}\n");
        prompt.push_str("  ],\n");
        prompt.push_str(
            "  \"output_fields\": {\"title\": \"String\", \"author_name\": \"String\"},\n",
        );
        prompt.push_str("  \"rust_transform\": \"fn transform_impl(input: Value) -> Value {\\n    let inputs = &input[\\\"inputs\\\"];\\n    let posts = inputs[\\\"BlogPost\\\"].as_array().unwrap_or(&vec![]);\\n    let authors = inputs[\\\"Author\\\"].as_array().unwrap_or(&vec![]);\\n    let first_author = authors.first().map(|a| a[\\\"fields\\\"][\\\"name\\\"].as_str().unwrap_or(\\\"Unknown\\\")).unwrap_or(\\\"Unknown\\\");\\n    let records: Vec<Value> = posts.iter().map(|post| {\\n        serde_json::json!({\\n            \\\"key\\\": post[\\\"key\\\"],\\n            \\\"fields\\\": {\\n                \\\"title\\\": post[\\\"fields\\\"][\\\"title\\\"],\\n                \\\"author_name\\\": first_author\\n            }\\n        })\\n    }).collect();\\n    serde_json::json!({ \\\"records\\\": records })\\n}\"\n");
        prompt.push_str("}}\n");
        prompt.push_str("```\n\n");

        prompt.push_str("### discovery_opt_in\n");
        prompt.push_str("Opt a schema into the discovery network so its embeddings are published for pseudonymous matching.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- schema_name (string, required): Name of the schema to opt in\n");
        prompt.push_str("- category (string, required): Discovery category (e.g. \"recipes\", \"research\", \"music\")\n");
        prompt.push_str("- field_privacy (object, optional): Map of field_name to privacy class. Classes: \"NeverPublish\", \"PublishIfAnonymous\", \"AlwaysPublish\". Fields not listed get an auto-inferred default based on field name.\n");
        prompt.push_str("- include_preview (boolean, optional): Whether to include text previews in published entries (default false)\n");
        prompt.push_str("Example: {\"tool\": \"discovery_opt_in\", \"params\": {\"schema_name\": \"Recipe\", \"category\": \"recipes\", \"field_privacy\": {\"ingredients\": \"AlwaysPublish\", \"author_name\": \"NeverPublish\"}}}\n\n");

        prompt.push_str("### discovery_opt_out\n");
        prompt.push_str("Remove a schema from the discovery network. Stops publishing and requests removal of previously published embeddings.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- schema_name (string, required): Name of the schema to opt out\n");
        prompt.push_str("Example: {\"tool\": \"discovery_opt_out\", \"params\": {\"schema_name\": \"Recipe\"}}\n\n");

        prompt.push_str("### discovery_status\n");
        prompt.push_str("List all schemas currently opted into the discovery network, with their categories and field privacy settings.\n");
        prompt.push_str("Parameters: none\n");
        prompt.push_str("Example: {\"tool\": \"discovery_status\", \"params\": {}}\n\n");

        prompt.push_str("### ingest_json\n");
        prompt.push_str("Ingest structured JSON data into the database. The data flows through the full ingestion pipeline: AI analyzes the structure, proposes a schema, validates it with the schema service, and creates mutations.\n");
        prompt.push_str("Use this when you want to CREATE or STORE new structured data — e.g., itinerary plans, research notes, comparison tables, or any structured information the user asks you to save.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- data (object or array, required): The JSON data to ingest. Use an array of objects for multiple records (creates a Range/HashRange schema). Use a single object for one record.\n");
        prompt.push_str("- source_context (string, optional): A descriptive label for the data source (e.g. \"vacation_itinerary\", \"restaurant_comparison\"). Helps the AI pick a good schema name.\n");
        prompt.push_str("Returns: success status, schema_used, whether a new schema was created, mutation counts.\n");
        prompt.push_str("Tips:\n");
        prompt.push_str("- For arrays, include a field suitable as a range key (e.g., \"date\", \"day_number\", \"order\") so records are sortable.\n");
        prompt.push_str("- Keep field names descriptive and consistent across records.\n");
        prompt.push_str("- You can call this multiple times with different data structures — each will get its own schema.\n");
        prompt.push_str("IMPORTANT: Use real JSON arrays for list-valued fields (e.g., \"tags\": [\"hiking\", \"food\"]), NOT comma-separated strings.\n");
        prompt.push_str("Example (scalar fields): {\"tool\": \"ingest_json\", \"params\": {\"data\": [{\"day\": 1, \"date\": \"2026-06-01\", \"city\": \"Taipei\", \"activity\": \"Night market food tour\"}, {\"day\": 2, \"date\": \"2026-06-02\", \"city\": \"Taipei\", \"activity\": \"Dim sum breakfast\"}], \"source_context\": \"vacation_itinerary\"}}\n");
        prompt.push_str("Example (array fields): {\"tool\": \"ingest_json\", \"params\": {\"data\": [{\"traveler\": \"Alice\", \"destination\": \"Tokyo\", \"must_see\": [\"Meiji Shrine\", \"Shibuya Crossing\"], \"dietary_restrictions\": [\"vegetarian\"], \"interests\": [\"hiking\", \"street food\"]}], \"source_context\": \"travel_preferences\"}}\n\n");

        prompt.push_str("### update_record\n");
        prompt.push_str("Update an existing record in a schema. Use this when the user wants to modify, change, or correct data that already exists — e.g., update a budget, change a date, swap a hotel, fix a typo.\n");
        prompt.push_str("IMPORTANT: First use **query** or **search** to find the record and confirm its schema_name and key values, then call update_record with the fields to change.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- schema_name (string, required): Name of the schema (use the schema ID, not the descriptive name)\n");
        prompt.push_str("- key (object, required): Record identifier. Must include the key fields that identify the record:\n");
        prompt.push_str("  - hash_key (string, optional): The hash key value\n");
        prompt.push_str("  - range_key (string, optional): The range key value\n");
        prompt.push_str("- fields (object, required): Fields to update as {\"field_name\": new_value}. Only include the fields you want to change — other fields remain unchanged.\n");
        prompt.push_str("Returns: success status and mutation_id.\n");
        prompt.push_str("Example: {\"tool\": \"update_record\", \"params\": {\"schema_name\": \"VacationItinerary\", \"key\": {\"range_key\": \"2026-06-03\"}, \"fields\": {\"hotel\": \"Grand Hyatt\", \"budget\": 3000}}}\n\n");

        prompt.push_str("### web_search\n");
        prompt.push_str("Search the web for real-time information. Use this when the user's question requires external knowledge not available in the local database — e.g., restaurant recommendations, travel logistics, current events, prices, directions, reviews.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- query (string, required): Search query (e.g. \"best restaurants in Maui Hawaii\", \"flights from SFO to OGG\")\n");
        prompt.push_str(
            "- count (integer, optional): Number of results to return (default: 5, max: 5)\n",
        );
        prompt.push_str("Returns: array of results with title, url, and snippet for each.\n");
        prompt.push_str("After getting results, use **fetch_url** to read full page content for the most relevant results.\n");
        prompt.push_str("Example: {\"tool\": \"web_search\", \"params\": {\"query\": \"best snorkeling spots Maui\"}}\n\n");

        prompt.push_str("### fetch_url\n");
        prompt.push_str("Fetch a URL and extract its text content. Use this after web_search to get detailed information from a specific page.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- url (string, required): The URL to fetch (must be a full URL starting with http:// or https://)\n");
        prompt.push_str("Returns: extracted text content from the page (HTML tags stripped, truncated to ~50K chars).\n");
        prompt.push_str("Example: {\"tool\": \"fetch_url\", \"params\": {\"url\": \"https://example.com/maui-guide\"}}\n\n");

        prompt.push_str("### set_field_policy\n");
        prompt.push_str("Set the access control policy on a schema field. Controls who can read/write the field based on trust distance.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- schema_name (string, required): Name of the schema\n");
        prompt.push_str("- field_name (string, required): Name of the field\n");
        prompt.push_str("- read_max (integer, required): Maximum trust distance for reads. 0 = owner only, 1 = direct trust, 18446744073709551615 = public\n");
        prompt.push_str(
            "- write_max (integer, required): Maximum trust distance for writes. 0 = owner only.\n",
        );
        prompt.push_str("Common patterns:\n");
        prompt.push_str("- Owner only: read_max=0, write_max=0\n");
        prompt.push_str("- Public read, owner write: read_max=18446744073709551615, write_max=0\n");
        prompt.push_str("- Trusted circle read: read_max=2, write_max=0\n");
        prompt.push_str("Example: {\"tool\": \"set_field_policy\", \"params\": {\"schema_name\": \"BlogPost\", \"field_name\": \"content\", \"read_max\": 18446744073709551615, \"write_max\": 0}}\n\n");

        prompt.push_str("### get_field_policies\n");
        prompt.push_str("Get the current access control policies for all fields in a schema.\n");
        prompt.push_str("Parameters:\n");
        prompt.push_str("- schema_name (string, required): Name of the schema\n");
        prompt.push_str("Returns an object mapping field_name to policy (read_max, write_max, etc.). Fields without policies show \"none (legacy)\".\n");
        prompt.push_str("Example: {\"tool\": \"get_field_policies\", \"params\": {\"schema_name\": \"BlogPost\"}}\n\n");

        prompt.push_str("## Available Schemas\n\n");
        prompt.push_str("When referring to schemas in tool calls, always use the schema ID (not the display name).\n\n");
        for schema in schemas {
            let has_descriptive = schema.schema.descriptive_name.is_some();
            let display_name = schema
                .schema
                .descriptive_name
                .as_deref()
                .unwrap_or(&schema.schema.name);
            if has_descriptive {
                prompt.push_str(&format!(
                    "- **{}** (ID: `{}`, Type: {:?}, State: {:?})\n",
                    display_name, schema.schema.name, schema.schema.schema_type, schema.state
                ));
            } else {
                prompt.push_str(&format!(
                    "- **{}** (Type: {:?}, State: {:?})\n",
                    display_name, schema.schema.schema_type, schema.state
                ));
            }

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
        prompt.push_str("3. **For questions requiring external/real-world information** (vacation planning, restaurant recommendations, travel logistics, current events, prices), use the **web_search** tool. Follow up with **fetch_url** on the most relevant results for detailed information.\n");
        prompt.push_str("4. **For tasks that create structured data** (planning, organizing, comparing, building lists), use **web_search** to research first, then use **ingest_json** to store the results in the database. The data will be schema-validated and queryable in the dashboard.\n");
        prompt.push_str("5. **For tasks that modify existing data** (change a budget, update a date, swap a hotel, fix a value), first **query** the schema to find the record's key, then use **update_record** to change specific fields. Do NOT re-ingest the entire record — just update the fields that changed.\n");
        prompt.push_str("6. Use other tools to gather additional information as needed\n");
        prompt.push_str(
            "7. When you have enough information to answer, provide your final response\n",
        );
        prompt.push_str("8. Use the current date/time above to determine temporal context. Events with dates before today are in the PAST. Events with dates after today are in the FUTURE. Label them accordingly (e.g. \"upcoming\" vs \"past\").\n\n");
        prompt.push_str("## Reference Fields\n\n");
        prompt.push_str("Some fields are References to records in other schemas. Query results automatically resolve references one level deep.\n");
        prompt.push_str("If a field value is an array of objects with \"schema\" and \"key\" properties, those are references to child records.\n");
        prompt.push_str("The referenced data will be included inline when available. If you need deeper data (references within references), ");
        prompt.push_str("use get_schema to find the child schema's fields, then use query to fetch the child schema's data directly.\n\n");
        prompt.push_str(qp::AGENT_RESPONSE_FORMAT);

        prompt
    }

    /// Call the LLM service
    pub(super) async fn call_llm(&self, prompt: &str) -> Result<String, String> {
        self.backend
            .call(prompt)
            .await
            .map_err(|e| format!("AI backend error: {}", e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fold_db::schema::types::{DeclarativeSchemaDefinition, KeyConfig};
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
        schema
            .field_classifications
            .insert("author".to_string(), vec!["word".to_string()]);
        schema
            .field_classifications
            .insert("publish_date".to_string(), vec!["word".to_string()]);

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
