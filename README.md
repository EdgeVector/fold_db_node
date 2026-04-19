# fold_db_node

The app/node layer for FoldDB — HTTP server, CLI, React UI, ingestion pipeline, fingerprints, and sharing. Sits on top of the `fold_db` core library.

## What's Here

| Component | Location | Description |
|-----------|----------|-------------|
| HTTP server | `src/server/` | Actix-web server on port 9101 (dev) / 9001 (prod) |
| React UI | `src/server/static-react/` | Tab-based dashboard (schemas, queries, people, settings) |
| CLI (`folddb`) | `src/bin/folddb/` | Query, mutate, ingest, search, ask |
| Ingestion pipeline | `src/ingestion/` | AI-powered file → schema → mutation |
| Fingerprints | `src/fingerprints/` | Personas, Identities, face detection, sharing |
| Schema service | `src/bin/schema_service.rs` | Local schema registry (port 9102 in dev) |

## Key API Endpoints

| Method | Path | Description |
|--------|------|-------------|
| POST | `/api/query` | Structured field query |
| POST | `/api/mutation` | Create/update records |
| POST | `/api/schemas` | Register a schema |
| POST | `/api/fingerprints/detect-faces` | Pure face detection (no persona writes) |
| GET/DELETE | `/api/fingerprints/personas` | List, filter, sort personas |
| GET | `/api/fingerprints/received-cards` | Poll received Identity Cards inbox |
| POST | `/api/fingerprints/identity-card/send` | Send Identity Card to a contact |
| GET | `/api/native-index/search` | Full-text keyword search |
| POST | `/api/ingestion/process` | Ingest a file via AI pipeline |

## Local Development

```bash
./run.sh --local --local-schema    # Fully offline (recommended)
./run.sh --local                   # Local storage + prod schema service
./run.sh --local --empty-db        # Fresh database
```

- Backend auto-picks port 9101–9199 (parallel agent safe)
- Vite frontend auto-picks port 5173–5199
- Check `~/.folddb-slots/*.json` for active ports

## CI

CI runs automatically on pushes to `main` and on pull requests. Three jobs run in parallel:

| Job | Trigger | What it checks |
|-----|---------|---------------|
| **Rust Tests** | `Cargo.toml` exists | clippy, AWS backend compilation, cargo test, integration tests |
| **Frontend Tests** | `src/server/static-react/package.json` exists | vitest unit tests |
| **E2E UI Tests** | `src/server/static-react/e2e/` exists | Playwright browser tests |

### Pre-PR Checklist

```bash
cargo clippy --workspace --all-targets -- -D warnings
cargo check --workspace --features aws-backend
cargo test --workspace --all-targets

cd src/server/static-react/
npm test
npm run test:e2e
```

## Feature Flags

| Flag | Effect |
|------|--------|
| `os-keychain` | Encrypt node identity + E2E key at rest via OS keychain. Enabled in Tauri release builds. |
| `aws-backend` | Enable DynamoDB backend (inherited from fold_db). |

## QA

A self-contained UI smoke-test harness ships at `scripts/qa-harness.sh`. Starts an isolated dev stack (own backend, schema, Vite) and runs a structured QA session. See `/qa-folddb` skill.
