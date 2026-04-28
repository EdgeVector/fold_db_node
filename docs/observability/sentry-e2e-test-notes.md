# Sentry ERROR-layer E2E test notes

Phase 4 / T10 design notes for `tests/observability_sentry_error_e2e.rs`.

## What the test guards

The upstream `observability::layers::error::build_error_layer` wires a
Sentry sink that:

1. Returns `None` when `OBS_SENTRY_DSN` is unset (strict opt-in).
2. When set, calls `sentry::init` and builds an ERROR-only
   `sentry_tracing::SentryLayer` whose `event_mapper` lifts the W3C
   `trace_id` / `span_id` off the parent span's `OtelData` extension and
   attaches them as Sentry tags.

The fold_db_node integration test's job is to validate that
fold_db_node, as a *consumer* of that public API, gets the expected
end-to-end behaviour: setting the env var → composing the layer →
emitting `tracing::error!` inside an OTel span → seeing a captured
event tagged with that span's `trace_id`.

The upstream crate already has tight unit tests for the layer's own
behaviour (`error_event_attaches_trace_id_tag`,
`non_error_events_are_filtered_out`,
`returns_some_when_dsn_set_and_composes_in_registry`). This integration
test is intentionally a thin pass-through — its value is catching
breakage in the *contract* (`build_error_layer` signature, env var
name, tag names) when fold_db_node bumps the observability rev.

## Public-surface choice: `build_error_layer` not inline

The upstream unit test constructs the Sentry layer inline
(`sentry_tracing::layer().event_mapper(...).with_filter(...)`) because
`event_mapper_with_trace_context` and `error_only_event_filter` are
private helpers. From fold_db_node we deliberately *cannot* reach
those, which is why the integration test exercises only the public
surface:

- `build_error_layer()` reading `OBS_SENTRY_DSN_ENV` from env.
- `ErrorLayer<S>` composing into `Registry::default().with(otel_layer).with(error_layer)`.
- `SentryGuard` lifetime contract.

If a future upstream refactor changes any of those, the test fails on
fold_db_node's CI rather than silently regressing.

## Why we don't assert against a literal trace_id

The test uses the W3C-spec example id `0af7651916cd43dd8448eb211c80319c`
as a span field literal (`trace_id = "..."`) so the value is greppable
in failure output. The actual assertion compares the captured Sentry
tag against the span's *resolved* `trace_id`, observed at runtime via
`Span::current().context().span().span_context().trace_id()`.

To make the trace_id deterministic we'd need a custom `IdGenerator`
plumbed through `SdkTracerProvider::builder()`. That's extra plumbing
for no extra signal — the load-bearing claim is "the Sentry tag
matches the span's trace_id", so the deep-link works regardless of
what the actual id is.

(The same divergence exists in `observability_e2e_trace_propagation.rs`
for the same reason; see that test's module docs for the underlying
`builder.trace_id` vs `parent_cx.trace_id` distinction in
`tracing-opentelemetry`.)

## Sentry hub interaction with `with_captured_events`

`build_error_layer` calls `sentry::init`, which binds a production
client to the global Sentry hub and returns a `ClientInitGuard` (held
inside `SentryGuard`). `sentry::test::with_captured_events` then *also*
binds a client — its in-memory `TestTransport` — to the hub for the
duration of its closure.

The flow is:

1. `build_error_layer` → hub bound to production client (writes to
   real DSN, but DSN is fake so any flush would 404).
2. Inside `with_captured_events` closure → hub temporarily rebound to
   test client. Events emitted go through `TestTransport`.
3. Closure exits → hub restored to production client. Test collects
   events from `TestTransport`.
4. End of test → `SentryGuard` drops, flushing the production client
   (no events buffered, since they all went through the test client
   while the hub was rebound).

This is why setting a fake DSN is safe: the production client is
created but its transport is never asked to send anything that wasn't
already intercepted.

## Process-global state serialization

The test file uses an `env_lock()` Mutex to serialize tests that touch
`OBS_SENTRY_DSN` and the global Sentry hub. There's only one test
today, but adding a second to this file would race without it: cargo
runs tests in parallel by default, and `set_var` / hub bindings are
process-globals. Cheap insurance — same pattern as the upstream
`error.rs` test module's `env_lock()`.

The test also restores `OBS_SENTRY_DSN` to its pre-test value (or
removes it) at the end. Combined with the env_lock, this means
adding more tests to this binary won't accidentally observe a leaked
DSN from a prior run.

## Dep changes that landed with this test

- `observability` git rev bumped from
  `4574614708cbb539443c2c1918b942afd88aaaba` to
  `868da9ab5419de1baa69e7a4b79e59141adb34a4` so
  `observability::layers::error` is reachable. Per `CLAUDE.md`,
  observability is bumped independently of `fold_db` because it has no
  `fold_db` dep itself.
- `[dev-dependencies]` added: `sentry = "0.47"` with the `test` feature
  (for `with_captured_events`) plus the same default-features set the
  upstream layer ships with, and `sentry-tracing = "0.47"` so the
  `ErrorLayer<S>` type alias resolves.

`tracing-opentelemetry` and `opentelemetry` are already in
`[dependencies]`, so cargo unifies them into the integration test
build without redeclaration.

## Out of scope

- Production Sentry DSN provisioning (separate ops task).
- T9 soak test that boots a real fold_db_node binary, points it at a
  Sentry-test mock, and asserts events for a long-running workload.
  Tracked separately.
- Wiring `build_error_layer` into `observability_setup::init_node_with_web`.
  The current init helper composes FMT/RELOAD/RING/WEB/OTel; adding
  ERROR is its own follow-up because it changes `NodeObsGuard` to also
  hold a `SentryGuard` (or equivalent). This test exercises the layer
  in isolation so the helper change can land independently.
