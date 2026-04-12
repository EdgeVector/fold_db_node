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
