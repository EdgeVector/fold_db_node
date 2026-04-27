//! LLM response parsers for query plans, followup analysis, and alternatives.

use super::super::types::{AgentAction, FollowupAnalysis, QueryPlan};
use fold_db::schema::types::Query;
use serde_json::Value;

use super::LlmQueryService;

/// Extract JSON from an LLM response by finding the outermost delimiters.
/// For objects use `('{', '}')`, for arrays use `('[', ']')`.
fn extract_json_delimited(response: &str, open: char, close: char) -> &str {
    match (response.find(open), response.rfind(close)) {
        (Some(start), Some(end)) if end >= start => &response[start..=end],
        _ => response,
    }
}

impl LlmQueryService {
    /// Parse the LLM response into a QueryPlan
    pub(super) fn parse_query_plan(&self, response: &str) -> Result<QueryPlan, String> {
        let json_str = extract_json_delimited(response, '{', '}');

        #[derive(serde::Deserialize)]
        struct LlmResponse {
            query: Query,
            reasoning: String,
        }

        let parsed: LlmResponse = serde_json::from_str(json_str).map_err(|e| {
            format!(
                "Failed to parse LLM response: {}. Response: {}",
                e, json_str
            )
        })?;

        Ok(QueryPlan {
            query: parsed.query,
            reasoning: parsed.reasoning,
        })
    }

    /// Parse the followup analysis response
    pub(super) fn parse_followup_analysis(
        &self,
        response: &str,
    ) -> Result<FollowupAnalysis, String> {
        let json_str = extract_json_delimited(response, '{', '}');

        #[derive(serde::Deserialize)]
        struct LlmFollowupResponse {
            needs_query: bool,
            query: Option<Query>,
            reasoning: String,
        }

        let parsed: LlmFollowupResponse = serde_json::from_str(json_str).map_err(|e| {
            format!(
                "Failed to parse followup analysis: {}. Response: {}",
                e, json_str
            )
        })?;

        Ok(FollowupAnalysis {
            needs_query: parsed.needs_query,
            query: parsed.query,
            reasoning: parsed.reasoning,
        })
    }

    /// Parse the query terms response
    pub(super) fn parse_query_terms_response(&self, response: &str) -> Result<Vec<String>, String> {
        let json_str = extract_json_delimited(response, '[', ']');

        let terms: Vec<String> = serde_json::from_str(json_str)
            .map_err(|e| format!("Failed to parse query terms: {}. Response: {}", e, json_str))?;

        if terms.is_empty() {
            return Err("No query terms generated".to_string());
        }

        Ok(terms)
    }

    /// Parse alternative query response
    pub(super) fn parse_alternative_query(
        &self,
        response: &str,
    ) -> Result<Option<QueryPlan>, String> {
        let json_str = extract_json_delimited(response, '{', '}');

        #[derive(serde::Deserialize)]
        struct LlmAlternativeResponse {
            has_alternative: bool,
            query: Option<Query>,
            reasoning: String,
        }

        let parsed: LlmAlternativeResponse = serde_json::from_str(json_str).map_err(|e| {
            format!(
                "Failed to parse alternative query: {}. Response: {}",
                e, json_str
            )
        })?;

        if parsed.has_alternative {
            if let Some(query) = parsed.query {
                Ok(Some(QueryPlan {
                    query,
                    reasoning: parsed.reasoning,
                }))
            } else {
                Err("has_alternative is true but no query provided".to_string())
            }
        } else {
            Ok(None)
        }
    }

    /// Extract the first complete JSON object from a string by tracking brace depth.
    /// Returns None if no complete object is found.
    fn extract_first_json_object(text: &str) -> Option<&str> {
        let start = text.find('{')?;
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escape_next = false;
        for (i, ch) in text[start..].char_indices() {
            if escape_next {
                escape_next = false;
                continue;
            }
            match ch {
                '\\' if in_string => escape_next = true,
                '"' => in_string = !in_string,
                '{' if !in_string => depth += 1,
                '}' if !in_string => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(&text[start..start + i + 1]);
                    }
                }
                _ => {}
            }
        }
        None
    }

    /// Parse an LLM response into an AgentAction
    pub(super) fn parse_agent_response(&self, response: &str) -> Result<AgentAction, String> {
        // Reject empty or whitespace-only LLM responses — these indicate a backend
        // issue (timeout, empty generation) and must not be silently accepted.
        let trimmed = response.trim();
        if trimmed.is_empty() {
            return Err("LLM returned an empty response".to_string());
        }

        // Extract the first complete JSON object (not first '{' to last '}')
        let json_str = match Self::extract_first_json_object(response) {
            Some(s) => s,
            None => {
                // No JSON object found - treat entire response as a plain-text answer
                return Ok(AgentAction::Answer(trimmed.to_string()));
            }
        };

        // Try parsing as-is first; if that fails, sanitize control characters
        // inside string values (LLMs sometimes put raw newlines inside JSON strings)
        let parsed: Value = match serde_json::from_str(json_str).or_else(|_| {
            let sanitized = json_str
                .replace('\n', "\\n")
                .replace('\r', "\\r")
                .replace('\t', "\\t");
            serde_json::from_str::<Value>(&sanitized)
        }) {
            Ok(v) => v,
            Err(_) => {
                // JSON parsing failed entirely - treat response as plain-text answer
                return Ok(AgentAction::Answer(response.trim().to_string()));
            }
        };

        // Check if it's a tool call
        if let Some(tool) = parsed.get("tool").and_then(|t| t.as_str()) {
            let params = parsed
                .get("params")
                .cloned()
                .unwrap_or(Value::Object(serde_json::Map::new()));
            return Ok(AgentAction::ToolCall {
                tool: tool.to_string(),
                params,
            });
        }

        // Check if it's a final answer
        if let Some(answer) = parsed.get("answer").and_then(|a| a.as_str()) {
            return Ok(AgentAction::Answer(answer.to_string()));
        }

        // Heuristic: if the JSON has `schema_name` it's likely a bare query the
        // model forgot to wrap in {"tool": "query", "params": ...}
        if parsed.get("schema_name").is_some() {
            tracing::info!("Agent: auto-wrapping bare query params as tool call");
            return Ok(AgentAction::ToolCall {
                tool: "query".to_string(),
                params: parsed,
            });
        }

        // If it has `terms` it's likely a bare search
        if parsed.get("terms").is_some() {
            tracing::info!("Agent: auto-wrapping bare search params as tool call");
            return Ok(AgentAction::ToolCall {
                tool: "search".to_string(),
                params: parsed,
            });
        }

        // If it has `path` it's likely a bare scan_folder
        if parsed.get("path").is_some() && parsed.get("tool").is_none() {
            tracing::info!("Agent: auto-wrapping bare scan params as tool call");
            return Ok(AgentAction::ToolCall {
                tool: "scan_folder".to_string(),
                params: parsed,
            });
        }

        // If it has `data` (object or array) it's likely a bare ingest_json
        if parsed.get("data").is_some() && parsed.get("tool").is_none() {
            tracing::info!("Agent: auto-wrapping bare data as ingest_json tool call");
            return Ok(AgentAction::ToolCall {
                tool: "ingest_json".to_string(),
                params: parsed,
            });
        }

        // If it has `query` (string) it's likely a bare web_search
        if parsed.get("query").and_then(|q| q.as_str()).is_some() && parsed.get("tool").is_none() {
            tracing::info!("Agent: auto-wrapping bare query string as web_search tool call");
            return Ok(AgentAction::ToolCall {
                tool: "web_search".to_string(),
                params: parsed,
            });
        }

        // If it has `url` (string) it's likely a bare fetch_url
        if parsed.get("url").and_then(|u| u.as_str()).is_some() && parsed.get("tool").is_none() {
            tracing::info!("Agent: auto-wrapping bare url as fetch_url tool call");
            return Ok(AgentAction::ToolCall {
                tool: "fetch_url".to_string(),
                params: parsed,
            });
        }

        Err(format!(
            "Agent response must contain either 'tool' or 'answer' field. Got: {}",
            json_str
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_service() -> LlmQueryService {
        let mut config = crate::ingestion::config::IngestionConfig::default();
        config.provider = crate::ingestion::config::AIProvider::Ollama;
        LlmQueryService::new(config).expect("Failed to create test service")
    }

    #[test]
    fn test_parse_agent_response_empty_string_is_error() {
        let service = make_service();
        let result = service.parse_agent_response("");
        assert!(result.is_err(), "empty string should be rejected");
        assert!(result.unwrap_err().contains("empty response"));
    }

    #[test]
    fn test_parse_agent_response_whitespace_only_is_error() {
        let service = make_service();
        let result = service.parse_agent_response("   \n\t  ");
        assert!(result.is_err(), "whitespace-only should be rejected");
        assert!(result.unwrap_err().contains("empty response"));
    }

    #[test]
    fn test_parse_agent_response_valid_answer() {
        let service = make_service();
        let result = service
            .parse_agent_response(r#"{"answer": "Here are the results"}"#)
            .unwrap();
        match result {
            AgentAction::Answer(a) => assert_eq!(a, "Here are the results"),
            _ => panic!("expected Answer"),
        }
    }

    #[test]
    fn test_parse_agent_response_valid_tool_call() {
        let service = make_service();
        let result = service
            .parse_agent_response(r#"{"tool": "web_search", "params": {"query": "test"}}"#)
            .unwrap();
        match result {
            AgentAction::ToolCall { tool, params } => {
                assert_eq!(tool, "web_search");
                assert_eq!(params["query"], "test");
            }
            _ => panic!("expected ToolCall"),
        }
    }

    #[test]
    fn test_parse_agent_response_plain_text_answer() {
        let service = make_service();
        let result = service
            .parse_agent_response("I don't have enough information to answer that.")
            .unwrap();
        match result {
            AgentAction::Answer(a) => {
                assert_eq!(a, "I don't have enough information to answer that.")
            }
            _ => panic!("expected Answer for plain text"),
        }
    }

    #[test]
    fn test_parse_agent_response_bare_web_search() {
        let service = make_service();
        let result = service
            .parse_agent_response(r#"{"query": "best restaurants maui"}"#)
            .unwrap();
        match result {
            AgentAction::ToolCall { tool, .. } => assert_eq!(tool, "web_search"),
            _ => panic!("expected auto-wrapped web_search"),
        }
    }

    #[test]
    fn test_parse_agent_response_bare_fetch_url() {
        let service = make_service();
        let result = service
            .parse_agent_response(r#"{"url": "https://example.com"}"#)
            .unwrap();
        match result {
            AgentAction::ToolCall { tool, .. } => assert_eq!(tool, "fetch_url"),
            _ => panic!("expected auto-wrapped fetch_url"),
        }
    }
}
