# Migration: file_to_json → file_to_markdown in fold_db_node

## Context

`file_to_json` sends file data to external LLM APIs (Anthropic/OpenRouter) for conversion, leaking user data. `file_to_markdown` is a fully offline/local alternative using Ollama, whisper.cpp, and Python extractors. This migration replaces file_to_json as the fallback converter for unstructured files (images, PDFs, Office docs, audio, etc.) while keeping native parsers for structured formats (CSV, JSON, Twitter JS) that produce per-row records.

The AI provider config (Anthropic/Ollama) stays — it's shared with the chat agent (`LlmQueryService`) and AI schema recommendation, which are unrelated to file conversion.

## Design Decisions

- **Keep native parsers** for CSV, JSON, Twitter JS, code, text — they produce structured `Value` arrays (per-row records) that the pipeline needs
- **Replace file_to_json** with file_to_markdown only for the fallback path (images, PDFs, Office docs, audio, archives, unknown formats)
- **Option B (typed)**: When file_to_markdown is used, push `FileMarkdown` into `IngestionRequest` as a typed field rather than converting to `Value` at the boundary
- **`IngestionRequest` gains `file_markdown: Option<FileMarkdown>`** with `#[serde(skip)]` — the HTTP API is unchanged, internal code sets it directly
- **When `file_markdown` is `Some`**: skip `flatten_root_layers()` (deterministic output), skip `check_has_nested_children()` (always flat), skip `enrich_image_json()` (metadata already typed); convert to `Value` only at the AI recommendation / mutation generation boundary
- **When `file_markdown` is `None`**: existing `Value`-based pipeline unchanged

## Files to Modify

### Phase 1: Dependency + Types

**`fold_db_node/Cargo.toml`**
- Remove: `file_to_json = "0.4.0"`
- Add: `file_to_markdown = { path = "../file_to_markdown" }`
- Keep: `csv`, `exif`, all other deps (native parsers still use them)

**`fold_db_node/src/ingestion/mod.rs`**
- Add `file_markdown: Option<file_to_markdown::FileMarkdown>` to `IngestionRequest` with `#[serde(skip)]`
- Add `pub use file_to_markdown::FileMarkdown;` re-export

### Phase 2: File Conversion Rewrite

**`fold_db_node/src/ingestion/json_processor.rs`**
- Delete: `convert_file_to_json()`, `convert_file_to_json_http()` (file_to_json calls)
- Add: `convert_file_to_markdown(file_path: &Path) -> Result<FileMarkdown, IngestionError>`
  - Builds `file_to_markdown::Config` using `Config::from_home_dir()` with Ollama base_url from `IngestionConfig`
  - Calls `Converter::new(config).convert_path(path).await` (fully async, no `spawn_blocking`)
  - Maps `ConvertError` → `IngestionError::FileConversionFailed`
- Add: `convert_file_to_markdown_http(file_path: &Path) -> Result<FileMarkdown, HttpResponse>` (HTTP wrapper)
- Add: `file_markdown_to_value(fm: &FileMarkdown) -> Value` — `serde_json::to_value(fm)` for AI recommendation step
- Keep: `flatten_root_layers()`, `flatten_array_elements()` (still used by JSON API path and native parser path)
- Keep: `enrich_image_json()`, `classify_image_type()`, `get_file_creation_date()`, `get_exif_date()` (still used by native parser path for image files from smart folder)
- Keep: `save_json_to_temp_file()` (debugging utility)
- Update tests: add tests for new functions

**`fold_db_node/src/ingestion/file_conversion.rs`**
- Keep all native parsers: `csv_to_json()`, `twitter_js_to_json()`, `extract_code_metadata()`, `wrap_text_content()`, `read_file_with_hash()`
- Remove only the `file_to_json` import if it exists here (it doesn't — file_to_json is only used in json_processor.rs)
- No changes needed

### Phase 3: Entry Points

**`fold_db_node/src/ingestion/file_upload.rs`**
- In `upload_file()` (~line 471): replace `convert_file_to_json_http()` with `convert_file_to_markdown_http()`
- When conversion succeeds, build `IngestionRequest` with:
  - `file_markdown: Some(fm)`
  - `data: serde_json::to_value(&fm).unwrap_or(Value::Null)` (for backward compat / debug JSON dump)
- For image detection: use `fm.image_format.is_some()` instead of `is_image_file(&name)`
- For image descriptive name: extract from `fm.title` if present
- Remove the `enrich_image_json()` call for this path (FileMarkdown already has typed metadata)
- Update comment: "file_to_json" → "file_to_markdown"

**`fold_db_node/src/ingestion/routes.rs` — `process_single_file_via_smart_folder()`**
- Current flow: try native parser (`read_file_with_hash`), fall back to `convert_file_to_json`
- New flow: try native parser (`read_file_with_hash`), fall back to `convert_file_to_markdown`
- On native parser success: `file_markdown: None`, `data: value` (existing behavior)
- On native parser failure (fallback): `file_markdown: Some(fm)`, `data: serde_json::to_value(&fm)`
- For the fallback path: remove `enrich_image_json()` call, use `fm.image_format.is_some()` for image detection

**`fold_db_node/src/fold_node/operation_processor/admin_ops.rs` — `ingest_single_file_with_tracker()`**
- Same pattern: try `read_file_as_json`, fall back to `convert_file_to_markdown` instead of `convert_file_to_json`
- Set `file_markdown: Some(fm)` on fallback path

### Phase 4: Pipeline Adaptation

**`fold_db_node/src/ingestion/ingestion_service/mod.rs`**
- In `process_json_with_node_and_progress()`:
  - When `request.file_markdown.is_some()`:
    - Skip `validate_input()` — FileMarkdown is always valid
    - Skip `flatten_root_layers()` — deterministic output, no LLM wrappers
    - Skip `check_has_nested_children()` — FileMarkdown is always flat
    - Go directly to flat path with `file_markdown_to_value(&fm)` as the data for AI recommendation
    - For image schema override: use `fm.image_format.is_some()`, `fm.created` for key fields
  - When `request.file_markdown.is_none()`:
    - Existing pipeline unchanged (flatten, decompose check, etc.)

### Phase 5: Cleanup + UI

**`fold_db_node/src/ingestion/smart_folder.rs`**
- Remove re-export of any file_to_json-related functions (if any)
- Update cost estimation: file_to_markdown conversions have $0 API cost (local processing)
- Update comments referencing file_to_json

**React UI: `src/server/static-react/src/components/tabs/upload/UploadInfoPanel.jsx`**
- Update text: "File is automatically converted to JSON using AI" → reflect local processing
- No config UI changes needed (AI provider config stays for chat agent / schema recommendation)

### Phase 6: Tests

- New unit tests for `convert_file_to_markdown()` (text file, no Ollama needed)
- New unit test for `file_markdown_to_value()` roundtrip
- Update `file_upload.rs` flow tests if they mock `convert_file_to_json`
- Existing native parser tests unchanged
- Existing `flatten_root_layers` tests unchanged (still used)

## Files NOT Changed

- `fold_db_node/src/ingestion/config.rs` — AIProvider, AnthropicConfig, OllamaConfig stay (used by chat agent + schema recommendation)
- `fold_db_node/src/ingestion/ai_client.rs` — AiBackend trait, OllamaBackend, AnthropicBackend, build_backend() all stay
- `fold_db_node/src/fold_node/llm_query/` — chat agent untouched
- `fold_db_node/src/ingestion/mutation_generator.rs` — no change (consumes HashMap<String, Value>)
- `fold_db_node/src/ingestion/key_extraction.rs` — no change
- `fold_db_node/src/ingestion/decomposer.rs` — no change (only triggered for Json path)
- `fold_db_node/src/ingestion/ingestion_service/decomposition.rs` — no change

## Config for file_to_markdown

The `file_to_markdown::Config` is built at conversion time:
```rust
fn build_ftm_config() -> Result<file_to_markdown::Config, IngestionError> {
    let ingestion_config = IngestionConfig::load()?;
    let ollama = file_to_markdown::OllamaConfig {
        base_url: ingestion_config.ollama.base_url.clone(),
        ..file_to_markdown::OllamaConfig::default()  // vision_model, ocr_model use defaults
    };
    let whisper = std::env::var("WHISPER_MODEL_PATH").ok().map(PathBuf::from);
    file_to_markdown::Config::from_home_dir(ollama, whisper)
        .map_err(|e| IngestionError::FileConversionFailed(e.to_string()))
}
```

## Verification

1. `cd fold_db_node && cargo check` — compiles without file_to_json
2. `cargo test --workspace --all-targets` — all existing tests pass
3. `cargo clippy --workspace --all-targets -- -D warnings` — no warnings
4. Manual test: upload a CSV via UI → should use native parser (per-row records, same as before)
5. Manual test: upload a PDF/image via UI → should use file_to_markdown (single FileMarkdown document)
6. Manual test: POST raw JSON to /api/ingestion/process → existing pipeline, unchanged
7. Manual test: smart folder scan with mixed file types → native parsers for CSV/JSON, file_to_markdown for images/PDFs
