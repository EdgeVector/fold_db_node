//! Integration tests for the AI view creation pipeline: Rust source → WASM
//! compilation → view registration → query execution.
//!
//! These tests exercise the `wasm_compiler` module end-to-end, verifying that
//! LLM-generated Rust code compiles to a valid WASM module that can be
//! registered as a transform view and queried through the OperationProcessor.

use fold_db_node::fold_node::wasm_compiler;

/// Skip the test if the wasm32-unknown-unknown target is not installed.
fn require_wasm_toolchain() -> bool {
    if wasm_compiler::check_wasm_toolchain().is_err() {
        eprintln!("SKIPPING: wasm32-unknown-unknown target not installed");
        false
    } else {
        true
    }
}

// ---------------------------------------------------------------------------
// Compiler-only tests (no feature flags needed)
// ---------------------------------------------------------------------------

#[test]
fn compile_word_count_transform() {
    if !require_wasm_toolchain() {
        return;
    }

    let rust_code = r#"
fn transform_impl(input: Value) -> Value {
    let empty = vec![];
    let inputs = &input["inputs"];
    let posts = inputs["BlogPost"].as_array().unwrap_or(&empty);
    let records: Vec<Value> = posts.iter().map(|post| {
        let content = post["fields"]["content"].as_str().unwrap_or("");
        let word_count = content.split_whitespace().count();
        serde_json::json!({
            "key": post["key"],
            "fields": {
                "word_count": word_count
            }
        })
    }).collect();
    serde_json::json!({ "records": records })
}
"#;

    let wasm_bytes = wasm_compiler::compile_rust_to_wasm(rust_code)
        .expect("Word count transform should compile");

    assert!(!wasm_bytes.is_empty());
    assert_eq!(&wasm_bytes[..4], b"\0asm", "Should produce valid WASM");
}

#[test]
fn compile_concatenation_transform() {
    if !require_wasm_toolchain() {
        return;
    }

    let rust_code = r#"
fn transform_impl(input: Value) -> Value {
    let empty = vec![];
    let inputs = &input["inputs"];
    let posts = inputs["BlogPost"].as_array().unwrap_or(&empty);
    let records: Vec<Value> = posts.iter().map(|post| {
        let title = post["fields"]["title"].as_str().unwrap_or("");
        let author = post["fields"]["author"].as_str().unwrap_or("");
        let summary = format!("{} by {}", title, author);
        serde_json::json!({
            "key": post["key"],
            "fields": {
                "summary": summary
            }
        })
    }).collect();
    serde_json::json!({ "records": records })
}
"#;

    let wasm_bytes = wasm_compiler::compile_rust_to_wasm(rust_code)
        .expect("Concatenation transform should compile");

    assert!(!wasm_bytes.is_empty());
    assert_eq!(&wasm_bytes[..4], b"\0asm");
}

#[test]
fn compile_with_syntax_error_fails() {
    if !require_wasm_toolchain() {
        return;
    }

    let bad_code = r#"
fn transform_impl(input: Value) -> Value {
    let x = ;; // syntax error
    input
}
"#;

    let result = wasm_compiler::compile_rust_to_wasm(bad_code);
    assert!(result.is_err(), "Syntax errors should fail compilation");
    let err = result.unwrap_err();
    assert!(
        err.contains("error"),
        "Error message should mention compilation failure: {}",
        err
    );
}

#[test]
fn compile_with_type_error_fails() {
    if !require_wasm_toolchain() {
        return;
    }

    let bad_code = r#"
fn transform_impl(input: Value) -> Value {
    let x: i32 = "not a number";
    let _ = x;
    input
}
"#;

    let result = wasm_compiler::compile_rust_to_wasm(bad_code);
    assert!(result.is_err(), "Type errors should fail compilation");
}

// ---------------------------------------------------------------------------
// Integration tests: compile → register → query
// (require `transform-wasm` feature for WASM execution engine)
// ---------------------------------------------------------------------------

#[cfg(feature = "transform-wasm")]
mod integration {
    use super::*;
    use fold_db::schema::types::field_value_type::FieldValueType;
    use fold_db::schema::types::key_value::KeyValue;
    use fold_db::schema::types::operations::Query;
    use fold_db::schema::types::schema::DeclarativeSchemaType as SchemaType;
    use fold_db::view::types::TransformView;
    use fold_db::MutationType;
    use fold_db_node::fold_node::config::NodeConfig;
    use fold_db_node::fold_node::FoldNode;
    use fold_db_node::fold_node::OperationProcessor;
    use serde_json::json;
    use std::collections::HashMap;
    use tempfile::TempDir;

    async fn setup_node() -> (FoldNode, TempDir) {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let temp_db_path = temp_dir.path().to_str().unwrap();

        let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
        let config = NodeConfig::new(temp_db_path.into())
            .with_schema_service_url("test://mock")
            .with_identity(&keypair.public_key_base64(), &keypair.secret_key_base64());
        let node = FoldNode::new(config)
            .await
            .expect("Failed to create FoldNode");

        (node, temp_dir)
    }

    async fn load_schema(node: &FoldNode, schema_filename: &str) {
        let schema_path = std::env::current_dir()
            .expect("Failed to get current directory")
            .join("tests/schemas_for_testing")
            .join(schema_filename);

        let mut fold_db = node.get_fold_db().await.expect("Failed to get FoldDB");
        fold_db
            .load_schema_from_file(&schema_path)
            .await
            .expect("Failed to load schema");
    }

    async fn insert_blog_post(
        processor: &OperationProcessor,
        title: &str,
        content: &str,
        date: &str,
    ) {
        let mut fields = HashMap::new();
        fields.insert("title".to_string(), json!(title));
        fields.insert("content".to_string(), json!(content));
        fields.insert("author".to_string(), json!("TestAuthor"));
        fields.insert("publish_date".to_string(), json!(date));
        fields.insert("tags".to_string(), json!(["test"]));

        processor
            .execute_mutation(
                "BlogPost".to_string(),
                fields,
                KeyValue::new(None, Some(date.to_string())),
                MutationType::Create,
            )
            .await
            .expect("Failed to insert BlogPost");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compile_and_query_word_count_view() {
        if !require_wasm_toolchain() {
            return;
        }

        let (node, _tmp) = setup_node().await;
        load_schema(&node, "BlogPost.json").await;
        let processor = OperationProcessor::new(node);

        insert_blog_post(
            &processor,
            "Hello World",
            "this has exactly five words",
            "2024-01-01",
        )
        .await;
        insert_blog_post(&processor, "Second Post", "three words here", "2024-02-01").await;

        // Compile Rust → WASM
        let rust_code = r#"
fn transform_impl(input: Value) -> Value {
    let empty = vec![];
    let inputs = &input["inputs"];
    let posts = inputs["BlogPost"].as_array().unwrap_or(&empty);
    let records: Vec<Value> = posts.iter().map(|post| {
        let content = post["fields"]["content"].as_str().unwrap_or("");
        let word_count = content.split_whitespace().count();
        serde_json::json!({
            "key": post["key"],
            "fields": {
                "word_count": word_count
            }
        })
    }).collect();
    serde_json::json!({ "records": records })
}
"#;
        let wasm_bytes = wasm_compiler::compile_rust_to_wasm(rust_code)
            .expect("Should compile word count transform");

        // Register the view
        let view = TransformView::new(
            "WordCountView",
            SchemaType::Single,
            None,
            vec![Query::new(
                "BlogPost".to_string(),
                vec!["content".to_string()],
            )],
            Some(wasm_bytes),
            HashMap::from([("word_count".to_string(), FieldValueType::Integer)]),
        );
        processor
            .create_view(view)
            .await
            .expect("Should register view");

        // Query the view
        let query = Query::new("WordCountView".to_string(), vec!["word_count".to_string()]);
        let results = processor
            .execute_query_json(query)
            .await
            .expect("View query should succeed");

        assert!(!results.is_empty(), "Should return results from WASM view");

        // Verify word counts are present and positive
        for result in &results {
            let wc = result
                .get("fields")
                .and_then(|f| f.get("word_count"))
                .expect("should have word_count field");
            let count = wc.as_u64().expect("word_count should be a number");
            assert!(count > 0, "word count should be positive, got {}", count);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compile_and_query_concatenation_view() {
        if !require_wasm_toolchain() {
            return;
        }

        let (node, _tmp) = setup_node().await;
        load_schema(&node, "BlogPost.json").await;
        let processor = OperationProcessor::new(node);

        insert_blog_post(&processor, "My Title", "some content", "2024-03-01").await;

        let rust_code = r#"
fn transform_impl(input: Value) -> Value {
    let empty = vec![];
    let inputs = &input["inputs"];
    let posts = inputs["BlogPost"].as_array().unwrap_or(&empty);
    let records: Vec<Value> = posts.iter().map(|post| {
        let title = post["fields"]["title"].as_str().unwrap_or("");
        let author = post["fields"]["author"].as_str().unwrap_or("");
        let summary = format!("{} by {}", title, author);
        serde_json::json!({
            "key": post["key"],
            "fields": {
                "summary": summary
            }
        })
    }).collect();
    serde_json::json!({ "records": records })
}
"#;
        let wasm_bytes = wasm_compiler::compile_rust_to_wasm(rust_code)
            .expect("Should compile concatenation transform");

        let view = TransformView::new(
            "AuthoredPostView",
            SchemaType::Single,
            None,
            vec![Query::new(
                "BlogPost".to_string(),
                vec!["title".to_string(), "author".to_string()],
            )],
            Some(wasm_bytes),
            HashMap::from([("summary".to_string(), FieldValueType::String)]),
        );
        processor
            .create_view(view)
            .await
            .expect("Should register view");

        let query = Query::new("AuthoredPostView".to_string(), vec!["summary".to_string()]);
        let results = processor
            .execute_query_json(query)
            .await
            .expect("View query should succeed");

        assert_eq!(results.len(), 1);
        let summary = results[0]
            .get("fields")
            .and_then(|f| f.get("summary"))
            .and_then(|v| v.as_str())
            .expect("should have summary field");
        assert_eq!(summary, "My Title by TestAuthor");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compiled_view_rejects_writes() {
        if !require_wasm_toolchain() {
            return;
        }

        let (node, _tmp) = setup_node().await;
        load_schema(&node, "BlogPost.json").await;
        let processor = OperationProcessor::new(node);

        let rust_code = r#"
fn transform_impl(input: Value) -> Value {
    serde_json::json!({ "fields": { "result": "constant" } })
}
"#;
        let wasm_bytes = wasm_compiler::compile_rust_to_wasm(rust_code).expect("Should compile");

        let view = TransformView::new(
            "ReadOnlyCompiledView",
            SchemaType::Single,
            None,
            vec![Query::new(
                "BlogPost".to_string(),
                vec!["title".to_string()],
            )],
            Some(wasm_bytes),
            HashMap::from([("result".to_string(), FieldValueType::String)]),
        );
        processor
            .create_view(view)
            .await
            .expect("Should register view");

        // Attempt write — should fail since WASM views are read-only
        let mut fields = HashMap::new();
        fields.insert("result".to_string(), json!("should fail"));
        let result = processor
            .execute_mutation(
                "ReadOnlyCompiledView".to_string(),
                fields,
                KeyValue::new(None, Some("k1".to_string())),
                MutationType::Update,
            )
            .await;

        assert!(
            result.is_err(),
            "Writing through a compiled WASM view should fail"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn compiled_view_refreshes_after_source_mutation() {
        if !require_wasm_toolchain() {
            return;
        }

        let (node, _tmp) = setup_node().await;
        load_schema(&node, "BlogPost.json").await;
        let processor = OperationProcessor::new(node);

        insert_blog_post(&processor, "Original", "one two three", "2024-01-01").await;

        let rust_code = r#"
fn transform_impl(input: Value) -> Value {
    let empty = vec![];
    let inputs = &input["inputs"];
    let posts = inputs["BlogPost"].as_array().unwrap_or(&empty);
    let records: Vec<Value> = posts.iter().map(|post| {
        let content = post["fields"]["content"].as_str().unwrap_or("");
        let word_count = content.split_whitespace().count();
        serde_json::json!({
            "key": post["key"],
            "fields": { "word_count": word_count }
        })
    }).collect();
    serde_json::json!({ "records": records })
}
"#;
        let wasm_bytes = wasm_compiler::compile_rust_to_wasm(rust_code).expect("Should compile");

        let view = TransformView::new(
            "LiveWordCount",
            SchemaType::Single,
            None,
            vec![Query::new(
                "BlogPost".to_string(),
                vec!["content".to_string()],
            )],
            Some(wasm_bytes),
            HashMap::from([("word_count".to_string(), FieldValueType::Integer)]),
        );
        processor.create_view(view).await.expect("Should register");

        // First query
        let query = Query::new("LiveWordCount".to_string(), vec!["word_count".to_string()]);
        let r1 = processor
            .execute_query_json(query.clone())
            .await
            .expect("First query should succeed");
        assert_eq!(r1.len(), 1);

        // Mutate source — add another post
        insert_blog_post(&processor, "New Post", "a b c d e f g", "2024-02-01").await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Re-query — should include the new record
        let r2 = processor
            .execute_query_json(query)
            .await
            .expect("Post-mutation query should succeed");
        assert!(
            r2.len() >= 2,
            "Should see both records after mutation, got {}",
            r2.len()
        );
    }
}
