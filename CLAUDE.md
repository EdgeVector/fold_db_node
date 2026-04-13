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

## Binaries

### `schema_service`
Standalone HTTP server for schema registry. Single source of truth for schema creation across FoldDB nodes.

- Source: `src/bin/schema_service.rs`, implementation in `src/schema_service/`
- Default port: 9002 (`DEFAULT_SCHEMA_SERVICE_PORT`)
- Storage: Sled at `--db-path` (default: `schema_registry`)
- Run: `cargo run --bin schema_service -- --port 9002 --db-path schema_registry`
- Used by `fold_db_node` HTTP server via `schema_service_url` config (use `test://mock` in integration tests)

## Local Development

Always use `run.sh` to start the dev server — never start binaries manually:
```bash
./run.sh --local --local-schema    # Fully offline development (preferred)
./run.sh --local                   # Local storage with prod schema service
./run.sh --local --empty-db        # Local with fresh database
```

The script handles process cleanup, building, schema service startup, and frontend (Vite on :5173).
- Backend: http://localhost:9001
- Schema service: http://localhost:9002
- UI: http://localhost:5173

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
