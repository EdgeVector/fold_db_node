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
    | jq 'length'
}

get_notification_count() {
  local port="$1" hash="$2"
  curl -fsS "http://127.0.0.1:$port/api/notifications" -H "X-User-Hash: $hash" \
    | jq 'length'
}

get_pending_requests() {
  local port="$1" hash="$2"
  curl -fsS "http://127.0.0.1:$port/api/discovery/connection-requests" \
    -H "X-User-Hash: $hash" \
    | jq '[.[] | select(.status == "pending")] | length'
}
