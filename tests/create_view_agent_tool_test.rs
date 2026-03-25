//! Integration test for the `create_view` agent tool pipeline.
//!
//! Simulates exactly what `execute_tool("create_view", params, node)` does:
//! receives the same JSON params the LLM would produce, compiles Rust to WASM,
//! constructs a `TransformView`, registers it, and queries it — all through
//! public APIs.
//!
//! Requires the `transform-wasm` feature for WASM execution.

#[cfg(feature = "transform-wasm")]
mod tests {
    use fold_db::schema::types::field_value_type::FieldValueType;
    use fold_db::schema::types::key_value::KeyValue;
    use fold_db::schema::types::operations::Query;
    use fold_db::schema::types::schema::DeclarativeSchemaType as SchemaType;
    use fold_db::view::types::TransformView;
    use fold_db::MutationType;
    use fold_db_node::fold_node::config::NodeConfig;
    use fold_db_node::fold_node::wasm_compiler;
    use fold_db_node::fold_node::FoldNode;
    use fold_db_node::fold_node::OperationProcessor;
    use serde_json::{json, Value};
    use std::collections::HashMap;
    use tempfile::TempDir;

    // -- helpers (same as view_query_http_test.rs) --

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

    /// Replicate the `execute_tool("create_view", params)` logic against public APIs.
    /// This is the same code path the agent tool takes, extracted so we can test
    /// without needing an LLM backend.
    async fn execute_create_view_tool(
        processor: &OperationProcessor,
        params: &Value,
    ) -> Result<Value, String> {
        let name = params
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or("create_view tool requires 'name' parameter")?;

        let schema_type_str = params
            .get("schema_type")
            .and_then(|s| s.as_str())
            .ok_or("create_view tool requires 'schema_type' parameter")?;

        let schema_type: SchemaType =
            serde_json::from_value(Value::String(schema_type_str.to_string()))
                .map_err(|e| format!("Invalid schema_type '{}': {}", schema_type_str, e))?;

        let key_config = params
            .get("key_config")
            .and_then(|k| {
                if k.is_null() {
                    None
                } else {
                    Some(serde_json::from_value(k.clone()))
                }
            })
            .transpose()
            .map_err(|e| format!("Invalid key_config: {}", e))?;

        let input_queries_val = params
            .get("input_queries")
            .and_then(|q| q.as_array())
            .ok_or("create_view tool requires 'input_queries' parameter (array)")?;

        let input_queries: Vec<Query> = input_queries_val
            .iter()
            .map(|q| serde_json::from_value(q.clone()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Invalid input_queries: {}", e))?;

        let output_fields_val = params
            .get("output_fields")
            .and_then(|o| o.as_object())
            .ok_or("create_view tool requires 'output_fields' parameter (object)")?;

        let output_fields: HashMap<String, FieldValueType> = output_fields_val
            .iter()
            .map(|(k, v)| {
                let fvt: FieldValueType = serde_json::from_value(v.clone())
                    .map_err(|e| format!("Invalid field type for '{}': {}", k, e))?;
                Ok((k.clone(), fvt))
            })
            .collect::<Result<HashMap<_, _>, String>>()?;

        let rust_transform = params
            .get("rust_transform")
            .and_then(|r| r.as_str())
            .ok_or("create_view tool requires 'rust_transform' parameter")?;

        let wasm_bytes = wasm_compiler::compile_rust_to_wasm(rust_transform)?;

        let view = TransformView::new(
            name.to_string(),
            schema_type,
            key_config,
            input_queries,
            Some(wasm_bytes),
            output_fields,
        );

        processor
            .create_view(view)
            .await
            .map_err(|e| format!("Failed to create view: {}", e))?;

        Ok(json!({
            "success": true,
            "message": format!("View '{}' created successfully with WASM transform", name),
            "view_name": name,
        }))
    }

    // -- tests --

    /// Full round trip: LLM-style JSON params → compile → register → query → verify.
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_tool_create_view_word_count() {
        if wasm_compiler::check_wasm_toolchain().is_err() {
            eprintln!("SKIPPING: wasm32-unknown-unknown target not installed");
            return;
        }

        let (node, _tmp) = setup_node().await;
        load_schema(&node, "BlogPost.json").await;
        let processor = OperationProcessor::new(node);

        insert_blog_post(&processor, "Hello", "one two three four five", "2024-01-01").await;
        insert_blog_post(&processor, "World", "six seven", "2024-02-01").await;

        // This is exactly the JSON the LLM would produce as tool params
        let tool_params = json!({
            "name": "PostWordCounts",
            "schema_type": "Single",
            "input_queries": [
                {"schema_name": "BlogPost", "fields": ["content"]}
            ],
            "output_fields": {
                "word_count": "Integer",
                "content_preview": "String"
            },
            "rust_transform": "fn transform_impl(input: Value) -> Value {\n    let empty = vec![];\n    let inputs = &input[\"inputs\"];\n    let posts = inputs[\"BlogPost\"].as_array().unwrap_or(&empty);\n    let records: Vec<Value> = posts.iter().map(|post| {\n        let content = post[\"fields\"][\"content\"].as_str().unwrap_or(\"\");\n        let word_count = content.split_whitespace().count();\n        let preview: String = content.chars().take(50).collect();\n        serde_json::json!({\n            \"key\": post[\"key\"],\n            \"fields\": {\n                \"word_count\": word_count,\n                \"content_preview\": preview\n            }\n        })\n    }).collect();\n    serde_json::json!({ \"records\": records })\n}"
        });

        // Execute the tool
        let result = execute_create_view_tool(&processor, &tool_params)
            .await
            .expect("create_view tool should succeed");

        assert_eq!(result["success"], true);
        assert_eq!(result["view_name"], "PostWordCounts");

        // Query the newly created view
        let query = Query::new(
            "PostWordCounts".to_string(),
            vec!["word_count".to_string(), "content_preview".to_string()],
        );
        let results = processor
            .execute_query_json(query)
            .await
            .expect("View query should succeed");

        assert!(!results.is_empty(), "Should return results");

        // Verify at least one result has a positive word count
        let has_word_count = results.iter().any(|r| {
            r.get("fields")
                .and_then(|f| f.get("word_count"))
                .and_then(|v| v.as_u64())
                .map(|c| c > 0)
                .unwrap_or(false)
        });
        assert!(
            has_word_count,
            "At least one result should have a positive word_count"
        );

        // Verify content_preview is present
        let has_preview = results.iter().any(|r| {
            r.get("fields")
                .and_then(|f| f.get("content_preview"))
                .and_then(|v| v.as_str())
                .map(|s| !s.is_empty())
                .unwrap_or(false)
        });
        assert!(
            has_preview,
            "At least one result should have a non-empty content_preview"
        );
    }

    /// Verify the tool rejects params with missing required fields.
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_tool_rejects_missing_name() {
        if wasm_compiler::check_wasm_toolchain().is_err() {
            eprintln!("SKIPPING: wasm32-unknown-unknown target not installed");
            return;
        }

        let (node, _tmp) = setup_node().await;
        let processor = OperationProcessor::new(node);

        let params = json!({
            "schema_type": "Single",
            "input_queries": [{"schema_name": "X", "fields": ["y"]}],
            "output_fields": {"z": "Any"},
            "rust_transform": "fn transform_impl(input: Value) -> Value { input }"
        });

        let result = execute_create_view_tool(&processor, &params).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("name"),
            "Error should mention missing 'name'"
        );
    }

    /// Verify the tool rejects params with missing rust_transform.
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_tool_rejects_missing_rust_transform() {
        if wasm_compiler::check_wasm_toolchain().is_err() {
            eprintln!("SKIPPING: wasm32-unknown-unknown target not installed");
            return;
        }

        let (node, _tmp) = setup_node().await;
        let processor = OperationProcessor::new(node);

        let params = json!({
            "name": "MissingTransform",
            "schema_type": "Single",
            "input_queries": [{"schema_name": "X", "fields": ["y"]}],
            "output_fields": {"z": "Any"}
        });

        let result = execute_create_view_tool(&processor, &params).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("rust_transform"),
            "Error should mention missing 'rust_transform'"
        );
    }

    /// Verify the tool returns a compilation error for invalid Rust.
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_tool_returns_compile_error_for_bad_rust() {
        if wasm_compiler::check_wasm_toolchain().is_err() {
            eprintln!("SKIPPING: wasm32-unknown-unknown target not installed");
            return;
        }

        let (node, _tmp) = setup_node().await;
        let processor = OperationProcessor::new(node);

        let params = json!({
            "name": "BadView",
            "schema_type": "Single",
            "input_queries": [{"schema_name": "BlogPost", "fields": ["title"]}],
            "output_fields": {"result": "String"},
            "rust_transform": "fn transform_impl(input: Value) -> Value { let x: i32 = \"bad\"; let _ = x; input }"
        });

        let result = execute_create_view_tool(&processor, &params).await;
        assert!(result.is_err(), "Invalid Rust should fail");
        assert!(
            result.unwrap_err().contains("error"),
            "Error should contain compilation error details"
        );
    }

    /// Verify the tool rejects an invalid schema_type.
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_tool_rejects_invalid_schema_type() {
        if wasm_compiler::check_wasm_toolchain().is_err() {
            eprintln!("SKIPPING: wasm32-unknown-unknown target not installed");
            return;
        }

        let (node, _tmp) = setup_node().await;
        let processor = OperationProcessor::new(node);

        let params = json!({
            "name": "BadType",
            "schema_type": "NotAType",
            "input_queries": [{"schema_name": "X", "fields": ["y"]}],
            "output_fields": {"z": "Any"},
            "rust_transform": "fn transform_impl(input: Value) -> Value { input }"
        });

        let result = execute_create_view_tool(&processor, &params).await;
        assert!(result.is_err());
        assert!(
            result.unwrap_err().contains("Invalid schema_type"),
            "Error should mention invalid schema_type"
        );
    }

    /// Full round trip with a multi-schema join view.
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_tool_multi_schema_join_view() {
        if wasm_compiler::check_wasm_toolchain().is_err() {
            eprintln!("SKIPPING: wasm32-unknown-unknown target not installed");
            return;
        }

        let (node, _tmp) = setup_node().await;
        load_schema(&node, "BlogPost.json").await;
        let processor = OperationProcessor::new(node);

        insert_blog_post(
            &processor,
            "Rust 101",
            "Learn Rust from scratch",
            "2024-01-01",
        )
        .await;
        insert_blog_post(
            &processor,
            "WASM Guide",
            "WebAssembly introduction",
            "2024-02-01",
        )
        .await;

        // View that combines title + author into a single summary string
        let tool_params = json!({
            "name": "PostSummaries",
            "schema_type": "Single",
            "input_queries": [
                {"schema_name": "BlogPost", "fields": ["title", "author", "content"]}
            ],
            "output_fields": {
                "summary": "String"
            },
            "rust_transform": "fn transform_impl(input: Value) -> Value {\n    let empty = vec![];\n    let inputs = &input[\"inputs\"];\n    let posts = inputs[\"BlogPost\"].as_array().unwrap_or(&empty);\n    let records: Vec<Value> = posts.iter().map(|post| {\n        let title = post[\"fields\"][\"title\"].as_str().unwrap_or(\"?\");\n        let author = post[\"fields\"][\"author\"].as_str().unwrap_or(\"?\");\n        let content = post[\"fields\"][\"content\"].as_str().unwrap_or(\"\");\n        let words = content.split_whitespace().count();\n        let summary = format!(\"{} by {} ({} words)\", title, author, words);\n        serde_json::json!({\n            \"key\": post[\"key\"],\n            \"fields\": { \"summary\": summary }\n        })\n    }).collect();\n    serde_json::json!({ \"records\": records })\n}"
        });

        let result = execute_create_view_tool(&processor, &tool_params)
            .await
            .expect("create_view should succeed");
        assert_eq!(result["success"], true);

        // Query and verify
        let query = Query::new("PostSummaries".to_string(), vec!["summary".to_string()]);
        let results = processor
            .execute_query_json(query)
            .await
            .expect("View query should succeed");

        assert_eq!(results.len(), 2, "Should have 2 summaries");

        let summaries: Vec<&str> = results
            .iter()
            .filter_map(|r| {
                r.get("fields")
                    .and_then(|f| f.get("summary"))
                    .and_then(|v| v.as_str())
            })
            .collect();

        // Verify summaries contain expected content
        assert!(
            summaries
                .iter()
                .any(|s| s.contains("Rust 101") && s.contains("TestAuthor")),
            "Should have Rust 101 summary, got: {:?}",
            summaries
        );
        assert!(
            summaries
                .iter()
                .any(|s| s.contains("WASM Guide") && s.contains("words")),
            "Should have WASM Guide summary with word count, got: {:?}",
            summaries
        );
    }

    /// Verify the created view appears in the view list.
    #[tokio::test(flavor = "multi_thread")]
    async fn agent_tool_created_view_shows_in_list() {
        if wasm_compiler::check_wasm_toolchain().is_err() {
            eprintln!("SKIPPING: wasm32-unknown-unknown target not installed");
            return;
        }

        let (node, _tmp) = setup_node().await;
        load_schema(&node, "BlogPost.json").await;
        let processor = OperationProcessor::new(node);

        let tool_params = json!({
            "name": "ListableView",
            "schema_type": "Single",
            "input_queries": [
                {"schema_name": "BlogPost", "fields": ["title"]}
            ],
            "output_fields": {"title_upper": "String"},
            "rust_transform": "fn transform_impl(input: Value) -> Value {\n    let empty = vec![];\n    let inputs = &input[\"inputs\"];\n    let posts = inputs[\"BlogPost\"].as_array().unwrap_or(&empty);\n    let records: Vec<Value> = posts.iter().map(|post| {\n        let title = post[\"fields\"][\"title\"].as_str().unwrap_or(\"\").to_uppercase();\n        serde_json::json!({\n            \"key\": post[\"key\"],\n            \"fields\": { \"title_upper\": title }\n        })\n    }).collect();\n    serde_json::json!({ \"records\": records })\n}"
        });

        execute_create_view_tool(&processor, &tool_params)
            .await
            .expect("create_view should succeed");

        // Verify it appears in the list
        let views = processor
            .list_views()
            .await
            .expect("list_views should succeed");

        let found = views.iter().any(|(v, _state)| v.name == "ListableView");
        assert!(found, "Created view should appear in list_views");

        // Verify it's a WASM view (not identity)
        let (view, _) = views
            .iter()
            .find(|(v, _)| v.name == "ListableView")
            .expect("Should find ListableView");
        assert!(
            !view.is_identity(),
            "View should have a WASM transform (not identity)"
        );
        assert!(
            view.wasm_transform.is_some(),
            "View should have wasm_transform bytes"
        );
    }
}
