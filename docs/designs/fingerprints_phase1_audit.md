# fold_db schema audit for Fingerprints Phase 1

Date: 2026-04-14
Source branch: `feat/fingerprints-phase1`
Parent design: `../../../docs/designs/fingerprints.md` (workspace root, since fold_db_node can reach it via relative path only in comments — the actual file is in exemem-workspace)

## TL;DR

fold_db's declarative schema system can host everything Phase 1 needs, **with one design adjustment**: there is no native "array-contains" query. Reverse lookups over multi-valued reference fields (e.g. "Mentions where fingerprint_ids contains X") require **HashRange junction schemas** — denormalized pairs written at ingest time. This is a fold_db-native pattern, not a workaround.

The design doc's "six schemas" becomes **nine** once junctions are counted: six primary + three junctions. Plus the three support schemas already in the plan (IngestionError, ExtractionStatus, ReceivedShare) brings total Phase 1 schemas to **twelve**.

## Findings by audit question

### Q1. Stable content-derived primary keys — CONFIRMED WORKS

`DeclarativeSchemaDefinition` uses `KeyConfig { hash_field, range_field }`. At mutation time, `KeyValue::from_mutation` reads the value of the declared `hash_field` from the mutation payload and uses it as the primary key:

```rust
// fold_db/src/schema/types/key_value.rs:31
pub fn from_mutation(mutation_fields: &HashMap<String, Value>, key_config: &KeyConfig) -> Self {
    if let Some(hash_field) = &key_config.hash_field {
        key_value.hash = resolve_field_as_string(mutation_fields, hash_field);
    }
    ...
}
```

This means content-derived primary keys work as designed: the ingest pipeline computes `fp_<sha256(kind, value)>` as a string field value before writing the record, and fold_db uses that string as the primary key. Two concurrent ingests of the same email both compute the same derived value → same key → fold_db upsert semantics handle dedup. **Silent correctness risk #3 (Fingerprint upsert race) remains eliminated.**

### Q2. Multi-field equality — LIMITED BY DESIGN

`Query` in `schema/types/operations.rs` supports `HashRangeFilter` (key-level) and `ValueFilter` (post-fetch numeric field comparisons). There is no SQL-style `WHERE field_a = X AND field_b = Y` for non-key fields.

This is fine for the fingerprint schemas because every query we need either:
- Looks up by primary key (exact record fetch), or
- Uses a HashKey filter on a schema where the hash_field encodes the lookup we want.

### Q3. Array-contains — NOT SUPPORTED, use junction schemas

There is **no** query like `Mention where fingerprint_ids contains X`. The design doc's reverse-lookup patterns assume this works; they need to be rewritten as junction schemas.

Pattern:

```
schema: MentionByFingerprint
type: HashRange
key:  { hash_field: fingerprint_id, range_field: mention_id }
fields:
  fingerprint_id  : String   (FingerprintId)
  mention_id      : String   (MentionId)

"Mentions containing fp_X" = HashRangeFilter::HashKey("fp_X") over MentionByFingerprint
→ returns all range keys (mention_ids) under that hash group.
```

Same pattern for edge traversal and source-record → mentions:

```
schema: EdgeByFingerprint
type: HashRange
key:  { hash_field: fingerprint_id, range_field: edge_id }
-- written twice per Edge, one row for each endpoint (a and b)
-- "edges touching fp_X" = HashKey("fp_X")

schema: MentionBySource
type: HashRange
key:  { hash_field: "<source_schema>:<source_key>", range_field: mention_id }
-- "mentions from Photos/IMG_1234" = HashKey("Photos:IMG_1234")
```

Write-time cost: each Fingerprint, Edge, Mention insert also writes 1–2 junction rows. This is O(1) per write and makes reverse lookups O(hash_group_size) via HashKey filter.

### Q4. Field types — RICH, FULLY SUPPORTED

`FieldValueType` (fold_db/src/schema/types/field_value_type.rs:10) covers:

- **Primitives**: `String`, `Integer`, `Float`, `Number`, `Boolean`, `Null`
- **Compound**: `Array(T)`, `Map(T)`, `Object({...})`
- **References**: `SchemaRef(String)` — native foreign key to another schema
- **Unions**: `OneOf(Vec<T>)` — used as `OneOf([T, Null])` for nullable fields
- **Escape hatch**: `Any`

Our requirements map cleanly:

| Design field | fold_db type |
|---|---|
| `Vec<f32>` face embedding | `Array(Float)` |
| `Vec<String>` aliases | `Array(String)` |
| `Vec<FingerprintId>` | `Array(String)` |
| `Option<IdentityId>` | `OneOf([SchemaRef("Identity"), Null])` |
| `enum Relationship` | `OneOf([String, ...])` or `String` with app-level validation |
| `enum EdgeKind` | `String` |
| Ed25519 signature bytes | `String` (hex or base64) |

**SchemaRef is native**. `Persona.identity_id: SchemaRef("Identity")` gives the resolver a typed reference the fold_db layer already understands — not just an opaque string.

### Q5. Runtime schema registration — PARTIAL AUDIT, confirmed to work

`DbOperations::store_schema(schema_name, schema)` (fold_db/src/db_operations/schema_operations.rs:21) is an async API that writes a Schema to the SchemaStore at runtime. Schemas are not code-generated. Phase 1 can register its twelve schemas at startup via this path or equivalent.

Deferred: I did not confirm whether schemas registered this way participate in the schema service's similarity detection (it might assume schemas flow through a specific registration path that differs from the direct store). **Action:** first Phase 1 coding task is to write a tiny integration test that registers a new schema, writes a record, reads it back, and confirms the full round trip works. If it doesn't, the audit reopens.

### Q6. Record size ceiling — NOT AUDITED

Did not find an explicit limit. Face embeddings are 512 floats × 8 bytes = 4 KB per Fingerprint. This is small compared to what fold_db already handles (schema records can carry multi-KB Objects). **Deferred to measurement during Phase 1 implementation.** If it becomes a problem, we can quantize to int8 (2 KB) or compress.

### Q7. Schema expansion compatibility — CONFIRMED NO TOUCH REQUIRED

The junction pattern is all-new schemas. No existing schemas (Photos, Notes, etc.) need new fields. The `MentionBySource` junction uses the source_schema + source_key as its hash, meaning a photo record does not know it has been extracted — only the junction does.

## Revised schema list — twelve schemas for Phase 1

### Primary data (six)

1. **Fingerprint** — `type: Hash`, `hash_field: id`. `id` computed at write time as `fp_<sha256(kind, canonical_value)>`.
2. **Mention** — `type: Hash`, `hash_field: id` (UUID `mn_...`).
3. **Edge** — `type: Hash`, `hash_field: id` computed as `eg_<sha256(a, b, kind)>`.
4. **Identity** — `type: Hash`, `hash_field: id` = `id_<pub_key>`.
5. **IdentityReceipt** — `type: Hash`, `hash_field: id` (UUID).
6. **Persona** — `type: Hash`, `hash_field: id` (UUID).

### Junctions (three) — NEW FROM THIS AUDIT

7. **EdgeByFingerprint** — `type: HashRange`, `(hash_field: fingerprint_id, range_field: edge_id)`. Two rows per Edge.
8. **MentionByFingerprint** — `type: HashRange`, `(hash_field: fingerprint_id, range_field: mention_id)`. One row per (Mention, Fingerprint) pair.
9. **MentionBySource** — `type: HashRange`, `(hash_field: "<source_schema>:<source_key>", range_field: mention_id)`. One row per Mention.

### Support (three)

10. **IngestionError** — `type: Hash`, `hash_field: id` (UUID).
11. **ExtractionStatus** — `type: Hash`, `hash_field: id` (composite: `"<source_schema>:<source_key>:<extractor>"`).
12. **ReceivedShare** — `type: Hash`, `hash_field: id` (UUID). Phase 3, but schema can be registered in Phase 1 for completeness.

## Impact on the parent design doc

The main design doc needs these adjustments:

1. Add the three junction schemas (EdgeByFingerprint, MentionByFingerprint, MentionBySource) to the data model section.
2. Update the "reverse lookups are standard schema queries" bullet to clarify that reverse lookups go through junctions, not direct field predicates.
3. Update the resolver's traversal pseudocode to fetch edges via EdgeByFingerprint HashKey, not via an imaginary "edges where a or b contains X".
4. Note the write-time cost: each insert writes 1-2 junction rows.
5. Update TODO-1 to mark the audit as complete.

Will apply these as a follow-up commit.

## What I did not audit (deferred)

- **Record size ceiling** (Q6) — deferred to measurement.
- **Schema registration via schema service vs direct store** (Q5) — need a round-trip integration test on day 1 of coding.
- **Multi-field equality on non-key fields** (Q2) — confirmed not supported, but I did not verify that our query needs never require it.
- **Sync behavior** — Phase 1 uses default fold_db sync semantics; no audit of how HashRange junctions behave under concurrent cross-device writes.

## Conclusion

**Phase 1 is unblocked.** The design doc needs a junction-schema patch but the core approach holds. The schema audit finding did not surface any showstoppers.
