//! Integration test for the Agent tool-based ingestion + query flow.
//!
//! This test exercises the full agent pipeline using the same tools the LLM calls:
//! 1. `scan_folder` tool via OperationProcessor to scan sample_data/
//! 2. `ingest_files` tool via OperationProcessor to ingest recommended files
//! 3. Verify schemas were created and data is queryable
//! 4. Run agent queries against the ingested data
//!
//! Requires:
//! - `ANTHROPIC_API_KEY` environment variable set
//!
//! Run with: `cargo test --test agent_ingestion_test -- --ignored --nocapture`

use fold_db::logging::core::run_with_user;
use fold_db_node::fold_node::llm_query::LlmQueryService;
use fold_db_node::fold_node::node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use fold_db_node::ingestion::IngestionConfig;
use fold_db_node::schema_service::server::{
    AddSchemaResponse, ConflictResponse, ErrorResponse, SchemaAddOutcome, SchemaServiceState,
    SchemasListResponse,
};
mod common;

use actix_web::{web, App, HttpResponse, HttpServer};
use serde::Deserialize;
use std::collections::HashMap;
use std::net::TcpListener;
use std::path::Path;
use tempfile::TempDir;

// -- Inline schema service handlers (reused from smart_folder_ingestion_test) --

#[derive(Debug, Deserialize)]
struct AddSchemaRequest {
    schema: fold_db::schema::types::Schema,
    mutation_mappers: HashMap<String, String>,
}

async fn handle_list_schemas(state: web::Data<SchemaServiceState>) -> HttpResponse {
    match state.get_schema_names() {
        Ok(names) => HttpResponse::Ok().json(SchemasListResponse { schemas: names }),
        Err(e) => HttpResponse::InternalServerError().json(ErrorResponse {
            error: format!("Failed to list schemas: {}", e),
        }),
    }
}

async fn handle_get_available_schemas(state: web::Data<SchemaServiceState>) -> HttpResponse {
    match state.get_all_schemas_cached() {
        Ok(schemas) => HttpResponse::Ok().json(serde_json::json!({ "schemas": schemas })),
        Err(e) => HttpResponse::InternalServerError().json(ErrorResponse {
            error: format!("Failed to get schemas: {}", e),
        }),
    }
}

async fn handle_get_schema(
    path: web::Path<String>,
    state: web::Data<SchemaServiceState>,
) -> HttpResponse {
    let schema_name = path.into_inner();
    match state.get_schema_by_name(&schema_name) {
        Ok(Some(schema)) => HttpResponse::Ok().json(schema),
        Ok(None) => HttpResponse::NotFound().json(ErrorResponse {
            error: "Schema not found".to_string(),
        }),
        Err(e) => HttpResponse::InternalServerError().json(ErrorResponse {
            error: format!("Failed to get schema: {}", e),
        }),
    }
}

async fn handle_add_schema(
    payload: web::Json<AddSchemaRequest>,
    state: web::Data<SchemaServiceState>,
) -> HttpResponse {
    let request = payload.into_inner();
    match state
        .add_schema(request.schema, request.mutation_mappers)
        .await
    {
        Ok(SchemaAddOutcome::Added(schema, mutation_mappers)) => {
            HttpResponse::Created().json(AddSchemaResponse {
                schema,
                mutation_mappers,
            })
        }
        Ok(SchemaAddOutcome::AlreadyExists(schema)) => {
            HttpResponse::Ok().json(AddSchemaResponse {
                schema,
                mutation_mappers: HashMap::new(),
            })
        }
        Ok(SchemaAddOutcome::TooSimilar(conflict)) => {
            HttpResponse::Conflict().json(ConflictResponse {
                error: "Schema too similar to existing schema".to_string(),
                similarity: conflict.similarity,
                closest_schema: conflict.closest_schema,
            })
        }
        Err(error) => HttpResponse::BadRequest().json(ErrorResponse {
            error: format!("Failed to add schema: {}", error),
        }),
    }
}

async fn spawn_local_schema_service() -> (String, actix_web::dev::ServerHandle, TempDir) {
    let temp_dir = TempDir::new().expect("failed to create tempdir for schema service");
    let db_path = temp_dir
        .path()
        .join("test_schema_db")
        .to_string_lossy()
        .to_string();

    let state = SchemaServiceState::new(db_path).expect("failed to create schema service state");
    let state_data = web::Data::new(state);

    let listener =
        TcpListener::bind(("127.0.0.1", 0)).expect("failed to bind schema service listener");
    let bound_address = listener
        .local_addr()
        .expect("failed to read bound address");

    let state_clone = state_data.clone();
    let server = HttpServer::new(move || {
        App::new()
            .app_data(state_clone.clone())
            .service(
                web::scope("/api")
                    .route("/schemas", web::get().to(handle_list_schemas))
                    .route("/schemas", web::post().to(handle_add_schema))
                    .route(
                        "/schemas/available",
                        web::get().to(handle_get_available_schemas),
                    )
                    .route("/schema/{name}", web::get().to(handle_get_schema)),
            )
    })
    .listen(listener)
    .expect("failed to listen")
    .run();

    let handle = server.handle();
    actix_web::rt::spawn(server);
    actix_web::rt::time::sleep(std::time::Duration::from_millis(100)).await;

    let base_url = format!("http://127.0.0.1:{}", bound_address.port());
    (base_url, handle, temp_dir)
}

/// Text-based file extensions we can ingest without external converters.
const TEXT_EXTENSIONS: &[&str] = &["json", "csv", "txt", "md"];

fn is_text_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    TEXT_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

// -- Integration test --------------------------------------------------------

#[actix_web::test]
#[ignore] // Requires ANTHROPIC_API_KEY and AI calls
async fn test_agent_scan_ingest_and_query() {
    // ── Phase 1: Setup ──────────────────────────────────────────────────

    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("Skipping: ANTHROPIC_API_KEY not set");
        return;
    }

    let (schema_url, schema_handle, _schema_temp_dir) = spawn_local_schema_service().await;
    eprintln!("Local schema service running at {}", schema_url);

    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_identity(&user_id, &keypair.secret_key_base64())
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    let processor = OperationProcessor::new(node.clone());

    let sample_data_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("sample_data");
    assert!(
        sample_data_path.exists(),
        "sample_data/ directory not found at {}",
        sample_data_path.display()
    );

    // ── Phase 2: Agent scan_folder tool ─────────────────────────────────

    eprintln!("\n=== Phase 2: Scanning via OperationProcessor ===");
    let scan_result = run_with_user(&user_id, async {
        processor
            .smart_folder_scan(&sample_data_path, 5, 500)
            .await
    })
    .await
    .expect("smart_folder_scan failed");

    eprintln!("Scan results:");
    eprintln!("  total_files: {}", scan_result.total_files);
    eprintln!(
        "  recommended_files: {}",
        scan_result.recommended_files.len()
    );
    eprintln!("  skipped_files: {}", scan_result.skipped_files.len());

    assert!(scan_result.success, "Scan should succeed");
    assert!(
        !scan_result.recommended_files.is_empty(),
        "Should recommend files for ingestion"
    );

    // Filter to text-only files (same as agent would see)
    let text_files: Vec<_> = scan_result
        .recommended_files
        .iter()
        .filter(|f| is_text_file(&f.path))
        .collect();

    eprintln!(
        "Text files to ingest: {} (of {} recommended)",
        text_files.len(),
        scan_result.recommended_files.len()
    );

    assert!(
        !text_files.is_empty(),
        "Should have text files to ingest"
    );

    // ── Phase 3: Agent ingest_files tool ────────────────────────────────

    eprintln!("\n=== Phase 3: Ingesting via OperationProcessor ===");

    let mut succeeded = 0usize;
    let mut failed = 0usize;

    for (idx, rec) in text_files.iter().enumerate() {
        let full_path = sample_data_path.join(&rec.path);
        eprintln!(
            "  [{}/{}] Ingesting: {}",
            idx + 1,
            text_files.len(),
            rec.path
        );

        let result = run_with_user(&user_id, async {
            processor.ingest_single_file(&full_path, true).await
        })
        .await;

        match result {
            Ok(resp) => {
                if resp.success && resp.mutations_executed > 0 {
                    eprintln!(
                        "    OK: schema={:?}, mutations={}",
                        resp.schema_used, resp.mutations_executed
                    );
                    succeeded += 1;
                } else {
                    eprintln!(
                        "    PARTIAL: schema={:?}, mutations={}, errors={:?}",
                        resp.schema_used, resp.mutations_executed, resp.errors
                    );
                    failed += 1;
                }
            }
            Err(e) => {
                eprintln!("    FAIL: {}", e);
                failed += 1;
            }
        }
    }

    eprintln!(
        "\nIngestion summary: {}/{} succeeded, {} failed",
        succeeded,
        text_files.len(),
        failed
    );

    let success_rate = succeeded as f64 / text_files.len() as f64;
    assert!(
        success_rate >= 0.5,
        "At least 50% of text files should ingest successfully, got {:.0}% ({}/{})",
        success_rate * 100.0,
        succeeded,
        text_files.len()
    );

    // ── Phase 4: Verify schemas were created ────────────────────────────

    eprintln!("\n=== Phase 4: Verifying schemas ===");

    // Wait for async processing to flush
    actix_web::rt::time::sleep(std::time::Duration::from_millis(500)).await;

    let schemas = processor
        .list_schemas()
        .await
        .expect("Failed to list schemas");

    let schema_names: Vec<&str> = schemas.iter().map(|s| s.name()).collect();
    eprintln!("Schemas created: {} total", schemas.len());
    for name in &schema_names {
        eprintln!("  - {}", name);
    }

    assert!(
        schemas.len() >= 3,
        "Should have at least 3 schemas from sample_data (got {})",
        schemas.len()
    );

    // ── Phase 5: Agent queries against ingested data ────────────────────

    eprintln!("\n=== Phase 5: Running agent queries ===");

    let ingestion_config = IngestionConfig::from_env().expect("IngestionConfig::from_env failed");
    let query_service =
        LlmQueryService::new(ingestion_config).expect("Failed to create LlmQueryService");

    // Query 1: Medications (from health/medications.json)
    // Query 2: Contacts (from contacts/address_book.json)
    // Query 3: Financial data (from finance/ files)
    // Query 4: General data summary
    let test_queries: Vec<(&str, Vec<&str>)> = vec![
        (
            "What medications are in the database?",
            vec!["medication", "lisinopril", "vitamin", "metformin", "drug", "prescription"],
        ),
        (
            "Who are the contacts in the address book?",
            vec!["contact", "mom", "dad", "sarah", "address", "phone", "name", "email"],
        ),
        (
            "What financial records do I have?",
            vec!["bank", "transaction", "finance", "payment", "deposit", "investment", "expense"],
        ),
        (
            "Give me a summary of all data in the database",
            vec!["schema", "record", "data", "contact", "health", "finance"],
        ),
    ];

    let mut queries_succeeded = 0usize;

    for (query_text, keywords) in &test_queries {
        eprintln!("\nAgent Query: \"{}\"", query_text);

        let result = query_service
            .run_agent_query(query_text, &schemas, &node, &user_id, 10, &[])
            .await;

        match result {
            Ok((answer, tool_calls)) => {
                eprintln!("  Tool calls made: {}", tool_calls.len());
                for tc in &tool_calls {
                    let result_preview = tc.result.to_string();
                    let preview = if result_preview.len() > 100 {
                        format!("{}...", &result_preview[..100])
                    } else {
                        result_preview
                    };
                    eprintln!("    - {}({}) -> {}", tc.tool, tc.params, preview);
                }

                let answer_preview = if answer.len() > 300 {
                    format!("{}...", &answer[..300])
                } else {
                    answer.clone()
                };
                eprintln!("  Answer: {}", answer_preview);

                // Hard assert: answer is non-empty
                assert!(
                    !answer.trim().is_empty(),
                    "Agent query '{}' returned empty answer",
                    query_text
                );

                // Hard assert: agent made at least one tool call
                assert!(
                    !tool_calls.is_empty(),
                    "Agent query '{}' should make at least one tool call",
                    query_text
                );

                // Soft check: answer contains at least one expected keyword
                let answer_lower = answer.to_lowercase();
                let matched = keywords.iter().any(|kw| answer_lower.contains(kw));
                if matched {
                    eprintln!("  Keyword match: PASS");
                } else {
                    eprintln!(
                        "  WARNING: Answer did not contain any of {:?}",
                        keywords
                    );
                }

                queries_succeeded += 1;
            }
            Err(e) => {
                eprintln!("  WARNING (non-deterministic): {}", e);
            }
        }
    }

    eprintln!(
        "\nAgent query summary: {}/{} succeeded",
        queries_succeeded,
        test_queries.len()
    );

    assert!(
        queries_succeeded >= 2,
        "At least 2 of {} agent queries should succeed, but only {} did",
        test_queries.len(),
        queries_succeeded
    );

    // ── Phase 6: Verify multi-turn context (scan → ingest flow) ─────────

    eprintln!("\n=== Phase 6: Multi-turn agent context test ===");

    // Simulate: first turn scans, second turn ingests based on context
    // This tests that prior_history works when passed to run_agent_query
    use fold_db_node::fold_node::llm_query::types::Message;
    use std::time::SystemTime;

    let prior_history = vec![
        Message {
            role: "user".to_string(),
            content: "Scan sample_data for files".to_string(),
            timestamp: SystemTime::now(),
        },
        Message {
            role: "tool_calls".to_string(),
            content: format!(
                "Tool: scan_folder\nParams: {{\"path\": \"{}\"}}\nResult: Found {} recommended files including {}",
                sample_data_path.display(),
                text_files.len(),
                text_files.iter().take(3).map(|f| f.path.as_str()).collect::<Vec<_>>().join(", ")
            ),
            timestamp: SystemTime::now(),
        },
        Message {
            role: "assistant".to_string(),
            content: format!(
                "I found {} files ready for ingestion from sample_data.",
                text_files.len()
            ),
            timestamp: SystemTime::now(),
        },
    ];

    let followup_result = query_service
        .run_agent_query(
            "What data do I have now? Summarize what schemas exist.",
            &schemas,
            &node,
            &user_id,
            10,
            &prior_history,
        )
        .await;

    match followup_result {
        Ok((answer, tool_calls)) => {
            eprintln!("  Multi-turn follow-up succeeded");
            eprintln!("  Tool calls: {}", tool_calls.len());
            let preview = if answer.len() > 300 {
                format!("{}...", &answer[..300])
            } else {
                answer.clone()
            };
            eprintln!("  Answer: {}", preview);

            assert!(
                !answer.trim().is_empty(),
                "Multi-turn follow-up returned empty answer"
            );
        }
        Err(e) => {
            eprintln!("  WARNING (non-deterministic): {}", e);
        }
    }

    // ── Cleanup ─────────────────────────────────────────────────────────
    schema_handle.stop(true).await;
    eprintln!("\nTest complete.");
}
