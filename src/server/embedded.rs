//! Embedded server functionality for running FoldDB in desktop applications.
//!
//! This module provides an embeddable version of the FoldDB HTTP server that can
//! be integrated into desktop applications (e.g., Tauri, Electron) without blocking
//! the main thread.

use super::http_server::FoldHttpServer;
use super::node_manager::{NodeManager, NodeManagerConfig};
use super::startup::StartupCtx;
use fold_db::error::FoldDbResult;
use tokio::task::{JoinHandle, JoinSet};

/// Handle to a running embedded server.
///
/// This handle can be used to manage the lifecycle of an embedded FoldDB server.
pub struct EmbeddedServerHandle {
    /// The join handle for the server task
    task_handle: JoinHandle<FoldDbResult<()>>,
    /// The bind address
    bind_address: String,
}

impl EmbeddedServerHandle {
    /// Get the bind address of the server.
    pub fn bind_address(&self) -> &str {
        &self.bind_address
    }

    /// Check if the server is still running.
    pub fn is_running(&self) -> bool {
        !self.task_handle.is_finished()
    }

    /// Wait for the server to finish (blocks until server stops).
    pub async fn wait(self) -> FoldDbResult<()> {
        self.task_handle.await.map_err(|e| {
            fold_db::error::FoldDbError::Other(format!("Server task panicked: {}", e))
        })?
    }

    /// Abort the server task.
    pub fn abort(&self) {
        self.task_handle.abort();
    }
}

/// Start an embedded FoldDB HTTP server with lazy database initialization.
///
/// This function creates and starts a FoldDB HTTP server without initializing
/// the database. The database is initialized lazily on the first API request.
/// This is ideal for desktop applications where you want the UI to appear
/// immediately without waiting for database locks.
///
/// # Arguments
///
/// * `config` - The NodeManagerConfig to use for lazy node creation
/// * `port` - The port to bind to (e.g., 9001)
///
/// # Returns
///
/// Returns an `EmbeddedServerHandle` that can be used to manage the server.
pub async fn start_embedded_server_lazy(
    config: NodeManagerConfig,
    port: u16,
) -> FoldDbResult<EmbeddedServerHandle> {
    let bind_address = format!("127.0.0.1:{}", port);

    // Create a NodeManager without pre-populating it. The FoldNode is created
    // lazily on first API request, but the rest of `StartupCtx::boot` runs
    // eagerly so background workers see fully-initialized state.
    let node_manager = NodeManager::new(config);
    let ctx = StartupCtx::boot(node_manager, None).await?;

    // Background workers are tracked on a JoinSet that lives as long as the
    // server task. Aborting the server task (via `EmbeddedServerHandle::abort`)
    // drops the set and cancels them.
    let mut tasks = JoinSet::new();
    ctx.spawn_workers(&mut tasks);

    let server = FoldHttpServer::new(ctx, &bind_address);

    let address = bind_address.clone();
    // lint:spawn-bare-ok boot-time embedded server runner — perpetual worker, no per-request parent span.
    let task_handle = tokio::spawn(async move {
        let _tasks = tasks; // Keep workers alive for the server's lifetime.
        server.run().await
    });

    Ok(EmbeddedServerHandle {
        task_handle,
        bind_address: address,
    })
}
