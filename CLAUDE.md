# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**fold_db_node** is a node implementation for the FoldDB distributed database network. This repo follows the same architecture and conventions as [fold_db](https://github.com/EdgeVector/fold_db).

## UI scope

**The web UI in `src/server/static-react/` is desktop-only.** Mobile is going to be an entirely different experience (separate app or separate codepath, design TBD), so don't spend cycles on responsive breakpoints, touch ergonomics, or mobile layouts here. If you're tempted to add `sm:` / `md:` rules or fix something that only manifests at 375px, stop — that work belongs in the future mobile experience, not in this codebase.

Existing `sm:` rules in this codebase predate this decision and can be left alone; just don't add more.

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

## Endpoint registry

All cross-environment URLs (Schema service, Exemem API, Discovery service — dev and prod) live in [`environments.json`](environments.json) at the repo root. **That file is the single source of truth.** Edits anywhere else cause drift; CI rejects them.

Wiring:

- **Rust code** — `build.rs` parses `environments.json` at compile time and emits per-(env, key) `pub const`s in `OUT_DIR/environments_generated.rs`. `src/endpoints.rs` includes them via `include!()`. Public API: `endpoints::schema_service_url()`, `exemem_api_url()`, `discovery_service_url()` (env-aware via `EXEMEM_ENV`), and `schema_service_url_for(Environment::Dev)` for code paths that need to pin one regardless of the calling process.
- **Shell scripts** — call `scripts/get-env-url.sh <dev|prod> <region|exemem_api|schema_service|discovery>`. Errors loudly if the registry is missing the key.
- **CI workflows** — same helper, called after checkout (see `.github/workflows/e2e-cloud.yml` "Resolve dev URLs from environments.json" step). Do NOT set the URLs as static `env:` values.

Anti-drift: [`scripts/lint-no-hardcoded-urls.sh`](scripts/lint-no-hardcoded-urls.sh) reads the gateway hostnames out of `environments.json` and fails CI (and the pre-commit hook) if any of them appear in any other tracked file. Allow-listed exceptions live in the script itself with a one-line reason.

Cross-repo consumers also feed off this registry:

- `schema_service/crates/client/src/lib.rs` — `SchemaServiceClient::new(url)` requires the URL from the caller; no hardcoded default. fold_db_node passes the value resolved here.
- `exemem-workspace/.claude/skills/dogfood/teardown.sh` and `SKILL.md` — call `~/code/edgevector/fold_db_node/scripts/get-env-url.sh` (sibling repo).
- `exemem-workspace/config/node_config.json` — sample config now omits `schema_service_url`; runtime resolution fills it in.
- `fold_db/CLAUDE.md` — references the registry instead of literal URLs.

Frozen exceptions kept on purpose: `exemem-workspace/docs/dogfood/*.md` (historical run reports — immutable records), `schema-infra/cdk/outputs.json` (CDK deploy output, the producer's record), and the `schema-infra/schema_service/...` git submodule (inherits when its pointer is bumped). The `exemem-infra` and `schema-infra` CDKs do not consume the URLs — they create them as outputs — so there's nothing to migrate on the producer side. The lint runs only inside fold_db_node; cross-repo regressions need a workspace-wide lint at `exemem-workspace/` if drift becomes a real problem again.

Release-build defaults: when `EXEMEM_ENV` is unset, debug builds default to dev (us-west-2), release builds default to prod (us-east-1). The Tauri bundle therefore hits prod without needing a runtime env var. Override with `EXEMEM_ENV=dev` or any of the per-URL env vars (`FOLD_SCHEMA_SERVICE_URL`, `EXEMEM_API_URL`, `DISCOVERY_SERVICE_URL`) for ad-hoc testing.

## Schema service

The schema service lives in its own repo, [EdgeVector/schema_service](https://github.com/EdgeVector/schema_service), and is deployed as a Lambda at `schema.folddb.com` via [EdgeVector/schema-infra](https://github.com/EdgeVector/schema-infra). fold_db_node consumes it as a client.

- **Client**: the `schema_service_client` crate (published from the `schema_service` workspace). `fold_db_node::fold_node` re-exports `SchemaServiceClient` for internal use. Integration tests can inject `test://mock` via the `schema_service_url` config.
- **Dep pinning**: `schema_service_core`, `schema_service_client`, and (dev-dep) `schema_service_server_http` are git deps in `Cargo.toml`, pinned by explicit `rev = "<40-hex>"` (Cargo.lock mirrors). The **cross-repo bump-cascade bot** keeps these in lockstep with `schema_service` — and the matching `fold_db` rev (dual-`fold_db` defense) — automatically. The fold_db_node leg runs on a **2-hour schedule** (was `repository_dispatch` per-merge until 2026-05-01; switched to stop polluting the merge queue when upstream bumps came faster than they could land). End-to-end cascade lag is ~10 min schema_service hop + up to 2h for the fold_db_node hop. Workflow file: `.github/workflows/bump-schema-service.yml`. To preempt the bot for a manual bump (e.g. consuming a new fold_db feature whose Rust changes have to land alongside the rev bump), disable the `bump-schema-service` workflow in repo Actions, do the work, re-enable. To force a bump immediately without waiting for the next slot, fire the workflow via `gh workflow run bump-schema-service.yml` or the Actions UI.
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

**Defense (production):** both repos pin fold_db to the **same explicit `rev`**, kept in sync by the bump-cascade bot. When fold_db merges, its `notify-downstream.yml` dispatches to schema_service; `bump-fold-db.yml` opens a PR pinning the new fold_db sha, auto-merges. fold_db_node's `bump-schema-service.yml` then runs **every 2 hours** (cron), picks up the latest schema_service tip and the matching fold_db rev (read from schema_service's Cargo.toml — that's the dual-`fold_db` defense), and opens a PR that auto-merges. End-to-end lag: ~10 min schema_service hop + up to 2h fold_db_node hop. The fold_db_node leg used to be dispatch-driven (per-merge); it was switched to a schedule on 2026-05-01 because per-merge bumps polluted the merge queue when fold_db moved faster than queue admission. Force a bump immediately via `gh workflow run bump-schema-service.yml`.

`scripts/lint-rev-pin-format.sh` (CI-enforced) keeps cascade-relevant git deps as single-line `name = { git = "...", rev = "<40-hex>", ... }` so the bot's `sed` regex can land deterministic edits. Multi-line `[dependencies.fold_db]` blocks fail CI.

To preempt the bot for a manual bump (e.g. consuming a new fold_db feature alongside the rev change): disable `bump-schema-service.yml` in repo Actions, do the work, re-enable. The bot is idempotent — if its target rev already matches, it no-ops.

(Pre-2026-04, this repo used `branch = "mainline"` for fold_db; that worked **only** while mainline HEAD coincidentally matched schema_service's pin. Don't go back to branch tracking — the bot's determinism depends on rev pinning.)

**Local dev:** `cargo build` fetches `fold_db` and `schema_service` crates from GitHub at the revs pinned in `Cargo.toml` (one git fetch the first time after a bump, cached after). Sibling-checkout edits in `../fold_db` or `../schema_service` no longer hot-reload — bump the pinned rev or add an ad-hoc `[patch.crates-io]` table to a local-only branch if you need that loop.

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
