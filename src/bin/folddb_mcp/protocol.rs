use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::FoldDbClient;
use crate::error;
use crate::error::McpError;
use crate::tools;

#[derive(Deserialize, Debug)]
pub struct JsonRpcRequest {
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub id: Option<Value>,
    pub method: String,
    pub params: Option<Value>,
}

#[derive(Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    pub fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id: Some(id),
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

/// Build the response for `initialize`
pub fn handle_initialize(id: Value) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {},
                "resources": {}
            },
            "serverInfo": {
                "name": "folddb",
                "version": env!("CARGO_PKG_VERSION")
            }
        }),
    )
}

/// Build the response for `resources/list` — enumerate all schemas as resources.
pub async fn handle_resources_list(id: Value, client: &FoldDbClient) -> JsonRpcResponse {
    match fetch_schema_resources(client).await {
        Ok(resources) => {
            JsonRpcResponse::success(id, serde_json::json!({ "resources": resources }))
        }
        Err(e) => JsonRpcResponse::error(
            Some(id),
            error::INTERNAL_ERROR,
            format!("Failed to list resources: {}", e),
        ),
    }
}

/// Build the response for `resources/read` — fetch a single schema definition.
pub async fn handle_resources_read(
    id: Value,
    params: &Value,
    client: &FoldDbClient,
) -> JsonRpcResponse {
    let uri = match params.get("uri").and_then(|v| v.as_str()) {
        Some(u) => u,
        None => {
            return JsonRpcResponse::error(
                Some(id),
                error::INVALID_PARAMS,
                "Missing required parameter: uri",
            );
        }
    };

    let schema_name = match uri.strip_prefix("folddb://schema/") {
        Some(name) => name,
        None => {
            return JsonRpcResponse::error(
                Some(id),
                error::INVALID_PARAMS,
                format!("Invalid resource URI: {}", uri),
            );
        }
    };

    match client
        .get(&format!("/api/schema/{}", urlencoding::encode(schema_name)))
        .await
    {
        Ok(resp) => {
            let payload = if resp.get("ok").is_some() {
                resp.get("data").unwrap_or(&resp)
            } else {
                &resp
            };
            let text = serde_json::to_string_pretty(payload).unwrap_or_else(|_| resp.to_string());
            JsonRpcResponse::success(
                id,
                serde_json::json!({
                    "contents": [{
                        "uri": uri,
                        "mimeType": "application/json",
                        "text": text
                    }]
                }),
            )
        }
        Err(e) => JsonRpcResponse::error(
            Some(id),
            error::INTERNAL_ERROR,
            format!("Failed to read resource: {}", e),
        ),
    }
}

/// Fetch schemas from the FoldDB API and convert them to MCP resource descriptors.
async fn fetch_schema_resources(client: &FoldDbClient) -> Result<Vec<Value>, McpError> {
    let resp = client.get("/api/schemas").await?;
    let empty = Value::Array(vec![]);
    let schemas = if resp.get("ok").is_some() {
        resp.get("data")
            .and_then(|d| d.get("schemas"))
            .unwrap_or(&empty)
    } else {
        resp.get("schemas").unwrap_or(&empty)
    };

    let mut resources = Vec::new();
    if let Some(arr) = schemas.as_array() {
        for schema in arr {
            let name = schema
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            resources.push(serde_json::json!({
                "uri": format!("folddb://schema/{}", name),
                "name": name,
                "description": format!("Schema definition for '{}'", name),
                "mimeType": "application/json"
            }));
        }
    }
    Ok(resources)
}

/// Build the response for `tools/list`
pub fn handle_tools_list(id: Value) -> JsonRpcResponse {
    JsonRpcResponse::success(
        id,
        serde_json::json!({
            "tools": tools::tool_definitions()
        }),
    )
}

/// Build the response for `ping`
pub fn handle_ping(id: Value) -> JsonRpcResponse {
    JsonRpcResponse::success(id, serde_json::json!({}))
}

/// Route an incoming JSON-RPC request. Returns None for notifications (no id).
pub async fn route(
    request: JsonRpcRequest,
    client: &crate::client::FoldDbClient,
) -> Option<JsonRpcResponse> {
    match request.method.as_str() {
        "initialize" => {
            let id = request.id?;
            Some(handle_initialize(id))
        }
        "notifications/initialized" => None,
        "tools/list" => {
            let id = request.id?;
            Some(handle_tools_list(id))
        }
        "resources/list" => {
            let id = request.id?;
            Some(handle_resources_list(id, client).await)
        }
        "resources/read" => {
            let id = request.id?;
            let params = request.params.unwrap_or(Value::Null);
            Some(handle_resources_read(id, &params, client).await)
        }
        "tools/call" => {
            let id = request.id?;
            let params = request.params.unwrap_or(Value::Null);
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Object(serde_json::Map::new()));

            match tools::dispatch(tool_name, arguments, client).await {
                Ok(text) => Some(JsonRpcResponse::success(
                    id,
                    serde_json::json!({
                        "content": [{"type": "text", "text": text}]
                    }),
                )),
                Err(e) => Some(JsonRpcResponse::success(
                    id,
                    serde_json::json!({
                        "content": [{"type": "text", "text": format!("{{\"error\": \"{}\"}}", e)}],
                        "isError": true
                    }),
                )),
            }
        }
        "ping" => {
            let id = request.id?;
            Some(handle_ping(id))
        }
        _ => {
            let id = request.id;
            Some(JsonRpcResponse::error(
                id,
                error::METHOD_NOT_FOUND,
                format!("Method not found: {}", request.method),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_initialize_response() {
        let resp = handle_initialize(serde_json::json!(1));
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert!(result["capabilities"]["tools"].is_object());
        assert!(result["capabilities"]["resources"].is_object());
        assert_eq!(result["serverInfo"]["name"], "folddb");
    }

    #[test]
    fn test_tools_list_returns_all_tools() {
        let resp = handle_tools_list(serde_json::json!(1));
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 8);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"folddb_status"));
        assert!(names.contains(&"folddb_query"));
        assert!(names.contains(&"folddb_mutate"));
        assert!(names.contains(&"folddb_ingest"));
        assert!(names.contains(&"folddb_ask"));
    }

    #[test]
    fn test_error_response() {
        let resp = JsonRpcResponse::error(Some(serde_json::json!(1)), -32601, "Not found");
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[test]
    fn test_parse_request() {
        let json = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
        let req: JsonRpcRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.method, "initialize");
        assert_eq!(req.id, Some(serde_json::json!(1)));
    }
}
