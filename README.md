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
| Schema service client | consumes [`schema_service_client`](https://github.com/EdgeVector/schema_service) crate | Typed client to `schema.folddb.com` (or local dev binary on port 9102) |

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

All endpoints (except `/api/health` and `/api/system/auto-identity`) require an `X-User-Hash` header. In local/desktop mode, fetch yours with `curl http://localhost:9101/api/system/auto-identity` and use the `user_hash` field — same identity the React UI auto-binds to.

For the full machine-readable spec, see `target/openapi.json` (regenerated via `cargo run --bin openapi_dump -- --out target/openapi.json` — don't redirect stderr into it, warnings will corrupt the JSON).

### API examples

#### `POST /api/query` — structured field read

`Query` is a JSON object (Rust source: `fold_db::schema::types::operations::Query`, `#[serde(deny_unknown_fields)]`). Required: `schema_name`, `fields`. Everything else is optional.

```bash
curl -sS -X POST http://localhost:9101/api/query \
  -H "Content-Type: application/json" \
  -H "X-User-Hash: $(curl -sS http://localhost:9101/api/system/auto-identity | jq -r .user_hash)" \
  -d '{
    "schema_name": "Apple Notes",
    "fields": ["title", "body", "modified_at"],
    "sort_order": "desc"
  }'
```

Field reference:

| Field | Type | Notes |
|-------|------|-------|
| `schema_name` | string | Schema to read from. Discover the exact name with `GET /api/schemas` — the LLM classifier picks the name at ingestion time, so e.g. Apple Photos may land in `Photography`. Task `67b1a` will echo the chosen name back on the ingestion result. |
| `fields` | string[] | Field names to return. Use `GET /api/schemas` to list fields. |
| `sort_order` | `"asc"` \| `"desc"` | Sorts by the schema's range key. String, not an array. Omit to leave order unspecified. |
| `filter` | object | Key-level filter for HashRange schemas only — externally-tagged `HashRangeFilter` enum. Examples: `{"HashKey": "<hash>"}`, `{"RangePrefix": "2026-"}`, `{"RangeRange": {"start": "a", "end": "z"}}`. For text-substring matching on values, use `/api/native-index/search` instead. |
| `value_filters` | array | Post-fetch numeric filters, AND'd together. Each entry is a single-key map: `{"GreaterThan": {"field": "score", "value": 0.5}}`, `{"LessThan": ...}`, `{"Equals": ...}`, `{"Between": {"field": "x", "min": 0.0, "max": 1.0}}`. |
| `as_of` | string (RFC 3339) | Time-travel read. Omit or `null` for current. |
| `rehydrate_depth` | integer | Reference-following depth. Omit or `null` for default. |

Unknown fields error loudly — `{"type":"list_schemas"}` returns `unknown field 'type'`. Use `GET /api/schemas` to list schemas.

#### `GET /api/native-index/search` — full-text keyword search

```bash
curl -sS -G "http://localhost:9101/api/native-index/search" \
  --data-urlencode "term=quantum mechanics" \
  -H "X-User-Hash: $(curl -sS http://localhost:9101/api/system/auto-identity | jq -r .user_hash)"
```

Query params:

| Param | Required | Notes |
|-------|----------|-------|
| `term` | yes | Search term. Note: `term`, not `q`. Empty/whitespace returns 400. |
| `include_internal` | no | `true`/`1`/`yes` to include bookkeeping schemas (`Mention`, `ExtractionStatus`, `IngestionError`, etc.). Defaults to `false`. |

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
