//! Integration test for ingesting a real Twitter export (tweets.js).
//!
//! This test exercises the full ingestion pipeline:
//! 1. Parse tweets.js (Twitter export format with JS prefix)
//! 2. AI-powered schema recommendation
//! 3. Schema submission to local schema service
//! 4. Mutation generation and execution
//!
//! Requires:
//! - `ANTHROPIC_API_KEY` environment variable set
//! - tests/fixtures/tweets.js file present
//!
//! Run with: `cargo test --test ingest_tweets_test -- --ignored --nocapture`

use fold_db::logging::core::run_with_user;
use fold_db_node::fold_node::node::FoldNode;
use fold_db_node::ingestion::ingestion_service::IngestionService;
use fold_db_node::ingestion::smart_folder::read_file_as_json;
use fold_db_node::ingestion::{create_progress_tracker, IngestionRequest, ProgressService};
mod common;

use std::path::Path;

use common::schema_service::{spawn_schema_service, SpawnedSchemaService};

async fn spawn_local_schema_service() -> SpawnedSchemaService {
    spawn_schema_service().await
}

// -- Integration test --------------------------------------------------------

#[actix_web::test]
#[ignore] // Requires ANTHROPIC_API_KEY and AI calls
async fn test_ingest_tweets_js() {
    // Skip if no API key
    if std::env::var("ANTHROPIC_API_KEY").is_err() {
        eprintln!("Skipping: ANTHROPIC_API_KEY not set");
        return;
    }

    // Check that the fixture file exists
    let tweets_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tweets.js");
    if !tweets_path.exists() {
        eprintln!(
            "Skipping: tweets.js fixture not found at {}",
            tweets_path.display()
        );
        return;
    }

    // 1. Spin up a local schema service
    let svc = spawn_local_schema_service().await;
    let schema_url = svc.url.clone();
    eprintln!("Local schema service running at {}", schema_url);

    // 2. Create FoldNode with the local schema service
    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair))
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    // 3. Parse tweets.js
    let tweet_data = read_file_as_json(&tweets_path).expect("Failed to parse tweets.js");
    eprintln!(
        "Parsed tweets.js: {} top-level items",
        tweet_data.as_array().map(|a| a.len()).unwrap_or(1)
    );

    // 4. Create IngestionService from environment
    let ingestion_service =
        IngestionService::from_env().expect("Failed to create ingestion service");

    // 5. Create progress tracking
    let progress_tracker = create_progress_tracker().await;
    let progress_service = ProgressService::new(progress_tracker);

    // 6. Build ingestion request
    let request = IngestionRequest {
        data: tweet_data,
        auto_execute: true,
        pub_key: user_id.clone(),
        source_file_name: Some("tweets.js".to_string()),
        progress_id: Some("test-tweets-ingestion".to_string()),
        file_hash: None,
        source_folder: None,
        image_descriptive_name: None,
        org_hash: None,
        image_bytes: None,
    };

    // 7. Run ingestion within user context
    let response = run_with_user(&user_id, async {
        ingestion_service
            .process_json_with_node_and_progress(
                request,
                &node,
                &progress_service,
                "test-tweets-ingestion".to_string(),
            )
            .await
    })
    .await;

    match &response {
        Ok(resp) => {
            eprintln!("Ingestion response:");
            eprintln!("  success: {}", resp.success);
            eprintln!("  schema_used: {:?}", resp.schema_used);
            eprintln!("  new_schema_created: {}", resp.new_schema_created);
            eprintln!("  mutations_generated: {}", resp.mutations_generated);
            eprintln!("  mutations_executed: {}", resp.mutations_executed);
            eprintln!("  errors: {:?}", resp.errors);
        }
        Err(e) => {
            eprintln!("Ingestion failed: {}", e);
        }
    }

    let response = response.expect("Ingestion should succeed");

    // 8. Assert results
    assert!(response.success, "Ingestion should succeed");
    assert!(
        response.mutations_generated > 0,
        "Should generate at least one mutation"
    );
    assert!(
        response.mutations_executed > 0,
        "Should execute at least one mutation"
    );
    assert!(response.schema_used.is_some(), "Should have used a schema");

    eprintln!(
        "Successfully ingested tweets.js: {} mutations generated, {} executed",
        response.mutations_generated, response.mutations_executed
    );

    // Cleanup
    svc.handle.stop(true).await;
}
