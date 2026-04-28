# Redaction & spawn-instrument lints (Phase 5 / P5-T1-fdnode)

Two static guards that fail CI on observability hazards:

1. **`scripts/lint-redaction.sh`** — fails if a `tracing` macro emits a
   sensitive field as a raw value instead of routing it through
   `observability::redact!()` / `observability::redact_id!()`.
2. **`scripts/lint-spawn-instrument.sh`** — fails if a
   `tokio::spawn(async { ... })` site lacks `.instrument(...)` /
   `.in_current_span()`, so the parent's `trace_id` (and other span
   fields) survive the spawn boundary.

Both lints are ports of the canonical fold_db versions; the rationale,
override grammar, and exit-code conventions are identical. Refer to
fold_db's docs for the long-form reasoning:

- `docs/observability/redaction-lint.md` (in fold_db) — guarded fields,
  what "redacted" means, override examples.
- `docs/observability/spawn-instrument-lint.md` (in fold_db) — why
  spawn instrumentation matters, canonical good shapes, the
  pre-approved bare-spawn rationales.

## fold_db_node-specific adjustments

- **Scope**: `src/` (single-crate repo) instead of fold_db's
  `crates/*/src/`. Top-level `tests/` integration tests are out of
  scope at the directory level, mirroring `lint-tracing-egress.sh` in
  this repo.
- **Spawn override count at port time**: 6 sites carry
  `// lint:spawn-bare-ok <reason>` after this PR. All are boot-time /
  perpetual workers with no per-request parent span:
  - `src/bin/folddb/update_check.rs` — CLI startup fire-and-forget.
  - `src/server/http_server.rs` × 2 — bootstrap resume and Exemem
    session-token refresh, both run during server start.
  - `src/server/embedded.rs` × 2 — embedded server runner task in
    `start_embedded_server_lazy` and `start_embedded_server`.
  - `src/server/routes/apple_import.rs` — the Apple auto-sync scheduler
    loop (`spawn_sync_scheduler`, perpetual 60-second tick).
  - `src/ingestion/smart_folder/batch.rs` — the deferred 5-minute batch
    controller cleanup.
- **Redaction override count at port time**: 0. Every sensitive-field
  call site is already wrapped or the field is constructed safely.

## Running locally

```sh
sh   scripts/lint-redaction.sh
bash scripts/lint-spawn-instrument.sh
```

Each exits `0` when clean, `1` otherwise. Both are wired into the
`Redaction Lint` CI job in `.github/workflows/ci-tests.yml` and run on
every PR and `push` to `main`. The job installs `ripgrep` and runs the
scripts directly — no Rust toolchain dependency, so it lights up green
or red within a minute regardless of the rest of the build.

## Override syntax

For both scripts:

```rust
// lint:redaction-ok <reason>          // for redaction lint
// lint:spawn-bare-ok <reason>         // for spawn-instrument lint
```

The marker may live on the violating line itself or on the line
immediately preceding it (a two-line window so `rustfmt` lifting a
trailing comment onto its own line doesn't break the override). For
the spawn lint the marker also matches if it appears anywhere inside
the `tokio::spawn(...)` call's parenthesised body.

Always include a short reason after the marker so the next reviewer
can tell at a glance whether the override is still load-bearing. Use
overrides sparingly — for the spawn lint in particular, anything on
the request path (handler, ingestion pipeline, mutation) should be
fixed with `.instrument(tracing::Span::current())` rather than marked.

## Out of scope

- Top-level `tests/` integration tests — same rationale as
  `lint-tracing-egress.sh`.
- Format-time deny-list in the FMT layer (lives upstream in fold_db's
  `crates/observability/src/layers/fmt.rs`).
