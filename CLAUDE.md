# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**fold_db_node** is a node implementation for the FoldDB distributed database network. This repo follows the same architecture and conventions as [fold_db](https://github.com/shiba4life/fold_db).

## CI Pipeline

CI triggers on push to `main` and on pull requests. Three jobs run in parallel, each skipping if its code doesn't exist yet:

- **Rust Tests**: Requires `Cargo.toml`. Runs clippy (`-D warnings`), `cargo check --features aws-backend`, `cargo test`, and integration tests (if API keys are configured).
- **Frontend Tests**: Requires `src/server/static-react/package.json`. Runs `npm test` (vitest).
- **E2E UI Tests**: Requires `src/server/static-react/e2e/` directory. Runs Playwright browser tests.

### Pre-PR Checklist

Before every push, first fetch and rebase on the latest base branch:
```bash
git fetch origin
git rebase origin/<base-branch>   # e.g. origin/main
```

Then run CI checks:
```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --features aws-backend
cargo test --workspace --all-targets
```

After creating the PR, enable auto-merge so it merges itself once CI passes:
```bash
gh pr merge --auto --squash <PR_URL>
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

## Coding Standards

Follow the same standards as fold_db:
- No silent failures — throw errors if anything goes wrong
- No branching logic where avoidable
- No inline crate imports — import in headers only
- No fallbacks — they hide broken code
- Always write tests
- Use `TODO` format for incomplete implementations
