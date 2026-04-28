//! Phase 4 / T10 — Sentry ERROR-layer end-to-end test.
//!
//! Verifies that fold_db_node's integration with the upstream
//! `observability::layers::error` Sentry sink works end-to-end:
//!
//!   1. Setting `OBS_SENTRY_DSN` flips the gate so
//!      [`observability::layers::error::build_error_layer`] returns
//!      `Some` rather than no-op.
//!   2. The returned layer composes into a `Registry` alongside the
//!      OTel layer.
//!   3. A `tracing::error!` event emitted inside an OTel-instrumented
//!      span is captured by Sentry (in-process via
//!      `sentry::test::with_captured_events` — no network).
//!   4. The captured event carries the originating span's W3C
//!      `trace_id` as a Sentry tag, so an alert page can deep-link
//!      back into the trace in Honeycomb.
//!
//! ## Why we observe the actual trace_id rather than asserting a
//!    literal
//!
//! The Sentry layer's `event_mapper` reads `OtelData.builder.trace_id`,
//! which the OTel layer assigns at `on_new_span` time. To make that id
//! deterministic the test would need a custom `IdGenerator`, which
//! complicates an E2E test for no extra signal — the W3C-example id
//! `0af7651916cd43dd8448eb211c80319c` is already used as a span field
//! ("trace_id") for grep-ability in failure output. The actual
//! assertion is "the Sentry tag equals the span's resolved trace_id",
//! which is the load-bearing claim: deep-link will work because the
//! same id appears in both places.
//!
//! ## Why `build_error_layer` + `with_captured_events` instead of
//!    inline construction
//!
//! The upstream layer's unit test (`error_event_attaches_trace_id_tag`)
//! constructs the Sentry layer inline because the private
//! `event_mapper_with_trace_context` and `error_only_event_filter`
//! helpers aren't exported. From fold_db_node we can't reach those, so
//! we exercise the public surface — `build_error_layer` reading
//! `OBS_SENTRY_DSN` from env — which is the actual production wiring
//! this test is meant to guard. `with_captured_events` swaps in a test
//! transport on the global Sentry hub for the duration of the closure;
//! the `SentryGuard` returned by `build_error_layer` keeps its
//! production `ClientInitGuard` alive but the hub-bound test client
//! intercepts emitted events instead.

use std::sync::Mutex;

use observability::layers::error::{build_error_layer, OBS_SENTRY_DSN_ENV};
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::TracerProvider as SdkTracerProvider;
use tracing::subscriber::with_default;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Registry;

/// W3C-spec example trace_id used as a span field literal so failure
/// output is greppable. The Sentry tag asserted against is the
/// span's resolved id, not this literal — see module docs.
const TRACE_ID_LITERAL: &str = "0af7651916cd43dd8448eb211c80319c";

/// Serialize tests in this binary that touch process-global state
/// (the Sentry hub and the `OBS_SENTRY_DSN` env var). Currently only
/// one test, but kept so adding a second test in this file can't
/// race with the first.
fn env_lock() -> &'static Mutex<()> {
    use std::sync::OnceLock;
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn error_event_inside_otel_span_is_captured_with_trace_id_tag() {
    let _env_guard = env_lock().lock().unwrap_or_else(|p| p.into_inner());

    // -- 1. Flip the env gate so `build_error_layer` returns Some -------
    //
    // The DSN is fake but syntactically valid — `sentry::init` parses
    // it without contacting the network. `with_captured_events` later
    // swaps in a test transport so even if the production client did
    // attempt a flush, no bytes leave the process.
    let prev_dsn = std::env::var(OBS_SENTRY_DSN_ENV).ok();
    std::env::set_var(OBS_SENTRY_DSN_ENV, "https://fake@dummy.ingest.sentry.io/1");

    // -- 2. Build the production observability subscriber with the
    //    ERROR layer enabled -------------------------------------------
    //
    // `build_error_layer` is the production wiring path; calling it is
    // the actual integration claim under test. The OTel layer is
    // composed under it so `OtelData` is present on the parent span
    // when the Sentry layer's event_mapper runs.
    let provider = SdkTracerProvider::builder().build();
    let tracer = provider.tracer("fold_db_node-sentry-e2e-test");
    let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

    let (error_layer, _sentry_guard) =
        build_error_layer().expect("OBS_SENTRY_DSN is set so build_error_layer must return Some");

    let subscriber = Registry::default().with(otel_layer).with(error_layer);

    // -- 3. Capture the actual span trace_id, emit the error -----------
    //
    // `with_captured_events` rebinds the global Sentry hub to a test
    // transport for the duration of the closure. `with_default` scopes
    // the tracing subscriber to the same closure so events emitted
    // inside flow through the ERROR layer → Sentry hub → test
    // transport.
    let observed_trace_id = Mutex::new(String::new());
    let events = sentry::test::with_captured_events(|| {
        with_default(subscriber, || {
            use opentelemetry::trace::TraceContextExt;
            use tracing_opentelemetry::OpenTelemetrySpanExt;

            let span = tracing::info_span!(
                "http.server.request",
                trace_id = TRACE_ID_LITERAL,
            );
            let _enter = span.enter();

            let trace_id_hex =
                format!("{:032x}", span.context().span().span_context().trace_id());
            *observed_trace_id.lock().unwrap() = trace_id_hex;

            tracing::error!("test error from e2e");
        });
    });

    // -- 4. Restore env so other tests in the same binary aren't
    //    affected (currently only this test, but cheap insurance) -----
    match prev_dsn {
        Some(v) => std::env::set_var(OBS_SENTRY_DSN_ENV, v),
        None => std::env::remove_var(OBS_SENTRY_DSN_ENV),
    }

    // -- 5. Assertions -------------------------------------------------
    assert!(
        !events.is_empty(),
        "expected at least one Sentry event captured from tracing::error!, got 0 \
         (events: {events:?})"
    );

    let event = &events[0];
    assert_eq!(
        event.level,
        sentry::Level::Error,
        "captured event must be ERROR level, got {:?}",
        event.level,
    );

    let observed = observed_trace_id.lock().unwrap();
    assert_eq!(
        event.tags.get("trace_id"),
        Some(&*observed),
        "captured event must carry the originating span's resolved trace_id as a tag; \
         observed span trace_id={observed:?}, event tags={:?}",
        event.tags,
    );
}
