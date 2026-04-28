//! Phase 4 / T10 — Sentry ERROR-layer end-to-end test.
//!
//! Verifies that an `tracing::error!` event emitted inside an
//! OpenTelemetry-instrumented span is captured by the upstream
//! `observability::layers::error` layer and surfaces in Sentry with the
//! span's W3C `trace_id` attached as a tag — without hitting the network.

#[test]
fn placeholder() {
    // Filled in next commit.
}
