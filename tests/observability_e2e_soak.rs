//! Phase 4 / T9 — End-to-end soak test for the OTLP traces pipeline.
//!
//! Verifies that with `OBS_OTLP_ENDPOINT` set, `observability::init_node`
//! composes the full Phase 4 stack (OTLP traces, OTLP metrics +
//! `SpanMetricsLayer`, optional Sentry) and that:
//!
//! 1. Spans emitted by the application thread reach the wire as
//!    OTLP/HTTP-binary `ExportTraceServiceRequest` payloads.
//! 2. Every captured span shares a single W3C `trace_id` (one trace
//!    tree, not a forest of orphan roots).
//! 3. The set of span names that actually crossed the wire is a subset
//!    of `observability::layers::span_metrics::PRE_REGISTERED_SPANS` —
//!    proving instrumentation in the test path is using the
//!    pre-registered names rather than ad-hoc ones that would never
//!    show up on the SpanMetrics histograms.
//! 4. The W3C `traceparent` header injected by `inject_w3c` on a
//!    downstream `reqwest` call carries the same `trace_id` as the
//!    captured spans, so a real schema_service receiving the call
//!    would join the same trace tree.
//!
//! ## Why a wiremock collector instead of an in-memory exporter
//!
//! `observability::init_node` builds the OTLP exporter from
//! `OBS_OTLP_ENDPOINT` and installs it on the global tracer provider.
//! There is no public hook to swap in an `InMemorySpanExporter`, so
//! the test would have to either fork `init_node` (defeats the
//! purpose of an integration test) or reach inside private wiring.
//! Pointing the production exporter at a wiremock that captures
//! request bodies exercises the *exact* HTTP/protobuf transport a
//! real collector would receive — which is the load-bearing claim
//! Phase 4 is meant to guarantee.
//!
//! ## Why we emit `http.server.request` manually instead of through
//!    `tracing-actix-web`
//!
//! The production HTTP server uses `tracing_actix_web::TracingLogger`,
//! whose default root-span name is `"HTTP request"`. That is *not* in
//! `PRE_REGISTERED_SPANS`, so the SpanMetrics layer ignores it — the
//! span never gets a histogram observation, even though it is the
//! most important span in the binary. Phase 5 will replace the default
//! root-span builder with one that names the span `http.server.request`
//! and sets `http.method` / `http.route` fields. Until then this soak
//! test fabricates the span manually so the OTLP pipeline assertion
//! is meaningful — see the gap list below.
//!
//! ## Pre-registered span names not yet emitted by production code
//!    (Phase 5 follow-up)
//!
//! - `http.server.request` — `tracing_actix_web::TracingLogger` uses
//!   `"HTTP request"`. Needs a custom `RootSpanBuilder` that sets the
//!   name + canonical `http.method` / `http.route` fields.
//! - `db.sled.put` / `db.sled.get` / `db.sled.scan` — fold_db's sled
//!   wrapper is not yet `#[tracing::instrument]`-decorated.
//! - `wasm.transform.execute` — fold_db's transform-runtime path is
//!   not yet instrumented.
//! - `lambda.handler.invoke` — fold_db's Lambda dispatch path is not
//!   yet instrumented (separate codebase from this binary, but the
//!   same `PRE_REGISTERED_SPANS` list).
//! - `schema_service.register` — schema_service's `/v1/schemas` POST
//!   handler is not yet instrumented under this name.
//!
//! This test emits `http.server.request` and `schema_service.register`
//! by hand to prove the wire pipeline; the gap list above is the
//! Phase 5 work item.

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::time::Duration;

use observability::layers::span_metrics::PRE_REGISTERED_SPANS;
use observability::propagation::inject_w3c;
use opentelemetry::trace::TraceContextExt;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use tracing::{Instrument, Span};
use tracing_opentelemetry::OpenTelemetrySpanExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Serialize across any future siblings in this binary that mutate the
/// process-global env. Currently only one test, kept for cheap insurance.
fn env_lock() -> &'static Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// RAII env var snapshot — set or unset on construction, restore on Drop.
struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, prev }
    }

    fn unset(key: &'static str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::remove_var(key);
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

/// Pull every `Span` out of an OTLP/HTTP-binary
/// `ExportTraceServiceRequest` body. Each captured wiremock POST
/// carries one request; one request can contain many `ResourceSpans`,
/// each with many `ScopeSpans`, each with many `Span`s.
fn decode_spans(body: &[u8]) -> Vec<opentelemetry_proto::tonic::trace::v1::Span> {
    let req = ExportTraceServiceRequest::decode(body)
        .expect("captured /v1/traces body must decode as OTLP ExportTraceServiceRequest");
    req.resource_spans
        .into_iter()
        .flat_map(|rs| rs.scope_spans.into_iter().flat_map(|ss| ss.spans))
        .collect()
}

// `env_lock`'s `MutexGuard` is held across `.await` points so the
// process-global env-var snapshot stays exclusive for the duration
// of the test — sibling tests in the same binary mustn't race the
// `OBS_*` reads inside `init_node`. Tokio's runtime can re-poll on
// another thread between awaits, but the guard is `Send`-safe in
// `std::sync` and we never block on a guarded resource that any
// async task will await on, so the standard async-Mutex motivation
// for this lint does not apply here.
#[allow(clippy::await_holding_lock)]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn otlp_pipeline_emits_single_trace_tree_under_init_node() {
    let _serial = env_lock().lock().unwrap_or_else(|p| p.into_inner());

    // -- 1. Wiremock OTLP collectors -----------------------------------
    //
    // The opentelemetry-otlp HTTP exporter, when given an endpoint via
    // `with_endpoint()` (which is what `observability::layers::otlp_*`
    // does), POSTs to that URL **as-is** — no `/v1/traces` or
    // `/v1/metrics` suffix is appended. (See
    // `opentelemetry_otlp::exporter::http::resolve_http_endpoint`: only
    // the env-var path appends a signal-specific suffix.) That means a
    // single MockServer can't cleanly distinguish trace exports from
    // metric exports by URL — they'd both arrive at `/`. We split them
    // across two MockServers and feed the metrics-specific override so
    // the trace assertion can scope strictly to the trace collector.
    let traces_collector = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&traces_collector)
        .await;
    let metrics_collector = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&metrics_collector)
        .await;

    // -- 2. Wiremock downstream service --------------------------------
    //
    // Stands in for schema_service so the `inject_w3c`-wrapped reqwest
    // call has a real socket to talk to. The traceparent header on
    // received requests is the egress-side propagation evidence.
    let downstream = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "schemas": [] })),
        )
        .mount(&downstream)
        .await;

    // -- 3. Tempdir for OBS_FILE_PATH so the FMT layer doesn't pollute
    //    `~/.folddb/observability.jsonl` -------------------------------
    let tmp = tempfile::tempdir().expect("tempdir for OBS_FILE_PATH");
    let obs_log = tmp.path().join("fold_db_node.jsonl");

    // -- 4. Snapshot + set env ----------------------------------------
    //
    // `always_on` sampler so 100% of spans flow through the pipeline
    // (the default `parentbased_traceidratio:1.0` would also keep this
    // test's spans, but `always_on` removes any doubt about the head
    // sampling decision masking a wiring bug). Sentry stays unset so
    // `init_node` skips that layer — Sentry E2E is covered by
    // `tests/observability_sentry_error_e2e.rs` (PR #716).
    let traces_collector_url = traces_collector.uri();
    let metrics_collector_url = metrics_collector.uri();
    let _e1 = EnvGuard::set("OBS_OTLP_ENDPOINT", &traces_collector_url);
    let _e2 = EnvGuard::set("OBS_OTLP_METRICS_ENDPOINT", &metrics_collector_url);
    let _e3 = EnvGuard::set("OBS_SAMPLER", "always_on");
    let _e4 = EnvGuard::set("OBS_FILE_PATH", obs_log.to_str().unwrap());
    let _e5 = EnvGuard::unset("OBS_SENTRY_DSN");
    // Speed up any metrics push the test triggers. We don't depend on
    // this for the trace assertion — shutdown forces a flush anyway —
    // but a 60s default would be wasteful in CI if the runtime ever
    // does decide to flush before shutdown.
    let _e6 = EnvGuard::set("OBS_OTLP_METRICS_INTERVAL", "200");
    let _e7 = EnvGuard::set("OBS_OTLP_METRICS_TIMEOUT", "1000");

    // -- 5. Install the production observability stack -----------------
    //
    // `init_node` is process-global and one-shot. This integration
    // test is the only installer in its compilation unit (`cargo test
    // --test observability_e2e_soak` runs in a dedicated process), so
    // it owns the global subscriber for the run. Holding the guard for
    // the whole body keeps the OTLP worker thread + metrics provider
    // alive until we explicitly drop it for shutdown-flush.
    let guard = observability::init_node("fold_db_node-soak", env!("CARGO_PKG_VERSION"))
        .expect("init_node must succeed with OBS_OTLP_ENDPOINT set");

    // -- 6. Drive the trace tree ---------------------------------------
    //
    // Two pre-registered spans, nested. Inside the inner span we
    // `inject_w3c` a `reqwest` call so the egress traceparent's
    // trace_id is the same one the captured spans show on the wire.
    let downstream_url = downstream.uri();
    let observed_traceparent_id: Mutex<Option<String>> = Mutex::new(None);
    let outer = tracing::info_span!(
        "http.server.request",
        http_method = "GET",
        http_route = "/api/observability/soak",
        service_name = "fold_db_node-soak",
    );
    async {
        let inner = tracing::info_span!("schema_service.register");
        async {
            // Capture the resolved trace_id for cross-checking against
            // the spans the collector decodes later. Resolved through
            // `OpenTelemetrySpanExt::context()`, which is the same
            // path `inject_w3c` uses, so the egress traceparent must
            // carry this id byte-for-byte.
            let trace_id = format!(
                "{:032x}",
                Span::current().context().span().span_context().trace_id()
            );
            *observed_traceparent_id.lock().unwrap() = Some(trace_id);

            let url = format!("{}/v1/schemas/available", downstream_url);
            let builder = reqwest::Client::new().get(&url);
            let builder = inject_w3c(builder);
            let resp = builder.send().await.expect("downstream call");
            assert!(resp.status().is_success(), "downstream wiremock 200");
        }
        .instrument(inner)
        .await;
    }
    .instrument(outer)
    .await;

    // -- 7. Drop the guard so the OTLP worker drains + the metrics
    //    provider shuts down --------------------------------------------
    //
    // The traces guard's Drop calls `provider.shutdown()` which the
    // BoundedDropProcessor implements as
    // "send Shutdown ctrl, drain spans, exporter.export()". Shutdown
    // is bounded by SHUTDOWN_BUDGET=3s — the test cannot hang on a
    // wedged collector. In CI the wiremock above always 200s.
    drop(guard);

    // Even after shutdown returns, the wiremock side may need a beat
    // to log the request into its in-memory ledger. Give it 100ms.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // -- 8. Read captured OTLP traces payloads -------------------------
    //
    // wiremock holds the bodies of every received request. Bodies on
    // the traces collector are `ExportTraceServiceRequest`s, which we
    // decode and flatten in step 9.
    let received = traces_collector
        .received_requests()
        .await
        .expect("traces collector must record received requests");
    let traces_bodies: Vec<&[u8]> = received.iter().map(|r| r.body.as_slice()).collect();
    assert!(
        !traces_bodies.is_empty(),
        "expected at least one POST to the OTLP traces collector; got 0",
    );

    // -- 9. Decode and flatten every captured span ---------------------
    let mut all_spans = Vec::new();
    for body in &traces_bodies {
        all_spans.extend(decode_spans(body));
    }
    assert!(
        !all_spans.is_empty(),
        "decoded {} OTLP /v1/traces request(s) but found 0 spans inside",
        traces_bodies.len(),
    );

    // -- 10. Assertion (a): at least one http.server.request span ------
    let observed_names: BTreeSet<String> = all_spans.iter().map(|s| s.name.clone()).collect();
    assert!(
        observed_names.contains("http.server.request"),
        "expected at least one span named `http.server.request`; \
         observed names = {:?}",
        observed_names,
    );

    // -- 11. Assertion (b): all spans share a single trace_id ----------
    let trace_ids: BTreeSet<Vec<u8>> = all_spans.iter().map(|s| s.trace_id.clone()).collect();
    assert_eq!(
        trace_ids.len(),
        1,
        "expected a single trace tree; saw {} distinct trace_ids: {:?}",
        trace_ids.len(),
        trace_ids
            .iter()
            .map(|id| format!(
                "{:032x}",
                u128::from_be_bytes(id.as_slice().try_into().unwrap_or([0u8; 16]))
            ))
            .collect::<Vec<_>>(),
    );

    // -- 12. Assertion (c): every observed span name is pre-registered
    //    (otherwise the SpanMetrics layer silently ignores the span and
    //    Honeycomb gets no histogram for it) ---------------------------
    let registered: BTreeSet<&str> = PRE_REGISTERED_SPANS.iter().copied().collect();
    let unregistered: Vec<&str> = observed_names
        .iter()
        .map(|s| s.as_str())
        .filter(|name| !registered.contains(*name))
        .collect();
    assert!(
        unregistered.is_empty(),
        "every emitted span must use a pre-registered name (otherwise \
         SpanMetrics drops it); unregistered span names = {unregistered:?}; \
         pre-registered = {:?}",
        registered,
    );

    // -- 13. Assertion (d): the egress traceparent matches the trace_id
    //    on the captured spans ----------------------------------------
    let downstream_received = downstream
        .received_requests()
        .await
        .expect("downstream wiremock must record received requests");
    assert_eq!(
        downstream_received.len(),
        1,
        "expected exactly one downstream call from the test handler; \
         got {}",
        downstream_received.len(),
    );
    let outgoing_traceparent = downstream_received[0]
        .headers
        .get("traceparent")
        .expect("inject_w3c-wrapped reqwest must add a traceparent header")
        .to_str()
        .expect("traceparent must be ASCII");
    let expected_trace_hex = observed_traceparent_id
        .lock()
        .unwrap()
        .clone()
        .expect("test should have recorded the resolved trace_id");
    assert!(
        outgoing_traceparent.contains(&expected_trace_hex),
        "downstream traceparent {outgoing_traceparent:?} should embed the \
         in-process trace_id {expected_trace_hex}",
    );

    // The captured spans' trace_id (16 raw bytes, hex-encoded) must
    // equal the same id. Pulls the single trace_id out of the set
    // built above.
    let captured_trace_hex = trace_ids
        .iter()
        .next()
        .map(|id| id.iter().map(|b| format!("{b:02x}")).collect::<String>())
        .expect("trace_ids set is non-empty (asserted above)");
    assert_eq!(
        captured_trace_hex, expected_trace_hex,
        "the trace_id on the captured spans ({captured_trace_hex}) must equal \
         the one the egress traceparent carries ({expected_trace_hex}) — \
         a mismatch means the OTLP exporter is reading from a different \
         tracer provider than `inject_w3c` resolves through",
    );
}
