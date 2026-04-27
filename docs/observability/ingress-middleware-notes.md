# Phase 2 / B1 — fold_db_node ingress middleware notes

Working notes from the Phase 2 B1 implementation (Actix middleware that
extracts the W3C `traceparent` header and attaches the resulting
`opentelemetry::Context` as the parent of the request span).

## 2026-04-27 — Dep gap surfaced when implementing B1

The B1 task description claimed:

> the `observability` crate must already be a workspace dependency
> (was added during Phase 1 T1b — see fold_db_node #707)

This is **not** what shipped. PR #707 ([d9f7328][pr-707]) was a
single-line tweak to `.cargo/config.toml` redirecting the local-dev
`fold_db` patch at the new `crates/core` workspace member after the
fold_db workspace conversion landed. It did not add `observability` to
`fold_db_node/Cargo.toml`, and the fold_db rev pin (`a0434b25…`) is
**pre-workspace** — so the `observability` crate isn't even reachable
through the current `fold_db` dep.

[pr-707]: https://github.com/EdgeVector/fold_db_node/pull/707

### What B1 had to add

- `observability` as a direct git dep on `fold_db.git` (rev pinned to
  a post-workspace mainline commit that has the crate landed).
- `tracing-actix-web` (root span source).
- `tracing` / `tracing-opentelemetry` / `opentelemetry` as explicit
  deps (versions matching `crates/observability/Cargo.toml`: tracing
  0.1, tracing-opentelemetry 0.28, opentelemetry 0.27). They were
  previously pulled in only transitively.

### Why two revs of the same git URL is OK

Adding `observability` at a different rev than `fold_db` looks like it
should hit the dual-`fold_db` trap CLAUDE.md warns about. It doesn't:

- The dual-`fold_db` trap fires when **the `fold_db` package** is
  compiled twice and types from each copy show up at the same call
  site (re-exported through `schema_service_core` while imported
  directly elsewhere).
- `observability` does not depend on `fold_db` (see
  `crates/observability/Cargo.toml`). Pinning it at a different rev
  brings in only the `observability` package; the workspace's other
  member (`fold_db`) at that rev is not compiled.
- The local-dev `.cargo/config.toml` patch was extended with
  `observability = { path = "../fold_db/crates/observability" }` so
  sibling-checkout dev still collapses every spec onto one path.

### Why we did not bump the `fold_db` rev

CLAUDE.md is explicit that `fold_db_node` and `schema_service` must
bump `fold_db` revs in lockstep — otherwise the dual-`fold_db` errors
fire. A lockstep bump is a cross-repo workflow that's out of scope for
B1 (which is purely middleware on the ingress path). Adding
`observability` as an independent dep sidesteps the lockstep
requirement entirely.

When the next coordinated `fold_db` rev bump lands across both
consumers (post-workspace), the independent `observability` dep can be
dropped in favor of `fold_db`'s transitive re-export — or kept,
depending on whether we want `observability` to evolve independently
of `fold_db`'s release cadence.

### Propagator install is still missing

`observability::propagation::extract_parent_context` relies on a global
text-map propagator (`TraceContextPropagator`) being installed.
`observability::init::init_node` installs it, but `fold_db_node` does
not yet call any of the `init_*` helpers — that's a separate Phase 1
follow-up. Until it lands, the middleware will run with **no
propagator installed**, and every extracted context will be empty
(invalid span context). That's a no-op, not a regression: spans simply
won't be parented across the HTTP boundary.

The middleware test (`extracts_traceparent_into_span_parent`) installs
the propagator ad-hoc to validate the header → span round-trip in
isolation.
