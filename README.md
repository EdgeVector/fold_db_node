# fold_db_node

A node implementation for the FoldDB distributed database network.

## CI

CI runs automatically on pushes to `main` and on pull requests. The pipeline has three jobs:

| Job | Trigger | What it checks |
|-----|---------|---------------|
| **Rust Tests** | `Cargo.toml` exists | clippy, AWS backend compilation, cargo test, integration tests |
| **Frontend Tests** | `src/server/static-react/package.json` exists | vitest unit tests |
| **E2E UI Tests** | `src/server/static-react/e2e/` exists | Playwright browser tests |

Each job skips gracefully if its code hasn't been added yet. Once you add the corresponding files, the job activates automatically — no workflow changes needed.
