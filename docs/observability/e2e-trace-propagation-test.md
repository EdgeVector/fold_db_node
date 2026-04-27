# Phase 2 / D1 — Headline E2E trace-propagation test

Working notes from the Phase 2 D1 implementation
(`tests/observability_e2e_trace_propagation.rs`). The test is the
cohort B headline check that a single W3C `trace_id` flows from an
upstream `traceparent` header all the way through fold_db_node's
production observability stack and out onto the egress wire.

## 2026-04-27 — Test approach: wiremock vs in-process schema_service

Two factors steered us off the "boot a real `schema_service` in-process"
path the original task description suggested:

1. **One global `tracing` subscriber per process.** `tracing` enforces a
   single global default subscriber, so two in-process servers can't
   write to two independent `OBS_FILE_PATH` files in the same test
   binary. Either both servers share one subscriber (and the file
   contains intermixed events from both), or the test takes the
   out-of-process route which is fragile in CI.

2. **`tests/common/schema_service.rs` mounts only `configure_routes`** —
   no `TracingLogger`, no `W3CParentContext`. Even if both servers
   funneled events through one subscriber, the schema-service handlers
   would emit events under fresh trace ids rather than the inherited
   upstream one, masking the very propagation we're trying to verify.
   (Schema_service's own ingress middleware is tracked separately and
   has not yet landed in the `crates/server_http` source.)

Wiremock side-steps both: it captures the egress request's
`traceparent` header byte-for-byte, which is the actual artifact a
real downstream service would key off of. The fold_db_node side runs
the production observability stack (`init_node` + `W3CParentContext`
+ `inject_w3c`), so the assertion that the upstream trace-id reaches
the downstream wire is the same end-to-end claim as a two-server
setup, with one fewer moving part.

`wiremock = "0.6"` was added to `[dev-dependencies]`.

## 2026-04-27 — Gotcha: `RingLayer` records `builder.trace_id`, not the
inherited parent trace_id

The first iteration of the test asserted on the RING layer's emitted
event metadata, expecting `trace_id == upstream`. The assertion failed
even though the propagation end-to-end was working. Root cause:

- `tracing-opentelemetry`'s `OtelData` stores **two** trace ids: the
  local span's `builder.trace_id` (assigned at `on_new_span` time) and
  the `parent_cx`'s span context (mutated later by
  `OpenTelemetrySpanExt::set_parent`).
- `RingLayer::on_event` reads `otel_data.builder.trace_id` only.
- Actix middleware ordering means `TracingLogger` creates the root
  span **before** `W3CParentContext` attaches the upstream parent. So
  `builder.trace_id` is a freshly minted local id, not the inherited
  upstream id.
- `tracing_opentelemetry::PreSampledTracer::sampled_context` (called
  via `OpenTelemetrySpanExt::context()` and `inject_w3c`) DOES walk
  `parent_cx` and return the inherited id, which is why egress
  propagation succeeds while RING's `trace_id` field disagrees.

The test's primary assertion targets the egress wire instead. It
also cross-checks the ingress side directly inside the handler via
`Span::current().context().span().span_context().trace_id()`, which
goes through the same `sampled_context` path `inject_w3c` uses.

This RING divergence is its own follow-up — the layer should prefer
`parent_cx`'s trace_id when one is present — but it's out of scope
for the headline E2E test.

## 2026-04-27 — Egress wrap is currently deferred at the production
call site

The actual `SchemaServiceClient::*` methods do **not** wrap their
outgoing `reqwest::RequestBuilder` with `inject_w3c` yet — see the
`// trace-egress: propagate (schema service; inject_w3c wrapping
deferred — pending fold_db rev bump)` markers landed by PR #709.
The headline test therefore calls `inject_w3c` directly inside a
test-only handler that stands in for the future-wrapped client. When
the deferred wrap lands, the production `SchemaServiceClient`
becomes the call site under test and this scaffolding can shrink
to a pure HTTP integration test.

The test exercises:
- `fold_db_node::server::middleware::otel::W3CParentContext` (PR #708)
- `tracing_actix_web::TracingLogger` (root span source)
- `observability::propagation::inject_w3c` (egress helper)
- `observability::init_node` (FMT + RELOAD + RING composition + W3C
  text-map propagator install)

so any future regression in the cohort B code surfaces here.

## Run

```bash
cargo test --test observability_e2e_trace_propagation -- --nocapture
```
