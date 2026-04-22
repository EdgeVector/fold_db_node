pub mod completions;
pub mod daemon;
pub mod org;
pub mod setup;
pub mod snapshot;
pub mod system;
pub mod trigger;

#[derive(Debug)]
pub enum CommandOutput {
    Message(String),
    /// Raw JSON from daemon HTTP API — passed through to output
    RawJson(serde_json::Value),
    Completions(String),
}
