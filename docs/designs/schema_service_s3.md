# Schema Service: S3-Backed Storage

**Status:** Proposed
**Owner:** @tom
**Created:** 2026-04-14
**Supersedes:** Sled-on-EFS approach from PR #6 (schema-infra)

## Summary

Replace the schema service's Sled-on-EFS storage backend with **four JSON
blobs in S3**, one per domain (`schemas`, `canonical_fields`, `views`,
`transforms`), plus an existing `wasm/{hash}.wasm` prefix for WASM bytes.
Writes use `PutObject` with `If-Match: {etag}` for optimistic concurrency;
reads load the full blob into Lambda memory at cold start and serve
subsequent requests from an in-memory cache. Semantic dedup (cosine
similarity over embeddings) stays in-memory via a `SemanticIndex` trait.

## Background

The schema service is the global, content-addressed registry for:

1. **Schemas** — immutable schema docs keyed by `sha256(normalized_doc)`
2. **Canonical fields** — the controlled vocabulary of field names with
   classification + interest category + embeddings
3. **Views** — computed schemas backed by transforms
4. **Transforms** — WASM transform metadata, NMI matrices, classifications
5. **Transform WASM bytes** — the raw WASM blobs (too large for most
   row-store backends)

Every fold_db_node instance fetches schemas from this service at startup.
Every schema proposal goes through it for similarity / dedup / expansion.
It is a read-heavy, write-rare, append-only registry.

### Why the previous design failed

PR #6 ported the Lambda from DynamoDB to Sled on `/mnt/schema`, expecting
an EFS mount. The CDK stack was never updated to provision EFS or put the
Lambda in a VPC, so at cold start `sled::open("/mnt/schema")` failed with
`Read-only file system (os error 30)` and `/health` returned HTTP 500.
Deploy-time fix was to point `SCHEMA_DB_PATH` at `/tmp/schema`
(ephemeral) as a stopgap — see PR #7 in `shiba4life/schema-infra`.

`/tmp` is not a viable long-term answer because user-submitted schemas
are lost on every Lambda cold start. We need a persistent,
multi-instance-writable backend.

## Goals

1. **Persistent** across Lambda cold starts and deploys
2. **Append-only** — content-addressed, nothing is ever deleted
3. **Atomic** dedup on content hash for concurrent writers
4. **Supersession** (updating an old schema's `superseded_by` pointer
   while adding the new schema) must be atomic
5. **Fast cold start** — bulk load dominates startup latency
6. **Cheap** — alpha cost should be well under $10/month
7. **No VPC, no EFS, no filesystem** in the Lambda path
8. **Trait-gated backend** so the self-hosted `schema_service` binary
   can keep using `SledSchemaStore` on a local filesystem

## Non-goals

- Sub-10ms point lookups on uncached keys (we cache everything at startup)
- Multi-region active-active replication (single-region is fine for alpha)
- Fine-grained per-item IAM (schema registry is global)
- Write throughput beyond ~10 writes/sec per domain
  (we're at <0.01/sec at alpha)

## Current state

```
┌─────────────────────────────────────────┐
│  Lambda (schema_service)                │
│  - Reads/writes Sled at /mnt/schema     │
│  - Expects EFS mount (not provisioned)  │
│  - Stopgap: /tmp/schema (ephemeral)     │
└─────────────────────────────────────────┘
```

Storage is broken for anything that isn't re-seeded deterministically on
every cold start. Phase 1 built-in schemas survive because `seed()` runs
at cold start. User-submitted schemas do not.

## Proposed design

### Storage layout

```
s3://schema-service-{env}/
├── schemas.json                 # all schemas, keyed by identity_hash
├── canonical_fields.json        # all canonical fields, keyed by name
├── views.json                   # all views, keyed by view name
├── transforms.json              # all transform metadata, keyed by wasm_hash
└── wasm/
    ├── {wasm_hash_1}.wasm       # content-addressed WASM bytes
    ├── {wasm_hash_2}.wasm
    └── ...
```

Each domain blob is a JSON document shaped like:

```json
{
  "version": 1,
  "items": {
    "{key}": { /* item body */ },
    "{key}": { /* item body */ }
  }
}
```

The `version` field is reserved for future schema migrations of the blob
format itself (not of the items inside it).

### System diagram

```
┌────────────────────────────────────────────────────────────────┐
│                      Lambda containers (N)                     │
│                                                                │
│   ┌──────────────┐  ┌──────────────┐  ┌──────────────┐        │
│   │  Lambda A    │  │  Lambda B    │  │  Lambda C    │        │
│   │              │  │              │  │              │        │
│   │  ┌────────┐  │  │  ┌────────┐  │  │  ┌────────┐  │        │
│   │  │ cache  │  │  │  │ cache  │  │  │  │ cache  │  │        │
│   │  │ (RO    │  │  │  │ (RO    │  │  │  │ (RO    │  │        │
│   │  │  proj) │  │  │  │  proj) │  │  │  │  proj) │  │        │
│   │  └────────┘  │  │  └────────┘  │  │  └────────┘  │        │
│   └──────┬───────┘  └──────┬───────┘  └──────┬───────┘        │
│          │                 │                 │                │
│          └─────────────────┼─────────────────┘                │
│                            │                                   │
│                            │ reads (cold start)                │
│                            │ writes (every write)              │
└────────────────────────────┼───────────────────────────────────┘
                             ▼
                ┌─────────────────────────┐
                │  S3 (source of truth)   │
                │                         │
                │  ├ schemas.json         │
                │  ├ canonical_fields.json│
                │  ├ views.json           │
                │  ├ transforms.json      │
                │  └ wasm/{hash}.wasm     │
                └─────────────────────────┘
```

### Read path

**Cold start (Lambda init or first request after container recycle):**

```
1. Spawn 4 parallel GetObject calls:
     schemas.json
     canonical_fields.json
     views.json
     transforms.json
2. Parse each JSON into HashMap<key, item>
3. Build SemanticIndex in-memory:
     - descriptive_name embeddings → schemas lookup
     - canonical_field description embeddings → canonical_field lookup
4. Cache becomes ready to serve reads
```

Expected cold-start cost at alpha scale:
- 4 parallel GETs, each <100ms for blobs under 1MB
- JSON parse: ~50ms per blob
- Embedding unpack: negligible (embeddings stored base64 in each item)
- **Total: ~200–400ms added to Lambda cold start**

**Warm-instance reads** (everything after the first request):
- Served from the in-memory HashMap — zero S3 reads, <1ms latency
- Content-addressed data is immutable, so cached values are always correct

**Cache invariant (the core rule):**

> The cache is an append-only read projection of S3. Cache hits are
> always correct because data is immutable. Writes never consult the
> cache. Every write is a `PutObject` with an ETag precondition against
> S3, and S3 is the sole arbiter of "who won."

This means a cache can be *incomplete* (missing items written by other
Lambdas) but never *wrong*. Incompleteness is self-healing — a duplicate
semantic match is caught on write via S3 conditional put, or by the
weekly reconciliation job (future TODO).

### Write path

Every write follows read-modify-write with ETag-based optimistic
concurrency:

```
fn add_to_domain<T>(
    domain: &str,              // e.g. "canonical_fields"
    key: &str,                 // e.g. "user_email"
    item: T,                   // the new item
) -> Result<AddOutcome> {
    for attempt in 0..MAX_RETRIES {
        // 1. READ current blob + ETag
        let resp = s3.get_object(&domain_key(domain)).send()?;
        let etag = resp.e_tag;
        let mut blob: DomainBlob<T> = serde_json::from_slice(&resp.body)?;

        // 2. Check idempotency on content-addressed key
        if blob.items.contains_key(key) {
            return Ok(AddOutcome::AlreadyExists);
        }

        // 3. MODIFY in memory
        blob.items.insert(key.to_string(), item.clone());
        let new_body = serde_json::to_vec(&blob)?;

        // 4. WRITE with If-Match precondition
        let result = s3.put_object(&domain_key(domain))
            .body(new_body)
            .if_match(&etag)
            .send();

        match result {
            Ok(_) => {
                // Populate local cache optimistically.
                local_cache.insert(domain, key, item);
                return Ok(AddOutcome::Added);
            }
            Err(e) if e.is_precondition_failed() => {
                // Another Lambda wrote between our GET and PUT.
                // Re-read, re-merge, retry.
                metrics::increment("s3.etag_retry", 1);
                continue;
            }
            Err(e) => return Err(e.into()),
        }
    }
    Err(Error::MaxRetriesExceeded)
}
```

**Supersession** (the multi-item atomic case) is trivially supported
because it modifies exactly one blob:

```
fn supersede_schema(old_hash: &str, new_schema: Schema) -> Result<()> {
    for attempt in 0..MAX_RETRIES {
        let (mut blob, etag) = read_schemas()?;
        blob.items.get_mut(old_hash).unwrap().superseded_by = Some(new_schema.identity_hash.clone());
        blob.items.insert(new_schema.identity_hash.clone(), new_schema.clone());
        match put_if_match("schemas.json", &blob, &etag) {
            Ok(_) => return Ok(()),
            Err(e) if e.is_precondition_failed() => continue,
            Err(e) => return Err(e),
        }
    }
    Err(Error::MaxRetriesExceeded)
}
```

Both mutations (update old + insert new) land in a single `PutObject`,
so supersession is atomic by construction. No transactions API needed.

### WASM byte storage

WASM blobs go to `wasm/{hash}.wasm` with **content-addressed
conditional writes**:

```
s3.put_object("wasm/{hash}.wasm")
  .body(wasm_bytes)
  .if_none_match("*")    // fail if key exists — idempotent
  .send()
```

- Same hash → same key → `If-None-Match: *` returns 412 on duplicate,
  which we treat as success
- Writes never need ETag read-modify-write because each WASM file is
  immutable
- `GET /api/transform/{hash}/wasm` can either stream S3 through the
  Lambda or return a presigned URL (saves Lambda bandwidth)

### SemanticIndex trait

Cosine similarity for canonical-field dedup and schema-descriptive-name
matching happens in-memory. Abstracted behind a trait so we can swap
backends at scale:

```rust
pub trait SemanticIndex: Send + Sync {
    fn add(&self, key: String, embedding: Vec<f32>);
    fn find_nearest(
        &self,
        query: &[f32],
        threshold: f32,
        k: usize,
    ) -> Vec<(String, f32)>;
    fn len(&self) -> usize;
}

pub struct InMemoryScanIndex {
    entries: RwLock<HashMap<String, Vec<f32>>>,
}

impl SemanticIndex for InMemoryScanIndex {
    fn add(&self, key: String, embedding: Vec<f32>) {
        self.entries.write().unwrap().insert(key, embedding);
    }

    fn find_nearest(&self, query: &[f32], threshold: f32, k: usize)
        -> Vec<(String, f32)>
    {
        let entries = self.entries.read().unwrap();
        let mut hits: Vec<_> = entries.iter()
            .map(|(k, e)| (k.clone(), cosine(query, e)))
            .filter(|(_, s)| *s >= threshold)
            .collect();
        hits.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        hits.truncate(k);
        hits
    }

    fn len(&self) -> usize {
        self.entries.read().unwrap().len()
    }
}
```

Two instances per Lambda container: one for canonical fields, one for
descriptive schema names. Each is populated during cold start from the
corresponding blob (embeddings are stored base64-encoded inside each
item body).

### Pre-populated canonical fields

The schema service ships with a curated list of ~1K–2K canonical fields
covering common concepts (`user_email`, `photo_caption`, `gps_latitude`,
`document_title`, etc.). These are seeded at Lambda cold start via the
same idempotent `seed()` pattern used for Phase 1 built-in schemas:

```rust
// fold_db::schema_service::builtin_canonical_fields
pub fn seed(store: &dyn SchemaStore) -> Result<()> {
    for field in ALL_PRE_POPULATED_FIELDS {
        // Idempotent — already-exists is not an error
        match store.add_canonical_field(field.clone()) {
            Ok(_) | Err(Error::AlreadyExists) => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
```

Pre-population dramatically reduces:
- **New canonical field writes** — most incoming schemas map to
  fields that already exist
- **Cross-Lambda dedup races** — brand-new fields are rare; duplicate
  concepts usually hit the pre-populated registry first
- **LLM classification cost** — pre-populated fields already carry
  classification and interest-category data, skipping the per-field
  Anthropic call

Source of truth for the list: hardcoded in
`fold_db::schema_service::builtin_canonical_fields` (same pattern as
`builtin_schemas`). Changes to the list deploy with the Lambda.

## Atomicity guarantees

| Concern                          | Mechanism                                     |
|----------------------------------|-----------------------------------------------|
| Same schema proposed twice       | Content hash → same key → idempotent merge    |
| Concurrent writes to same blob   | `If-Match: {etag}` → retry on precondition    |
| Schema supersession              | Single blob update → atomic by construction   |
| Canonical field name collision   | Same name → idempotent merge on key           |
| WASM byte dedup                  | Content-addressed key + `If-None-Match: *`    |
| Lost write during retry          | Retry loop with bounded attempts + metric     |

There are no locks, no coordinator, no leader election. Every atomicity
guarantee is either a content-addressed key or an ETag precondition.

## Cost model (alpha scale)

| Line item                                  | Volume                  | Cost/mo   |
|--------------------------------------------|-------------------------|-----------|
| S3 storage (4 blobs + ~100 WASM bytes)     | ~50 MB                  | < $0.002  |
| `GetObject` (cold starts + rare hot reads) | ~200/mo                 | < $0.001  |
| `PutObject` (all domain writes)            | ~3K/mo                  | $0.015    |
| `PutObject` (WASM uploads)                 | ~10/mo                  | < $0.001  |
| CloudWatch metrics + alarms                |                         | $0.10     |
| **Total**                                  |                         | **~$0.12**|

Three orders of magnitude cheaper than OpenSearch Serverless
(~$350/month baseline) and five times cheaper than DynamoDB at the same
workload. Alpha cost is dominated by CloudWatch, not S3.

## Scale ceiling and escape hatch

### Where the blob model breaks down

| Dimension              | Safe zone        | Warning  | Breakage            |
|------------------------|------------------|----------|---------------------|
| Items per blob         | < 5K             | 10–50K   | > 100K              |
| Blob size              | < 1 MB           | 1–10 MB  | > 50 MB             |
| Write rate per blob    | < 1/sec avg      | 1–10/sec | > 10/sec sustained  |
| Cold start load time   | < 500ms          | 1–3s     | > 5s                |

At alpha, we are three orders of magnitude below every ceiling.

### Migration trigger

CloudWatch alarm fires when **any of** the following thresholds are
crossed for more than 1 hour:

1. `S3_ETagRetryRate > 5%` of writes on any domain
2. `ColdStartLoadDuration > 2s`
3. `BlobSize > 10 MB`
4. `CanonicalFieldCount > 20,000`

Any alarm initiates the migration to `DynamoDbSchemaStore`.

### Migration path

The `SchemaStore` trait (already abstracted in fold_db) lets us swap
backends without touching handlers or caching:

```rust
pub trait SchemaStore: Send + Sync {
    fn add_schema(&self, schema: Schema) -> Result<SchemaAddOutcome>;
    fn get_schema(&self, hash: &str) -> Result<Option<Schema>>;
    fn list_schemas(&self) -> Result<Vec<Schema>>;
    // ... etc
}

// Implementations:
pub struct SledSchemaStore { /* self-hosted binary */ }
pub struct S3BlobSchemaStore { /* Lambda, alpha → beta */ }
pub struct DynamoDbSchemaStore { /* Lambda, scale */ }
```

**Important constraint:** we cannot mitigate contention by sharding the
blob. Semantic dedup requires scanning the entire canonical field set
in one place — a field starting with `u` can semantically match a field
starting with `e` (e.g. `user_email` vs `email_address`). Any shard
boundary would hide legal matches. If we hit the contention ceiling,
the only escape is migration to per-item writes (DynamoDB).

**One-shot migration script** (future work, ~50 lines):
1. Read all four domain blobs from S3
2. For each item, write a per-item row to DynamoDB via `ConditionalPut`
3. Flip `SCHEMA_STORE_BACKEND` env var from `s3` to `dynamodb`
4. Deploy the Lambda

Estimated migration time: 1 engineer-day, assuming the Dynamo backend
is pre-written and tested against the same `SchemaStore` trait tests.

## Migration from current state

1. **Land this design doc** (this PR)
2. **Implement `S3BlobSchemaStore`** in `fold_db::schema_service` behind
   the existing `SchemaStore` trait
3. **Move pre-populated canonical fields** into
   `fold_db::schema_service::builtin_canonical_fields`, mirroring
   `builtin_schemas`
4. **Update schema-infra CDK** to:
   - Remove the DynamoDB `schemasTable` (unused since PR #6)
   - Add an S3 bucket `schema-service-{env}` with versioning enabled
   - Grant the Lambda `s3:GetObject`, `s3:PutObject` on that bucket
   - Set `SCHEMA_STORE_BACKEND=s3` and `SCHEMA_STORE_BUCKET=<name>`
     env vars
5. **Implement bucket bootstrap** — on first cold start, if any of the
   four blobs is missing, create it with an empty `{ "version": 1,
   "items": {} }` body via `If-None-Match: *`
6. **Deploy to dev**, run smoke tests, verify pre-populated fields are
   visible
7. **Port the self-hosted binary** to select backend via CLI flag
   (`--backend sled|s3`) for completeness
8. **Deploy to prod** (`schema.folddb.com`)
9. **Delete the Sled/EFS path** from the Lambda binary once stable

## Open questions and TODOs

1. **Auth on writes.** `POST /api/schemas` and `POST /api/transforms`
   are currently unauthenticated. With a global registry this invites
   spam. Recommend: require Ed25519-signed requests (`created_by`
   pubkey verifies over canonical request body), rate-limit by pubkey.
   **Scope: separate PR after initial S3 migration.**

2. **Weekly reconciliation job.** Append-only + cache staleness can
   produce semantic duplicates (two canonical fields for the same
   concept with different literal names). A scheduled Lambda scans the
   full canonical field set weekly, flags pairs with cosine > 0.84,
   merges via `superseded_by`. **Scope: post-alpha.**

3. **Inverted field-name index.** At >50K schemas, Jaccard-over-all
   becomes painful even with in-memory scan. Build a `field_name →
   [schema_hashes]` inverted index to narrow the candidate set before
   scoring. Adds one write per field per schema but bounds similarity
   cost at O(candidates) not O(all). **Scope: post-alpha, only if
   needed.**

4. **Presigned WASM URLs.** `GET /api/transform/{hash}/wasm` currently
   streams WASM through the Lambda. Switching to presigned GET URLs
   offloads bandwidth and reduces Lambda memory pressure. **Scope:
   nice-to-have.**

5. **Versioned blob format.** The `"version": 1` field is reserved but
   no migration harness exists yet. If we ever need to change the
   on-disk JSON layout (unlikely for immutable data, but possible for
   adding compression or batch-embedding encoding), we need a migration
   path. **Scope: defer until a real change is needed.**

6. **NMI matrix spillover.** If a transform's NMI matrix ever exceeds
   the practical inline size (~50 KB JSON), we spill it to
   `s3://.../nmi/{hash}.json` alongside the WASM. **Scope: defer until
   we see a big transform.**

## Alternatives considered

### DynamoDB (4 tables + GSIs)

- **Pros:** Scales indefinitely on writes. No ETag contention. Standard
  AWS answer for "atomic dedup with conditional puts." No blob size
  or parse-time ceiling.
- **Cons:** No Dynamo strength is exercised by our workload — every
  read is cache-served and every write is idempotent on content hash.
  More complex CDK (4 tables + 5–8 GSIs), higher cost (~$0.50/month
  vs ~$0.12), worse debuggability (scan + unmarshal vs `cat`).
- **Verdict:** Over-engineered for alpha. Keep as the documented
  escape hatch for scale.

### Sled on EFS

- **Pros:** Single `SchemaStore` impl across Lambda and self-hosted
  binary. No code change from PR #6 other than provisioning.
- **Cons:** Requires VPC (adds ~1s cold-start overhead), requires
  EFS filesystem + access point + mount target, adds ongoing EFS cost
  (~$0.30/GB-month), Sled's single-writer-per-db constraint means
  only one Lambda container can write at a time across the fleet
  (contention at scale), debugging requires mounting the filesystem
  from another machine.
- **Verdict:** Net worse than S3 Pattern A on cost, cold-start
  latency, concurrency, and debuggability. The only thing it preserves
  is code reuse between Lambda and binary, which is already solved
  cleanly by the `SchemaStore` trait.

### OpenSearch Serverless

- **Pros:** Native vector search — no in-memory cosine scan needed.
  Scales to millions of vectors.
- **Cons:** Minimum cost ~$350/month for a single collection (two OCUs
  running 24/7), dwarfing everything else in our infra. Our canonical
  field corpus fits in-memory for the foreseeable future, so we'd be
  paying for capacity we don't use.
- **Verdict:** Premature. Revisit when `CanonicalFieldCount > 20,000`.

### S3 Pattern B (one object per item)

- **Pros:** No blob contention. Per-item conditional writes via
  `If-None-Match: *`. Scales further than Pattern A.
- **Cons:** Cold-start load requires `ListObjectsV2` + N parallel
  `GetObject` calls, which has long P99 tail latency and eats Lambda
  concurrency budget. Supersession loses atomicity (two PUTs, not one).
- **Verdict:** Better than Pattern A at high write rates, worse at
  low ones. We're not at high write rates.
