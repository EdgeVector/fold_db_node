# Egress classification notes (fold_db_node)

Phase 2 / T4 sister to fold_db's [PR #636](https://github.com/EdgeVector/fold_db/pull/636).
Companion to `scripts/lint-tracing-egress.sh` and the `// trace-egress: <class>`
comment regime. One-stop reference for the awkward / shared-client cases the
classifier comments alone don't fully convey.

Classes:

- `propagate` — call goes to one of our own services (Exemem auth Lambda, schema
  service, discovery service, the local node's own HTTP API over loopback).
  Eventual `.send()` callers should be wrapped with
  `observability::propagation::inject_w3c`.
- `loopback` — the same as `propagate` but specifically scoped to localhost
  loopback (CLI / MCP client → daemon, daemon health probes). Wrap.
- `skip-s3` — presigned-URL S3 calls. **DO NOT** wrap; injecting a `traceparent`
  header changes the canonical request and breaks the SigV4 signature.
- `skip-3p` — third-party (Brave Search, Ollama, GitHub Releases, arbitrary
  user-supplied URLs) that doesn't honour W3C trace context. Don't wrap.

## Deferred: `inject_w3c` wrapping

This sweep classifies all 20 `reqwest::Client` constructions in `src/` and
wires the lint script into CI, but does **not** add `observability::propagation::inject_w3c`
calls. Reason: `fold_db_node` pins `fold_db` at rev `a0434b25...`, which
predates the `crates/observability` workspace member (added in
[fold_db#630](https://github.com/EdgeVector/fold_db/pull/630)). Bumping the
rev to one that contains observability requires a lockstep bump of
`schema_service`'s fold_db rev too, per the dual-fold_db invariant documented
in [CLAUDE.md](../../CLAUDE.md#local-dev-gotchas-read-if-cargo-check-explodes).
That coordination is its own task.

`propagate` and `loopback` sites carry the suffix
"`inject_w3c` wrapping deferred — pending fold_db rev bump" in their classifier
comment so the follow-up sweep can grep for them. The follow-up:

1. Land a coordinated fold_db rev bump in `schema_service` and `fold_db_node`
   (lockstep — see CLAUDE.md).
2. Add `observability = { git = "...", rev = "<new-rev>", package = "observability" }`
   to `fold_db_node/Cargo.toml`.
3. For each propagate/loopback site flagged "wrapping deferred", wrap the
   eventual `.send()` call chain with `observability::propagation::inject_w3c`
   and drop the suffix from the classifier.

`skip-s3` / `skip-3p` sites are documentation-only — no wrapping was ever
intended, and they need no follow-up.

## fold_db_node sweep — Phase 2 / T4

### Shared `Arc<reqwest::Client>` between `propagate` (auth Lambda) and `skip-s3`

Three production sites construct an `Arc<reqwest::Client>` that flows into
BOTH `AuthClient` (propagate) and `S3Client` (skip-s3):

- `src/handlers/auth.rs:1258` — `bootstrap_from_cloud`
- `src/handlers/org.rs:494` — `shared_http_client()` LazyLock
- `src/fold_node/operation_processor/admin_ops.rs:406` — `setup_cloud_sync`

Each is classified `propagate` (the active class — the one that requires
wrapping). Once the rev bump lands, wrapping happens inside fold_db's
`AuthClient::post` (already wired in [fold_db#636](https://github.com/EdgeVector/fold_db/pull/636))
which calls `inject_w3c`. `S3Client::{upload,download,delete}` deliberately
does NOT wrap to preserve the SigV4 signature.

If this gets confusing, the cleanup path is to split into two distinct
`Arc<reqwest::Client>` — one classified `propagate` for `AuthClient`, one
classified `skip-s3` for `S3Client`. Today they share a connection pool, which
is desirable; the structural split would only be cosmetic. Same reasoning as
the fold_db sweep.

### Detached AuthClient/S3Client construction in `admin_ops.rs`

`src/fold_node/operation_processor/admin_ops.rs` later builds two more
short-lived clients (lines 472, 485) — one purely for `AuthClient` (propagate;
will inherit wrapping from `AuthClient::post` once observability is wired) and
one purely for `S3Client` (skip-s3; must not be wrapped). Each is classified
individually.

### `discovery/publisher.rs` builder + fallback

The `DiscoveryPublisher::new` constructor at `src/discovery/publisher.rs:117`
uses a `reqwest::Client::builder()...build().unwrap_or_else(|_| reqwest::Client::new())`
fallback. Both branches target the discovery service, so both are classified
`propagate`. Two classifier comments — one above the builder, one above the
fallback — keep the lint script's 3-line window happy without restructuring
the expression.

`DiscoveryPublisher` has 10 outgoing `.send()` call sites (publish, opt-out,
search, connect, poll_messages, browse_categories, get_public_key,
poll_requests, store_trust_invite, fetch_trust_invite). The deferred wrapping
follow-up will likely want a small helper (`fn wrapped_request(&self, ...)
-> reqwest::RequestBuilder`) to avoid 10× `inject_w3c(...)` boilerplate.

### Loopback CLI clients

Four sites talk to the local daemon over `http://127.0.0.1:<port>`:

- `src/bin/folddb/client.rs:21` — `FoldDbClient` for the CLI data commands
- `src/bin/folddb_mcp/client.rs:27` — MCP server's daemon client
- `src/bin/folddb/main.rs:1129` — `fetch_pubkey_from_daemon`
- `src/bin/folddb/commands/daemon.rs:54` — `check_daemon_health`

They're classified `loopback` and slated for wrapping (the `traceparent`
header helps stitch CLI spans onto daemon-side spans even though the hop is
local).

### Third-party APIs (no wrapping, no follow-up)

- Brave Search (`src/fold_node/llm_query/service/web_tools.rs:36`) — `skip-3p`
- Arbitrary user URLs (`src/fold_node/llm_query/service/web_tools.rs:95`) — `skip-3p`
- Ollama `/api/tags` (`src/ingestion/helpers.rs:317`) — `skip-3p`
- GitHub Releases (`src/bin/folddb/update_check.rs:31`) — `skip-3p`

W3C `traceparent` is harmless to send to most third parties (they ignore it),
but the policy is to keep outbound traffic to vendors clean. The classifier
serves as documentation, not behaviour.

## Cross-repo consistency

This sweep follows the conventions established in
[fold_db PR #636](https://github.com/EdgeVector/fold_db/pull/636):

- One classifier comment per construction, on one of the 3 lines immediately
  preceding `reqwest::(Client|ClientBuilder)::(new|default|builder)()`.
- For `propagate` / `loopback` clients with multiple call sites, prefer wrapping
  inside the lowest-level helper that builds the request (e.g. `FoldDbClient::get`
  / `post`, the typed methods on `DiscoveryPublisher`) so adding a new endpoint
  method doesn't require remembering to wrap.
- For shared-client awkwardness (the `propagate` + `skip-s3` case), classify
  on the active class and document the dual use here rather than splitting
  clients prematurely.
