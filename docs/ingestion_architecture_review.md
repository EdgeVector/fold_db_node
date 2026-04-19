# FoldDB Node — Ingestion System Architecture Review

**Date:** 2026-03-14
**Scope:** `fold_db_node/src/ingestion/` (~14,400 lines across 32 files)

---

## 1. Executive Summary

The ingestion system is an AI-powered pipeline that accepts arbitrary data (JSON, CSV, Twitter archives, images, PDFs, code files) and automatically creates FoldDB schemas, generates mutations, and stores data — all without requiring users to define schemas upfront. It supports both Anthropic and Ollama as AI backends.

The system handles four entry points: single-file HTTP upload, multi-file batch ingestion, smart folder watching, and direct JSON POST. All paths converge on a shared core pipeline that validates, flattens, sends to an LLM for schema recommendation, creates/expands schemas, and executes mutations.

---

## 2. Module Map

```
src/ingestion/
├── mod.rs                          # Module root, IngestionRequest/Response types
├── config.rs                       # AIProvider, IngestionConfig, SavedConfig, env loading
├── error.rs                        # IngestionError (13 variants), error classification
├── progress.rs                     # ProgressService, PhaseTracker, IngestionPhase
├── routes.rs                       # HTTP handlers: process_json, get_status, validate, config
├── routes_helpers.rs               # IngestionServiceState, spawn helpers, smart folder logic
├── routes_batch.rs                 # Batch folder ingestion (pause/resume/cancel)
├── structure_analyzer.rs           # JSON skeleton extraction, StructureStats
├── ai/
│   ├── mod.rs                      # AI submodule root
│   ├── client.rs                   # AiBackend trait, AnthropicBackend, OllamaBackend
│   ├── helpers.rs                  # AISchemaResponse, retry logic, response parsing, prompts
│   └── prompts.rs                  # PROMPT_HEADER, PROMPT_ACTIONS, FIELD_DESCRIPTIONS_PROMPT
├── ingestion_service/
│   ├── mod.rs                      # IngestionService struct, orchestration, decomposed path
│   ├── flat_path.rs                # Flat ingestion path (no nested arrays-of-objects)
│   ├── ai_methods.rs               # call_ai_raw, get_ai_recommendation, fill_missing_fields
│   ├── schema_creation.rs          # determine_schema_to_use, create_new_schema_with_node
│   └── schema_cache.rs            # SchemaCache (dual-level dedup)
├── file_handling/
│   ├── mod.rs                      # File handling submodule root
│   ├── conversion.rs               # csv_to_json, twitter_js_to_json, code metadata extraction
│   ├── json_processor.rs           # convert_file_to_json, flatten_root_layers, image enrichment
│   └── upload.rs                   # Multipart parsing, file save, content-addressed dedup
└── smart_folder/
    ├── mod.rs                      # SmartFolderService, file watching
    ├── scanner.rs                  # Directory scanning, file filtering
    └── batch_controller.rs         # BatchController state machine
```

---

## 3. Entry Points

### 3.1 Direct JSON Ingestion (`POST /api/ingest/process`)

The primary entry point. Accepts an `IngestionRequest` containing raw JSON data, an optional schema name hint, and configuration overrides. The handler in `routes.rs` validates the request, obtains an `IngestionService` instance from `IngestionServiceState`, and calls `process_json_with_node_and_progress`.

### 3.2 File Upload (`POST /api/ingest/upload`)

Handles multipart form uploads. The `upload.rs` module parses the multipart stream, saves files to a content-addressed store (SHA256-based dedup per user), then converts the file to JSON via `convert_file_to_json` (supporting CSV, Twitter JS exports, images, PDFs, code files, and raw JSON). The resulting JSON is fed into the standard ingestion pipeline.

### 3.3 Batch Folder Ingestion (`POST /api/ingest/batch`)

Accepts a directory path and processes all eligible files within it. The `BatchController` provides pause/resume/cancel semantics as an in-memory state machine. Files are processed sequentially with per-file progress tracking. Status is queryable via `GET /api/ingest/batch/{id}`.

### 3.4 Smart Folder Watching

The `SmartFolderService` monitors configured directories for new files. When files appear, they're processed via `process_single_file_via_smart_folder` in `routes_helpers.rs`, which handles conversion, ingestion, and cleanup.

---

## 4. Core Pipeline Flow

All entry points converge on `IngestionService::process_json_with_node_and_progress`, which orchestrates the following phases:

```
Input JSON
    │
    ▼
[1. Validate & Flatten]
    │  - flatten_root_layers: unwrap single-key wrapper objects
    │  - Reject empty arrays, non-object/non-array data
    │
    ▼
[2. Structure Analysis]
    │  - StructureAnalyzer::extract_structure_skeleton
    │  - Produces flattened dot-path type map (e.g., "profile.age": "number")
    │  - Includes _meta for arrays ("array(N items)")
    │
    ▼
[3. Schema Cache Check]
    │  - SchemaCache checks if an identical structure was already processed
    │  - Two levels: per-call local cache + cross-file shared cache
    │  - Cache hit → skip AI call, reuse schema + mappers
    │
    ▼
[4. AI Recommendation] (on cache miss)
    │  - Build prompt: skeleton + PROMPT_HEADER + PROMPT_ACTIONS
    │  - call_with_retries: up to max_retries attempts
    │  - extract_json_from_response: multi-strategy JSON extraction
    │  - validate_and_convert_response: structural validation
    │  - Returns AISchemaResponse with schema def + mutation_mappers
    │
    ▼
[5. Path Decision]
    │  - check_has_nested_children: scans for arrays-of-objects within objects
    │  - Flat path: no nested children → process_flat_path
    │  - Decomposed path: nested children → process_decomposed_path
    │    (recursively extracts nested arrays into separate schemas)
    │
    ▼
[6. Schema Creation]
    │  - determine_schema_to_use → create_new_schema_with_node
    │  - Deserialize AI JSON → fold_db Schema struct
    │  - Fill missing field_descriptions and classifications
    │  - Set key config (hash field) if missing
    │  - Compute identity_hash for dedup
    │  - Submit to schema service → handles expansion/superseding
    │  - Load schema locally, approve, block old if expanded
    │
    ▼
[7. Mutation Generation]
    │  - Flat: generate_flat_mutations from data rows
    │  - Decomposed: recursive per-child-array mutation generation
    │  - Apply mutation_mappers (field renames from AI + service)
    │
    ▼
[8. Execution]
    │  - If auto_execute_mutations: submit mutations to FoldNode
    │  - Otherwise: return mutations in response for manual execution
    │
    ▼
IngestionResult { schema_name, mutations_applied, records_processed }
```

---

## 5. AI Subsystem

### 5.1 Backend Abstraction (`client.rs`)

The `AiBackend` trait defines a single async method:

```rust
async fn call(&self, prompt: &str, system_prompt: Option<&str>) -> Result<String, IngestionError>;
```

Two implementations:

- **`AnthropicBackend`**: Calls the Anthropic Messages API (`/v1/messages`). Configured via `AnthropicConfig` (api_key, model, base_url). Default model: `claude-haiku-4-5-20251001` (Haiku 4.5 — matches Sonnet 4 quality on the 8-case ingestion eval at ~67% the cost; query path overrides to Sonnet 4 in `IngestionConfig::default()`). Uses `reqwest` with configurable timeout.

- **`OllamaBackend`**: Calls the Ollama `/api/generate` endpoint. Configured via `OllamaConfig` (model, base_url). Default model: `llama3.3`, default URL: `http://localhost:11434`.

The `build_backend` factory selects the implementation based on `AIProvider` enum.

### 5.2 Error Classification (`error.rs`)

HTTP responses from LLM providers are classified into specific error variants:

| Status | Error Variant | User Message |
|--------|--------------|--------------|
| 401 | `AuthenticationError` | "API key invalid or expired" |
| 402 | `ConfigurationError` | "insufficient credits" |
| 404 | `ConfigurationError` | "model not found" |
| 429 | `RateLimitError` | "Too many requests" |
| 5xx | `ConnectionError` | "Server error, try again later" |

Transport-level errors (timeout, connection refused) are similarly classified via `classify_transport_error`.

### 5.3 Prompt Construction (`prompts.rs`, `helpers.rs`)

The prompt is built in `analyze_and_build_prompt`:

1. Extract structure skeleton (type map, not actual values)
2. Prepend `PROMPT_HEADER` (instructs AI to output a JSON schema definition)
3. Append `PROMPT_ACTIONS` (mutation_mappers, field naming conventions)
4. Append `FIELD_DESCRIPTIONS_PROMPT` (requires field_descriptions for every field)
5. Include a sample of actual data values (first few rows, truncated)

The skeleton ensures the AI sees the full superset of fields across all array elements without seeing all the data.

### 5.4 Response Parsing (`helpers.rs`)

`extract_json_from_response` uses multiple strategies in order:

1. **Markdown fence extraction**: Look for ```json ... ``` blocks
2. **Stream/SSE parsing**: Handle streaming response formats
3. **Brace matching**: Find outermost `{ ... }` with brace-depth tracking
4. **Direct parse**: Try `serde_json::from_str` on the raw response

After extraction, `validate_and_convert_response` checks:
- Schema has a non-empty, descriptive name (not "Schema" or "data")
- Schema has `field_classifications` for at least some fields
- Schema has `fields` list
- `mutation_mappers` is a valid string-to-string map

Failed validation triggers a retry with the validation error appended to the prompt.

### 5.5 Retry Logic

`call_with_retries` wraps the AI call with:
- Up to `max_retries` attempts (default 3)
- On validation failure: re-prompts with error context
- On transport/rate-limit errors: retries with backoff
- On auth errors: fails immediately (no retry)

---

## 6. Schema Lifecycle

### 6.1 Schema Cache (`schema_cache.rs`)

The `SchemaCache` prevents redundant AI calls when ingesting files with identical structure:

- **Local level**: Per-ingestion-call cache. When processing a batch of files, if file B has the same skeleton as file A (already processed in this call), reuse A's schema and mappers.
- **Shared level**: Cross-call cache (in-memory, lifetime of the process). If a previous ingestion call already processed this structure, reuse it.

Cache key is the JSON-serialized skeleton (deterministic because `serde_json::Map` is ordered).

### 6.2 Schema Creation (`schema_creation.rs`)

`create_new_schema_with_node` is the central schema creation path:

1. **Deserialize** the AI's JSON into a `fold_db::schema::types::Schema`
2. **Fill defaults**: Generate `field_descriptions` and `field_classifications` for any fields the AI missed
3. **Set key config**: If no `key` specified, use the first field as the hash key
4. **Compute identity hash**: Content-addressed hash of the schema structure for dedup
5. **Name resolution**: Keep AI's semantic name unless it's blank or generic
6. **Schema service submission**: `node.add_schema_to_service(&schema)` — the schema service handles similarity detection and may return a `replaced_schema` (expansion case)
7. **Local loading**: Load the schema into the local SchemaManager (skip if already loaded to preserve molecule state)
8. **Expansion handling**: If the schema service expanded an existing schema:
   - Load the old schema locally (needed for field mapper resolution)
   - Approve the new schema (triggers `apply_field_mappers`)
   - Block the old schema with `block_and_supersede`

The entire create-load-approve-block sequence is serialized via `schema_creation_lock` (tokio Mutex) to prevent races between concurrent ingestions.

### 6.3 Schema Expansion

When the schema service detects that a new schema is a superset of an existing one:
- It returns `replaced_schema: Some("old_name")`
- The new schema's shared fields get `FieldMapper` entries pointing to the old schema's fields
- On approval, `apply_field_mappers` copies molecule UUIDs from the old schema
- The old schema is blocked and marked as superseded
- No data migration needed — molecules are shared

---

## 7. Ingestion Paths

### 7.1 Flat Path (`flat_path.rs`)

Used when the input has no nested arrays-of-objects. This is the common case for tabular data (CSV imports, simple JSON arrays).

`process_flat_path`:
1. Get AI recommendation (or cache hit)
2. Create/expand schema
3. `generate_flat_mutations`: iterate over data rows, map fields via `mutation_mappers`, produce one mutation per row
4. Execute mutations if `auto_execute_mutations` is true

### 7.2 Decomposed Path (`ingestion_service/mod.rs`)

Used when the input contains nested arrays-of-objects (e.g., orders with line items). `process_decomposed_path`:

1. Identify nested children via `check_has_nested_children`
2. Extract each nested array into a separate top-level dataset
3. Process the parent object (with nested arrays replaced by reference keys) via flat path
4. Recursively process each extracted child array
5. Each child gets its own schema, allowing proper normalization

This produces multiple schemas from a single input — e.g., `customer_orders` and `order_line_items`.

---

## 8. File Handling

### 8.1 File Conversion (`conversion.rs`)

Converts various file formats to JSON:

| Format | Method | Notes |
|--------|--------|-------|
| CSV | `csv_to_json` | Headers become field names, rows become objects |
| Twitter JS | `twitter_js_to_json` | Strips `window.YTD.*` wrapper, parses inner JSON |
| Images | Via `file_to_json` crate | Extracts EXIF metadata, dimensions, format info |
| PDF | Via `file_to_json` crate | Extracts text content, metadata |
| Code files | `extract_code_metadata` | Language detection, line counts, structure info |
| JSON | Direct parse | Passed through as-is |

### 8.2 JSON Processing (`json_processor.rs`)

`convert_file_to_json` is the main entry point for file-to-JSON conversion. After conversion:

- `flatten_root_layers`: Unwraps single-key wrapper objects (e.g., `{"data": [...]}` → `[...]`)
- `enrich_image_json`: For image files, adds classification metadata (photo, screenshot, document, etc.)
- `classify_image_type`: Heuristic classification based on dimensions and aspect ratio

### 8.3 Upload & Dedup (`upload.rs`)

File uploads use content-addressed storage:

1. Parse multipart form data
2. Compute SHA256 hash of file contents
3. Store at `{upload_dir}/{user_hash}/{sha256}.{ext}`
4. If file with same hash exists for this user, skip write (dedup)
5. Return file path for subsequent processing

The `serve_file` handler serves uploaded files back with proper MIME types.

---

## 9. Progress Tracking (`progress.rs`)

### 9.1 Architecture

Two-layer progress system:

- **`ProgressService`**: Global registry of active ingestion operations. Maps `ingestion_id` → progress state. Thread-safe via `Arc<RwLock<...>>`.

- **`PhaseTracker`**: Per-operation progress tracker. Defines phases with percentage ranges:

```
Validating:      0% -  10%
Analyzing:      10% -  30%
SchemaCreation: 30% -  50%
Mutations:      50% -  80%
Executing:      80% - 100%
```

### 9.2 Phase Tracking

Each phase maps to an `IngestionPhase` enum. The `PhaseTracker` provides:
- `set_phase(phase)`: Move to a new phase, update percentage to phase start
- `set_progress_within_phase(fraction)`: Set progress within current phase (0.0–1.0), interpolated to phase's percentage range
- `complete()`: Set to 100%

Progress is queryable via `GET /api/ingest/status/{id}`.

---

## 10. Batch & Smart Folder

### 10.1 BatchController (`batch_controller.rs`)

In-memory state machine for batch operations:

```
Created → Running → Completed
              ↓         ↑
           Paused ──────┘
              ↓
           Cancelled
```

Supports:
- `pause()` / `resume()`: Suspend/resume file processing
- `cancel()`: Stop processing, mark remaining files as skipped
- Per-file status tracking (pending, processing, completed, failed, skipped)

### 10.2 Smart Folder (`smart_folder/`)

The `SmartFolderService` watches configured directories:
- `scanner.rs`: Scans for new files matching configured patterns
- Filters by extension, ignores already-processed files
- Each new file triggers `process_single_file_via_smart_folder`

---

## 11. Configuration (`config.rs`)

### 11.1 Loading Precedence

Configuration loads with this precedence (highest wins):

1. `ANTHROPIC_API_KEY` env var (secrets never in files)
2. Saved config file at `$FOLD_CONFIG_DIR/ingestion_config.json`
3. Other env vars (`AI_PROVIDER`, `OLLAMA_MODEL`, etc.) — only when no saved config
4. Compiled defaults

### 11.2 Key Settings

| Setting | Default | Source |
|---------|---------|--------|
| `provider` | `Anthropic` | Config file or `AI_PROVIDER` env |
| `anthropic.model` | `claude-haiku-4-5-20251001` | Config file or `ANTHROPIC_MODEL` env |
| `anthropic.api_key` | (empty) | `ANTHROPIC_API_KEY` env only |
| `ollama.model` | `llama3.3` | Config file or `OLLAMA_MODEL` env |
| `ollama.base_url` | `http://localhost:11434` | Config file or `OLLAMA_BASE_URL` env |
| `enabled` | `false` | `INGESTION_ENABLED` env (default `true` when set) |
| `max_retries` | `3` | `INGESTION_MAX_RETRIES` env |
| `timeout_seconds` | `300` | `INGESTION_TIMEOUT_SECONDS` env |
| `auto_execute_mutations` | `true` | `INGESTION_AUTO_EXECUTE` env |

### 11.3 Config Persistence

The UI can save provider/model choices via `POST /api/ingest/config`. `IngestionConfig::save_to_file` writes a `SavedConfig` to disk, preserving existing API keys if the incoming value is empty or redacted (`***configured***`). The `redacted()` method produces a safe-to-display copy.

---

## 12. Error Handling (`error.rs`)

The `IngestionError` enum has 13 variants organized by failure domain:

| Category | Variants |
|----------|----------|
| AI/LLM | `OllamaError`, `AIResponseValidationError` |
| Network | `HttpError`, `ConnectionError`, `TimeoutError`, `RateLimitError`, `AuthenticationError` |
| Schema | `SchemaCreationError`, `SchemaSystemError` |
| Data | `InvalidInput`, `JsonError`, `FileConversionFailed` |
| Config | `ConfigurationError` |

Every variant has a `user_message()` method returning concise, UI-friendly text. The `classify_llm_error` and `classify_transport_error` functions map raw HTTP/network errors into the appropriate variant.

---

## 13. Concurrency & Thread Safety

- **`IngestionService`** is `Send + Sync`, shareable across request handlers
- **`schema_creation_lock`**: Tokio Mutex serializing schema create/load/approve/block sequences
- **`ProgressService`**: `Arc<RwLock<HashMap>>` for concurrent progress reads/writes
- **`SchemaCache`**: `Arc<Mutex>` for shared-level cache access
- **`BatchController`**: `Arc<RwLock>` for state machine transitions
- All AI calls are async, non-blocking

---

## 14. Key Design Decisions

1. **Skeleton-based prompting**: The AI sees type maps, not raw data. This minimizes token usage and prevents data leakage to the LLM (only structure + a small sample is sent).

2. **Dual ingestion paths**: Flat vs. decomposed paths handle both tabular and hierarchical data without a one-size-fits-all approach that would be complex for simple cases.

3. **Schema cache**: Prevents redundant AI calls during batch operations. A folder of 100 identically-structured CSVs makes 1 AI call, not 100.

4. **Content-addressed file storage**: SHA256 dedup prevents storing duplicate files per user.

5. **Schema service integration**: Schema creation goes through the centralized schema service for similarity detection and expansion, ensuring structural dedup across all nodes.

6. **Serialized schema creation**: The `schema_creation_lock` prevents race conditions when concurrent ingestions try to create/expand the same schema simultaneously.

7. **Error classification**: Raw HTTP errors are mapped to semantic variants (auth, rate limit, timeout, connection) enabling appropriate retry behavior and user-facing messages.

---

## 15. Statistics

| Metric | Value |
|--------|-------|
| Total files | 32 |
| Total lines | ~14,400 |
| Largest file | `upload.rs` (733 lines) |
| Error variants | 13 |
| AI backends | 2 (Anthropic, Ollama) |
| File formats supported | 7+ (JSON, CSV, Twitter JS, images, PDF, code, text) |
| Config settings | 9 |
| HTTP endpoints | 10 |
