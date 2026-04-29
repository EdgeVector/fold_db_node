//! Thin re-export of the upstream node-with-WEB observability bootstrap.
//!
//! Before fold_db PR #669, upstream `observability::init_node` did not
//! compose the WEB broadcast layer that `/api/logs/stream` subscribes to,
//! so this module hand-typed an `init_node_with_web` mirror. PR #669
//! hoisted the composition upstream as
//! [`observability::init_node_with_web`]; the mirror is now redundant.
//!
//! Keep this module a re-export. When you need additional layers (or
//! want to tweak the FMT / RING / WEB / Sentry composition), modify
//! upstream `crates/observability/src/init.rs` — bumping the local mirror
//! is what caused #734 (FOLDDB_HOME drift) and we don't want a second
//! copy to drift again.
//!
//! `NodeObsGuard` and `SetupError` aliases keep the existing fold_db_node
//! callsites compiling without churn.
pub use observability::{init_node_with_web, NodeObsGuardWithWeb, ObsHandles};

/// Pre-#669 alias retained so existing callsites keep compiling.
pub type NodeObsGuard = NodeObsGuardWithWeb;

/// Pre-#669 alias retained so existing callsites keep compiling.
pub type SetupError = observability::ObsError;
