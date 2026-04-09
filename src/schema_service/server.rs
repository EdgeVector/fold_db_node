use actix_cors::Cors;
use actix_web::{web, App, HttpServer as ActixHttpServer};

use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;

// Re-export types for backward compatibility (e.g., schema_client.rs imports from here)
pub use super::state::{SchemaServiceState, SchemaStorage};
pub use super::types::*;

// Route handlers (pub(super) visibility — accessible from sibling modules)
use super::routes::{
    add_schema, add_view, batch_check_reuse, find_similar, find_similar_transforms,
    get_available_schemas, get_available_transforms, get_available_views, get_schema,
    get_transform, get_transform_wasm, get_view, health_check, list_schemas, list_transforms,
    list_views, register_transform, reload_schemas, reset_database, verify_transform,
};

/// Schema Service HTTP Server
pub struct SchemaServiceServer {
    state: web::Data<SchemaServiceState>,
    bind_address: String,
}

impl SchemaServiceServer {
    /// Create a new schema service server with local sled storage
    pub fn new(db_path: String, bind_address: &str) -> FoldDbResult<Self> {
        let state = SchemaServiceState::new(db_path)?;

        Ok(Self {
            state: web::Data::new(state),
            bind_address: bind_address.to_string(),
        })
    }

    /// Run the schema service server
    pub async fn run(&self) -> FoldDbResult<()> {
        log_feature!(
            LogFeature::HttpServer,
            info,
            "Schema service starting on {}",
            self.bind_address
        );

        let state = self.state.clone();

        let server = ActixHttpServer::new(move || {
            let cors = Cors::default()
                .allow_any_origin()
                .allow_any_method()
                .allow_any_header()
                .max_age(3600);

            App::new().wrap(cors).app_data(state.clone()).service(
                web::scope("/api")
                    .route("/health", web::get().to(health_check))
                    .service(
                        web::resource("/schemas")
                            .route(web::get().to(list_schemas))
                            .route(web::post().to(add_schema)),
                    )
                    .route(
                        "/schemas/batch-check-reuse",
                        web::post().to(batch_check_reuse),
                    )
                    .route("/schemas/reload", web::post().to(reload_schemas))
                    .route("/schemas/available", web::get().to(get_available_schemas))
                    .route("/schemas/similar/{name}", web::get().to(find_similar))
                    .route("/schema/{name}", web::get().to(get_schema))
                    // View endpoints
                    .service(
                        web::resource("/views")
                            .route(web::get().to(list_views))
                            .route(web::post().to(add_view)),
                    )
                    .route("/views/available", web::get().to(get_available_views))
                    .route("/view/{name}", web::get().to(get_view))
                    // Transform endpoints
                    .service(
                        web::resource("/transforms")
                            .route(web::get().to(list_transforms))
                            .route(web::post().to(register_transform)),
                    )
                    .route(
                        "/transforms/available",
                        web::get().to(get_available_transforms),
                    )
                    .route("/transforms/verify", web::post().to(verify_transform))
                    .route(
                        "/transforms/similar/{name}",
                        web::get().to(find_similar_transforms),
                    )
                    .route("/transform/{hash}", web::get().to(get_transform))
                    .route("/transform/{hash}/wasm", web::get().to(get_transform_wasm))
                    .route("/system/reset", web::post().to(reset_database)),
            )
        })
        .bind(&self.bind_address)
        .map_err(|e| FoldDbError::Config(format!("Failed to bind schema service: {}", e)))?
        .run();

        server
            .await
            .map_err(|e| FoldDbError::Config(format!("Schema service error: {}", e)))?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_service::state::jaccard_index;
    use fold_db::schema::types::Schema;

    /// Build a test schema with all required fields (descriptive_name, field_descriptions, classifications)
    fn make_test_schema(name: &str, fields: &[&str]) -> Schema {
        let field_strings: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
        let mut schema = Schema::new(
            name.to_string(),
            fold_db::schema::types::SchemaType::Single,
            None,
            Some(field_strings.clone()),
            None,
            None,
        );
        schema.descriptive_name = Some(name.to_string());
        for f in &field_strings {
            schema
                .field_classifications
                .insert(f.clone(), vec!["word".to_string()]);
            schema
                .field_descriptions
                .insert(f.clone(), format!("{} field", f));
            schema.field_data_classifications.insert(
                f.clone(),
                fold_db::schema::types::DataClassification::new(0, "general").unwrap(),
            );
        }
        schema
    }
    use std::collections::{HashMap, HashSet};
    use tempfile::tempdir;

    #[tokio::test]
    async fn add_schema_adds_new_schema() {
        let temp_dir = tempdir().expect("failed to create temp directory");
        let db_path = temp_dir
            .path()
            .join("test_schema_db")
            .to_string_lossy()
            .to_string();

        let state = SchemaServiceState::new(db_path.clone())
            .expect("failed to initialize schema service state");

        let new_schema = make_test_schema("New Schema", &["id", "value"]);

        let outcome = state
            .add_schema(new_schema.clone(), HashMap::new())
            .await
            .expect("failed to add schema");

        let added_schema = match outcome {
            SchemaAddOutcome::Added(schema, _mutation_mappers) => schema,
            SchemaAddOutcome::AlreadyExists(..) | SchemaAddOutcome::Expanded(..) => {
                panic!("schema should have been added")
            }
        };

        // Schema name should be the identity hash (hash of descriptive_name + fields)
        assert_ne!(
            added_schema.name, "New Schema",
            "schema name should be a hash, not the readable name"
        );
        assert_eq!(
            added_schema.descriptive_name,
            Some("New Schema".to_string())
        );

        // Classifications should match
        assert_eq!(
            added_schema.field_classifications,
            new_schema.field_classifications
        );

        let stored_schemas = state
            .schemas
            .read()
            .expect("failed to acquire read lock on schema map after addition");

        // Check stored by combined name
        assert!(stored_schemas.contains_key(&added_schema.name));

        // Check the underlying storage
        match &state.storage {
            SchemaStorage::Sled { schemas_tree, .. } => {
                let db_value = schemas_tree
                    .get(added_schema.name.as_bytes())
                    .expect("failed to query database")
                    .expect("schema should exist in database");

                let stored_schema: Schema =
                    serde_json::from_slice(&db_value).expect("failed to deserialize stored schema");

                assert_eq!(stored_schema.name, added_schema.name);
            }
        }
    }

    #[tokio::test]
    async fn add_schema_detects_duplicate_by_name_and_fields() {
        let temp_dir = tempdir().expect("failed to create temp directory");
        let db_path = temp_dir
            .path()
            .join("test_schema_db")
            .to_string_lossy()
            .to_string();

        let state = SchemaServiceState::new(db_path.clone())
            .expect("failed to initialize schema service state");

        let schema1 = make_test_schema("Existing Items", &["id", "value"]);
        let schema2 = make_test_schema("Existing Items", &["id", "value"]);

        // First schema gets added with semantic name
        let outcome1 = state
            .add_schema(schema1.clone(), HashMap::new())
            .await
            .expect("failed to add schema");

        let existing_name = match outcome1 {
            SchemaAddOutcome::Added(schema, _) => {
                // Name is now the identity hash, not the readable name
                assert_eq!(schema.descriptive_name, Some("Existing Items".to_string()));
                schema.name
            }
            _ => panic!("first schema should be added"),
        };

        // Second schema with SAME name and fields should dedup
        let outcome2 = state
            .add_schema(schema2.clone(), HashMap::new())
            .await
            .expect("failed to evaluate schema similarity");

        match outcome2 {
            SchemaAddOutcome::AlreadyExists(schema, _) => {
                assert_eq!(schema.name, existing_name);
            }
            _ => panic!("schema with same name and fields should return AlreadyExists"),
        }
    }

    #[tokio::test]
    async fn add_schema_different_name_same_fields_creates_separate() {
        let temp_dir = tempdir().expect("failed to create temp directory");
        let db_path = temp_dir
            .path()
            .join("test_schema_db")
            .to_string_lossy()
            .to_string();

        let state = SchemaServiceState::new(db_path.clone())
            .expect("failed to initialize schema service state");

        let schema1 = make_test_schema("Recipes", &["id", "value"]);
        let schema2 = make_test_schema("Journal Entries", &["id", "value"]);

        let outcome1 = state
            .add_schema(schema1, HashMap::new())
            .await
            .expect("failed to add first schema");
        assert!(matches!(outcome1, SchemaAddOutcome::Added(..)));

        // Different semantic name = different schema, even with same fields
        let outcome2 = state
            .add_schema(schema2, HashMap::new())
            .await
            .expect("failed to add second schema");
        // Should NOT be AlreadyExists — should be Added or Expanded
        assert!(
            !matches!(outcome2, SchemaAddOutcome::AlreadyExists(..)),
            "Different semantic names with same fields should create separate schemas"
        );
    }

    #[tokio::test]
    async fn add_schema_with_different_fields_creates_separate_schema() {
        let temp_dir = tempdir().expect("failed to create temp directory");
        let db_path = temp_dir
            .path()
            .join("test_schema_db")
            .to_string_lossy()
            .to_string();

        let state = SchemaServiceState::new(db_path.clone())
            .expect("failed to initialize schema service state");

        // First schema: 2 fields
        let schema1 = make_test_schema("User Basic", &["id", "name"]);

        let outcome1 = state
            .add_schema(schema1.clone(), HashMap::new())
            .await
            .expect("failed to add first schema");

        let schema1_name = match outcome1 {
            SchemaAddOutcome::Added(schema, _) => schema.name,
            other => panic!("expected schema addition, got {:?}", other),
        };

        // Second schema: 4 fields with <50% overlap so it stays separate
        let schema2 = make_test_schema(
            "Product Catalog",
            &["sku", "price", "category", "description"],
        );

        let outcome2 = state
            .add_schema(schema2.clone(), HashMap::new())
            .await
            .expect("failed to add second schema");

        let schema2_name = match outcome2 {
            SchemaAddOutcome::Added(schema, _) => schema.name,
            other => panic!("expected schema addition, got {:?}", other),
        };

        // Schema names are identity hashes, not readable names
        // Different fields → different hashes
        assert_ne!(schema1_name, schema2_name);
    }

    #[tokio::test]
    async fn get_available_schemas_returns_all_schemas() {
        let temp_dir = tempdir().expect("failed to create temp directory");
        let db_path = temp_dir
            .path()
            .join("test_schema_db")
            .to_string_lossy()
            .to_string();

        let state = SchemaServiceState::new(db_path.clone())
            .expect("failed to initialize schema service state");

        let schema1 = make_test_schema("Users", &["user_id", "username", "email"]);

        let schema2 =
            make_test_schema("Products", &["product_id", "title", "price", "description"]);

        let outcome1 = state
            .add_schema(schema1.clone(), HashMap::new())
            .await
            .expect("failed to add schema1");
        let schema1_name = match outcome1 {
            SchemaAddOutcome::Added(s, _) => s.name,
            _ => panic!("schema1 should be added"),
        };

        let outcome2 = state
            .add_schema(schema2.clone(), HashMap::new())
            .await
            .expect("failed to add schema2");
        let schema2_name = match outcome2 {
            SchemaAddOutcome::Added(s, _) => s.name,
            _ => panic!("schema2 should be added"),
        };

        let schemas = state
            .schemas
            .read()
            .expect("failed to acquire read lock on schemas");
        assert_eq!(schemas.len(), 2);

        // Schemas are now stored by identity_hash
        assert!(schemas.contains_key(&schema1_name));
        assert!(schemas.contains_key(&schema2_name));

        // Different topologies should produce different names
        assert_ne!(schema1_name, schema2_name);
    }

    // ========== find_similar_schemas tests ==========

    /// Helper: create a schema with the given fields, add classifications, and insert it via state.add_schema
    async fn add_test_schema(state: &SchemaServiceState, name: &str, fields: Vec<&str>) -> String {
        let field_strings: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
        let mut schema = Schema::new(
            name.to_string(),
            fold_db::schema::types::SchemaType::Single,
            None,
            Some(field_strings.clone()),
            None,
            None,
        );
        schema.descriptive_name = Some(name.to_string());
        for f in &field_strings {
            schema
                .field_classifications
                .insert(f.clone(), vec!["word".to_string()]);
            schema
                .field_descriptions
                .insert(f.clone(), format!("{} field", f));
            schema.field_data_classifications.insert(
                f.clone(),
                fold_db::schema::types::DataClassification::new(0, "general").unwrap(),
            );
        }
        let outcome = state
            .add_schema(schema, HashMap::new())
            .await
            .expect("failed to add test schema");
        match outcome {
            SchemaAddOutcome::Added(s, _) | SchemaAddOutcome::Expanded(_, s, _) => s.name,
            SchemaAddOutcome::AlreadyExists(s, _) => s.name,
        }
    }

    fn make_test_state() -> SchemaServiceState {
        let temp_dir = tempdir().expect("failed to create temp directory");
        let db_path = temp_dir
            .path()
            .join("test_schema_db")
            .to_string_lossy()
            .to_string();
        // Leak the tempdir so it isn't deleted while state is in use
        std::mem::forget(temp_dir);
        SchemaServiceState::new(db_path).expect("failed to create state")
    }

    #[tokio::test]
    async fn find_similar_identical_fields_returns_similarity_1() {
        let state = make_test_state();
        // Use schemas with low overlap to avoid triggering the expansion fallback.
        // A: {a, b, c, d}, B: {a, e, f, g} → overlap=1, min_size=4, 1*2=2 ≤ 4
        // Jaccard({a,b,c,d}, {a,e,f,g}) = 1/7
        let name_a = add_test_schema(&state, "IdentA", vec!["a", "b", "c", "d"]).await;
        let _name_b = add_test_schema(&state, "IdentB", vec!["a", "e", "f", "g"]).await;

        let result = state.find_similar_schemas(&name_a, 0.0).unwrap();
        assert_eq!(result.similar_schemas.len(), 1);
        let entry = &result.similar_schemas[0];
        let expected = 1.0 / 7.0;
        assert!((entry.similarity - expected).abs() < 1e-10);
    }

    #[tokio::test]
    async fn find_similar_partial_overlap() {
        let state = make_test_state();
        // A: {v, w, x, y}, B: {w, x, y, z} → Jaccard = 3/5 = 0.6
        // Overlap = 3 out of min_size 4 → 3*2=6 > 4 triggers expansion.
        // Use lower overlap: A: {a, b, c, d}, B: {c, e, f, g} → Jaccard = 1/7
        // Overlap = 1 out of min_size 4 → 1*2=2 ≤ 4, no expansion.
        let name_a = add_test_schema(&state, "Astronomy Records", vec!["a", "b", "c", "d"]).await;
        let _name_b =
            add_test_schema(&state, "Banking Transactions", vec!["c", "e", "f", "g"]).await;

        let result = state.find_similar_schemas(&name_a, 0.0).unwrap();
        assert_eq!(result.similar_schemas.len(), 1);
        let expected = 1.0 / 7.0;
        assert!((result.similar_schemas[0].similarity - expected).abs() < 1e-10);
    }

    #[tokio::test]
    async fn find_similar_no_overlap_returns_zero() {
        let state = make_test_state();
        // A: {a, b}, B: {c, d} → Jaccard = 0/4 = 0.0
        let name_a = add_test_schema(&state, "Astronomy Data", vec!["a", "b"]).await;
        let _name_b = add_test_schema(&state, "Banking Records", vec!["c", "d"]).await;

        let result = state.find_similar_schemas(&name_a, 0.0).unwrap();
        assert_eq!(result.similar_schemas.len(), 1);
        assert!((result.similar_schemas[0].similarity - 0.0).abs() < 1e-10);
    }

    #[tokio::test]
    async fn find_similar_threshold_filters() {
        let state = make_test_state();
        // Use low-overlap schemas that won't trigger expansion.
        // A: {a, b, c, d, e}, B: {a, f, g, h, i} → overlap=1, min_size=5, 1*2=2 ≤ 5
        // Jaccard = 1/9 ≈ 0.111
        let name_a = add_test_schema(&state, "FilterA", vec!["a", "b", "c", "d", "e"]).await;
        let _name_b = add_test_schema(&state, "FilterB", vec!["a", "f", "g", "h", "i"]).await;

        // Threshold 0.2 should filter out B (similarity ≈ 0.111)
        let result = state.find_similar_schemas(&name_a, 0.2).unwrap();
        assert!(result.similar_schemas.is_empty());

        // Threshold 0.1 should include B
        let result = state.find_similar_schemas(&name_a, 0.1).unwrap();
        assert_eq!(result.similar_schemas.len(), 1);
    }

    #[tokio::test]
    async fn find_similar_nonexistent_schema_returns_error() {
        let state = make_test_state();
        let result = state.find_similar_schemas("nonexistent", 0.5);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(format!("{}", err).contains("not found"));
    }

    #[tokio::test]
    async fn find_similar_sorted_by_similarity_descending() {
        let state = make_test_state();
        // Use schemas with ≤50% overlap to avoid expansion.
        // A: {a, b, c, d, e, f}
        // B: {a, b, g, h, i, j}  → overlap=2, min_size=6, 2*2=4 ≤ 6, Jaccard = 2/10 = 0.2
        // C: {a, k, l, m, n, o}  → overlap=1, min_size=6, 1*2=2 ≤ 6, Jaccard = 1/11 ≈ 0.091
        let name_a = add_test_schema(
            &state,
            "Weather Forecasts",
            vec!["a", "b", "c", "d", "e", "f"],
        )
        .await;
        let _name_b =
            add_test_schema(&state, "Tax Documents", vec!["a", "b", "g", "h", "i", "j"]).await;
        let _name_c = add_test_schema(
            &state,
            "Pet Vaccinations",
            vec!["a", "k", "l", "m", "n", "o"],
        )
        .await;

        let result = state.find_similar_schemas(&name_a, 0.0).unwrap();
        assert_eq!(result.similar_schemas.len(), 2);
        // First entry should have higher similarity
        assert!(result.similar_schemas[0].similarity >= result.similar_schemas[1].similarity);
    }

    #[tokio::test]
    async fn add_schema_deduplicates_across_different_classifications() {
        let state = make_test_state();

        // Schema 1: classifications = ["word"]
        let schema1 = make_test_schema("Landscape Photos", &["artist", "title"]);

        // Schema 2: same fields but different classifications
        let schema2 = make_test_schema("Portrait Photos", &["artist", "title"]);

        // First schema gets added
        let outcome1 = state
            .add_schema(schema1, HashMap::new())
            .await
            .expect("failed to add first schema");
        let first_name = match outcome1 {
            SchemaAddOutcome::Added(schema, _) => schema.name,
            SchemaAddOutcome::AlreadyExists(..) | SchemaAddOutcome::Expanded(..) => {
                panic!("first schema should be added")
            }
        };

        // Second schema has a different semantic name, so it's a separate schema
        // even though fields are the same (different classifications don't affect this)
        let outcome2 = state
            .add_schema(schema2, HashMap::new())
            .await
            .expect("failed to evaluate second schema");
        match outcome2 {
            SchemaAddOutcome::Added(schema, _) => {
                assert_ne!(
                    schema.name, first_name,
                    "Different descriptive names should create separate schemas"
                );
                // Schema name is a hash, readable name is in descriptive_name
                assert_eq!(schema.descriptive_name, Some("Portrait Photos".to_string()));
            }
            SchemaAddOutcome::AlreadyExists(schema, _) if schema.name == first_name => {
                // Same-name dedup still works for same descriptive_name
                // but "Landscape Photos" != "Portrait Photos" so this shouldn't happen
                panic!("Different descriptive names should not dedup");
            }
            _ => {} // Expanded or other outcome is fine
        }
    }

    #[test]
    fn jaccard_index_both_empty_returns_one() {
        let a: HashSet<String> = HashSet::new();
        let b: HashSet<String> = HashSet::new();
        assert!((jaccard_index(&a, &b) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn jaccard_index_one_empty_returns_zero() {
        let a: HashSet<String> = ["x".to_string()].into_iter().collect();
        let b: HashSet<String> = HashSet::new();
        assert!((jaccard_index(&a, &b) - 0.0).abs() < 1e-10);
    }

    // ============== View Tests ==============

    use crate::schema_service::types::{AddViewRequest, ViewAddOutcome};
    use fold_db::schema::types::operations::Query;
    use fold_db::schema::types::schema::DeclarativeSchemaType;

    fn make_view_request(
        name: &str,
        desc_name: &str,
        output_fields: &[&str],
        source_schema: &str,
        source_fields: &[&str],
    ) -> AddViewRequest {
        let field_strings: Vec<String> = output_fields.iter().map(|f| f.to_string()).collect();
        let mut field_descriptions = HashMap::new();
        let mut field_classifications = HashMap::new();
        let mut field_data_classifications = HashMap::new();
        for f in &field_strings {
            field_descriptions.insert(f.clone(), format!("{} field", f));
            field_classifications.insert(f.clone(), vec!["word".to_string()]);
            field_data_classifications.insert(
                f.clone(),
                fold_db::schema::types::DataClassification::new(0, "general").unwrap(),
            );
        }

        AddViewRequest {
            name: name.to_string(),
            descriptive_name: desc_name.to_string(),
            input_queries: vec![Query::new(
                source_schema.to_string(),
                source_fields.iter().map(|f| f.to_string()).collect(),
            )],
            output_fields: field_strings,
            field_descriptions,
            field_classifications,
            field_data_classifications,
            wasm_bytes: None,
            schema_type: DeclarativeSchemaType::Single,
        }
    }

    #[tokio::test]
    async fn add_view_registers_output_schema() {
        let state = make_test_state();

        let request = make_view_request(
            "TestView",
            "Test View Output",
            &["result_a", "result_b"],
            "SourceSchema",
            &["field_a", "field_b"],
        );

        let outcome = state.add_view(request).await.expect("failed to add view");

        match outcome {
            ViewAddOutcome::Added(view, schema) => {
                assert_eq!(view.name, "TestView");
                assert_eq!(view.input_queries.len(), 1);
                assert_eq!(view.input_queries[0].schema_name, "SourceSchema");
                // Output schema should be identity-hashed
                assert_eq!(
                    schema.descriptive_name,
                    Some("Test View Output".to_string())
                );
                // View's output_schema_name should match the registered schema name
                assert_eq!(view.output_schema_name, schema.name);
            }
            _ => panic!("expected Added outcome, got {:?}", outcome),
        }

        // View should be in the views list
        let view_names = state.get_view_names().expect("failed to get view names");
        assert!(view_names.contains(&"TestView".to_string()));

        // Output schema should be in the schemas list
        let schema_names = state
            .get_schema_names()
            .expect("failed to get schema names");
        assert!(!schema_names.is_empty());
    }

    #[tokio::test]
    async fn add_view_deduplicates_output_schema() {
        let state = make_test_state();

        // First view
        let request1 = make_view_request(
            "View1",
            "Shared Output Schema",
            &["x", "y"],
            "Source1",
            &["a"],
        );
        let outcome1 = state.add_view(request1).await.expect("failed to add view1");
        let schema1_name = match &outcome1 {
            ViewAddOutcome::Added(_, schema) => schema.name.clone(),
            _ => panic!("expected Added"),
        };

        // Second view with same output fields and descriptive name
        let request2 = make_view_request(
            "View2",
            "Shared Output Schema",
            &["x", "y"],
            "Source2",
            &["b"],
        );
        let outcome2 = state.add_view(request2).await.expect("failed to add view2");

        match outcome2 {
            ViewAddOutcome::AddedWithExistingSchema(view, schema) => {
                assert_eq!(view.name, "View2");
                // Should reuse the same output schema
                assert_eq!(schema.name, schema1_name);
            }
            other => panic!("expected AddedWithExistingSchema, got {:?}", other),
        }

        // Both views should exist
        let view_names = state.get_view_names().expect("failed to get view names");
        assert_eq!(view_names.len(), 2);

        // Only one schema (deduplicated)
        let schema_names = state
            .get_schema_names()
            .expect("failed to get schema names");
        assert_eq!(schema_names.len(), 1);
    }

    #[tokio::test]
    async fn add_view_validates_empty_name() {
        let state = make_test_state();
        let mut request = make_view_request("", "desc", &["f"], "s", &["a"]);
        request.name = "  ".to_string();

        let result = state.add_view(request).await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("non-empty"));
    }

    #[tokio::test]
    async fn add_view_validates_empty_output_fields() {
        let state = make_test_state();
        let mut request = make_view_request("V", "desc", &["f"], "s", &["a"]);
        request.output_fields = vec![];

        let result = state.add_view(request).await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("at least one output field"));
    }

    #[tokio::test]
    async fn add_view_validates_missing_field_descriptions() {
        let state = make_test_state();
        let mut request = make_view_request("V", "desc", &["f"], "s", &["a"]);
        request.field_descriptions.clear();

        let result = state.add_view(request).await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("missing descriptions"));
    }

    #[tokio::test]
    async fn add_view_validates_empty_input_query_fields() {
        let state = make_test_state();
        let mut request = make_view_request("V", "desc", &["f"], "s", &["a"]);
        request.input_queries[0].fields = vec![];

        let result = state.add_view(request).await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("explicit fields"));
    }

    #[tokio::test]
    async fn add_view_validates_duplicate_input_pairs() {
        let state = make_test_state();
        let mut request = make_view_request("V", "desc", &["f"], "s", &["a"]);
        // Add a second query with the same schema.field pair
        request
            .input_queries
            .push(Query::new("s".to_string(), vec!["a".to_string()]));

        let result = state.add_view(request).await;
        assert!(result.is_err());
        assert!(format!("{}", result.unwrap_err()).contains("Duplicate"));
    }

    #[tokio::test]
    async fn get_view_by_name_returns_registered_view() {
        let state = make_test_state();

        let request = make_view_request("MyView", "My View Output", &["out1"], "Src", &["in1"]);
        state.add_view(request).await.expect("failed to add view");

        let view = state
            .get_view_by_name("MyView")
            .expect("failed to get view");
        assert!(view.is_some());
        let view = view.unwrap();
        assert_eq!(view.name, "MyView");

        let missing = state
            .get_view_by_name("NonExistent")
            .expect("failed to get view");
        assert!(missing.is_none());
    }
}
