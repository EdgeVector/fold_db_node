//! Phase 2 / D1 — Headline end-to-end trace-propagation test.
//!
//! Verifies that a single W3C `trace_id` flows through fold_db_node's full
//! observability pipeline:
//!
//!   upstream `traceparent` header
//!     → `W3CParentContext` ingress middleware (PR #708)
//!         → `tracing-actix-web` root span (parent attached onto upstream cx)
//!             → events emitted during request handling resolve the
//!               span's effective trace_id off the inherited parent_cx
//!             → `observability::propagation::inject_w3c` re-injects the
//!                same trace into the outgoing `reqwest` request
//!         → downstream service receives a `traceparent` whose trace_id
//!           matches the upstream one byte-for-byte.
//!
//! ## Test approach: wiremock for the downstream service (option (a))
//!
//! Two factors steered us off the "boot a real schema_service in-process"
//! path:
//!
//! 1. **One global tracing subscriber per process.** `tracing` enforces a
//!    single global default subscriber, so two in-process servers can't
//!    write to two independent `OBS_FILE_PATH` files in the same test
//!    binary. Either both servers share one subscriber (and the file
//!    contains intermixed events from both), or the test takes the
//!    out-of-process route which is fragile in CI.
//!
//! 2. **`tests/common/schema_service.rs` mounts only `configure_routes`** —
//!    no `TracingLogger` + `W3CParentContext`. Even if both servers
//!    funneled events through one subscriber, the schema-service
//!    handlers would emit under fresh trace ids rather than the
//!    inherited upstream one, masking the very propagation we're
//!    trying to verify.
//!
//! Wiremock side-steps both: it captures the egress request's
//! `traceparent` header byte-for-byte, which is the actual artifact a
//! real downstream service would key off of. The fold_db_node side runs
//! the production observability stack (`init_node` + `W3CParentContext`
//! + `inject_w3c`), so the assertion that the upstream trace-id reaches
//!   the downstream wire is the same end-to-end claim as a two-server
//!   setup, with one fewer moving part.
//!
//! ## Why the assertion targets the egress wire, not the RING entry's
//!    `trace_id` field
//!
//! `tracing-opentelemetry`'s `OtelData` stores the local span's
//! `builder.trace_id` (assigned at `on_new_span` time) and the
//! `parent_cx` (mutated later by `OpenTelemetrySpanExt::set_parent`).
//! The current `RingLayer` reads `builder.trace_id` only — and because
//! actix middleware ordering means `TracingLogger` creates the root
//! span **before** `W3CParentContext` attaches the upstream parent,
//! `builder.trace_id` is a freshly minted local id rather than the
//! inherited one. Span context resolution via
//! `tracing_opentelemetry::PreSampledTracer::sampled_context` (called
//! from `OpenTelemetrySpanExt::context()` and `inject_w3c`) walks the
//! parent_cx and returns the inherited id correctly, which is why the
//! egress-side assertion succeeds today while the RING field would
//! disagree. That divergence is its own follow-up — the RING layer
//! should prefer the parent_cx trace_id when one is present — but it's
//! out of scope for this headline E2E test.
//!
//! ## What this test does NOT cover
//!
//! - The actual `SchemaServiceClient` does **not** wrap its outgoing
//!   `reqwest::RequestBuilder` with `inject_w3c` yet — see the
//!   `// trace-egress: propagate (schema service; inject_w3c wrapping
//!   deferred — pending fold_db rev bump)` markers landed by PR #709.
//!   This test calls `inject_w3c` directly inside a test handler that
//!   stands in for the future wrapped client. When the deferred wrap
//!   lands, the production `SchemaServiceClient` becomes the call site
//!   under test and this scaffolding can shrink to a pure HTTP
//!   integration test.
//! - schema_service's own ingress middleware (cohort B work tracked
//!   separately) is not exercised here — that's a follow-up E2E once
//!   both ingress stacks share a process boundary.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use actix_web::{test, web, App, HttpResponse};
use observability::propagation::inject_w3c;
use opentelemetry::trace::TraceContextExt;
use serde_json::Value;
use tracing::Span;
use tracing_actix_web::TracingLogger;
use tracing_opentelemetry::OpenTelemetrySpanExt;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use fold_db_node::server::middleware::otel::W3CParentContext;

/// W3C v00 traceparent fixture used by the assertion. The 32-hex
/// `trace_id` and 16-hex `span_id` are from the W3C trace-context spec
/// example, so they are easy to grep for in failure output while still
/// being a valid concrete trace.
const UPSTREAM_TRACEPARENT: &str = "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01";
const UPSTREAM_TRACE_ID: &str = "0af7651916cd43dd8448eb211c80319c";

/// Captures the trace_id observed by the handler so the test body can
/// cross-check the ingress side without depending on RING's
/// `builder.trace_id` view (see module docs for why those diverge under
/// late `set_parent`).
type ObservedTraceId = web::Data<Mutex<Option<String>>>;

/// Test handler wired into the test Actix app. Stands in for the
/// future-wrapped `SchemaServiceClient`: makes an HTTP request to the
/// downstream URL with `inject_w3c` so the outgoing builder carries the
/// current span's `traceparent` header. Also records the trace_id seen
/// by `Span::current().context()` for the test body's ingress assert.
async fn proxy_to_downstream(
    downstream: web::Data<Arc<reqwest::Client>>,
    target: web::Data<String>,
    observed: ObservedTraceId,
) -> HttpResponse {
    let url = format!("{}/v1/schemas/available", target.get_ref());

    // Resolve the effective trace_id for the current span via the same
    // `sampled_context` path `inject_w3c` uses. This is the canonical
    // ingress check — see module docs for why we don't read the RING
    // event's `trace_id` field instead.
    let observed_trace = format!(
        "{:032x}",
        Span::current().context().span().span_context().trace_id()
    );
    *observed.lock().unwrap() = Some(observed_trace);

    // Emit an info-level event so the FMT layer flushes a JSON line
    // for this request. tracing_actix_web's TracingLogger creates the
    // root span but doesn't emit an explicit event itself, so without
    // this the production sinks have no event-level evidence to assert
    // against.
    tracing::info!(target: "fold_db_node_e2e", url = %url, "proxying to downstream");

    let builder = downstream.get(&url);
    let builder = inject_w3c(builder);
    match builder.send().await {
        Ok(r) if r.status().is_success() => HttpResponse::Ok().body("ok"),
        Ok(r) => HttpResponse::BadGateway().body(format!("downstream {}", r.status())),
        Err(e) => HttpResponse::BadGateway().body(format!("downstream error: {}", e)),
    }
}

/// Headline E2E: incoming `traceparent` is extracted by the production
/// ingress middleware and re-injected on a `reqwest` egress wrapped by
/// the production `inject_w3c` helper.
#[actix_web::test]
async fn upstream_traceparent_propagates_through_ingress_and_egress() {
    // -- 1. Tempdir + OBS_FILE_PATH -------------------------------------
    //
    // Stamping a per-test override on the shared env keeps the
    // `init_node` log file inside this test's tempdir even when the
    // host machine's `~/.folddb/observability.jsonl` already exists.
    let tmp = tempfile::tempdir().expect("tempdir for OBS_FILE_PATH");
    let obs_log = tmp.path().join("fold_db_node.jsonl");
    std::env::set_var("OBS_FILE_PATH", &obs_log);

    // -- 2. Install the production observability stack ------------------
    //
    // `init_node` is process-global and one-shot. This test is the only
    // installer in its compilation unit (`cargo test --test
    // observability_e2e_trace_propagation` runs in a dedicated process),
    // so it owns the global subscriber for the run. We hold the guard
    // for the whole test body so the FMT worker's flush thread keeps
    // draining as events come in.
    let guard = observability::init_node("fold_db_node-e2e-test", env!("CARGO_PKG_VERSION"))
        .expect("init_node");

    // -- 3. Spawn the wiremock stand-in for schema_service --------------
    //
    // Responds 200 with an empty schemas list so the egress `reqwest`
    // call resolves cleanly. The test only inspects the captured request
    // metadata; the response body is incidental.
    let mock_server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({ "schemas": [] })),
        )
        .mount(&mock_server)
        .await;
    let downstream_url = mock_server.uri();

    // -- 4. Build the test Actix app ------------------------------------
    //
    // Same middleware order as `FoldHttpServer::run`: TracingLogger is
    // outer, W3CParentContext is inner, so the root span exists by the
    // time we attach the upstream parent. The test handler stands in
    // for a future-wrapped `SchemaServiceClient`.
    let downstream_client: Arc<reqwest::Client> = Arc::new(reqwest::Client::new());
    let observed: ObservedTraceId = web::Data::new(Mutex::new(None));
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(downstream_client))
            .app_data(web::Data::new(downstream_url.clone()))
            .app_data(observed.clone())
            .wrap(W3CParentContext)
            .wrap(TracingLogger::default())
            .route("/proxy", web::get().to(proxy_to_downstream)),
    )
    .await;

    // -- 5. Send the request with the upstream traceparent --------------
    let req = test::TestRequest::get()
        .uri("/proxy")
        .insert_header(("traceparent", UPSTREAM_TRACEPARENT))
        .to_request();
    let resp = test::call_service(&app, req).await;
    assert!(
        resp.status().is_success(),
        "proxy handler should succeed; got {}",
        resp.status()
    );

    // Give the non-blocking FMT writer's worker thread a beat to drain.
    actix_web::rt::time::sleep(Duration::from_millis(150)).await;

    // -- 6. Assert ingress: the handler observed the upstream trace_id
    //    on the current span -------------------------------------------
    //
    // `Span::current().context().span().span_context().trace_id()` runs
    // through `tracing_opentelemetry`'s `sampled_context` which walks
    // the parent_cx attached by `W3CParentContext`. This is the same
    // resolution path `inject_w3c` takes, so a passing assertion here
    // is what makes the egress assertion below meaningful.
    let observed_trace = observed
        .lock()
        .unwrap()
        .clone()
        .expect("handler should have recorded an observed trace_id");
    assert_eq!(
        observed_trace, UPSTREAM_TRACE_ID,
        "ingress middleware should have surfaced the upstream trace_id onto the handler's current span"
    );

    // -- 7. Assert FMT: OBS_FILE_PATH file is non-empty JSONL -----------
    //
    // The production FMT layer doesn't stamp `trace_id` into its file
    // output (the on-the-wire OTel-shaped JSON has a `span` slot but
    // not a `trace_id` slot — that joins on the OTLP side once the
    // exporter lands in Phase 4). So we assert the weaker but still
    // load-bearing claim: events were written, they parse as JSON, and
    // at least one is the explicit "proxying to downstream" event our
    // handler emitted within the request's root span.
    let contents = std::fs::read_to_string(&obs_log).unwrap_or_default();
    let parsed_lines: Vec<Value> = contents
        .lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert!(
        !parsed_lines.is_empty(),
        "expected the FMT layer to have written JSON lines to {}; file was empty",
        obs_log.display()
    );
    let proxy_emitted = parsed_lines
        .iter()
        .filter(|line| {
            line.get("target")
                .and_then(|t| t.as_str())
                .map(|t| t == "fold_db_node_e2e")
                .unwrap_or(false)
        })
        .count();
    assert!(
        proxy_emitted >= 1,
        "expected at least one fold_db_node_e2e event in {}; saw targets: {:?}",
        obs_log.display(),
        parsed_lines
            .iter()
            .filter_map(|l| l.get("target").and_then(|t| t.as_str()).map(String::from))
            .collect::<Vec<_>>()
    );

    // -- 8. Assert egress: wiremock captured a request whose
    //    `traceparent` header carries the upstream trace_id -------------
    //
    // This is the canonical end-to-end assertion: the same trace_id
    // that came in on the upstream `traceparent` header reaches the
    // downstream wire after passing through actix ingress middleware
    // and the production `inject_w3c` egress helper.
    let received = mock_server
        .received_requests()
        .await
        .expect("wiremock must record received requests");
    assert_eq!(
        received.len(),
        1,
        "expected exactly one downstream call from the proxy handler"
    );
    let outgoing_traceparent = received[0]
        .headers
        .get("traceparent")
        .expect("egress should carry a traceparent header (inject_w3c wraps the builder)")
        .to_str()
        .expect("traceparent header must be ASCII");
    assert!(
        outgoing_traceparent.contains(UPSTREAM_TRACE_ID),
        "downstream traceparent {outgoing_traceparent:?} should embed the upstream trace_id {UPSTREAM_TRACE_ID}",
    );

    // Drop the ObsGuard at end of scope — its `FmtGuard::Drop` impl
    // joins the non-blocking writer's worker thread, so any remaining
    // events buffered in the channel land on disk before the tempdir
    // is cleaned up.
    drop(guard);
}
