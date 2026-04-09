// Note: some command modules below are no longer called from main.rs dispatch
// (data commands now go through HTTP). They're retained for the cloud-recovery
// feature branch and as reference for the HTTP endpoint contracts.
#[cfg(target_os = "macos")]
pub mod apple;
pub mod ask;
pub mod cloud;
pub mod completions;
pub mod daemon;
pub mod ingest;
pub mod mutate;
pub mod query;
pub mod recovery;
pub mod schema;
pub mod search;
pub mod setup;
pub mod system;

use crate::cli::{Command, ConfigCommand, DaemonCommand};
use crate::error::CliError;
use crate::output::OutputMode;
use fold_db::db_operations::native_index::IndexResult;
use fold_db::fold_db_core::orchestration::index_status::IndexingStatus;
use fold_db::schema::schema_types::SchemaWithState;
use fold_db::storage::DatabaseConfig;
use fold_db_node::fold_node::OperationProcessor;
use fold_db_node::ingestion::smart_folder::SmartFolderScanResponse;
use serde_json::Value;

#[derive(Debug)]
pub enum CommandOutput {
    SchemaList(Vec<SchemaWithState>),
    SchemaGet(Box<SchemaWithState>),
    SchemaApproved {
        name: String,
    },
    SchemaBlocked {
        name: String,
    },
    SchemaLoaded {
        available: usize,
        loaded: usize,
        failed: Vec<String>,
    },
    QueryResults(Vec<Value>),
    SearchResults(Vec<IndexResult>),
    MutationSuccess {
        id: String,
    },
    MutationBatch {
        ids: Vec<String>,
    },
    IngestSuccess {
        count: usize,
        ids: Vec<String>,
    },
    SmartScan(SmartFolderScanResponse),
    SmartIngestResults {
        total: usize,
        succeeded: usize,
        failed: usize,
        results: Vec<Value>,
    },
    AskAnswer {
        answer: String,
        tool_calls: Vec<fold_db_node::fold_node::llm_query::types::ToolCallRecord>,
    },
    Status {
        pub_key: String,
        user_hash: String,
        db_config: DatabaseConfig,
        indexing_status: IndexingStatus,
    },
    Config(DatabaseConfig),
    ConfigPath(String),
    ResetComplete,
    Completions(String),
    Message(String),
    /// Raw JSON from daemon HTTP API — passed through to output
    RawJson(serde_json::Value),
    #[cfg(target_os = "macos")]
    AppleIngestSuccess {
        source: String,
        total: usize,
        ingested: usize,
        ids: Vec<String>,
    },
}

pub async fn dispatch(
    command: &Command,
    processor: &OperationProcessor,
    user_hash: &str,
    mode: OutputMode,
    config_path: Option<&str>,
    verbose: bool,
    dev: bool,
) -> Result<CommandOutput, CliError> {
    match command {
        Command::Schema { action } => schema::run(action, processor, mode).await,
        Command::Query {
            schema,
            fields,
            hash,
            range,
        } => query::run(schema, fields, hash.as_deref(), range.as_deref(), processor).await,
        Command::Search { term } => search::run(term, processor).await,
        Command::Mutate { action } => mutate::run(action, processor).await,
        Command::Ingest { action } => ingest::run(action, processor, user_hash, mode).await,
        Command::Ask {
            query,
            max_iterations,
        } => ask::run(query, user_hash, *max_iterations, processor, mode).await,
        Command::Status => system::status(processor, user_hash).await,
        Command::Config { action } => {
            let action = action.as_ref().unwrap_or(&ConfigCommand::Show);
            match action {
                ConfigCommand::Set { key, value } => {
                    system::config_set(key, value, config_path).await
                }
                _ => system::config(action, processor, config_path).await,
            }
        }
        Command::Daemon { action } => match action {
            DaemonCommand::Start { port } => {
                let msg = daemon::start(*port, dev).await?;
                Ok(CommandOutput::Message(msg))
            }
            DaemonCommand::Stop => {
                let msg = daemon::stop()?;
                Ok(CommandOutput::Message(msg))
            }
            DaemonCommand::Status => {
                let msg = daemon::status().await?;
                Ok(CommandOutput::Message(msg))
            }
        },
        Command::Cloud { action } => cloud::run(action, processor, config_path).await,
        Command::RecoveryPhrase => recovery::recovery_phrase(processor),
        Command::Restore => recovery::restore().await,
        Command::Reset { confirm } => system::reset(*confirm, processor, user_hash, mode).await,
        Command::Completions { shell } => completions::run(*shell, verbose),
    }
}
