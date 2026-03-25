//! Schema Service HTTP Layer
//!
//! A standalone HTTP service that provides schema discovery and management.
//! The schema service loads schemas from a sled database on startup and
//! provides them via HTTP API to the main FoldDB node.
//!
//! Core logic (state, types, classify) lives in `fold_db::schema_service`.
//! This module provides the HTTP layer (server + routes).

mod routes;
pub mod server;

pub use fold_db::schema_service::state;
pub use fold_db::schema_service::types;

pub use fold_db::TransformResolver;
pub use server::SchemaServiceServer;
