//! Schema Service
//!
//! A standalone HTTP service that provides schema discovery and management.
//! The schema service loads schemas from a sled database on startup and
//! provides them via HTTP API to the main FoldDB node.

mod classify;
mod routes;
pub mod server;
pub mod state;
mod state_expansion;
mod state_fields;
mod state_matching;
pub mod types;

pub use server::SchemaServiceServer;
