pub mod admin;
pub mod apple_import;
pub mod auth;
pub mod common;
pub mod config;
pub mod discovery;
pub mod feed;
pub mod file_upload;
pub mod filesystem;
pub mod ingestion;
pub mod log;
pub mod org;
pub mod query;
pub mod remote;
pub mod schema;
pub mod security;
pub mod smart_folder;
pub mod sync;
pub mod system;
pub mod trust;
pub mod views;

// Re-export common utilities for convenience
pub(crate) use common::node_or_return;
pub use common::{
    handler_error_to_response, handler_result_to_response, require_node, require_node_read,
    require_user_context,
};
