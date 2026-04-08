use serde_json::{json, Value};

use crate::client::FoldDbClient;
use crate::error::McpError;

/// Return all tool definitions for `tools/list`.
pub fn tool_definitions() -> Vec<Value> {
    vec![
        json!({
            "name": "folddb_status",
            "description": "Check if FoldDB is running and healthy. Returns status, uptime, and version.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "folddb_schema_list",
            "description": "List all schemas in FoldDB with their approval states.",
            "inputSchema": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
        json!({
            "name": "folddb_schema_get",
            "description": "Get the full definition of a specific schema by name, including fields, key config, and approval state.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Schema name to look up"
                    }
                },
                "required": ["name"]
            }
        }),
        json!({
            "name": "folddb_query",
            "description": "Run a structured query against a schema. Returns matching records with their field values.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "schema_name": {
                        "type": "string",
                        "description": "Name of the schema to query"
                    },
                    "fields": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "List of field names to return"
                    },
                    "filter": {
                        "description": "Optional filter: {\"HashKey\":\"value\"}, {\"RangePrefix\":\"prefix\"}, {\"HashRangeKey\":{\"hash\":\"h\",\"range\":\"r\"}}, or {\"SampleN\":10}"
                    },
                    "sort_order": {
                        "type": "string",
                        "enum": ["asc", "desc"],
                        "description": "Sort results by range key. Use \"desc\" for most recent first."
                    }
                },
                "required": ["schema_name", "fields"]
            }
        }),
        json!({
            "name": "folddb_search",
            "description": "Search the native word index for records matching a keyword. Returns schema, field, and key for each match.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "term": {
                        "type": "string",
                        "description": "Search keyword"
                    }
                },
                "required": ["term"]
            }
        }),
        json!({
            "name": "folddb_mutate",
            "description": "Create or update a record in FoldDB. Delete is not permitted via this tool.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "schema": {
                        "type": "string",
                        "description": "Schema name"
                    },
                    "fields_and_values": {
                        "type": "object",
                        "description": "Field name to value mapping"
                    },
                    "mutation_type": {
                        "type": "string",
                        "enum": ["Create", "Update"],
                        "description": "Type of mutation (Create or Update)"
                    },
                    "key_value": {
                        "type": "object",
                        "description": "Key fields for the record, e.g. {\"hash\":\"my_key\"} or {\"hash\":\"h\",\"range\":\"r\"}"
                    }
                },
                "required": ["schema", "fields_and_values", "mutation_type"]
            }
        }),
        json!({
            "name": "folddb_ingest",
            "description": "Ingest JSON data into FoldDB with AI-powered schema detection. The AI recommends or creates a schema and generates mutations automatically.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "data": {
                        "type": "object",
                        "description": "JSON object to ingest"
                    },
                    "source_file_name": {
                        "type": "string",
                        "description": "Optional source filename for tracking"
                    }
                },
                "required": ["data"]
            }
        }),
        json!({
            "name": "folddb_ask",
            "description": "Ask a natural language question about data in FoldDB. An AI agent autonomously searches schemas and queries data to find the answer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Natural language question about your data"
                    },
                    "session_id": {
                        "type": "string",
                        "description": "Optional session ID for conversation continuity"
                    }
                },
                "required": ["query"]
            }
        }),
    ]
}

/// Dispatch a tool call to the appropriate HTTP endpoint.
pub async fn dispatch(
    tool_name: &str,
    args: Value,
    client: &FoldDbClient,
) -> Result<String, McpError> {
    match tool_name {
        "folddb_status" => {
            let resp = client.get("/api/system/status").await?;
            Ok(serde_json::to_string_pretty(&resp)?)
        }

        "folddb_schema_list" => {
            let resp = client.get("/api/schemas").await?;
            Ok(serde_json::to_string_pretty(&resp)?)
        }

        "folddb_schema_get" => {
            let name = args
                .get("name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::ToolError("Missing required parameter: name".into()))?;
            let resp = client
                .get(&format!("/api/schema/{}", urlencoding::encode(name)))
                .await?;
            Ok(serde_json::to_string_pretty(&resp)?)
        }

        "folddb_query" => {
            let schema_name = args
                .get("schema_name")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    McpError::ToolError("Missing required parameter: schema_name".into())
                })?;
            let fields = args
                .get("fields")
                .ok_or_else(|| McpError::ToolError("Missing required parameter: fields".into()))?;

            let mut body = json!({
                "schema_name": schema_name,
                "fields": fields,
            });
            if let Some(filter) = args.get("filter") {
                body["filter"] = filter.clone();
            }
            if let Some(sort_order) = args.get("sort_order") {
                body["sort_order"] = sort_order.clone();
            }

            let resp = client.post_unsigned("/api/query", &body).await?;
            // Extract results array from {"ok": true, "results": [...]}
            let empty = json!([]);
            let results = resp.get("results").unwrap_or(&empty);
            Ok(serde_json::to_string_pretty(results)?)
        }

        "folddb_search" => {
            let term = args
                .get("term")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::ToolError("Missing required parameter: term".into()))?;
            let resp = client
                .get(&format!(
                    "/api/native-index/search?term={}",
                    urlencoding::encode(term)
                ))
                .await?;
            Ok(serde_json::to_string_pretty(&resp)?)
        }

        "folddb_mutate" => {
            let schema = args
                .get("schema")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::ToolError("Missing required parameter: schema".into()))?;
            let fields_and_values = args.get("fields_and_values").ok_or_else(|| {
                McpError::ToolError("Missing required parameter: fields_and_values".into())
            })?;
            let mutation_type = args
                .get("mutation_type")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    McpError::ToolError("Missing required parameter: mutation_type".into())
                })?;

            // Reject Delete — agents cannot delete records
            if mutation_type == "Delete" {
                return Err(McpError::ToolError(
                    "Delete mutations are not permitted via the MCP interface".into(),
                ));
            }

            let mut body = json!({
                "type": "mutation",
                "schema": schema,
                "fields_and_values": fields_and_values,
                "mutation_type": mutation_type,
            });
            if let Some(key_value) = args.get("key_value") {
                body["key_value"] = key_value.clone();
            }

            let resp = client.post_signed("/api/mutation", &body).await?;
            Ok(serde_json::to_string_pretty(&resp)?)
        }

        "folddb_ingest" => {
            let data = args
                .get("data")
                .ok_or_else(|| McpError::ToolError("Missing required parameter: data".into()))?;

            let mut body = json!({
                "data": data,
                "auto_execute": true,
            });
            if let Some(source_file_name) = args.get("source_file_name") {
                body["source_file_name"] = source_file_name.clone();
            }

            let resp = client.post_signed("/api/ingestion/process", &body).await?;
            Ok(serde_json::to_string_pretty(&resp)?)
        }

        "folddb_ask" => {
            let query = args
                .get("query")
                .and_then(|v| v.as_str())
                .ok_or_else(|| McpError::ToolError("Missing required parameter: query".into()))?;

            let mut body = json!({"query": query});
            if let Some(session_id) = args.get("session_id") {
                body["session_id"] = session_id.clone();
            }

            let resp = client.post_signed("/api/llm-query/agent", &body).await?;
            Ok(serde_json::to_string_pretty(&resp)?)
        }

        _ => Err(McpError::ToolError(format!("Unknown tool: {}", tool_name))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_definitions_count() {
        assert_eq!(tool_definitions().len(), 8);
    }

    #[test]
    fn test_all_tools_have_required_fields() {
        for tool in tool_definitions() {
            assert!(tool.get("name").is_some(), "Tool missing name");
            assert!(
                tool.get("description").is_some(),
                "Tool missing description"
            );
            assert!(
                tool.get("inputSchema").is_some(),
                "Tool missing inputSchema"
            );
        }
    }

    #[test]
    fn test_mutate_tool_excludes_delete() {
        let mutate_tool = tool_definitions()
            .into_iter()
            .find(|t| t["name"] == "folddb_mutate")
            .unwrap();
        let enum_values = mutate_tool["inputSchema"]["properties"]["mutation_type"]["enum"]
            .as_array()
            .unwrap();
        let types: Vec<&str> = enum_values.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(types.contains(&"Create"));
        assert!(types.contains(&"Update"));
        assert!(!types.contains(&"Delete"));
    }
}
