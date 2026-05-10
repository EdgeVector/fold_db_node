//! Pagination metadata on the query handler (`POST /api/query`).
//!
//! The handler used to silently truncate to 100 records — fold_db's default
//! `HashRangeFilter::SampleN(100)` kicks in when `Query.filter` is `None`, and
//! callers had no `total_count` / `has_more` to detect it. The handler now
//! overrides that default with `SampleN(INTERNAL_FETCH_CAP)` so `total_count`
//! is accurate for typical user databases, and exposes the page metadata.
//!
//! These tests prove the contract called out in the kanban task: `total_count`
//! is non-truncated even when `results.len() == limit`.

use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::operations::Query;
use fold_db::MutationType;
use fold_db_node::fold_node::{FoldNode, OperationProcessor};
use fold_db_node::handlers::query as query_handlers;
use serde_json::json;
use std::collections::HashMap;

mod common;
use common::setup_node;

const TOTAL_RECORDS: usize = 150;

async fn load_blog_post_schema(node: &FoldNode) {
    let schema_path = std::env::current_dir()
        .expect("Failed to get current directory")
        .join("tests/schemas_for_testing/BlogPost.json");

    let fold_db = node.get_fold_db().expect("Failed to get FoldDB");
    fold_db
        .load_schema_from_file(&schema_path)
        .await
        .expect("Failed to load BlogPost schema");
}

async fn insert_n_blog_posts(processor: &OperationProcessor, n: usize) {
    for i in 0..n {
        // Pad the index so the range_field sorts lexicographically and each
        // record has a unique key.
        let date = format!("2024-{:04}", i);
        let mut fields = HashMap::new();
        fields.insert("title".to_string(), json!(format!("Post {:04}", i)));
        fields.insert("content".to_string(), json!(format!("Body {}", i)));
        fields.insert("author".to_string(), json!("TestAuthor"));
        fields.insert("publish_date".to_string(), json!(date.clone()));
        fields.insert("tags".to_string(), json!(["test"]));

        processor
            .execute_mutation(
                "BlogPost".to_string(),
                fields,
                KeyValue::new(None, Some(date)),
                MutationType::Create,
            )
            .await
            .expect("Failed to insert BlogPost");
    }
}

fn unwrap_response(
    api: fold_db_node::handlers::response::ApiResponse<query_handlers::QueryResponse>,
) -> query_handlers::QueryResponse {
    assert!(api.ok, "handler returned ok=false: {:?}", api.error);
    api.data.expect("handler returned no data")
}

async fn setup_with_n_records(n: usize) -> (FoldNode, tempfile::TempDir) {
    let (node, tmp) = setup_node().await;
    load_blog_post_schema(&node).await;
    let processor = OperationProcessor::from_ref(&node);
    insert_n_blog_posts(&processor, n).await;
    (node, tmp)
}

/// The bug repro: 150 records inserted, no `limit` passed → `total_count`
/// must reflect the full set, even though `results.len()` equals the default
/// page size (100). Without this signal callers cannot detect truncation.
#[tokio::test(flavor = "multi_thread")]
async fn unfiltered_query_reports_total_count_when_results_equal_default_limit() {
    let (node, _tmp) = setup_with_n_records(TOTAL_RECORDS).await;

    let query = Query::new("BlogPost".to_string(), vec!["title".to_string()]);
    let response = query_handlers::execute_query(query, None, None, "test-user", &node)
        .await
        .expect("execute_query should succeed");
    let body = unwrap_response(response);

    assert_eq!(
        body.total_count, TOTAL_RECORDS,
        "total_count must reflect the full set, not the page size"
    );
    assert_eq!(
        body.returned_count,
        query_handlers::DEFAULT_QUERY_LIMIT,
        "returned_count should equal the default page size"
    );
    assert_eq!(body.limit, query_handlers::DEFAULT_QUERY_LIMIT);
    assert_eq!(body.offset, 0);
    assert!(
        body.has_more,
        "has_more must be true when total_count > returned_count"
    );

    let arr = body
        .results
        .as_array()
        .expect("results must be a JSON array");
    assert_eq!(arr.len(), query_handlers::DEFAULT_QUERY_LIMIT);
}

/// `limit` larger than the dataset returns everything and reports
/// `has_more = false`.
#[tokio::test(flavor = "multi_thread")]
async fn limit_above_dataset_returns_full_set() {
    let (node, _tmp) = setup_with_n_records(TOTAL_RECORDS).await;

    let query = Query::new("BlogPost".to_string(), vec!["title".to_string()]);
    let response = query_handlers::execute_query(query, Some(500), None, "test-user", &node)
        .await
        .expect("execute_query should succeed");
    let body = unwrap_response(response);

    // 500 is below MAX_QUERY_LIMIT (1000) so the requested limit is honoured.
    assert_eq!(body.limit, 500);
    assert_eq!(body.total_count, TOTAL_RECORDS);
    assert_eq!(body.returned_count, TOTAL_RECORDS);
    assert!(!body.has_more);
}

/// `offset` + smaller `limit` returns the requested slice and signals more.
#[tokio::test(flavor = "multi_thread")]
async fn offset_and_limit_slice_the_result_set() {
    let (node, _tmp) = setup_with_n_records(TOTAL_RECORDS).await;

    let query = Query::new("BlogPost".to_string(), vec!["title".to_string()]);
    let response = query_handlers::execute_query(query, Some(50), Some(50), "test-user", &node)
        .await
        .expect("execute_query should succeed");
    let body = unwrap_response(response);

    assert_eq!(body.total_count, TOTAL_RECORDS);
    assert_eq!(body.returned_count, 50);
    assert_eq!(body.limit, 50);
    assert_eq!(body.offset, 50);
    assert!(
        body.has_more,
        "has_more must be true when offset+returned < total"
    );
}

/// `limit` is clamped to `MAX_QUERY_LIMIT`.
#[tokio::test(flavor = "multi_thread")]
async fn excessive_limit_is_clamped_to_max() {
    let (node, _tmp) = setup_with_n_records(TOTAL_RECORDS).await;

    let query = Query::new("BlogPost".to_string(), vec!["title".to_string()]);
    let response = query_handlers::execute_query(query, Some(1_000_000), None, "test-user", &node)
        .await
        .expect("execute_query should succeed");
    let body = unwrap_response(response);

    assert_eq!(body.limit, query_handlers::MAX_QUERY_LIMIT);
}
