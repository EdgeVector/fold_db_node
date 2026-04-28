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

use fold_db::user_context::run_with_user;
use fold_db_node::fold_node::llm_query::LlmQueryService;
use fold_db_node::fold_node::node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use fold_db_node::ingestion::IngestionConfig;
mod common;

use std::path::Path;

use common::schema_service::{spawn_schema_service, SpawnedSchemaService};

async fn spawn_local_schema_service() -> SpawnedSchemaService {
    spawn_schema_service().await
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

    let svc = spawn_local_schema_service().await;
    let schema_url = svc.url.clone();
    eprintln!("Local schema service running at {}", schema_url);

    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair))
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));

    let sample_data_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("sample_data");
    assert!(
        sample_data_path.exists(),
        "sample_data/ directory not found at {}",
        sample_data_path.display()
    );

    // ── Phase 2: Agent scan_folder tool ─────────────────────────────────

    eprintln!("\n=== Phase 2: Scanning via OperationProcessor ===");
    let scan_result = run_with_user(&user_id, async {
        processor.smart_folder_scan(&sample_data_path, 5, 500).await
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

    assert!(!text_files.is_empty(), "Should have text files to ingest");

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
            vec![
                "medication",
                "lisinopril",
                "vitamin",
                "metformin",
                "drug",
                "prescription",
            ],
        ),
        (
            "Who are the contacts in the address book?",
            vec![
                "contact", "mom", "dad", "sarah", "address", "phone", "name", "email",
            ],
        ),
        (
            "What financial records do I have?",
            vec![
                "bank",
                "transaction",
                "finance",
                "payment",
                "deposit",
                "investment",
                "expense",
            ],
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
            .run_agent_query(query_text, &schemas, &node, &user_id, 10, &[], None)
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
                    eprintln!("  WARNING: Answer did not contain any of {:?}", keywords);
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
            None,
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
    svc.handle.stop(true).await;
    eprintln!("\nTest complete.");
}
