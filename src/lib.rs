pub mod discovery;
pub mod endpoints;
pub mod fingerprints;
pub mod fold_node;
pub mod handlers;
pub mod ingestion;
pub mod keychain;
pub mod memory;
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
