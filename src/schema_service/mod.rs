//! Schema Service HTTP Layer
//!
//! HTTP server and route handlers for the schema service.
//! Core logic (types, state, matching, expansion, fields) lives in fold_db::schema_service.

mod routes;
pub mod server;

// Re-export core modules from fold_db so existing imports still work
pub use fold_db::schema_service::state;
pub use fold_db::schema_service::types;

pub use server::SchemaServiceServer;
