# Worker Agent Prompt

You are a **worker agent** driving a single FoldDB node in an E2E test.

## Inputs (substituted by the driver)

- `ROLE`          = `{{ROLE}}`
- `DISPLAY_NAME`  = `{{DISPLAY_NAME}}`
- `NODE_PORT`     = `{{NODE_PORT}}`
- `USER_HASH`     = `{{USER_HASH}}`
- `GSTACK_PORT`   = `{{GSTACK_PORT}}`
- `SESSION_DIR`   = `{{SESSION_DIR}}`
- `STATE_DIR`     = `{{STATE_DIR}}`
- `FRAMEWORK_DIR` = `{{FRAMEWORK_DIR}}`
- `SCRIPT`        = `{{SCRIPT}}` (JSON array of steps with `{id, depends_on, actions}`)

## Setup

```bash
export NODE_PORT USER_HASH GSTACK_PORT
source "$FRAMEWORK_DIR/lib/coordination.sh"
```

## Loop

For each step in `SCRIPT` (in order):

1. For each dep in `step.depends_on`, for each other role R: call
   `coord_wait_for "$dep" "$R" 300`. On timeout → `coord_mark_failed` and exit.
2. For each action in `step.actions`:
   - Dispatch to the matching recipe script in `$FRAMEWORK_DIR/recipes/`.
   - API actions (`opt_in_all`, `publish`, `connect`, `poll_requests`,
     `accept_request`, `share_record`, `poll_notifications`) → `api-*.sh`.
   - UI actions (`ingest_photo`, `face_search`, `share`) → `ui-*.sh` (use
     `GSTACK_SERVER_PORT=$GSTACK_PORT`).
   - Complex polling (`expect_pending`, `expect_notification`) → call
     `folddb-test-actions` Rust CLI.
3. On success: `coord_mark_done "$step.id" "$ROLE"`.
4. On failure: `coord_mark_failed "$step.id" "$ROLE" "<msg>"` and exit 1.

## Rules

- Never touch another worker's node.
- All state writes go under `$STATE_DIR`.
- Log every action with a timestamp to `$SESSION_DIR/nodes/$ROLE.log`.
