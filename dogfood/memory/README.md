# Memory-agent dogfood harness

Tight iteration loop for building the memory consolidation agent described in
[`docs/design/memory_agent.md`](../../../docs/design/memory_agent.md).

## What it does

Boots an isolated `fold_db_server` + `schema_service` on dedicated ports
(default `19700` + `19701`), registers the `Memory` schema via
`POST /api/memory/register`, seeds a fixture set of ~12 memories with labeled
expected clusters, and exposes CLI commands for adding, listing, searching,
and scoring clusters.

Everything lives under `/tmp/folddb-memory-dogfood/` by default — stays out of
the way of your real dev stack. Reset wipes the whole dir.

## Quickstart

```bash
cd dogfood/memory
./dogfood-memory.sh start

# see what's there
./dogfood-memory.sh list

# semantic search
./dogfood-memory.sh search "rebase before pushing"
./dogfood-memory.sh search "schema deduplication"

# scoring: how well does raw semantic retrieval surface the labeled clusters?
./dogfood-memory.sh eval

# add your own
./dogfood-memory.sh add "The consolidation agent should be a TransformView, not a Rust agent." reference

# wipe + re-seed
./dogfood-memory.sh reset
```

## Commands

| Command | Purpose |
|---|---|
| `start` | Build binaries, boot node + schema service, register Memory schema, seed fixtures |
| `stop` | Kill processes; keep data dir |
| `reset` | Stop, wipe data dir, start, seed |
| `status` | Is it running? which ports? canonical schema name |
| `seed` | Re-seed fixtures (idempotent-ish; duplicates existing memory ids) |
| `add <body> [kind]` | Write a memory with a random `mem_*` id |
| `list` | Dump all memories on the Memory schema |
| `search <query>` | Semantic search via native-index; filter to Memory hits only |
| `get <memory_id>` | Fetch a single memory by id |
| `clusters` | (Phase 1a placeholder) will query the `TopicClusters` view when registered |
| `eval` | Score: for each labeled cluster in fixtures, how many cluster-mates surface in top-10 semantic search? |
| `logs [node\|schema]` | Tail the log for a service |

## Env overrides

- `FOLDDB_DOGFOOD_DIR` — data + log root (default `/tmp/folddb-memory-dogfood`)
- `FOLDDB_NODE_PORT` — fold_db_server port (default `19700`)
- `FOLDDB_SCHEMA_PORT` — schema service port (default `19701`)

## Fixtures

[`fixtures.json`](fixtures.json) contains three labeled semantic clusters and
some noise. The labels drive `eval`'s scoring and will drive Phase 1a's
cluster-correctness test too:

| Cluster | Expected members |
|---|---|
| `deploy_policy` | rebase-before-push + auto-merge --squash + post-merge-trash-and-switchback |
| `schema_service` | dedup via embedding similarity + bidirectional best-match + schema expansion |
| `transforms` | multi-query/multi-row WASM + reactive cache + WASM views not writable |
| `noise` | hiking trail, espresso descaling, async-vs-sync review preferences |

Why these three? They all show up in the current memory-agent design work, so
the harness output is legible when you read it.

## Phase 1a extension

Once Phase 1a lands (`register_memory_consolidation_views` + `TopicClusters`
TransformView + `union_find_cluster.wasm`), this harness will:

1. Call a new `POST /api/memory/views/register` endpoint on `start`
2. Light up `./dogfood-memory.sh clusters` to pretty-print the cached view output
3. Add an `eval-clusters` mode that compares the view's emitted clusters against
   the fixture labels (precision / recall / F1 per expected cluster)

## What this harness is NOT

- Not a benchmark. Not stable enough to compare across machines.
- Not a QA test. The companion to this is `tests/memory_roundtrip_test.rs`
  (integration test) and (soon) the Phase 1a cluster-correctness test.
- Not hermetic across invocations — uses a persistent data dir to speed up
  iteration. Run `reset` for a clean slate.
