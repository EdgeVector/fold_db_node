use std::fmt;

#[derive(Debug)]
pub enum McpError {
    Http(reqwest::Error),
    Json(serde_json::Error),
    Signing(String),
    ServerNotRunning(String),
    Io(std::io::Error),
    ToolError(String),
}

impl fmt::Display for McpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Http(e) => write!(f, "HTTP error: {}", e),
            Self::Json(e) => write!(f, "JSON error: {}", e),
            Self::Signing(msg) => write!(f, "Signing error: {}", msg),
            Self::ServerNotRunning(msg) => write!(f, "Server not running: {}", msg),
            Self::Io(e) => write!(f, "IO error: {}", e),
            Self::ToolError(msg) => write!(f, "Tool error: {}", msg),
        }
    }
}

impl std::error::Error for McpError {}

impl From<reqwest::Error> for McpError {
    fn from(e: reqwest::Error) -> Self {
        Self::Http(e)
    }
}

impl From<serde_json::Error> for McpError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}

impl From<std::io::Error> for McpError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// JSON-RPC error codes
pub const PARSE_ERROR: i32 = -32700;
pub const METHOD_NOT_FOUND: i32 = -32601;
