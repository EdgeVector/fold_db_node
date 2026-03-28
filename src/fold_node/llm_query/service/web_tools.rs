//! Web search and URL fetch tools for the LLM agent.
//!
//! Provides `web_search` (Brave Search API) and `fetch_url` (HTML text extraction)
//! so the agent can research external information.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Environment variable for the web search API key (Brave Search).
pub const WEB_SEARCH_API_KEY_ENV: &str = "WEB_SEARCH_API_KEY";

/// Brave Search API endpoint.
const BRAVE_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";

/// Maximum number of results to return from web search.
const MAX_RESULTS: usize = 5;

/// Maximum characters to extract from a fetched URL.
const MAX_FETCH_CHARS: usize = 50_000;

/// A single web search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Perform a web search using the Brave Search API.
pub async fn web_search(query: &str, count: usize) -> Result<Vec<WebSearchResult>, String> {
    let api_key = std::env::var(WEB_SEARCH_API_KEY_ENV)
        .map_err(|_| format!("Web search unavailable: {} environment variable not set. Get a free API key at https://brave.com/search/api/", WEB_SEARCH_API_KEY_ENV))?;

    let count = count.min(MAX_RESULTS);

    let client = reqwest::Client::new();
    let response = client
        .get(BRAVE_SEARCH_URL)
        .header("X-Subscription-Token", &api_key)
        .header("Accept", "application/json")
        .query(&[("q", query), ("count", &count.to_string())])
        .send()
        .await
        .map_err(|e| format!("Web search request failed: {}", e))?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "Web search API returned HTTP {}: {}",
            status, body
        ));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse web search response: {}", e))?;

    parse_brave_response(&body)
}

/// Parse the Brave Search API response into our result type.
fn parse_brave_response(body: &Value) -> Result<Vec<WebSearchResult>, String> {
    let results = body
        .get("web")
        .and_then(|w| w.get("results"))
        .and_then(|r| r.as_array())
        .unwrap_or(&Vec::new())
        .iter()
        .map(|item| WebSearchResult {
            title: item
                .get("title")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string(),
            url: item
                .get("url")
                .and_then(|u| u.as_str())
                .unwrap_or("")
                .to_string(),
            snippet: item
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string(),
        })
        .collect();

    Ok(results)
}

/// Fetch a URL and extract its text content (strips HTML tags).
pub async fn fetch_url(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .get(url)
        .header("User-Agent", "FoldDB-Agent/1.0")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch URL '{}': {}", url, e))?;

    let status = response.status();
    if !status.is_success() {
        return Err(format!("URL returned HTTP {}: {}", status, url));
    }

    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    // Only process text-based content
    if !content_type.contains("text/") && !content_type.contains("application/json") {
        return Err(format!(
            "URL returned non-text content type: {}",
            content_type
        ));
    }

    let body = response
        .text()
        .await
        .map_err(|e| format!("Failed to read response body: {}", e))?;

    let text = if content_type.contains("text/html") {
        strip_html(&body)
    } else {
        body
    };

    // Truncate to prevent context overflow
    if text.len() > MAX_FETCH_CHARS {
        Ok(format!(
            "{}\n\n[TRUNCATED: content was {} chars, showing first {}]",
            &text[..MAX_FETCH_CHARS],
            text.len(),
            MAX_FETCH_CHARS
        ))
    } else {
        Ok(text)
    }
}

/// Strip HTML tags and decode common entities, keeping just the text content.
fn strip_html(html: &str) -> String {
    let mut result = String::with_capacity(html.len() / 2);
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut last_was_whitespace = false;

    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        if !in_tag && i + 7 < len && &lower[i..i + 7] == "<script" {
            in_script = true;
            in_tag = true;
        } else if in_script && i + 9 <= len && &lower[i..i + 9] == "</script>" {
            in_script = false;
            i += 9;
            continue;
        } else if !in_tag && i + 6 < len && &lower[i..i + 6] == "<style" {
            in_style = true;
            in_tag = true;
        } else if in_style && i + 8 <= len && &lower[i..i + 8] == "</style>" {
            in_style = false;
            i += 8;
            continue;
        }

        if in_script || in_style {
            i += 1;
            continue;
        }

        let ch = chars[i];
        if ch == '<' {
            in_tag = true;
            // Block-level tags get a newline
            if i + 3 < len {
                let next3: String = lower_chars[i + 1..len.min(i + 4)]
                    .iter()
                    .collect::<String>();
                if next3.starts_with("br")
                    || next3.starts_with("p ")
                    || next3.starts_with("p>")
                    || next3.starts_with("di")
                    || next3.starts_with("li")
                    || next3.starts_with("h1")
                    || next3.starts_with("h2")
                    || next3.starts_with("h3")
                    || next3.starts_with("tr")
                {
                    if !result.ends_with('\n') {
                        result.push('\n');
                    }
                    last_was_whitespace = true;
                }
            }
        } else if ch == '>' {
            in_tag = false;
        } else if !in_tag {
            // Decode HTML entities
            if ch == '&' {
                let rest: String = chars[i..len.min(i + 10)].iter().collect();
                if rest.starts_with("&amp;") {
                    result.push('&');
                    i += 5;
                    last_was_whitespace = false;
                    continue;
                } else if rest.starts_with("&lt;") {
                    result.push('<');
                    i += 4;
                    last_was_whitespace = false;
                    continue;
                } else if rest.starts_with("&gt;") {
                    result.push('>');
                    i += 4;
                    last_was_whitespace = false;
                    continue;
                } else if rest.starts_with("&quot;") {
                    result.push('"');
                    i += 6;
                    last_was_whitespace = false;
                    continue;
                } else if rest.starts_with("&apos;") || rest.starts_with("&#39;") {
                    result.push('\'');
                    i += if rest.starts_with("&apos;") { 6 } else { 5 };
                    last_was_whitespace = false;
                    continue;
                } else if rest.starts_with("&nbsp;") {
                    result.push(' ');
                    i += 6;
                    last_was_whitespace = true;
                    continue;
                }
            }

            if ch.is_whitespace() {
                if !last_was_whitespace {
                    result.push(' ');
                    last_was_whitespace = true;
                }
            } else {
                result.push(ch);
                last_was_whitespace = false;
            }
        }
        i += 1;
    }

    // Collapse multiple blank lines
    let mut cleaned = String::with_capacity(result.len());
    let mut consecutive_newlines = 0;
    for ch in result.chars() {
        if ch == '\n' {
            consecutive_newlines += 1;
            if consecutive_newlines <= 2 {
                cleaned.push(ch);
            }
        } else {
            consecutive_newlines = 0;
            cleaned.push(ch);
        }
    }

    cleaned.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_brave_response() {
        let response = serde_json::json!({
            "web": {
                "results": [
                    {
                        "title": "Best Restaurants in Maui",
                        "url": "https://example.com/maui-restaurants",
                        "description": "Top 10 restaurants to visit in Maui, Hawaii"
                    },
                    {
                        "title": "Maui Travel Guide",
                        "url": "https://example.com/maui-guide",
                        "description": "Complete guide to visiting Maui"
                    }
                ]
            }
        });

        let results = parse_brave_response(&response).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Best Restaurants in Maui");
        assert_eq!(results[0].url, "https://example.com/maui-restaurants");
        assert_eq!(
            results[0].snippet,
            "Top 10 restaurants to visit in Maui, Hawaii"
        );
        assert_eq!(results[1].title, "Maui Travel Guide");
    }

    #[test]
    fn test_parse_brave_response_empty() {
        let response = serde_json::json!({});
        let results = parse_brave_response(&response).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_parse_brave_response_missing_fields() {
        let response = serde_json::json!({
            "web": {
                "results": [
                    {
                        "title": "Only Title",
                    }
                ]
            }
        });

        let results = parse_brave_response(&response).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Only Title");
        assert_eq!(results[0].url, "");
        assert_eq!(results[0].snippet, "");
    }

    #[test]
    fn test_strip_html_basic() {
        let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let text = strip_html(html);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
        assert!(!text.contains("<h1>"));
        assert!(!text.contains("<p>"));
    }

    #[test]
    fn test_strip_html_script_and_style() {
        let html = r#"<html>
            <head><style>body { color: red; }</style></head>
            <body>
                <script>alert('hi');</script>
                <p>Visible content</p>
            </body>
        </html>"#;
        let text = strip_html(html);
        assert!(text.contains("Visible content"));
        assert!(!text.contains("alert"));
        assert!(!text.contains("color: red"));
    }

    #[test]
    fn test_strip_html_entities() {
        let html = "<p>Tom &amp; Jerry &lt;3 &gt; &quot;friends&quot;</p>";
        let text = strip_html(html);
        assert!(text.contains("Tom & Jerry <3 > \"friends\""));
    }

    #[test]
    fn test_strip_html_whitespace_collapse() {
        let html = "<p>Hello     World</p>";
        let text = strip_html(html);
        assert_eq!(text, "Hello World");
    }

    #[test]
    fn test_web_search_result_serialization() {
        let result = WebSearchResult {
            title: "Test".to_string(),
            url: "https://example.com".to_string(),
            snippet: "A test result".to_string(),
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["title"], "Test");
        assert_eq!(json["url"], "https://example.com");
        assert_eq!(json["snippet"], "A test result");
    }

    #[tokio::test]
    async fn test_web_search_missing_api_key() {
        // Ensure the env var is not set
        std::env::remove_var(WEB_SEARCH_API_KEY_ENV);
        let result = web_search("test query", 3).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not set"));
    }

    #[tokio::test]
    async fn test_fetch_url_invalid_url() {
        let result = fetch_url("not-a-valid-url").await;
        assert!(result.is_err());
    }
}
