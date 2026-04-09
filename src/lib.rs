pub mod discovery;
pub mod endpoints;
pub mod fold_node;
pub mod handlers;
pub mod ingestion;
pub mod keychain;
/// Temporary stub types for `fold_db::storage::node_config_store`.
/// Delete this module once the fold_db NodeConfigStore PR merges and switch
/// all imports to `fold_db::storage::node_config_store`.
pub mod node_config_store;
pub mod schema_service;
#[cfg(feature = "os-keychain")]
pub mod secure_store;
pub mod sensitive_io;
pub mod server;
pub mod trust;
pub mod utils;

// Re-export core library for convenience
pub use fold_db;

// Re-export key app-layer types
pub use fold_node::config::{load_node_config, NodeConfig};
pub use fold_node::FoldNode;
pub use ingestion::IngestionConfig;
