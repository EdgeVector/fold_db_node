# face-discovery-3node debug notes

Persistent record of investigation into the
`bob.shared_record_count[Photography] >= 1` assertion failure, surfaced by
the 2026-05-04 nightly E2E Cloud Tests run
(<https://github.com/EdgeVector/fold_db_node/actions/runs/25310985819>).

This file is the post-mortem; the live BLOCKED sentinel for the failure
lives at the repo root in `.task-blocked.md` while the upstream fix is
pending.

## Symptom

Run 25310985819, scenario `scenarios/face-discovery-3node.yaml`:

```
[PASS] alice.contact_count: 1 >= 1
[PASS] bob.notification_count: 1 >= 1
[FAIL] bob.shared_record_count[Photography]: expected>=1 actual=0
```

Every upstream step (alice_ingest, alice_publish, bob_ingest, bob_search,
bob_connect, alice_accept, alice_share, bob_receive) succeeded. The notification
fired (Bob's count goes 0 → 1). The third assertion — added by PR #408 to
verify that `author_pub_key` survives serialization to Bob's local DB —
failed.

## Root cause

Two compounding plumbing gaps in `fold_db` (introduced around PR #544
"feat: add Ed25519 signatures to molecule writes", 2026-04-15) make
`author_pub_key` unrecoverable for HashRange schemas like Photography.

### Gap 1 — receive path doesn't preserve sender pubkey

`fold_db_node::handlers::discovery::inbound::process_data_share` constructs
a `Mutation` with `mutation.pub_key = payload.sender_public_key`
(Alice's pubkey) and writes it via `MutationManager`. Inside fold_db,
the per-key signing layer
(`MoleculeHashRange::set_atom_uuid_from_values`, etc.) signs each
`AtomEntry` with the **local node's** keypair (`self.signer` on
`MutationManager`), not `mutation.pub_key`. Result: `AtomEntry.writer_pubkey`
is Bob's pubkey for Alice's shared record.

Reference: `crates/core/src/atom/molecule_hash_range.rs:185-194`,
`crates/core/src/fold_db_core/mutation_manager.rs:704-715`.

### Gap 2 — query path doesn't surface per-key writer_pubkey

`fetch_atoms_with_key_metadata_async_with_org`
(`crates/core/src/schema/types/field/filter_utils.rs:125-137`) hardcodes
`writer_pubkey: None` on every `FieldValue`. `FieldVariant::resolve_value`
then sets `fv.writer_pubkey = self.molecule_writer_pubkey()`, but
`molecule_writer_pubkey` returns `None` for Hash/Range/HashRange variants
by design (only Single molecules carry a single molecule-level key).
Net effect: even if Gap 1 were fixed, the per-AtomEntry pubkey still
wouldn't reach the query response for HashRange schemas.

Reference: `crates/core/src/schema/types/field/variant.rs:232-240`.

### Why the schedule was silently masked

PR #408 added the assertion on 2026-04-13 (fold_db rev pre-#544 — the
old `source_pub_key` plumbing was per-atom and worked for HashRange).
fold_db PR #544 landed 2026-04-15 and removed that path. The bump-cascade
bot subsequently bumped fold_db_node past #544, but every nightly
between 2026-04-25 and 2026-05-03 either failed at build (cancelled
runs from runner churn) or failed at `alice_ingest` before the
assertion could run. The 2026-05-04 nightly is the first run where
every upstream step succeeded, exposing the dormant regression.

## Why no fold_db_node-side fix is possible

- The data fold_db_node would need (Alice's pubkey on Bob's
  per-AtomEntry `writer_pubkey`) is not produced by fold_db at the
  current pin (`25605233`). No public fold_db API surfaces it for
  HashRange schemas.

- A defensive pin-back of fold_db is not viable: every fold_db PR
  since #544 is load-bearing — workspace conversion (#629), schema
  utoipa derives required by `src/openapi.rs` (#683), the
  `FOLD_STORAGE_PATH` drop paired with this repo's PR #829 (#685),
  and others. Reverting past #544 would break the `src/openapi.rs`
  registration and reintroduce env-based storage path on the consumer
  side.

## Diagnostic commands

```bash
# Pull the failing run's session logs
gh run download 25310985819 --repo EdgeVector/fold_db_node \
  -n e2e-session-logs -D /tmp/e2e-25310985819

# Bob's observability shows the share IS persisted before the assertion runs
grep -E "write_mutations_batch_async|Received.*records|execute_query" \
  /tmp/e2e-25310985819/run-20260504-092229-15400/nodes/bob/observability.jsonl
```

The session logs confirm Bob's mutation manager completed the batch
of 1 mutations 150 ms before the assertion query was issued, ruling
out a timing race or cleanup leak.

## Recommended fix shape (upstream)

Both of the following are required; either alone is insufficient.

1. `fetch_atoms_with_key_metadata_async_with_org` looks up the per-key
   `AtomEntry.writer_pubkey` from the molecule for Hash/Range/HashRange
   variants and stamps it on each `FieldValue`. (Single already gets it
   via `FieldVariant::molecule_writer_pubkey()`.)

2. `MutationManager` honors `mutation.pub_key` /
   `mutation.provenance` when set (e.g. `Provenance::User { pubkey,
   signature }`) and propagates it onto the AtomEntry instead of always
   re-signing via `self.signer`. Alternatively, fold_db could add a
   "replay/import" code path that stamps a non-local writer_pubkey + provenance
   directly onto the AtomEntry without re-signing locally.

The assertion (`test-framework/scenarios/face-discovery-3node.yaml:91`)
and the helper (`test-framework/lib/assertions.sh:79-102`) are correctly
shaped for a post-fix world — no test-framework change required.
