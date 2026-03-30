# Plan: Extract fold_db_node from fold_db (Incremental Commits)

## Context

FoldDB is being open-sourced as a database protocol. The core database library (`fold_db`) stays open source. All application-layer code (server, ingestion, frontend, CLI, handlers) moves to `fold_db_node` (proprietary). Each step is a separate commit with tests passing.

## Two repos involved
- **fold_db** — branch `mainline`, remote `origin` (github.com/EdgeVector/fold_db.git)
- **fold_db_node** — branch `main`, remote `origin` (github.com/EdgeVector/fold_db_node.git)

Changes alternate between repos. Each commit must leave that repo's tests passing.

---

## Commits

### Commit 1 — fold_db: Make FoldDB struct fields public
**Repo:** `fold_db/`
**Why:** fold_db_node needs to access FoldDB internals (schema_manager, db_ops, etc.). Currently `pub(crate)`.

- `src/fold_db_core/fold_db.rs`: Change all `pub(crate)` fields to `pub` on FoldDB struct (8 fields)
- `src/fold_db_core/fold_db.rs`: Make `initialize_from_db_ops` method `pub` instead of `pub(crate)`
- **Test:** `cargo test --lib` (151 tests pass — no behavior change)

### Commit 2 — fold_db: Feature-gate clap on MutationType
**Repo:** `fold_db/`
**Why:** clap is an app-layer dep that should be optional in the core library.

- `Cargo.toml`: Add `cli = ["dep:clap", "dep:clap_complete"]` feature; make clap + clap_complete optional
- `src/schema/types/operations.rs`: `use clap::ValueEnum` → `#[cfg(feature = "cli")] use clap::ValueEnum`; derive → `#[cfg_attr(feature = "cli", derive(ValueEnum))]`
- **Test:** `cargo test --lib` (151 tests pass)

### Commit 3 — fold_db: Remove NodeConfig from testing_utils
**Repo:** `fold_db/`
**Why:** `testing_utils::create_test_node_config()` references `fold_node::config::NodeConfig` which will move. Remove it now so fold_db stays clean when we later strip app modules.

- `src/testing_utils.rs`: Remove `create_test_node_config()` function
- **Test:** `cargo test --lib` (151 tests pass)

### Commit 4 — fold_db_node: Create scaffold with Cargo.toml and empty lib
**Repo:** `fold_db_node/`
**Why:** Set up the new crate structure before copying any code.

- Create `Cargo.toml` with:
  - `fold_db = { path = "../fold_db", features = ["cli"] }` dependency
  - All app-layer deps: actix-web, actix-multipart, file_to_json, clap, etc.
  - `sled` (used directly by schema_service)
  - Feature flags: `aws-backend = ["fold_db/aws-backend"]`, `ts-bindings = ["fold_db/ts-bindings"]`
  - Binary declarations (all 5: folddb_server, folddb, schema_service, openapi_dump, ensure_identity)
- Create `src/lib.rs` with module declarations (commented out initially so it compiles)
- **Test:** `cargo check --lib` (empty lib compiles)

### Commit 5 — fold_db_node: Copy utils module
**Repo:** `fold_db_node/`
**Why:** `utils/http_errors.rs` uses actix-web, no deps on other app modules — safe leaf to move first.

- Copy `fold_db/src/utils/` → `fold_db_node/src/utils/`
- Rewrite imports: `use crate::<core_mod>::` → `use fold_db::<core_mod>::`
- Uncomment `pub mod utils;` in lib.rs
- **Test:** `cargo check --lib`

### Commit 6 — fold_db_node: Copy fold_node module
**Repo:** `fold_db_node/`
**Why:** FoldNode is the main orchestrator. Depends only on core modules.

- Copy `fold_db/src/fold_node/` → `fold_db_node/src/fold_node/`
- Rewrite imports: `use crate::<core_mod>::` → `use fold_db::<core_mod>::` for all core modules
- Keep `use crate::server::` refs (server will arrive later — temporarily comment out or `#[cfg(FALSE)]` the embedded server re-export in mod.rs)
- Uncomment `pub mod fold_node;` and re-exports in lib.rs
- **Test:** `cargo check --lib`

### Commit 7 — fold_db_node: Copy handlers module
**Repo:** `fold_db_node/`
**Why:** Framework-agnostic handlers depend on fold_node (now in same crate) and core.

- Copy `fold_db/src/handlers/` → `fold_db_node/src/handlers/`
- Rewrite core imports to `fold_db::`, keep `crate::fold_node` and `crate::ingestion` refs
- Uncomment `pub mod handlers;` in lib.rs
- **Test:** `cargo check --lib`

### Commit 8 — fold_db_node: Copy ingestion module
**Repo:** `fold_db_node/`
**Why:** Ingestion depends on fold_node, handlers, and core.

- Copy `fold_db/src/ingestion/` → `fold_db_node/src/ingestion/`
- Rewrite core imports, keep intra-app `crate::` refs
- Uncomment `pub mod ingestion;` and `IngestionConfig` re-export in lib.rs
- **Test:** `cargo check --lib`

### Commit 9 — fold_db_node: Copy server module
**Repo:** `fold_db_node/`
**Why:** Server depends on all other app modules. Includes React frontend.

- Copy `fold_db/src/server/` → `fold_db_node/src/server/` (including `static-react/`)
- Rewrite core imports, keep intra-app refs
- Fix fold_node/mod.rs embedded server re-export (uncomment now that server module exists)
- Fix doc comments referencing `fold_db::server::` → `fold_db_node::server::`
- **Test:** `cargo check --lib`

### Commit 10 — fold_db_node: Copy schema_service module
**Repo:** `fold_db_node/`
**Why:** Schema service depends on core + uses sled directly.

- Copy `fold_db/src/schema_service/` → `fold_db_node/src/schema_service/`
- Rewrite core imports
- Fix test code referencing `fold_db::schema_service::` → `crate::schema_service::`
- Uncomment `pub mod schema_service;` in lib.rs
- **Test:** `cargo check --lib`

### Commit 11 — fold_db_node: Copy all binaries
**Repo:** `fold_db_node/`
**Why:** All 5 binaries reference app-layer modules that are now in fold_db_node.

- Copy `fold_db/src/bin/` → `fold_db_node/src/bin/`
- Fix imports in each binary:
  - Core types (`fold_db::storage::`, `fold_db::error::`, etc.) stay as `fold_db::`
  - App types (`fold_db::fold_node::`, `fold_db::server::`) → `fold_db_node::`
  - folddb CLI binary's internal `crate::error::CliError` stays as `crate::`
  - folddb_server.rs multi-line import block needs manual split
- **Test:** `cargo check` (all binaries compile)

### Commit 12 — fold_db_node: Copy integration tests, scripts, config
**Repo:** `fold_db_node/`
**Why:** Tests that use FoldNode/ingestion/handlers belong in fold_db_node.

- Copy `tests/common/`, `tests/fixtures/`, `tests/schemas_for_testing/` from fold_db
- Copy 17 app-layer test files (all except atom_deduplication, dynamodb_mock, field_mapper_approval, repro_schema_error)
- Rewrite test imports: `fold_db::fold_node` → `fold_db_node::fold_node`, etc.
- Copy scripts: `run.sh`, `run_dev.sh`, `run_tauri_dev.sh`, `build_macos_app.sh`, `install.sh`, `test_rehydration.sh`
- Copy `config/`, `sample_data/`, `examples/`, `data/` directories
- **Test:** `cargo test --lib` (220 tests pass)

### Commit 13 — fold_db: Remove app-layer modules and trim deps
**Repo:** `fold_db/`
**Why:** Strip fold_db down to the open-source core library.

- `src/lib.rs`: Remove `pub mod` for fold_node, handlers, ingestion, server, schema_service, utils; remove app re-exports (FoldNode, NodeConfig, IngestionConfig, load_node_config)
- Delete directories: `src/fold_node/`, `src/handlers/`, `src/ingestion/`, `src/server/`, `src/schema_service/`, `src/utils/`, `src/bin/`
- `Cargo.toml`: Remove all `[[bin]]` sections; remove app deps (actix-web, actix-http, actix-cors, actix-multipart, file_to_json, csv, mime_guess, kamadak-exif, rust-embed, dirs, indicatif, comfy-table, dialoguer, console)
- Remove app-layer integration tests (17 files); keep 4 core-only tests
- **Test:** `cargo test --lib` (151 tests pass), `cargo check --tests` (integration tests compile)

---

## Key Edge Cases (learned from first attempt)

| Issue | Resolution |
|-------|------------|
| `clap::ValueEnum` on `MutationType` | Feature-gate behind `cli` feature; fold_db_node enables it |
| FoldDB struct fields `pub(crate)` | Make them `pub` — this IS the public API for node builders |
| `testing_utils::create_test_node_config` | Remove from fold_db (references NodeConfig); fold_db_node tests create their own configs |
| `log_feature!` macro | Works cross-crate — just delegates to `log::$level!()` |
| `utils/http_errors.rs` uses actix-web | Moves entirely to fold_db_node |
| `fold_node/mod.rs` re-exports `server::start_embedded_server` | Both in fold_db_node — stays as `use crate::server::` |
| Schema_service uses `sled` directly | Add `sled` as dep of fold_db_node |
| Binary `crate::error::CliError` | folddb CLI has its own `error.rs` — `crate::` is correct, don't rewrite to `fold_db::` |
| folddb_server.rs mixed import block | Must manually split `use fold_db::{constants::..., fold_node::..., server::...}` into separate imports |
| `utoipa::ToSchema` on core types | Keep utoipa in fold_db — just derive macros |

## Verification (after all commits)

1. `cd fold_db && cargo build --lib` — core compiles with no app deps
2. `cd fold_db && cargo test --lib` — 151 core tests pass
3. `cd fold_db_node && cargo build` — node + all binaries compile
4. `cd fold_db_node && cargo test --lib` — 220 app tests pass
