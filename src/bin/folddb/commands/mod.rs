pub mod completions;
pub mod daemon;
pub mod setup;
pub mod system;

#[derive(Debug)]
pub enum CommandOutput {
    Message(String),
    /// Raw JSON from daemon HTTP API — passed through to output
    RawJson(serde_json::Value),
    Completions(String),
}
