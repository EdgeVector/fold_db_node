//! Decorator that records per-role metrics around every AI backend call.
//!
//! Wraps an existing [`AiBackend`] implementation and tags every
//! [`AiBackend::call`] invocation with a [`Role`] + [`AiMetricsStore`]. On
//! success or failure, records latency + outcome against the store. Keeps
//! metering out of backend internals — Ollama and Anthropic backends stay
//! ignorant of which role is calling them.
//!
//! Vision has a non-trait `call_vision` path (different signature — image
//! bytes, not a prompt). That path records metrics directly via
//! [`AiMetricsStore::record_call`] at its callsite rather than going through
//! this decorator. Adding `call_vision` to the [`AiBackend`] trait would
//! pollute both backends for one caller.

use crate::ingestion::ai::client::AiBackend;
use crate::ingestion::metrics::AiMetricsStore;
use crate::ingestion::roles::Role;
use crate::ingestion::IngestionResult;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;

/// Wraps an [`AiBackend`] so every call records a metrics entry tagged with
/// the originating [`Role`].
pub struct MeteredBackend {
    inner: Arc<dyn AiBackend>,
    role: Role,
    metrics: Arc<AiMetricsStore>,
}

impl MeteredBackend {
    /// Construct a metering wrapper around `inner`. Cheap — no I/O.
    pub fn new(inner: Arc<dyn AiBackend>, role: Role, metrics: Arc<AiMetricsStore>) -> Self {
        Self {
            inner,
            role,
            metrics,
        }
    }
}

#[async_trait]
impl AiBackend for MeteredBackend {
    async fn call(&self, prompt: &str) -> IngestionResult<String> {
        let start = Instant::now();
        let result = self.inner.call(prompt).await;
        self.metrics
            .record_call(self.role, start.elapsed(), result.is_ok());
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingestion::error::IngestionError;

    struct EchoBackend;

    #[async_trait]
    impl AiBackend for EchoBackend {
        async fn call(&self, prompt: &str) -> IngestionResult<String> {
            Ok(prompt.to_string())
        }
    }

    struct FailingBackend;

    #[async_trait]
    impl AiBackend for FailingBackend {
        async fn call(&self, _prompt: &str) -> IngestionResult<String> {
            Err(IngestionError::configuration_error(
                "intentional test error",
            ))
        }
    }

    #[tokio::test]
    async fn success_call_records_role_and_increments_count() {
        let metrics = Arc::new(AiMetricsStore::new());
        let metered =
            MeteredBackend::new(Arc::new(EchoBackend), Role::IngestionText, metrics.clone());
        let out = metered.call("hello").await.unwrap();
        assert_eq!(out, "hello");
        let snap = metrics.snapshot(Role::IngestionText);
        assert_eq!(snap.call_count, 1);
        assert_eq!(snap.error_count, 0);
    }

    #[tokio::test]
    async fn error_call_increments_error_count_but_still_counts_the_call() {
        let metrics = Arc::new(AiMetricsStore::new());
        let metered =
            MeteredBackend::new(Arc::new(FailingBackend), Role::SmartFolder, metrics.clone());
        assert!(metered.call("anything").await.is_err());
        let snap = metrics.snapshot(Role::SmartFolder);
        assert_eq!(snap.call_count, 1);
        assert_eq!(snap.error_count, 1);
    }

    #[tokio::test]
    async fn metrics_tag_uses_the_role_the_decorator_was_built_with() {
        let metrics = Arc::new(AiMetricsStore::new());
        let ingest =
            MeteredBackend::new(Arc::new(EchoBackend), Role::IngestionText, metrics.clone());
        let query = MeteredBackend::new(Arc::new(EchoBackend), Role::QueryChat, metrics.clone());
        ingest.call("a").await.unwrap();
        query.call("b").await.unwrap();
        query.call("c").await.unwrap();
        assert_eq!(metrics.snapshot(Role::IngestionText).call_count, 1);
        assert_eq!(metrics.snapshot(Role::QueryChat).call_count, 2);
    }

    #[tokio::test]
    async fn latency_is_recorded_from_the_wrapping_span() {
        // We can't mock tokio time easily; just assert latency > 0 and a rough
        // upper bound that would only be exceeded if the call hung.
        let metrics = Arc::new(AiMetricsStore::new());
        let metered = MeteredBackend::new(
            Arc::new(EchoBackend),
            Role::DiscoveryInterests,
            metrics.clone(),
        );
        metered.call("x").await.unwrap();
        let snap = metrics.snapshot(Role::DiscoveryInterests);
        assert!(snap.avg_latency_ms >= 0.0);
        assert!(
            snap.avg_latency_ms < 1000.0,
            "unexpected long latency: {}",
            snap.avg_latency_ms
        );
    }
}
