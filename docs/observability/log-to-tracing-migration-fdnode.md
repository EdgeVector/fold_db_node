# Phase 3 T2 — fold_db_node `log` → `tracing` migration

## Scope

Mechanical rewrite of every `log::{info,warn,error,debug,trace}!` call site, every
`use log::*` import, and every `log::log_enabled!` gate in `fold_db_node/src/` to
their `tracing` equivalents. Sibling task to P3-T2-folddb (fold_db core) and
P3-T2-schema-service (schema_service). Removing the `log` crate from `Cargo.toml`
is **out of scope** here — that lands in Phase 3 T7 once all three repos and
their transitive deps are cut over.

## What changed

- `find src -name '*.rs' | sed -i '' s/log::<level>!/tracing::<level>!/` for the
  five level macros. 253 call sites across 62 files.
- `src/ingestion/ingestion_service/mod.rs:743` — the lone `log::log_enabled!`
  gate became `tracing::enabled!(tracing::Level::DEBUG)`. Different level enum
  shape (`Level::Debug` → `Level::DEBUG`), verified by `cargo check`.
- No `use log::` imports existed in `src/` prior to this PR (every call site was
  already fully qualified `log::*!`), so step 2 of the playbook was a no-op.

## What did **not** change

- Cargo metadata: `log` stays in `Cargo.toml` because git deps (fold_db,
  schema_service_*) still emit `log` records via their own crates until their
  Phase 3 cutovers land. Removing the dep here would cause linker fallout once
  those crates are pulled in. Phase 3 T7 sweeps the dep after the upstream
  cutovers settle.
- `Cargo.lock` is unchanged. Local verification was run with `.cargo/config.toml`
  temporarily moved aside so cargo could resolve the git pins (sibling
  `../fold_db`, `../schema_service` worktrees aren't present in the
  `.cline/worktrees/4ae07/` slot). The lockfile diff that produced was reverted
  before commit; CI removes `.cargo/config.toml` and exercises the same git
  pin path.
- Behaviour: `tracing` macros forward to the `log` facade when no
  `tracing-subscriber` is installed, and vice versa via `tracing-log`, so output
  is unchanged for any consumer that hasn't yet wired up a `tracing` subscriber.

## Verification

Run from this worktree with `.cargo/config.toml` moved aside (sibling repos not
checked out alongside this worktree):

- `cargo fmt --all` — clean.
- `cargo clippy --workspace --all-targets -- -D warnings` — clean.
- `cargo build --workspace` — clean.
- `cargo test --workspace --all-targets` — all suites pass (unit + integration,
  ~900+ tests across the workspace).
- `bash scripts/lint-tracing-egress.sh` — `ok — all 20 reqwest construction
  sites in src/ are classified.`

CI repeats this with the canonical git-pinned deps (it removes
`.cargo/config.toml` per `ci-tests.yml`).

## Follow-ups

- **Phase 3 T7**: drop `log` from `Cargo.toml` once fold_db and schema_service
  have published their Phase 3 cutovers and the dep is pinned no transitively.
- The `log_feature!` macro is re-exported from `fold_db` and used in three
  files here (`fold_node/node.rs`, `fold_node/config.rs`,
  `fold_node/operation_processor/mutation_ops.rs`). It's untouched — its
  facade gets fixed when fold_db's Phase 3 T2 lands.
