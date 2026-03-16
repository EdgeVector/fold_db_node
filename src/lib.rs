pub mod discovery;
pub mod fold_node;
pub mod handlers;
pub mod ingestion;
pub mod server;
pub mod schema_service;
pub mod utils;

// Re-export core library for convenience
pub use fold_db;

// Re-export key app-layer types
pub use fold_node::config::{load_node_config, NodeConfig};
pub use fold_node::FoldNode;
pub use ingestion::IngestionConfig;
