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
  local port hash public_key gstack_port
  port=$(jq -r --arg role "$role" '.[] | select(.role == $role) | .port' "$nodes_json")
  hash=$(jq -r --arg role "$role" '.[] | select(.role == $role) | .hash' "$nodes_json")
  public_key=$(jq -r --arg role "$role" '.[] | select(.role == $role) | .public_key' "$nodes_json")
  gstack_port=$(jq -r --arg role "$role" '.[] | select(.role == $role) | .gstack_port // 9400' "$nodes_json")

  if [[ -z "$port" || -z "$hash" ]]; then
    echo "[step] role $role not found in nodes.json" >&2
    return 1
  fi

  export NODE_PORT="$port"
  export USER_HASH="$hash"
  export NODE_PUBLIC_KEY="$public_key"
  # Per-node gstack daemon port — isolates each node's browser session so
  # parallel UI recipes across roles cannot clobber each other's tabs/cookies.
  export GSTACK_PORT="$gstack_port"
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

      # Poll for scan result. The smart-folder scan endpoint returns:
      #   200 + { recommended_files: [...] }  → done
      #   404 + { error: "Scan not yet complete" } → still processing (poll again)
      #   5xx / connection error → real failure
      # Capture HTTP status separately so 404 (in-progress) doesn't get conflated
      # with a broken endpoint. Bail after 5 consecutive *non-200, non-404*
      # responses (or curl transport failures) so a genuinely dead endpoint fails
      # fast instead of silently hanging 60s.
      local scan_result="" files_to_ingest="[]"
      local poll_i scan_errs=0
      for ((poll_i=0; poll_i<60; poll_i++)); do
        local scan_raw scan_code curl_rc
        scan_raw=$(curl -sS --max-time 5 -w '\n__SCAN_HTTP__%{http_code}' \
            "http://127.0.0.1:$port/api/ingestion/smart-folder/scan/$progress_id" \
            -H "X-User-Hash: $hash" 2>&1) && curl_rc=0 || curl_rc=$?
        if (( curl_rc != 0 )); then
          scan_errs=$((scan_errs + 1))
          if (( scan_errs >= 5 )); then
            echo "[step] scan endpoint failing repeatedly (curl rc=$curl_rc): $scan_raw" >&2
            return 1
          fi
          sleep 1
          continue
        fi
        scan_code="${scan_raw##*__SCAN_HTTP__}"
        scan_result="${scan_raw%$'\n'__SCAN_HTTP__*}"
        case "$scan_code" in
          200) scan_errs=0 ;;
          404)
            # Scan still running — poll again without burning the error budget.
            sleep 1
            continue
            ;;
          *)
            scan_errs=$((scan_errs + 1))
            if (( scan_errs >= 5 )); then
              echo "[step] scan endpoint failing repeatedly (http $scan_code): $scan_result" >&2
              return 1
            fi
            sleep 1
            continue
            ;;
        esac
        local recs
        recs=$(echo "$scan_result" | jq '(.recommended_files // []) | length')
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
      # Wait for ingestion to complete (async). Same fail-fast pattern: bail
      # after 5 consecutive HTTP errors so a dead /api/schemas endpoint fails
      # loudly instead of hanging the full 120s window.
      local i ing_errs=0
      for ((i=0; i<120; i++)); do
        local schemas_resp n
        if ! schemas_resp=$(curl -fsS --max-time 5 \
            "http://127.0.0.1:$port/api/schemas" -H "X-User-Hash: $hash" 2>&1); then
          ing_errs=$((ing_errs + 1))
          if (( ing_errs >= 5 )); then
            echo "[step] /api/schemas failing repeatedly: $schemas_resp" >&2
            return 1
          fi
          sleep 1
          continue
        fi
        ing_errs=0
        # Count + look up Photography only when state=Approved. Available copies
        # left over from prior dev-cloud test runs are loaded but unqueryable;
        # filtering avoids picking one and timing out waiting for records.
        n=$(echo "$schemas_resp" | jq '[.schemas[] | select(.descriptive_name == "Photography" and .state == "Approved")] | length')
        if (( n > 0 )); then
          # Check if records exist
          local schema_hash
          schema_hash=$(curl -fsS "http://127.0.0.1:$port/api/schemas" -H "X-User-Hash: $hash" \
            | jq -r '.schemas[] | select(.descriptive_name == "Photography" and .state == "Approved") | .name' | head -1)
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
                echo "[step] ERROR: ingestion wrote $recs records but face detection produced 0 faces after 90s." >&2
                echo "[step] Either the folddb_server binary was built without --features face-detection," >&2
                echo "[step] the face detection model failed to load, or the fixture contains no detectable face." >&2
                echo "[step] Check nodes/$role/stderr.log for face processor errors." >&2
                return 1
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
      # Resolve Photography schema hash; require Approved state — see ingest_photo
      # poll loop above for the dev-cloud "duplicate descriptive_name" rationale.
      schema_hash=$(curl -fsS "http://127.0.0.1:$port/api/schemas" -H "X-User-Hash: $hash" \
        | jq -r '.schemas[] | select(.descriptive_name == "Photography" and .state == "Approved") | .name' | head -1)
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
        | jq -r --arg name "$schema_name" '.schemas[] | select(.descriptive_name == $name and .state == "Approved") | .name' | head -1)
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
      # Capture HTTP status + body separately so a non-2xx response hard-fails
      # with the actual error body instead of silently reporting 0 results.
      local resp http_body http_code
      resp=$(curl -sS -o /tmp/folddb-face-search-$$.body -w '%{http_code}' \
        -X POST "http://127.0.0.1:$port/api/discovery/face-search" \
        -H "Content-Type: application/json" \
        -H "X-User-Hash: $hash" \
        -d "{\"source_schema\":\"$schema_hash\",\"source_key\":\"$record_key\",\"face_index\":$face_index,\"top_k\":50}")
      http_code="$resp"
      http_body=$(cat /tmp/folddb-face-search-$$.body 2>/dev/null || echo '')
      rm -f /tmp/folddb-face-search-$$.body
      if [[ "$http_code" != "200" ]]; then
        echo "[step] face_search FAILED: HTTP $http_code body=$(echo "$http_body" | head -c 300)" >&2
        return 1
      fi
      resp="$http_body"
      local n
      n=$(echo "$resp" | jq '.results | length' 2>/dev/null || echo "")
      [[ -n "$n" ]] || { echo "[step] face_search: response missing .results array: $(echo "$resp" | head -c 300)" >&2; return 1; }
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
        | jq -r --arg name "$schema_name" '.schemas[] | select(.descriptive_name == $name and .state == "Approved") | .name' | head -1)
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

    org_create)
      # Action: { action: org_create, name: "qa-dogfood-org" }
      # Creates an org on this node. Saves the full CreateOrgResponse (incl.
      # invite_bundle + org.org_hash) so org_join on another role can reference it.
      local org_name
      org_name=$(echo "$action_json" | jq -r '.name // "qa-org"')
      local resp
      if ! resp=$(curl -fsS -X POST "http://127.0.0.1:$port/api/org" \
          -H 'Content-Type: application/json' \
          -H "X-User-Hash: $hash" \
          -d "{\"name\":\"$org_name\"}" 2>&1); then
        echo "[step] org_create failed: $resp" >&2
        return 1
      fi
      # Handler returns ApiResponse<CreateOrgResponse> — success body is .data
      # but older callers expect the inner fields. Save the whole response and
      # unwrap in org_join below so both shapes work.
      echo "$resp" > "$FOLDDB_TEST_SESSION_DIR/state/org-create-$role.json"
      local org_hash
      org_hash=$(echo "$resp" | jq -r '(.data.org.org_hash // .org.org_hash // "")')
      [[ -n "$org_hash" && "$org_hash" != "null" ]] || {
        echo "[step] org_create: no org_hash in response: $(echo "$resp" | head -c 300)" >&2
        return 1
      }
      echo "[step] $role created org '$org_name' hash=$org_hash"
      ;;

    org_join)
      # Action: { action: org_join, from_role: alice }
      # Reads the org-create state from $from_role and POSTs the invite bundle.
      local from_role
      from_role=$(echo "$action_json" | jq -r '.from_role // ""')
      [[ -n "$from_role" ]] || { echo "[step] org_join: from_role required" >&2; return 1; }
      local create_file="$FOLDDB_TEST_SESSION_DIR/state/org-create-$from_role.json"
      [[ -f "$create_file" ]] || {
        echo "[step] org_join: no org-create state for role $from_role (file=$create_file)" >&2
        return 1
      }
      local bundle
      bundle=$(jq -c '.data.invite_bundle // .invite_bundle' "$create_file")
      [[ -n "$bundle" && "$bundle" != "null" ]] || {
        echo "[step] org_join: no invite_bundle in $create_file" >&2
        return 1
      }
      local resp
      if ! resp=$(curl -fsS -X POST "http://127.0.0.1:$port/api/org/join" \
          -H 'Content-Type: application/json' \
          -H "X-User-Hash: $hash" \
          -d "$bundle" 2>&1); then
        echo "[step] org_join failed: $resp" >&2
        return 1
      fi
      local joined_hash
      joined_hash=$(echo "$resp" | jq -r '(.data.org.org_hash // .org.org_hash // "")')
      [[ -n "$joined_hash" && "$joined_hash" != "null" ]] || {
        echo "[step] org_join: no org_hash in response: $(echo "$resp" | head -c 300)" >&2
        return 1
      }
      echo "[step] $role joined org hash=$joined_hash (from $from_role)"
      ;;

    sync_trigger)
      # Action: { action: sync_trigger }
      # Forces a personal+org sync cycle on this node (upload pending + poll).
      # Does not wait for completion — use `sleep` after to let the cycle run.
      if ! curl -fsS -X POST "http://127.0.0.1:$port/api/sync/trigger" \
          -H 'Content-Type: application/json' \
          -H "X-User-Hash: $hash" \
          -d '{}' > /dev/null 2>&1; then
        echo "[step] sync_trigger failed on $role" >&2
        return 1
      fi
      echo "[step] $role triggered sync"
      ;;

    schema_register)
      # Action: { action: schema_register, fixture: "<file under test-framework/fixtures/>", persist_as: "<key>" }
      # Registers a fixture schema on the dev schema service (global registry),
      # then loads + approves it on this node. Writes the returned identity-hash
      # schema name to state/schema-$persist_as.json so later actions on any
      # role can reference the same schema by persist_as.
      local sr_fixture sr_persist_as sr_path
      sr_fixture=$(echo "$action_json" | jq -r '.fixture // ""')
      sr_persist_as=$(echo "$action_json" | jq -r '.persist_as // ""')
      [[ -n "$sr_fixture" ]]    || { echo "[step] schema_register: fixture required" >&2; return 1; }
      [[ -n "$sr_persist_as" ]] || { echo "[step] schema_register: persist_as required" >&2; return 1; }
      sr_path="$framework_dir/fixtures/$sr_fixture"
      [[ -f "$sr_path" ]] || { echo "[step] schema_register: fixture not found: $sr_path" >&2; return 1; }

      local sr_state="$FOLDDB_TEST_SESSION_DIR/state/schema-$sr_persist_as.json"
      local sr_name=""
      if [[ -f "$sr_state" ]]; then
        # Idempotent: another role already registered this schema globally.
        sr_name=$(jq -r '.name // ""' "$sr_state")
      fi
      if [[ -z "$sr_name" ]]; then
        local sr_body sr_resp
        sr_body=$(jq -c --slurpfile s "$sr_path" -n '{schema: $s[0], mutation_mappers: {}}')
        # Phase 1 cutover (schema_service repo) dropped /api/* dispatch — the new
        # server_lambda only matches /v1/*. The CDK still registers /api/* routes
        # but they fall through to the Lambda's catch-all 404. Use the canonical
        # /v1/schemas path that schema_service_client uses.
        if ! sr_resp=$(curl -fsS -X POST "${FOLDDB_TEST_DEV_SCHEMA:?}/v1/schemas" \
            -H 'Content-Type: application/json' \
            -d "$sr_body" 2>&1); then
          echo "[step] schema_register: dev schema service rejected fixture: $sr_resp" >&2
          return 1
        fi
        sr_name=$(echo "$sr_resp" | jq -r '.schema.name // ""')
        if [[ -z "$sr_name" || "$sr_name" == "null" ]]; then
          echo "[step] schema_register: no .schema.name in response: $(echo "$sr_resp" | head -c 300)" >&2
          return 1
        fi
        jq -cn --arg name "$sr_name" --arg persist_as "$sr_persist_as" \
               --arg fixture "$sr_fixture" \
               '{name:$name, persist_as:$persist_as, fixture:$fixture}' > "$sr_state"
      fi

      # Pull the registry on this node and flip the schema to Approved. This
      # is required on every role that will later query — d2f07 surfaced
      # that peer-synced org schemas arrive in state=Available, which is
      # not queryable. Load is idempotent; approve is idempotent.
      if ! curl -fsS -X POST "http://127.0.0.1:$port/api/schemas/load" \
          -H 'Content-Type: application/json' \
          -H "X-User-Hash: $hash" -d '{}' > /dev/null 2>&1; then
        echo "[step] schema_register: /api/schemas/load failed on $role" >&2
        return 1
      fi
      if ! curl -fsS -X POST "http://127.0.0.1:$port/api/schema/$sr_name/approve" \
          -H 'Content-Type: application/json' \
          -H "X-User-Hash: $hash" -d '{}' > /dev/null 2>&1; then
        echo "[step] schema_register: approve failed on $role for $sr_name" >&2
        return 1
      fi
      echo "[step] $role registered+approved schema $sr_persist_as=$sr_name"
      ;;

    mutation_write)
      # Action: { action: mutation_write, schema_persisted_as: "<key>",
      #          fields: {..}, key: {"range": "..."}, mutation_type?: "create" }
      # Writes one mutation on this role's node against a schema previously
      # registered via schema_register.
      local mw_persist_as mw_fields mw_key mw_mtype
      mw_persist_as=$(echo "$action_json" | jq -r '.schema_persisted_as // ""')
      mw_fields=$(echo "$action_json" | jq -c '.fields // {}')
      mw_key=$(echo "$action_json" | jq -c '.key // {}')
      mw_mtype=$(echo "$action_json" | jq -r '.mutation_type // "create"')
      [[ -n "$mw_persist_as" ]] || { echo "[step] mutation_write: schema_persisted_as required" >&2; return 1; }

      local mw_state="$FOLDDB_TEST_SESSION_DIR/state/schema-$mw_persist_as.json"
      [[ -f "$mw_state" ]] || { echo "[step] mutation_write: no state for schema '$mw_persist_as' (run schema_register first)" >&2; return 1; }
      local mw_name
      mw_name=$(jq -r '.name // ""' "$mw_state")
      [[ -n "$mw_name" ]] || { echo "[step] mutation_write: empty schema name in $mw_state" >&2; return 1; }

      local mw_body
      mw_body=$(jq -cn --arg s "$mw_name" \
                       --arg mt "$mw_mtype" \
                       --argjson fv "$mw_fields" \
                       --argjson kv "$mw_key" \
        '{type:"mutation", schema:$s, fields_and_values:$fv, key_value:$kv, mutation_type:$mt}')

      local mw_resp
      if ! mw_resp=$(curl -fsS -X POST "http://127.0.0.1:$port/api/mutation" \
          -H 'Content-Type: application/json' \
          -H "X-User-Hash: $hash" \
          -d "$mw_body" 2>&1); then
        echo "[step] mutation_write: non-2xx on $role for $mw_name: $mw_resp" >&2
        return 1
      fi
      echo "[step] $role wrote mutation on $mw_persist_as=$mw_name"
      ;;

    set_org_hash)
      # Action: { action: set_org_hash, schema_persisted_as: "<key>",
      #          from_role: alice | from_hash: "<hex>" | clear: true }
      # Tags the schema with org_hash. `from_role` resolves the org_hash
      # from that role's org_create state file (the usual path). `from_hash`
      # is an escape hatch for fake-tag scenarios. `clear: true` reverts to
      # personal.
      local soh_persist_as soh_from_role soh_from_hash soh_clear
      soh_persist_as=$(echo "$action_json" | jq -r '.schema_persisted_as // ""')
      soh_from_role=$(echo "$action_json" | jq -r '.from_role // ""')
      soh_from_hash=$(echo "$action_json" | jq -r '.from_hash // ""')
      soh_clear=$(echo "$action_json" | jq -r '.clear // false')
      [[ -n "$soh_persist_as" ]] || { echo "[step] set_org_hash: schema_persisted_as required" >&2; return 1; }

      local soh_state="$FOLDDB_TEST_SESSION_DIR/state/schema-$soh_persist_as.json"
      [[ -f "$soh_state" ]] || { echo "[step] set_org_hash: no state for schema '$soh_persist_as'" >&2; return 1; }
      local soh_name
      soh_name=$(jq -r '.name // ""' "$soh_state")
      [[ -n "$soh_name" ]] || { echo "[step] set_org_hash: empty schema name in $soh_state" >&2; return 1; }

      local soh_body
      if [[ "$soh_clear" == "true" ]]; then
        soh_body='{"org_hash":null}'
      elif [[ -n "$soh_from_role" ]]; then
        local soh_create="$FOLDDB_TEST_SESSION_DIR/state/org-create-$soh_from_role.json"
        [[ -f "$soh_create" ]] || { echo "[step] set_org_hash: no org-create state for role $soh_from_role" >&2; return 1; }
        local soh_hash
        soh_hash=$(jq -r '(.data.org.org_hash // .org.org_hash // "")' "$soh_create")
        [[ -n "$soh_hash" && "$soh_hash" != "null" ]] || { echo "[step] set_org_hash: no org_hash in $soh_create" >&2; return 1; }
        soh_body=$(jq -cn --arg h "$soh_hash" '{org_hash:$h}')
      elif [[ -n "$soh_from_hash" ]]; then
        soh_body=$(jq -cn --arg h "$soh_from_hash" '{org_hash:$h}')
      else
        echo "[step] set_org_hash: requires from_role OR from_hash OR clear:true" >&2
        return 1
      fi

      local soh_resp
      if ! soh_resp=$(curl -fsS -X POST "http://127.0.0.1:$port/api/schema/$soh_name/set-org-hash" \
          -H 'Content-Type: application/json' \
          -H "X-User-Hash: $hash" \
          -d "$soh_body" 2>&1); then
        echo "[step] set_org_hash: non-2xx on $role for $soh_name: $soh_resp" >&2
        return 1
      fi
      echo "[step] $role tagged $soh_persist_as=$soh_name with org_hash body=$soh_body"
      ;;

    query_schema)
      # Action: { action: query_schema, schema_persisted_as: "<key>",
      #          persist_as: "<query_name>", [fields: [..]], [expect_min: N] }
      # Runs an unfiltered /api/query on this role's node. Response saved to
      # state/query-$persist_as-$role.json so the last_query_result_count
      # assertion can inspect it. An HTTP non-2xx is a hard step failure —
      # this is the primary 4b171 guard: the server must not return 500
      # "Atom not found for key" for an unfiltered org-shared query.
      local qs_persist_as qs_save_as qs_fields_override qs_expect_min
      qs_persist_as=$(echo "$action_json" | jq -r '.schema_persisted_as // ""')
      qs_save_as=$(echo "$action_json" | jq -r '.persist_as // ""')
      qs_fields_override=$(echo "$action_json" | jq -c '.fields // null')
      qs_expect_min=$(echo "$action_json" | jq -r '.expect_min // ""')
      [[ -n "$qs_persist_as" ]] || { echo "[step] query_schema: schema_persisted_as required" >&2; return 1; }
      [[ -n "$qs_save_as" ]]    || { echo "[step] query_schema: persist_as required" >&2; return 1; }

      local qs_state="$FOLDDB_TEST_SESSION_DIR/state/schema-$qs_persist_as.json"
      [[ -f "$qs_state" ]] || { echo "[step] query_schema: no state for schema '$qs_persist_as'" >&2; return 1; }
      local qs_name qs_fixture qs_path qs_fields
      qs_name=$(jq -r '.name // ""' "$qs_state")
      qs_fixture=$(jq -r '.fixture // ""' "$qs_state")
      [[ -n "$qs_name" ]] || { echo "[step] query_schema: empty schema name in $qs_state" >&2; return 1; }
      if [[ "$qs_fields_override" != "null" && -n "$qs_fields_override" ]]; then
        qs_fields="$qs_fields_override"
      else
        qs_path="$framework_dir/fixtures/$qs_fixture"
        [[ -f "$qs_path" ]] || { echo "[step] query_schema: cannot resolve fields (fixture $qs_path missing, no fields override)" >&2; return 1; }
        qs_fields=$(jq -c '.fields' "$qs_path")
      fi

      local qs_body qs_tmp qs_code qs_body_out
      qs_body=$(jq -cn --arg s "$qs_name" --argjson f "$qs_fields" '{schema_name:$s, fields:$f}')
      qs_tmp="$FOLDDB_TEST_SESSION_DIR/state/query-$qs_save_as-$role.body"
      qs_code=$(curl -sS -o "$qs_tmp" -w '%{http_code}' -X POST \
        "http://127.0.0.1:$port/api/query" \
        -H 'Content-Type: application/json' \
        -H "X-User-Hash: $hash" \
        -d "$qs_body" || true)
      qs_body_out="$FOLDDB_TEST_SESSION_DIR/state/query-$qs_save_as-$role.json"
      mv "$qs_tmp" "$qs_body_out"
      if [[ "$qs_code" != "200" ]]; then
        echo "[step] query_schema: 4b171 guard FAILED — $role HTTP $qs_code on unfiltered query for $qs_name" >&2
        echo "[step]   body: $(head -c 400 "$qs_body_out")" >&2
        return 1
      fi
      local qs_count
      qs_count=$(jq '.results | length' "$qs_body_out" 2>/dev/null || echo 0)
      [[ "$qs_count" =~ ^[0-9]+$ ]] || qs_count=0
      if [[ -n "$qs_expect_min" ]]; then
        if (( qs_count < qs_expect_min )); then
          echo "[step] query_schema: $role got $qs_count results < expect_min=$qs_expect_min for $qs_name" >&2
          echo "[step]   body: $(head -c 400 "$qs_body_out")" >&2
          return 1
        fi
      fi
      echo "[step] $role queried $qs_persist_as=$qs_name → $qs_count results (saved as '$qs_save_as')"
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
