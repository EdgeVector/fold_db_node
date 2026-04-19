#!/usr/bin/env bash
# Assertion helpers + state getters.
set -euo pipefail

assert_eq() {
  local actual="$1" expected="$2" label="${3:-assert_eq}"
  if [[ "$actual" != "$expected" ]]; then
    echo "[FAIL] $label: expected=$expected actual=$actual" >&2
    return 1
  fi
  echo "[PASS] $label: $actual == $expected"
}

assert_ge() {
  local actual="$1" expected="$2" label="${3:-assert_ge}"
  if (( actual < expected )); then
    echo "[FAIL] $label: expected>=$expected actual=$actual" >&2
    return 1
  fi
  echo "[PASS] $label: $actual >= $expected"
}

assert_contains() {
  local haystack="$1" needle="$2" label="${3:-assert_contains}"
  if [[ "$haystack" != *"$needle"* ]]; then
    echo "[FAIL] $label: '$needle' not found in '$haystack'" >&2
    return 1
  fi
  echo "[PASS] $label: contains $needle"
}

get_contact_count() {
  local port="$1" hash="$2"
  curl -fsS "http://127.0.0.1:$port/api/contacts" -H "X-User-Hash: $hash" \
    | jq '(.contacts // .) | length'
}

get_notification_count() {
  local port="$1" hash="$2"
  curl -fsS "http://127.0.0.1:$port/api/notifications" -H "X-User-Hash: $hash" \
    | jq '.count // ((.notifications // .) | length)'
}

get_pending_requests() {
  local port="$1" hash="$2"
  curl -fsS "http://127.0.0.1:$port/api/discovery/connection-requests" \
    -H "X-User-Hash: $hash" \
    | jq '[(.requests // .)[] | select(.status == "pending")] | length'
}

# get_schema_record_count NODE_PORT HASH SCHEMA_NAME
# Returns the number of records in a schema (looks up by descriptive_name).
get_schema_record_count() {
  local port="$1" hash="$2" schema_name="$3"
  # Look up schema by descriptive_name, then query all records
  local schema_hash
  schema_hash=$(curl -fsS "http://127.0.0.1:$port/api/schemas" \
    -H "X-User-Hash: $hash" \
    | jq -r --arg name "$schema_name" '.schemas[] | select(.descriptive_name == $name) | .name' \
    | head -1)
  if [[ -z "$schema_hash" ]]; then
    echo 0
    return 0
  fi
  # List all keys for the schema. Response: {keys: [{hash, range}, ...], total_count}
  curl -fsS "http://127.0.0.1:$port/api/schema/$schema_hash/keys" \
    -H "X-User-Hash: $hash" \
    | jq '.total_count // ((.keys // []) | length)' 2>/dev/null || echo 0
}

# get_shared_record_count NODE_PORT HASH SCHEMA_NAME EXPECTED_AUTHOR_PUBKEY
# Returns the count of records in a schema whose author_pub_key matches the expected key.
# Used to verify the shared_by attribution feature (PR #396).
get_shared_record_count() {
  local port="$1" hash="$2" schema_name="$3" expected_pk="$4"
  local schema_hash
  schema_hash=$(curl -fsS "http://127.0.0.1:$port/api/schemas" \
    -H "X-User-Hash: $hash" \
    | jq -r --arg name "$schema_name" '.schemas[] | select(.descriptive_name == $name) | .name' \
    | head -1)
  [[ -n "$schema_hash" ]] || { echo 0; return 0; }

  # List all fields for the schema to build a valid query
  local fields
  fields=$(curl -fsS "http://127.0.0.1:$port/api/schemas" -H "X-User-Hash: $hash" \
    | jq -c --arg h "$schema_hash" '.schemas[] | select(.name == $h) | .fields')
  [[ "$fields" != "null" && -n "$fields" ]] || { echo 0; return 0; }

  # Query all records in the schema and count those with matching author_pub_key.
  local resp
  resp=$(curl -fsS -X POST "http://127.0.0.1:$port/api/query" \
    -H "Content-Type: application/json" \
    -H "X-User-Hash: $hash" \
    -d "{\"schema_name\":\"$schema_hash\",\"fields\":$fields}" 2>/dev/null || echo '{}')
  echo "$resp" | jq --arg pk "$expected_pk" \
    '[(.data // .results // [])[] | select(.author_pub_key == $pk)] | length' 2>/dev/null || echo 0
}

# get_org_member_count NODE_PORT HASH ORG_HASH
# Returns the number of members on the given org as seen by this node. The
# other node's membership molecule propagates via the OrgSyncEngine (alpha M1),
# so this count is the acceptance test for "the other node's join molecule
# arrived on me" — see projects/alpha-org-member-propagation-gap.
get_org_member_count() {
  local port="$1" hash="$2" org_hash="$3"
  [[ -n "$org_hash" ]] || { echo 0; return 0; }
  curl -fsS --max-time 5 "http://127.0.0.1:$port/api/org" \
    -H "X-User-Hash: $hash" \
    | jq --arg oh "$org_hash" -r \
        '(((.data.orgs // .orgs // [])[] | select(.org_hash == $oh)).members // []) | length' \
    2>/dev/null || echo 0
}

# Run a single assertion from YAML: {node, field, op, value, [schema]}
# Args: NODES_JSON ASSERTION_JSON
run_assertion() {
  local nodes_json="$1"
  local assertion="$2"
  local node field op value schema
  node=$(echo "$assertion" | jq -r '.node')
  field=$(echo "$assertion" | jq -r '.field')
  op=$(echo "$assertion" | jq -r '.op')
  value=$(echo "$assertion" | jq -r '.value')
  schema=$(echo "$assertion" | jq -r '.schema // ""')

  local port hash
  port=$(jq -r --arg role "$node" '.[] | select(.role == $role) | .port' "$nodes_json")
  hash=$(jq -r --arg role "$node" '.[] | select(.role == $role) | .hash' "$nodes_json")

  if [[ -z "$port" || -z "$hash" ]]; then
    echo "[FAIL] assertion: node $node not found in nodes.json" >&2
    return 1
  fi

  local actual
  case "$field" in
    contact_count)
      actual=$(get_contact_count "$port" "$hash")
      ;;
    notification_count)
      actual=$(get_notification_count "$port" "$hash")
      ;;
    pending_requests)
      actual=$(get_pending_requests "$port" "$hash")
      ;;
    schema_record_count)
      actual=$(get_schema_record_count "$port" "$hash" "$schema")
      ;;
    shared_record_count)
      # Counts records in schema whose author_pub_key matches another role's public_key.
      # assertion YAML: { node: bob, field: shared_record_count, schema: Photography, author_role: alice, op: ">=", value: 1 }
      local author_role author_pk
      author_role=$(echo "$assertion" | jq -r '.author_role // ""')
      [[ -n "$author_role" ]] || { echo "[FAIL] shared_record_count: author_role required" >&2; return 1; }
      author_pk=$(jq -r --arg role "$author_role" '.[] | select(.role == $role) | .public_key' "$nodes_json")
      [[ -n "$author_pk" && "$author_pk" != "null" ]] || {
        echo "[FAIL] shared_record_count: no public_key for role $author_role" >&2
        return 1
      }
      actual=$(get_shared_record_count "$port" "$hash" "$schema" "$author_pk")
      ;;
    my_pseudonym_count)
      actual=$(curl -fsS "http://127.0.0.1:$port/api/discovery/my-pseudonyms" \
        -H "X-User-Hash: $hash" | jq '.count // 0')
      ;;
    org_member_count)
      # assertion YAML: { node: alice, field: org_member_count, org_role: alice, op: ">=", value: 2 }
      # `org_role` names the role that ran `org_create`; we resolve its org_hash
      # from the state file dropped by the org_create action.
      local org_role create_file org_hash_ref
      org_role=$(echo "$assertion" | jq -r '.org_role // ""')
      [[ -n "$org_role" ]] || { echo "[FAIL] org_member_count: org_role required" >&2; return 1; }
      create_file="$FOLDDB_TEST_SESSION_DIR/state/org-create-$org_role.json"
      [[ -f "$create_file" ]] || {
        echo "[FAIL] org_member_count: no org-create state for role $org_role (file=$create_file)" >&2
        return 1
      }
      org_hash_ref=$(jq -r '(.data.org.org_hash // .org.org_hash // "")' "$create_file")
      [[ -n "$org_hash_ref" && "$org_hash_ref" != "null" ]] || {
        echo "[FAIL] org_member_count: no org_hash in $create_file" >&2
        return 1
      }
      actual=$(get_org_member_count "$port" "$hash" "$org_hash_ref")
      ;;
    *)
      echo "[FAIL] unknown field: $field" >&2
      return 1 ;;
  esac

  local label="$node.$field"
  [[ -n "$schema" ]] && label="$label[$schema]"

  case "$op" in
    "==") assert_eq "$actual" "$value" "$label" ;;
    ">=") assert_ge "$actual" "$value" "$label" ;;
    "<=")
      if (( actual <= value )); then
        echo "[PASS] $label: $actual <= $value"
      else
        echo "[FAIL] $label: expected<=$value actual=$actual" >&2
        return 1
      fi ;;
    ">")
      if (( actual > value )); then
        echo "[PASS] $label: $actual > $value"
      else
        echo "[FAIL] $label: expected>$value actual=$actual" >&2
        return 1
      fi ;;
    *)
      echo "[FAIL] unknown op: $op" >&2
      return 1 ;;
  esac
}

# run_assertions NODES_JSON SCENARIO_YAML
# Returns 0 if all pass, non-zero on any failure.
run_assertions() {
  local nodes_json="$1" scenario="$2"
  local count pass fail
  count=$(yq '.assertions | length' "$scenario")
  pass=0
  fail=0
  if [[ "$count" == "0" || "$count" == "null" ]]; then
    echo "[assertions] no assertions in scenario"
    return 0
  fi

  echo "[assertions] running $count assertions"
  local i
  for ((i=0; i<count; i++)); do
    local assertion
    assertion=$(yq -o=json ".assertions[$i]" "$scenario")
    if run_assertion "$nodes_json" "$assertion"; then
      pass=$((pass+1))
    else
      fail=$((fail+1))
    fi
  done

  echo "[assertions] $pass passed, $fail failed"
  [[ "$fail" == "0" ]]
}
