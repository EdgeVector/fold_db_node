#!/usr/bin/env bash
# Step executor for the test framework.
# Iterates scenario steps, dispatches actions inline against node HTTP APIs.
#
# On polling timeouts, emit the last response to stderr so schema drift is
# visible as "actual payload" rather than a generic "timed out" error.
set -euo pipefail

# execute_action NODES_JSON ROLE FRAMEWORK_DIR ACTION_JSON
# Dispatches an action to the matching recipe. Sets NODE_PORT + USER_HASH from nodes.json.
execute_action() {
  local nodes_json="$1" role="$2" framework_dir="$3" action_json="$4"
  local action
  action=$(echo "$action_json" | jq -r '.action // ""')
  [[ -n "$action" ]] || { echo "[step] missing action name" >&2; return 1; }

  # Look up node info for this role
  local port hash public_key
  port=$(jq -r --arg role "$role" '.[] | select(.role == $role) | .port' "$nodes_json")
  hash=$(jq -r --arg role "$role" '.[] | select(.role == $role) | .hash' "$nodes_json")
  public_key=$(jq -r --arg role "$role" '.[] | select(.role == $role) | .public_key' "$nodes_json")

  if [[ -z "$port" || -z "$hash" ]]; then
    echo "[step] role $role not found in nodes.json" >&2
    return 1
  fi

  export NODE_PORT="$port"
  export USER_HASH="$hash"
  export NODE_PUBLIC_KEY="$public_key"
  export GSTACK_PORT="${GSTACK_PORT:-9400}"
  export FOLDDB_TEST_FRAMEWORK_DIR="$framework_dir"

  local recipes="$framework_dir/recipes"

  echo "[step] $role.$action"

  case "$action" in
    ingest_photo)
      # Actions: { action: ingest_photo, fixture: "photo" }
      local fixture_key fixture_path
      fixture_key=$(echo "$action_json" | jq -r '.fixture // "photo"')
      # Resolve fixture path from session dir or framework fixtures/
      fixture_path="$framework_dir/fixtures/$(basename "$fixture_key")"
      if [[ ! -f "$fixture_path" ]]; then
        # Try direct lookup in fixtures/
        fixture_path=$(ls "$framework_dir/fixtures/"*.jpg 2>/dev/null | head -1)
      fi
      [[ -f "$fixture_path" ]] || { echo "[step] no fixture found" >&2; return 1; }
      # Copy fixture to a node-local folder for scanning
      local scan_dir="$FOLDDB_TEST_SESSION_DIR/scan-$role"
      mkdir -p "$scan_dir"
      cp "$fixture_path" "$scan_dir/"
      echo "[step] copied $fixture_path to $scan_dir"

      # Smart folder scan is async: POST returns progress_id, poll for result
      local scan_start progress_id
      scan_start=$(curl -fsS -X POST "http://127.0.0.1:$port/api/ingestion/smart-folder/scan" \
        -H "Content-Type: application/json" \
        -H "X-User-Hash: $hash" \
        -d "{\"folder_path\":\"$scan_dir\"}")
      progress_id=$(echo "$scan_start" | jq -r '.progress_id // ""')
      [[ -n "$progress_id" ]] || { echo "[step] no progress_id from scan" >&2; return 1; }
      echo "[step] scan started: $progress_id"

      # Poll for scan result
      local scan_result="" files_to_ingest="[]"
      local poll_i
      for ((poll_i=0; poll_i<60; poll_i++)); do
        scan_result=$(curl -fsS "http://127.0.0.1:$port/api/ingestion/smart-folder/scan/$progress_id" \
          -H "X-User-Hash: $hash" 2>/dev/null || echo '{}')
        local recs
        recs=$(echo "$scan_result" | jq '(.recommended_files // []) | length' 2>/dev/null || echo 0)
        if (( recs > 0 )); then
          files_to_ingest=$(echo "$scan_result" | jq -c '[.recommended_files[].path]')
          echo "[step] scan complete: $recs files"
          break
        fi
        sleep 1
      done
      [[ "$files_to_ingest" == "[]" ]] && { echo "[step] scan returned 0 files (details: $(echo "$scan_result" | head -c 300))" >&2; return 1; }

      # Ingest with file list
      local ingest_resp
      ingest_resp=$(curl -fsS -X POST "http://127.0.0.1:$port/api/ingestion/smart-folder/ingest" \
        -H "Content-Type: application/json" \
        -H "X-User-Hash: $hash" \
        -d "{\"folder_path\":\"$scan_dir\",\"files_to_ingest\":$files_to_ingest,\"spend_limit\":2.0,\"auto_execute\":true}" \
        2>&1) || {
          echo "[step] ingest failed: $ingest_resp" >&2
          return 1
        }
      echo "[step] ingest response: $(echo "$ingest_resp" | head -c 300)"
      # Wait for ingestion to complete (async)
      local i
      for ((i=0; i<120; i++)); do
        local n
        n=$(curl -fsS "http://127.0.0.1:$port/api/schemas" -H "X-User-Hash: $hash" \
          2>/dev/null | jq '[.schemas[] | select(.descriptive_name == "Photography")] | length' 2>/dev/null || echo 0)
        if (( n > 0 )); then
          # Check if records exist
          local schema_hash
          schema_hash=$(curl -fsS "http://127.0.0.1:$port/api/schemas" -H "X-User-Hash: $hash" \
            | jq -r '.schemas[] | select(.descriptive_name == "Photography") | .name' | head -1)
          if [[ -n "$schema_hash" ]]; then
            local recs
            recs=$(curl -fsS "http://127.0.0.1:$port/api/schema/$schema_hash/keys" \
              -H "X-User-Hash: $hash" 2>/dev/null | jq '.total_count // ((.keys // []) | length)' 2>/dev/null || echo 0)
            if (( recs > 0 )); then
              # Mutations written — now wait for face detection to complete
              # (face detection runs async after mutations; takes time for model load + detection)
              local first_key
              first_key=$(curl -fsS "http://127.0.0.1:$port/api/schema/$schema_hash/keys" \
                -H "X-User-Hash: $hash" 2>/dev/null \
                | jq -r '(.keys // [])[0].hash // (.keys // [])[0].range // ""')
              if [[ -n "$first_key" && "$first_key" != "null" ]]; then
                local face_wait
                for ((face_wait=0; face_wait<90; face_wait++)); do
                  local face_count
                  face_count=$(curl -fsS "http://127.0.0.1:$port/api/discovery/faces/$schema_hash/$first_key" \
                    -H "X-User-Hash: $hash" 2>/dev/null | jq '(.faces // []) | length' 2>/dev/null || echo 0)
                  if (( face_count > 0 )); then
                    echo "[step] ingestion complete: $recs records, $face_count faces after $((i + face_wait))s"
                    return 0
                  fi
                  sleep 1
                done
                echo "[step] ingestion: $recs records, NO faces detected (face detection may have failed or no faces in image)"
              fi
              echo "[step] ingestion complete: $recs records after ${i}s"
              return 0
            fi
          fi
        fi
        sleep 1
      done
      # On timeout, dump the final state so we can see whether schemas never
      # loaded, records never appeared, or face detection hung. Without this
      # the caller just sees "timed out" and has to rerun with --keep-session.
      local last_schemas
      last_schemas=$(curl -fsS "http://127.0.0.1:$port/api/schemas" \
        -H "X-User-Hash: $hash" 2>/dev/null | head -c 800)
      echo "[step] ingestion timed out after 120s" >&2
      echo "[step]   last /api/schemas: $last_schemas" >&2
      return 1
      ;;

    opt_in_photography)
      local publish_faces schema_hash
      publish_faces=$(echo "$action_json" | jq -r '.publish_faces // false')
      # Resolve Photography schema hash
      schema_hash=$(curl -fsS "http://127.0.0.1:$port/api/schemas" -H "X-User-Hash: $hash" \
        | jq -r '.schemas[] | select(.descriptive_name == "Photography") | .name' | head -1)
      [[ -n "$schema_hash" ]] || { echo "[step] Photography schema not found" >&2; return 1; }
      curl -fsS -X POST "http://127.0.0.1:$port/api/discovery/opt-in" \
        -H "Content-Type: application/json" \
        -H "X-User-Hash: $hash" \
        -d "{\"schema_name\":\"$schema_hash\",\"category\":\"Photography\",\"include_preview\":false,\"publish_faces\":$publish_faces}" \
        > /dev/null
      echo "[step] opted in Photography (publish_faces=$publish_faces)"
      ;;

    opt_in_all)
      curl -fsS "http://127.0.0.1:$port/api/schemas" -H "X-User-Hash: $hash" \
        | jq -c '.schemas[] | {name: .name, descriptive_name: .descriptive_name}' \
        | while IFS= read -r s; do
            local name cat
            name=$(echo "$s" | jq -r '.name')
            cat=$(echo "$s" | jq -r '.descriptive_name')
            curl -fsS -X POST "http://127.0.0.1:$port/api/discovery/opt-in" \
              -H "Content-Type: application/json" \
              -H "X-User-Hash: $hash" \
              -d "{\"schema_name\":\"$name\",\"category\":\"$cat\",\"include_preview\":false,\"publish_faces\":false}" \
              > /dev/null || true
          done
      echo "[step] opted in all schemas"
      ;;

    publish)
      local resp
      resp=$(curl -fsS -X POST "http://127.0.0.1:$port/api/discovery/publish" \
        -H "Content-Type: application/json" \
        -H "X-User-Hash: $hash" -d '{}')
      local accepted quarantined
      accepted=$(echo "$resp" | jq -r '.accepted // 0')
      quarantined=$(echo "$resp" | jq -r '.quarantined // 0')
      echo "[step] published: accepted=$accepted quarantined=$quarantined"
      ;;

    face_search)
      # Action: { action: face_search, face_index: 0, schema?: "Photography", record_key?: "..." }
      local face_index schema_name schema_hash record_key
      face_index=$(echo "$action_json" | jq -r '.face_index // 0')
      schema_name=$(echo "$action_json" | jq -r '.schema // "Photography"')
      schema_hash=$(curl -fsS "http://127.0.0.1:$port/api/schemas" -H "X-User-Hash: $hash" \
        | jq -r --arg name "$schema_name" '.schemas[] | select(.descriptive_name == $name) | .name' | head -1)
      [[ -n "$schema_hash" ]] || { echo "[step] schema $schema_name not found" >&2; return 1; }
      # Get the first record key
      record_key=$(echo "$action_json" | jq -r '.record_key // ""')
      if [[ -z "$record_key" ]]; then
        # /api/schema/{name}/keys returns {keys: [{hash, range}, ...]}
        record_key=$(curl -fsS "http://127.0.0.1:$port/api/schema/$schema_hash/keys" \
          -H "X-User-Hash: $hash" 2>/dev/null \
          | jq -r '(.keys // [])[0].hash // (.keys // [])[0].range // ""')
      fi
      [[ -n "$record_key" && "$record_key" != "null" ]] || { echo "[step] no records to face-search" >&2; return 1; }
      local resp
      resp=$(curl -fsS -X POST "http://127.0.0.1:$port/api/discovery/face-search" \
        -H "Content-Type: application/json" \
        -H "X-User-Hash: $hash" \
        -d "{\"source_schema\":\"$schema_hash\",\"source_key\":\"$record_key\",\"face_index\":$face_index,\"top_k\":50}")
      local n
      n=$(echo "$resp" | jq '.results | length')
      echo "[step] face_search returned $n results"
      # Save results for subsequent actions (expect_results_min, connect)
      echo "$resp" > "$FOLDDB_TEST_SESSION_DIR/state/last-face-search-$role.json"
      ;;

    expect_results_min)
      local min
      min=$(echo "$action_json" | jq -r '.value // 1')
      local file="$FOLDDB_TEST_SESSION_DIR/state/last-face-search-$role.json"
      [[ -f "$file" ]] || { echo "[step] no prior face_search results" >&2; return 1; }
      local n
      n=$(jq '.results | length' "$file")
      if (( n >= min )); then
        echo "[step] expect_results_min: $n >= $min ✓"
      else
        echo "[step] expect_results_min FAIL: $n < $min" >&2
        return 1
      fi
      ;;

    connect_all_results)
      # Action: { action: connect_all_results, message, role, top_k }
      # Connects to each result from the last face_search (any role).
      # Only the one that's actually owned by Alice will route to her; others spray nowhere.
      local source_role message connect_role top_k
      source_role=$(echo "$action_json" | jq -r '.source_role // ""')
      message=$(echo "$action_json" | jq -r '.message // "E2E test connect"')
      connect_role=$(echo "$action_json" | jq -r '.role // "acquaintance"')
      top_k=$(echo "$action_json" | jq -r '.top_k // 5')
      local search_file
      if [[ -n "$source_role" ]]; then
        search_file="$FOLDDB_TEST_SESSION_DIR/state/last-face-search-$source_role.json"
      else
        search_file="$FOLDDB_TEST_SESSION_DIR/state/last-face-search-$role.json"
      fi
      [[ -f "$search_file" ]] || { echo "[step] no face search results in $search_file" >&2; return 1; }
      local results_count
      results_count=$(jq '.results | length' "$search_file")
      local effective_k=$(( results_count < top_k ? results_count : top_k ))
      echo "[step] $role connecting to top $effective_k of $results_count face search results"
      local connected=0 k
      for ((k=0; k<effective_k; k++)); do
        local pseudo
        pseudo=$(jq -r ".results[$k].pseudonym" "$search_file")
        [[ "$pseudo" == "null" || -z "$pseudo" ]] && continue
        if curl -fsS -X POST "http://127.0.0.1:$port/api/discovery/connect" \
          -H "Content-Type: application/json" \
          -H "X-User-Hash: $hash" \
          -d "{\"target_pseudonym\":\"$pseudo\",\"message\":\"$message\",\"preferred_role\":\"$connect_role\"}" \
          > /dev/null 2>&1; then
          connected=$((connected+1))
          echo "[step]   -> connected to $pseudo"
        else
          echo "[step]   -> skipped $pseudo (connect failed)"
        fi
      done
      echo "[step] $role connected to $connected/$effective_k pseudonyms"
      [[ "$connected" -gt 0 ]] || return 1
      ;;

    export_pseudonyms)
      # Save this role's pseudonym list to state for cross-node reference.
      # Other nodes can then connect to "role.pseudonym[i]".
      local resp
      resp=$(curl -fsS "http://127.0.0.1:$port/api/discovery/my-pseudonyms" \
        -H "X-User-Hash: $hash")
      echo "$resp" > "$FOLDDB_TEST_SESSION_DIR/state/pseudonyms-$role.json"
      local n
      n=$(echo "$resp" | jq '.count // ((.pseudonyms // []) | length)')
      echo "[step] exported $n pseudonyms for $role"
      ;;

    connect)
      # Action: { action: connect, target: "last_face_search[0]" |
      #                                    "<role>.face_search[i]" |
      #                                    "<role>.pseudonym[i]" |
      #                                    "<uuid>", message, role }
      local target message connect_role
      target=$(echo "$action_json" | jq -r '.target // "last_face_search[0]"')
      message=$(echo "$action_json" | jq -r '.message // "E2E test connect"')
      connect_role=$(echo "$action_json" | jq -r '.role // "acquaintance"')
      local target_pseudonym=""
      if [[ "$target" == last_face_search* ]]; then
        local idx
        idx=$(echo "$target" | sed -E 's/last_face_search\[([0-9]+)\]/\1/')
        [[ "$idx" == "$target" ]] && idx=0
        target_pseudonym=$(jq -r ".results[$idx].pseudonym" \
          "$FOLDDB_TEST_SESSION_DIR/state/last-face-search-$role.json")
      elif [[ "$target" == *.face_search* ]]; then
        # Reference like "alice.face_search[0]" — another role's face-search results
        local target_role idx
        target_role=$(echo "$target" | cut -d. -f1)
        idx=$(echo "$target" | sed -E 's/.*face_search\[([0-9]+)\]/\1/')
        [[ "$idx" == "$target" ]] && idx=0
        target_pseudonym=$(jq -r ".results[$idx].pseudonym" \
          "$FOLDDB_TEST_SESSION_DIR/state/last-face-search-$target_role.json")
      elif [[ "$target" == *.pseudonym* ]]; then
        # Reference like "alice.pseudonym[0]" — look up another role's exported pseudonyms
        local target_role idx
        target_role=$(echo "$target" | cut -d. -f1)
        idx=$(echo "$target" | sed -E 's/.*pseudonym\[([0-9]+)\]/\1/')
        [[ "$idx" == "$target" ]] && idx=0
        target_pseudonym=$(jq -r ".pseudonyms[$idx]" \
          "$FOLDDB_TEST_SESSION_DIR/state/pseudonyms-$target_role.json")
      else
        target_pseudonym="$target"
      fi
      [[ -n "$target_pseudonym" && "$target_pseudonym" != "null" ]] || {
        echo "[step] no target pseudonym (target=$target)" >&2; return 1;
      }
      curl -fsS -X POST "http://127.0.0.1:$port/api/discovery/connect" \
        -H "Content-Type: application/json" \
        -H "X-User-Hash: $hash" \
        -d "{\"target_pseudonym\":\"$target_pseudonym\",\"message\":\"$message\",\"preferred_role\":\"$connect_role\"}" \
        > /dev/null
      echo "[step] $role → connect to $target_pseudonym"
      ;;

    poll_requests)
      # Also processes data_share messages (they share the poll loop on the backend).
      curl -fsS "http://127.0.0.1:$port/api/discovery/connection-requests" \
        -H "X-User-Hash: $hash" > "$FOLDDB_TEST_SESSION_DIR/state/last-poll-$role.json"
      local n
      n=$(jq '(.requests // .) | length' "$FOLDDB_TEST_SESSION_DIR/state/last-poll-$role.json" 2>/dev/null || echo 0)
      echo "[step] $role polled: $n requests"
      ;;

    expect_pending_min)
      # Poll with retry until pending count reaches the threshold (or timeout).
      # Action: { action: expect_pending_min, value: 1, timeout_seconds: 60 }
      local min to_secs pwait_i pending
      min=$(echo "$action_json" | jq -r '.value // 1')
      to_secs=$(echo "$action_json" | jq -r '.timeout_seconds // 60')
      for ((pwait_i=0; pwait_i<to_secs; pwait_i++)); do
        curl -fsS "http://127.0.0.1:$port/api/discovery/connection-requests" \
          -H "X-User-Hash: $hash" > "$FOLDDB_TEST_SESSION_DIR/state/last-poll-$role.json"
        pending=$(jq '[(.requests // .)[] | select(.status == "pending")] | length' \
          "$FOLDDB_TEST_SESSION_DIR/state/last-poll-$role.json" 2>/dev/null || echo 0)
        if (( pending >= min )); then
          echo "[step] $role pending: $pending >= $min ✓ (after ${pwait_i}s)"
          return 0
        fi
        sleep 1
      done
      echo "[step] $role expect_pending_min FAIL: $pending < $min after ${to_secs}s" >&2
      return 1
      ;;

    expect_notification_min)
      # Action: { action: expect_notification_min, value: 1, timeout_seconds: 60 }
      local min to_secs nwait_i ncount
      min=$(echo "$action_json" | jq -r '.value // 1')
      to_secs=$(echo "$action_json" | jq -r '.timeout_seconds // 60')
      for ((nwait_i=0; nwait_i<to_secs; nwait_i++)); do
        # Trigger backend polling first — this decrypts any new bulletin board messages
        # including data shares, which then generate notifications.
        curl -fsS "http://127.0.0.1:$port/api/discovery/connection-requests" \
          -H "X-User-Hash: $hash" > /dev/null
        ncount=$(curl -fsS "http://127.0.0.1:$port/api/notifications" \
          -H "X-User-Hash: $hash" | jq '.count // 0')
        if (( ncount >= min )); then
          echo "[step] $role notifications: $ncount >= $min ✓ (after ${nwait_i}s)"
          return 0
        fi
        sleep 1
      done
      echo "[step] $role expect_notification_min FAIL: $ncount < $min after ${to_secs}s" >&2
      return 1
      ;;

    accept_request)
      # Use the last poll result if present, otherwise poll fresh
      local req_file req_id
      req_file="$FOLDDB_TEST_SESSION_DIR/state/last-poll-$role.json"
      if [[ ! -f "$req_file" ]]; then
        curl -fsS "http://127.0.0.1:$port/api/discovery/connection-requests" \
          -H "X-User-Hash: $hash" > "$req_file"
      fi
      req_id=$(jq -r '(.requests // .)[] | select(.status == "pending") | .request_id' "$req_file" | head -1)
      [[ -n "$req_id" && "$req_id" != "null" ]] || { echo "[step] no pending request to accept" >&2; return 1; }
      local accept_resp
      if ! accept_resp=$(curl -fsS -X POST "http://127.0.0.1:$port/api/discovery/connection-requests/respond" \
        -H "Content-Type: application/json" \
        -H "X-User-Hash: $hash" \
        -d "{\"request_id\":\"$req_id\",\"action\":\"accept\",\"role\":\"friend\"}" 2>&1); then
        echo "[step] accept failed: $accept_resp" >&2
        return 1
      fi
      echo "[step] accepted request $req_id"
      ;;

    share_record)
      local target_role contact_pk schema_name schema_hash record_key
      target_role=$(echo "$action_json" | jq -r '.to // ""')
      schema_name=$(echo "$action_json" | jq -r '.schema // "Photography"')
      # Look up target contact pub key from our contact book
      contact_pk=$(curl -fsS "http://127.0.0.1:$port/api/contacts" -H "X-User-Hash: $hash" \
        | jq -r '(.contacts // .)[0].public_key')
      [[ -n "$contact_pk" && "$contact_pk" != "null" ]] || { echo "[step] no contact to share with" >&2; return 1; }
      schema_hash=$(curl -fsS "http://127.0.0.1:$port/api/schemas" -H "X-User-Hash: $hash" \
        | jq -r --arg name "$schema_name" '.schemas[] | select(.descriptive_name == $name) | .name' | head -1)
      record_key=$(curl -fsS "http://127.0.0.1:$port/api/schema/$schema_hash/keys" \
        -H "X-User-Hash: $hash" | jq -r '(.keys // [])[0].hash // (.keys // [])[0].range // ""')
      curl -fsS -X POST "http://127.0.0.1:$port/api/discovery/share" \
        -H "Content-Type: application/json" \
        -H "X-User-Hash: $hash" \
        -d "{\"recipient_public_key\":\"$contact_pk\",\"records\":[{\"schema_name\":\"$schema_hash\",\"record_key\":\"$record_key\"}]}" \
        > /dev/null
      echo "[step] shared record with $contact_pk"
      ;;

    sleep)
      local secs
      secs=$(echo "$action_json" | jq -r '.seconds // 1')
      sleep "$secs"
      ;;

    *)
      echo "[step] unknown action: $action" >&2
      return 1
      ;;
  esac
}

# run_steps NODES_JSON SCENARIO_YAML FRAMEWORK_DIR
# Executes all scenario steps sequentially. Each step runs its actions for each role.
run_steps() {
  local nodes_json="$1" scenario="$2" framework_dir="$3"
  local step_count
  step_count=$(yq '.steps | length' "$scenario")
  if [[ "$step_count" == "0" || "$step_count" == "null" ]]; then
    echo "[steps] no steps in scenario"
    return 0
  fi
  echo "[steps] running $step_count steps"

  local i
  for ((i=0; i<step_count; i++)); do
    local step_id roles action_count j k
    step_id=$(yq -r ".steps[$i].id" "$scenario")
    roles=$(yq -r ".steps[$i].roles[]" "$scenario")
    action_count=$(yq ".steps[$i].actions | length" "$scenario")
    echo "[steps] step[$i] $step_id (roles=$(echo "$roles" | tr '\n' ','))"

    for role in $roles; do
      for ((j=0; j<action_count; j++)); do
        local action_json
        action_json=$(yq -o=json ".steps[$i].actions[$j]" "$scenario")
        if ! execute_action "$nodes_json" "$role" "$framework_dir" "$action_json"; then
          echo "[steps] step $step_id FAILED on role=$role action[$j]" >&2
          return 1
        fi
      done
    done
  done

  echo "[steps] all $step_count steps complete"
  return 0
}
