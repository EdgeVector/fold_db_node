//! One-time, idempotent boot-time migrations.
//!
//! Each submodule owns the detection, mutation, and persistent-marker logic
//! for a single schema-shape repair. Migrations are wired into
//! `crate::server::startup::StartupCtx::spawn_workers` as background one-shots
//! so they don't block boot when a node has accumulated a lot of legacy data.

pub mod apple_consolidation;
