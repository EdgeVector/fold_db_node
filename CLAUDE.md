# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**fold_db_node** is a node implementation for the FoldDB distributed database network. This repo follows the same architecture and conventions as [fold_db](https://github.com/EdgeVector/fold_db).

## CI Pipeline

CI triggers on push to `main` and on pull requests. Three jobs run in parallel, each skipping if its code doesn't exist yet:

- **Rust Tests**: Requires `Cargo.toml`. Runs clippy (`-D warnings`), `cargo check --features aws-backend`, `cargo test`, and integration tests (if API keys are configured).
- **Frontend Tests**: Requires `src/server/static-react/package.json`. Runs `npm test` (vitest).
- **E2E UI Tests**: Requires `src/server/static-react/e2e/` directory. Runs Playwright browser tests.

### Pre-PR Checklist

Once Rust code exists, run before every push:
```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --features aws-backend
cargo test --workspace --all-targets
```

Once frontend code exists:
```bash
cd src/server/static-react/
npm test
npm run test:e2e
```

## Schema service

The schema service lives in its own repo, [EdgeVector/schema_service](https://github.com/EdgeVector/schema_service), and is deployed as a Lambda at `schema.folddb.com` via [EdgeVector/schema-infra](https://github.com/EdgeVector/schema-infra). fold_db_node consumes it as a client.

- **Client**: the `schema_service_client` crate (published from the `schema_service` workspace). `fold_db_node::fold_node` re-exports `SchemaServiceClient` for internal use. Integration tests can inject `test://mock` via the `schema_service_url` config.
- **Dep pinning**: `schema_service_core`, `schema_service_client`, and (dev-dep) `schema_service_server_http` are git deps in `Cargo.toml`, pinned to a specific commit via `Cargo.lock`. CI builds from that pin. `.cargo/config.toml` patches them to the sibling `../schema_service/crates/*` checkout for local dev; CI removes that file before building so the lockfile pin wins and main-branch drift can't break PR CI. Mirrors the same pattern `schema_service` uses for its own `fold_db` dep.
- **Bumping a patched dep**: use `bash scripts/cargo-update.sh -p <pkg>`. The wrapper moves `.cargo/config.toml` aside before invoking cargo, restores it on any exit (success, error, or kill), and runs the lockfile lint at the end. To bump `schema_service`: `cd ../schema_service && git pull`, then in fold_db_node run `bash scripts/cargo-update.sh -p schema_service_core`. **Never run `cargo update -p` directly while the patch is active** — every cargo invocation with the patch active rewrites Cargo.lock to drop `source = "git+..."` lines from the patched packages, and committing that produces the missing-source trap below. The PreToolUse `cargo-lock-guard.sh` hook blocks the *purely-noise* commit, but a commit that combines a real bump with the noise still gets through.
- **Missing-source recovery**: if a prior commit landed with `[[package]]` entries that lost their `source = "git+..."` lines, `cargo update -p <pkg>` errors with `package ID specification "<pkg>" did not match any packages` (cargo identifies a package by `(name, version, source)`, so a sourceless entry is unreachable from the CLI). `bash scripts/lint-cargo-lock-sources.sh` (also wired into CI) flags the offenders. To repair, surgically re-inject each missing source line with the correct git URL and commit hash from the matching `Cargo.toml` git dep:
  ```bash
  python3 - <<'PY'
  # Map of {package_name: source-line value}. Pull the rev/branch/hash
  # from the matching Cargo.toml entry and use the resolved commit SHA
  # (look at a sibling clone or git ls-remote) for the trailing `#<sha>`.
  sources = {
      "fold_db": 'git+https://github.com/EdgeVector/fold_db.git?rev=<sha>#<sha>',
      # ...add an entry per offender
  }
  import re, pathlib
  text = pathlib.Path("Cargo.lock").read_text()
  for name, src in sources.items():
      pat = re.compile(rf'(\[\[package\]\]\nname = "{re.escape(name)}"\nversion = "[^"]+"\n)(?!source = )')
      text, n = pat.subn(rf'\1source = "{src}"\n', text, count=1)
      assert n == 1, f"failed to patch {name}"
  pathlib.Path("Cargo.lock").write_text(text)
  PY
  ```
  Run the lint after to confirm clean, then commit only the Cargo.lock fix.
- **Dev binary** (optional): `cargo run -p schema_service_server_http --bin schema_service -- --port 9102 --db-path schema_registry` in the sibling `schema_service/` checkout. `./run.sh --local-schema` orchestrates this automatically.
- **No in-tree code**: `src/bin/schema_service.rs` and `src/schema_service/` were removed in Phase 0; `src/fold_node/schema_client.rs` was removed in Phase 3 T2.

## Local Development

Always use `run.sh` to start the dev server — never start binaries manually:
```bash
./run.sh --local --local-schema    # Fully offline development (preferred)
./run.sh --local                   # Local storage with prod schema service
./run.sh --local --empty-db        # Local with fresh database
```

The script handles process cleanup, building, schema service startup, and frontend (Vite).
- Backend: http://localhost:9101 (dev default; auto-picked in 9101..=9199 when parallel agents run)
- Schema service: http://localhost:9102
- UI: http://localhost:5173 (dev default; auto-picked in 5173..=5299 when parallel agents run)

Dev uses the 9101 range so it doesn't collide with the prod Tauri bundle, which owns 9001 (and falls back to 9002..=9010). The Vite frontend auto-slots independently in 5173..=5299 (127 ports). Check `~/.folddb-slots/*.json` for the backend/schema/vite ports a running `run.sh` picked, or pin any of them with `--port`, `--schema-port`, or `VITE_PORT`. Widen/shift the Vite scan with `VITE_PORT_BASE` / `VITE_PORT_COUNT` if another stack already holds the 5173+ block.

## Local-dev gotchas (read if `cargo check` explodes)

Two mines, both already defused — but if you find yourself debugging either, this is what's going on:

### Dual `fold_db` in the dep graph → wall of type-mismatch errors

Symptoms: dozens of errors like `expected fold_db::triggers::Trigger, found schema_service_core::types::Trigger`, all from types re-exported through `schema_service_core`. Root cause: cargo compiles **two copies** of `fold_db` whenever `fold_db_node/Cargo.toml` and the sibling `schema_service/Cargo.toml` use different source specs (e.g. `branch = "mainline"` vs `rev = "..."`) — cargo treats them as different packages even when both SHAs match.

**Defense (production):** both repos pin fold_db to the **same explicit `rev`**. `fold_db_node/Cargo.toml` and `schema_service/Cargo.toml` must match: bump them in lockstep. To bump:

1. Land your fold_db PR; copy the squash-commit SHA from `main`.
2. Open a `schema_service` PR setting `fold_db = { ..., rev = "<sha>" }`. Merge.
3. Open a `fold_db_node` PR setting the same `rev` AND running `cargo update -p schema_service_core` to pull schema_service's new pin into Cargo.lock. Merge.

If only one side bumps, CI here surfaces the dual-`fold_db` errors above. (Pre-2026-04, this repo used `branch = "mainline"`; that worked **only** while mainline HEAD coincidentally matched schema_service's pin. Don't go back to branch tracking unless schema_service drops its rev pin too — but it can't, because its CDK Docker build needs deterministic pinning without a parent lockfile.)

**Defense (local dev):** `.cargo/config.toml` patches **both** `fold_db` and `schema_service` to their sibling paths (`../fold_db`, `../schema_service/crates/*`). Every git-spec variation collapses onto one local path = one `fold_db` in the graph. If you remove or rename the sibling, the patch silently no-ops and the two-copies problem comes back — that's the fingerprint.

### Fresh clone fails on RustEmbed (`static-react/dist` missing)

Symptom: `#[derive(RustEmbed)] folder '...src/server/static-react/dist' does not exist`. Happens after `git clone`, after creating a worktree, or any time `cargo clean` wipes a previous stub.

**Defense:** [build.rs](build.rs) writes a stub `dist/index.html` if the directory is absent. A real `npm --prefix src/server/static-react run build` overwrites the stub. CI always builds the frontend before Rust jobs, so prod is unaffected.

## Feature Flags

- `os-keychain` — Encrypts node identity, E2E key, and credentials at rest using an OS keychain master key. Enabled in Tauri release builds. Disabled by default for dev/test (plaintext with 0o600 permissions).
- `aws-backend` — Enables DynamoDB backend (inherited from fold_db).

## Coding Standards

Follow the same standards as fold_db:
- No silent failures — throw errors if anything goes wrong
- No branching logic where avoidable
- No inline crate imports — import in headers only
- No fallbacks — they hide broken code
- Always write tests
- Use `TODO` format for incomplete implementations
- Platform-specific APIs (e.g., OS keychain) must be behind feature flags

## Observability conventions

When you touch any `tracing::*!`, `tokio::spawn`, `reqwest::Client::new()`, or sensitive-field log site, the conventions live at `gbrain get concepts/observability-conventions`. Long-form: `exemem-workspace/docs/observability/migration-guide.md`. CI-enforced rules:

- **Structured fields** — `tracing::info!(field = %value, "msg")`. Positional-arg interpolation gets a warn-only nudge.
- **Redaction** — `password / token / api_key / secret / auth_token / email / phone / ssn` MUST go through `redact!()` or `redact_id!()`. Override per-line: `// lint:redaction-ok <reason>`.
- **`tokio::spawn`** — chain `.instrument(Span::current())` or `.in_current_span()`. Bare spawn fails CI. Override: `// lint:spawn-bare-ok <reason>`.
- **Outbound `reqwest`** — every `Client::new() / ::builder() / ::default()` site needs a comment within 3 preceding lines: `propagate / loopback / skip-s3 / skip-3p`. Wrap propagating requests with `observability::propagation::inject_w3c`.

The cloud-stack cleanup (2026-04-28) removed OTLP / Honeycomb / SpanMetrics. Do NOT add `opentelemetry-otlp` / `opentelemetry-proto` deps or set `OBS_OTLP_ENDPOINT`. Sentry stays (off-by-default; activates if `OBS_SENTRY_DSN` is set).

## Trust boundary: loopback owner context

**Intentional invariant (as of 2026-04-13)**: `src/handlers/query.rs` and `src/handlers/mutation.rs` hardcode the HTTP caller as the node's own pubkey (`caller_pub_key = node.get_node_public_key()`). This gives every local HTTP request owner context via `build_access_context`'s owner short-circuit, so trust-tier enforcement is effectively disabled for anything reaching `http://localhost:9001`.

This is **correct for the Tauri single-user model**: the owner of the device is the owner of the data, and the node is only reachable from localhost via the Tauri UI / CLI. Access enforcement on the remote discovery/messaging path (non-owner callers) is separate and works correctly.

**What's NOT protected**:
- Other processes on the user's machine that can reach `localhost:9001`
- Browser extensions with permissive CORS
- Any future shared / headless / multi-user distribution

**Before shipping a non-Tauri distribution (headless daemon, shared mode, hosted-for-many, etc)** you MUST:

1. Verify the CORS / bind config rejects non-Tauri origins in release builds (audit `actix-cors` config in `src/server/http_server.rs`; confirm release binds loopback-only and rejects cross-origin requests that don't originate from the Tauri webview).
2. Add per-request caller authentication — loopback token, session cookie, or signed identity header — so the hardcoded `get_node_public_key()` can be replaced with a verified caller identity.
3. Update `src/handlers/query.rs` and `src/handlers/mutation.rs` to consume that verified identity instead of the hardcoded owner pubkey.

See `docs/designs/trust_domains.md` (workspace root) for the access enforcement design. See PR #436 for the feed.rs Case-A audit that formalized this invariant.
