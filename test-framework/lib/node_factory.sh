#!/usr/bin/env bash
# Node factory: create invite codes, spawn nodes, register, configure.
# Requires: jq, aws, curl, openssl.

set -euo pipefail

nf_create_invite_codes() {
  local n="$1"
  local i
  for ((i=0; i<n; i++)); do
    local code
    code="test-$(date +%s)-$RANDOM-$i"
    aws dynamodb put-item \
      --table-name ExememInviteCodes-dev \
      --item "{\"code\":{\"S\":\"$code\"},\"used\":{\"BOOL\":false},\"created_at\":{\"N\":\"$(date +%s)\"}}" \
      >/dev/null
    echo "$code"
  done
}

nf_find_binary() {
  local dir
  dir="$(pwd)"
  while [[ "$dir" != "/" ]]; do
    if [[ -x "$dir/target/release/folddb_server" ]]; then
      echo "$dir/target/release/folddb_server"; return 0
    fi
    if [[ -x "$dir/target/debug/folddb_server" ]]; then
      echo "$dir/target/debug/folddb_server"; return 0
    fi
    dir="$(dirname "$dir")"
  done
  echo "folddb_server binary not found" >&2
  return 1
}

nf_spawn_node() {
  local name="$1" port="$2" session_dir="$3"
  local node_dir="$session_dir/nodes/$name"
  mkdir -p "$node_dir/data" "$node_dir/config"

  # Generate unique Ed25519 keys for this node so each test node has a distinct
  # identity (user_hash + public_key). Without this, nodes fall back to a shared
  # default identity and all assertions collapse.
  local keys priv pub
  keys="$(python3 -c '
import base64, nacl.signing
sk = nacl.signing.SigningKey.generate()
print(base64.b64encode(sk.encode()).decode())
print(base64.b64encode(sk.verify_key.encode()).decode())
')"
  priv="$(echo "$keys" | sed -n '1p')"
  pub="$(echo "$keys"  | sed -n '2p')"

  cat > "$node_dir/config/node_config.json" <<EOCONF
{
  "database": {"type": "local", "path": "$node_dir/data"},
  "storage_path": "$node_dir/data",
  "schema_service_url": "${FOLDDB_TEST_DEV_SCHEMA:?}",
  "private_key": "$priv",
  "public_key": "$pub"
}
EOCONF

  local bin
  bin="$(nf_find_binary)"

  NODE_CONFIG="$node_dir/config/node_config.json" \
  FOLDDB_HOME="$node_dir" \
    nohup "$bin" \
      --port "$port" \
      --schema-service-url "${FOLDDB_TEST_DEV_SCHEMA:?}" \
      >"$node_dir/stdout.log" 2>"$node_dir/stderr.log" &
  local pid=$!
  echo "$pid" > "$node_dir/pid"
  echo "$pid"
}

nf_wait_healthy() {
  local port="$1" timeout="${2:-30}"
  local deadline=$(( $(date +%s) + timeout ))
  while (( $(date +%s) < deadline )); do
    if curl -fsS "http://127.0.0.1:$port/api/system/auto-identity" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done
  echo "node on port $port not healthy after ${timeout}s" >&2
  return 1
}

nf_register_node() {
  local port="$1" invite="$2" name="${3:-}" session_dir="${4:-}"
  local hash
  hash="$(curl -fsS "http://127.0.0.1:$port/api/system/auto-identity" | jq -r '.user_hash // .hash // ""')"
  local resp
  resp="$(curl -fsS -X POST "http://127.0.0.1:$port/api/auth/register" \
    -H 'content-type: application/json' \
    -H "X-User-Hash: $hash" \
    -d "{\"invite_code\":\"$invite\"}")"

  # Registration may cause an internal server restart that holds the Sled lock.
  # Give it a moment, then if the server is down, kill any stale process on the
  # port and respawn with the same config (preserves the identity).
  sleep 2
  if ! curl -fsS --max-time 2 "http://127.0.0.1:$port/api/system/auto-identity" >/dev/null 2>&1; then
    if [[ -n "$name" && -n "$session_dir" ]]; then
      local stale
      for stale in $(lsof -t -i ":$port" 2>/dev/null); do
        kill -9 "$stale" 2>/dev/null || true
      done
      sleep 1
      local node_dir="$session_dir/nodes/$name"
      local bin
      bin="$(nf_find_binary)"
      NODE_CONFIG="$node_dir/config/node_config.json" \
      FOLDDB_HOME="$node_dir" \
        nohup "$bin" \
          --port "$port" \
          --schema-service-url "${FOLDDB_TEST_DEV_SCHEMA:?}" \
          >"$node_dir/stdout.log" 2>"$node_dir/stderr.log" &
      echo $! > "$node_dir/pid"
      nf_wait_healthy "$port" 30 >/dev/null || true
    fi
  fi
  echo "$resp"
}

nf_set_display_name() {
  local port="$1" hash="$2" name="$3"
  curl -fsS -X PUT "http://127.0.0.1:$port/api/identity/card" \
    -H 'content-type: application/json' \
    -H "X-User-Hash: $hash" \
    -d "{\"display_name\":\"$name\"}" >/dev/null
}

nf_shutdown_node() {
  local name="$1" session_dir="$2"
  local pidfile="$session_dir/nodes/$name/pid"
  if [[ -f "$pidfile" ]]; then
    local pid
    pid="$(cat "$pidfile")"
    kill "$pid" 2>/dev/null || true
    sleep 0.2
    kill -9 "$pid" 2>/dev/null || true
  fi
}
