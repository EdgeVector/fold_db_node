# FoldDB E2E Test Framework

Agent-driven end-to-end test framework for FoldDB / Exemem. A driver agent parses
a scenario YAML, spawns up to 5 real `folddb_server` nodes, launches one worker
agent per node in parallel, and asserts state via HTTP APIs and the UI (via gstack).

## Quick start

```bash
# Dry-run validate a scenario
./run-scenario.sh scenarios/framework-smoke.yaml --dry-run

# Run a scenario
./run-scenario.sh scenarios/face-discovery-3node.yaml

# Keep session dir for inspection
./run-scenario.sh scenarios/face-discovery-3node.yaml --keep-session
```

## Architecture

- `run-scenario.sh` — entry point; validates scenario, sets up session dir, prints
  driver-agent launch instructions.
- `driver.md` — template prompt for the driver agent (orchestrates the run).
- `worker.md` — template prompt for each worker agent (one per node).
- `scenarios/*.yaml` — scenario definitions, validated against `scenarios/schema.json`.
- `recipes/*.sh` — action scripts the workers call (API = curl, UI = gstack).
- `lib/*.sh` — bash helpers (node factory, coordination, cleanup, assertions).
- `crates/folddb-test-actions/` — Rust CLI for complex actions (polling, etc).

## Limits

- Max 5 nodes per scenario.
- Session state under `logs/<run_id>/` with `state/` subdir for coordination.

## Requirements

- bash 4+
- jq, yq
- aws CLI (for invite code ops)
- gstack (for UI recipes)
- Rust toolchain (for the test-actions binary)
- Optional: ajv CLI (for JSON Schema validation of scenarios)

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

Minimum IAM policy for the E2E user:

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

Session logs are uploaded as `e2e-session-logs` artifact on failure (7-day retention).
Artifact contains `logs/<run_id>/state/` (barrier files), `nodes/*/stdout.log`,
`nodes/*/stderr.log`, plus `nodes.json` for post-mortem inspection.
