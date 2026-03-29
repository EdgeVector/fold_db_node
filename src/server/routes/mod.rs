pub mod admin;
pub mod auth;
pub mod common;
pub mod config;
pub mod discovery;
pub mod filesystem;
pub mod log;
pub mod query;
pub mod remote;
pub mod schema;
pub mod security;
pub mod system;
pub mod trust;
pub mod views;

// Re-export common utilities for convenience
pub(crate) use common::node_or_return;
pub use common::{
    get_node_for_user, handler_error_to_response, handler_result_to_response, require_node,
    require_node_read, require_user_context,
};
