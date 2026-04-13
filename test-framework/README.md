# FoldDB E2E Test Framework

Sequential bash runner for FoldDB / Exemem end-to-end tests against the real
dev Exemem cloud. A scenario YAML lists a set of nodes, steps, and assertions;
the runner spawns one `folddb_server` per role with a unique Ed25519 identity,
executes the steps in order, and tears down all cloud state on exit.

This framework used to carry an aspirational "Claude sub-agent per node"
architecture (driver.md + worker.md + recipes/ + a Rust helper binary). None
of that was ever wired into the execution path, so it was removed. If you
need true parallelism across nodes, build it intentionally — don't restore
the stub.

## Quick start

```bash
# Dry-run: validate the scenario YAML against scenarios/schema.json and exit
./run-scenario.sh scenarios/face-discovery-3node.yaml --dry-run

# Real run
./run-scenario.sh scenarios/face-discovery-3node.yaml

# Debug a failed run — preserves accounts, invite codes, messages, and pids
./run-scenario.sh scenarios/face-discovery-3node.yaml --keep-session
```

## Layout

```
test-framework/
├── run-scenario.sh          Entry point (bash). Orchestrates everything.
├── scenarios/
│   ├── schema.json          JSON Schema for scenario YAML. Kept in sync with
│   │                        step_executor.sh (actions) and assertions.sh
│   │                        (fields) — update all three in the same commit.
│   ├── framework-smoke.yaml
│   ├── face-discovery-self.yaml
│   ├── face-discovery-3node.yaml
│   ├── network-intersection.yaml
│   └── referral-queries.yaml
├── fixtures/                Real JPEG files used by ingest_photo actions.
├── lib/
│   ├── node_factory.sh      Spawn / register / shut down folddb_server nodes.
│   ├── step_executor.sh     Dispatch YAML actions to inlined HTTP calls.
│   ├── assertions.sh        Evaluate assertions against live nodes.
│   └── cleanup.sh           Tear down Exemem accounts + DynamoDB + local pids.
└── logs/<run_id>/           Per-run state. Contains nodes/<role>/stdout.log,
                             nodes/<role>/stderr.log, nodes.json, state/.
```

## Requirements

- bash 4+
- `jq`, `yq`
- `ajv` CLI (`npm i -g ajv-cli`) — **required** for scenario schema validation.
  Missing ajv is a hard error; `run-scenario.sh` refuses to start without it
  so typos in action names fail at parse time instead of silently no-op'ing.
- `aws` CLI (invite code management + admin Lambda invokes)
- `curl`
- `python3` with `pynacl` (per-node Ed25519 key generation — `pip3 install pynacl`)

## How a scenario runs

1. Parse `nodes[]` from the scenario YAML. Cap at 5 nodes.
2. For each role: mint an invite code in `ExememInviteCodes-dev`, spawn
   `folddb_server` with a fresh Ed25519 key in its own `$SESSION_DIR/nodes/$role/`,
   register with Exemem via the invite code, handle the post-registration
   Sled-lock restart, set a display name, and append to `nodes.json`.
3. For each scenario step (in YAML order): for each role in the step, for
   each action: look up `action` in the `case` arms of `step_executor.sh`
   and execute inline via `curl`. Actions write intermediate state to
   `$SESSION_DIR/state/` (e.g. `last-face-search-alice.json`) so subsequent
   actions can reference it.
4. Run every assertion in the scenario's `assertions:` block via
   `assertions.sh::run_assertion`.
5. Teardown (unless `--keep-session`): for each node, delete the Exemem
   account via the API Gateway, delete per-public-key rows from Aurora via
   direct Lambda invoke on `ExememDiscovery-dev`, clear bulletin-board
   messages for the node's pseudonyms via direct Lambda invoke on
   `ExememMessagingService-dev`, revoke the invite code, kill the local PID.

Execution is strictly sequential. A 3-node scenario takes ~3× the wall time
of a 1-node scenario on the slow parts (ingest + face detection), because
nothing overlaps.

## Adding a new action

1. Add a `case` arm to `lib/step_executor.sh::execute_action`.
2. Add the action name to the `enum` in `scenarios/schema.json`.
3. If it produces intermediate state (like `face_search` writing
   `last-face-search-<role>.json`), document the filename so other actions
   can reference it via `<role>.face_search[i]` or similar.
4. Reference it from a scenario YAML and run `--dry-run` (with `ajv`
   installed) to verify the schema accepts it.

## Adding a new assertion field

1. Add a `get_<field>` helper to `lib/assertions.sh`.
2. Add a `case` arm in `run_assertion`.
3. Add the field to the assertion `field` enum in `scenarios/schema.json`.

## CI integration

The framework runs against dev Exemem cloud via `.github/workflows/e2e-cloud.yml`:

- **Manual trigger**: Actions → "E2E Cloud Tests" → Run workflow. Pick a scenario path.
- **Nightly**: runs `face-discovery-self.yaml` + `face-discovery-3node.yaml` at 07:00 UTC.

### Required GitHub secrets

| Secret | Purpose |
|---|---|
| `AWS_E2E_ACCESS_KEY_ID` | IAM credentials with access to `ExememInviteCodes-dev` DynamoDB + Lambda invoke on `ExememDiscovery-dev` + `ExememMessagingService-dev` |
| `AWS_E2E_SECRET_ACCESS_KEY` | Paired secret |
| `FOLDDB_TEST_ADMIN_SECRET` | Shared secret for `/admin/*` endpoints (from `ExememTestAdminSecret-dev` in Secrets Manager) |
| `PRIVATE_DEPS_TOKEN` | Existing secret for private repo access during `cargo build` |

### IAM permissions needed

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Effect": "Allow",
      "Action": [
        "dynamodb:PutItem",
        "dynamodb:DeleteItem",
        "dynamodb:GetItem"
      ],
      "Resource": "arn:aws:dynamodb:us-west-2:*:table/ExememInviteCodes-dev"
    },
    {
      "Effect": "Allow",
      "Action": ["lambda:InvokeFunction"],
      "Resource": [
        "arn:aws:lambda:us-west-2:*:function:ExememDiscovery-dev",
        "arn:aws:lambda:us-west-2:*:function:ExememMessagingService-dev"
      ]
    }
  ]
}
```

### Concurrency

Only one E2E run executes at a time (GitHub Actions `concurrency.group: e2e-cloud`).
This prevents multiple runs from clashing on dev cloud state. Additional runs queue
behind the first — no cancel-in-progress.

### Failure debugging

Session logs are uploaded as `e2e-session-logs` artifact on failure
(7-day retention). Artifact contains `logs/<run_id>/state/`,
`nodes/*/stdout.log`, `nodes/*/stderr.log`, plus `nodes.json` for
post-mortem inspection.

For local debugging, use `--keep-session` and then poke at:

- `logs/<run_id>/nodes/<role>/stderr.log` — server logs
- `logs/<run_id>/state/*.json` — intermediate action state
- Aurora `discovery_vectors`, `discovery_face_vectors` tables — filter by
  the `public_key` in `nodes.json` for that run
- DynamoDB `connection-messages` — filter by the pseudonyms in
  `logs/<run_id>/state/pseudonyms-<role>.json`
