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
    # Record in pending list so SIGINT/crash before nodes.json update still cleans up.
    if [[ -n "${FOLDDB_TEST_SESSION_DIR:-}" ]]; then
      echo "$code" >> "$FOLDDB_TEST_SESSION_DIR/state/pending-invites.txt"
    fi
    echo "$code"
  done
}

# nf_find_free_port <start> [max_tries]
# Returns the first TCP port >= start that is not bound by any local process.
nf_find_free_port() {
  local start="${1:?}" max="${2:-200}"
  local p=$start
  local tried=0
  while (( tried < max )); do
    if ! lsof -nP -iTCP:"$p" -sTCP:LISTEN >/dev/null 2>&1; then
      echo "$p"
      return 0
    fi
    p=$((p + 1))
    tried=$((tried + 1))
  done
  echo "nf_find_free_port: no free port in [$start, $((start + max)))" >&2
  return 1
}

# Locate the fold_db_node repo root (the directory containing a Cargo.toml
# that declares the `face-detection` feature).
nf_repo_root() {
  local d
  d="$(pwd)"
  while [[ "$d" != "/" ]]; do
    if [[ -f "$d/Cargo.toml" ]] && grep -q '^face-detection' "$d/Cargo.toml" 2>/dev/null; then
      echo "$d"; return 0
    fi
    d="$(dirname "$d")"
  done
  echo "[nf_repo_root] fold_db_node repo root not found (no Cargo.toml with face-detection feature above $(pwd))" >&2
  return 1
}

# Hard-error if the built binary was compiled without `--features face-detection`.
# The face detection code paths are fully gated behind that cfg, so an opt-out
# build would silently index zero faces — violating no-silent-failures and
# masking real discovery bugs in scenarios that use ingest_photo.
nf_assert_face_detection_compiled_in() {
  local bin="$1"
  # Use grep -c (drains stdin) instead of grep -q — with `set -o pipefail`,
  # grep -q short-circuits and sends SIGPIPE to nm, failing the whole pipeline.
  local sym_count
  sym_count="$(nm "$bin" 2>/dev/null | grep -cE 'run_face_detection|has_face_processor|index_faces' || true)"
  if (( sym_count == 0 )); then
    cat >&2 <<EOF
[nf] ERROR: $bin was built without --features face-detection.
[nf] Face detection symbols (run_face_detection / has_face_processor / index_faces)
[nf] are absent from the binary, so every ingest_photo step would silently
[nf] produce zero face embeddings and mask discovery bugs.
[nf] Rebuild with:
[nf]   cargo build --bin folddb_server --features face-detection
EOF
    return 1
  fi
}

# Build folddb_server with the face-detection feature (idempotent — cargo is a
# no-op when nothing changed) and echo the path to the debug binary. Replaces
# the old "walk up and hope a pre-built binary exists" lookup, which was the
# footgun that let a feature-less binary slip into E2E runs.
nf_find_binary() {
  # Cache across multiple calls in the same run (cargo is a no-op on no-change
  # but still spams stderr and takes a beat; we only need to build once).
  if [[ -n "${FOLDDB_TEST_BINARY:-}" && -x "$FOLDDB_TEST_BINARY" ]]; then
    echo "$FOLDDB_TEST_BINARY"; return 0
  fi
  local repo_root bin
  repo_root="$(nf_repo_root)" || return 1
  echo "[nf] building folddb_server --features face-detection (repo: $repo_root)" >&2
  ( cd "$repo_root" && cargo build --bin folddb_server --features face-detection >&2 ) \
    || { echo "[nf] cargo build failed" >&2; return 1; }
  bin="$repo_root/target/debug/folddb_server"
  [[ -x "$bin" ]] || { echo "[nf] expected binary at $bin but not found" >&2; return 1; }
  nf_assert_face_detection_compiled_in "$bin" || return 1
  export FOLDDB_TEST_BINARY="$bin"
  echo "$bin"
}

nf_spawn_node() {
  local name="$1" port="$2" session_dir="$3"
  local node_dir="$session_dir/nodes/$name"
  mkdir -p "$node_dir/data" "$node_dir/config"

  # Generate unique Ed25519 keys for this node so each test node has a distinct
  # identity (user_hash + public_key). Without this, nodes fall back to a shared
  # default identity and all assertions collapse.
  #
  # NOTE: depends on python3 + pynacl. pynacl is documented as a requirement in
  # test-framework/README.md. Fail loudly if it's missing — a cryptic NameError
  # from python becomes a mystery "all nodes have the same hash" bug otherwise.
  if ! python3 -c 'import nacl.signing' >/dev/null 2>&1; then
    echo "[node_factory] python3 + pynacl required for Ed25519 keygen." >&2
    echo "[node_factory]   install: pip3 install pynacl" >&2
    return 1
  fi
  local keys priv pub
  keys="$(python3 -c '
import base64, nacl.signing
sk = nacl.signing.SigningKey.generate()
print(base64.b64encode(sk.encode()).decode())
print(base64.b64encode(sk.verify_key.encode()).decode())
')"
  priv="$(echo "$keys" | sed -n '1p')"
  pub="$(echo "$keys"  | sed -n '2p')"
  [[ -n "$priv" && -n "$pub" ]] || { echo "[node_factory] Ed25519 keygen failed" >&2; return 1; }

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
  # Poll briefly for the server to come back. If it's still down after a few
  # seconds, kill any stale process on the port and respawn with the same
  # config (preserves the identity).
  local rwait_deadline=$(( $(date +%s) + 5 ))
  local healthy=0
  while (( $(date +%s) < rwait_deadline )); do
    if curl -fsS --max-time 2 "http://127.0.0.1:$port/api/system/auto-identity" >/dev/null 2>&1; then
      healthy=1
      break
    fi
    sleep 0.3
  done
  if (( healthy == 0 )); then
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

# nf_shutdown_gstack <gstack_port>
# Stops a per-node gstack daemon. gstack auto-starts on first recipe call, so
# there is no spawn helper — we only need to tear it down.
nf_shutdown_gstack() {
  local gport="${1:?}"
  local browse="$HOME/.claude/skills/gstack/browse/dist/browse"
  if [[ -x "$browse" ]]; then
    GSTACK_SERVER_PORT="$gport" "$browse" stop >/dev/null 2>&1 || true
  fi
  # Backstop: kill anything still bound to the port.
  local stale
  for stale in $(lsof -t -nP -iTCP:"$gport" -sTCP:LISTEN 2>/dev/null); do
    kill -9 "$stale" 2>/dev/null || true
  done
}
