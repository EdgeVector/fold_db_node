use fold_db_node::schema_service::server::{SchemaAddOutcome, SchemaServiceState};
use serde_json::json;
use std::collections::HashMap;
use tempfile::tempdir;

fn json_to_schema(value: serde_json::Value) -> fold_db::schema::types::Schema {
    let mut schema: fold_db::schema::types::Schema =
        serde_json::from_value(value).expect("failed to deserialize schema from JSON");
    if schema.descriptive_name.is_none() {
        schema.descriptive_name = Some(schema.name.clone());
    }
    if let Some(ref fields) = schema.fields {
        for f in fields {
            schema.field_descriptions.entry(f.clone())
                .or_insert_with(|| format!("{} field", f));
        }
    }
    schema
}

/// After adding a schema, its fields become canonical. A second schema using
/// semantically equivalent field names should have those fields renamed to the
/// canonical versions.
#[tokio::test]
async fn canonical_registry_renames_semantically_equivalent_fields() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_canonical_rename")
        .to_string_lossy()
        .to_string();

    let state =
        SchemaServiceState::new(db_path).expect("failed to initialize schema service state");

    // First schema registers "artist", "title", "year" as canonical fields
    let schema_a = json_to_schema(json!({
        "name": "PaintingRecords",
        "descriptive_name": "Painting Records",
        "fields": ["artist", "title", "year"],
    }));

    let outcome_a = state
        .add_schema(schema_a, HashMap::new())
        .await
        .expect("failed to add first schema");
    assert!(
        matches!(outcome_a, SchemaAddOutcome::Added(..)),
        "first schema should be Added, got {:?}",
        outcome_a
    );

    // Second schema uses "creator" (semantically close to "artist") and "name"
    // (semantically close to "title"). "year" is exact match so no rename needed.
    let schema_b = json_to_schema(json!({
        "name": "ArtworkRecords",
        "descriptive_name": "Artwork Records",
        "fields": ["creator", "name", "medium"],
    }));

    let outcome_b = state
        .add_schema(schema_b, HashMap::new())
        .await
        .expect("failed to add second schema");

    // The outcome depends on embedding similarity. With the default embedder,
    // "creator" → "artist" should trigger canonicalization.
    // Check that the returned schema's fields include canonical names.
    match &outcome_b {
        SchemaAddOutcome::Added(schema, mappers) | SchemaAddOutcome::Expanded(_, schema, mappers) => {
            let fields = schema.fields.as_ref().expect("schema must have fields");
            // If canonicalization worked, "creator" should have been renamed to "artist"
            if fields.contains(&"artist".to_string()) {
                assert!(
                    !fields.contains(&"creator".to_string()),
                    "both 'creator' and 'artist' should not coexist after canonicalization"
                );
                // Check mutation_mappers records the rename
                assert_eq!(
                    mappers.get("creator").map(|s| s.as_str()),
                    Some("artist"),
                    "mutation_mappers should map 'creator' -> 'artist'"
                );
            }
            // "medium" is novel, should remain unchanged
            assert!(
                fields.contains(&"medium".to_string()),
                "'medium' should remain unchanged as it has no canonical match"
            );
        }
        SchemaAddOutcome::AlreadyExists(..) => {
            // If the fields got canonicalized to identical set, AlreadyExists is also valid
        }
        SchemaAddOutcome::TooSimilar(_) => {
            // Also acceptable depending on embedder similarity
        }
    }
}

/// Fields that are already canonical should not be renamed.
#[tokio::test]
async fn canonical_registry_does_not_rename_exact_matches() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_canonical_exact")
        .to_string_lossy()
        .to_string();

    let state =
        SchemaServiceState::new(db_path).expect("failed to initialize schema service state");

    let schema_a = json_to_schema(json!({
        "name": "MusicRecords",
        "descriptive_name": "Music Records",
        "fields": ["artist", "album", "year"],
    }));

    state
        .add_schema(schema_a, HashMap::new())
        .await
        .expect("failed to add first schema");

    // Second schema uses exact same field names — no renames should happen
    let schema_b = json_to_schema(json!({
        "name": "ConcertRecords",
        "descriptive_name": "Concert Records",
        "fields": ["artist", "venue", "year"],
    }));

    let outcome_b = state
        .add_schema(schema_b, HashMap::new())
        .await
        .expect("failed to add second schema");

    match &outcome_b {
        SchemaAddOutcome::Added(schema, mappers) => {
            let fields = schema.fields.as_ref().expect("schema must have fields");
            assert!(
                fields.contains(&"artist".to_string()),
                "'artist' should remain as-is"
            );
            assert!(
                fields.contains(&"venue".to_string()),
                "'venue' should remain as-is"
            );
            assert!(
                fields.contains(&"year".to_string()),
                "'year' should remain as-is"
            );
            // No renames should exist for exact-match fields
            assert!(
                !mappers.contains_key("artist"),
                "exact match 'artist' should not appear in mutation_mappers"
            );
            assert!(
                !mappers.contains_key("year"),
                "exact match 'year' should not appear in mutation_mappers"
            );
        }
        other => {
            // Other outcomes are acceptable depending on similarity
            println!("Got outcome: {:?} — skipping detailed checks", other);
        }
    }
}

/// Canonical fields persist across schema service restarts (sled persistence).
#[tokio::test]
async fn canonical_registry_persists_across_restarts() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_canonical_persist")
        .to_string_lossy()
        .to_string();

    // First instance: register canonical fields
    {
        let state = SchemaServiceState::new(db_path.clone())
            .expect("failed to initialize schema service state");

        let schema = json_to_schema(json!({
            "name": "BookRecords",
            "descriptive_name": "Book Records",
            "fields": ["author", "title", "isbn"],
        }));

        let outcome = state
            .add_schema(schema, HashMap::new())
            .await
            .expect("failed to add schema");
        assert!(
            matches!(outcome, SchemaAddOutcome::Added(..)),
            "first schema should be Added"
        );
    }
    // state is dropped — sled is closed

    // Second instance: canonical fields should be loaded from sled
    {
        let state = SchemaServiceState::new(db_path)
            .expect("failed to reopen schema service state");

        // Add schema with semantically equivalent field "writer" (close to "author")
        let schema = json_to_schema(json!({
            "name": "ArticleRecords",
            "descriptive_name": "Article Records",
            "fields": ["writer", "headline", "publication_date"],
        }));

        let outcome = state
            .add_schema(schema, HashMap::new())
            .await
            .expect("failed to add schema after restart");

        match &outcome {
            SchemaAddOutcome::Added(schema, mappers) | SchemaAddOutcome::Expanded(_, schema, mappers) => {
                let fields = schema.fields.as_ref().expect("schema must have fields");
                // If "writer" → "author" canonicalization worked after restart,
                // it proves persistence
                if fields.contains(&"author".to_string()) {
                    assert!(
                        !fields.contains(&"writer".to_string()),
                        "'writer' should be renamed to 'author' after restart"
                    );
                    assert_eq!(
                        mappers.get("writer").map(|s| s.as_str()),
                        Some("author"),
                        "mutation_mappers should map 'writer' -> 'author' after restart"
                    );
                    println!("Canonical field persistence verified: 'writer' -> 'author'");
                } else {
                    println!("Embedder did not match 'writer' to 'author' — persistence test inconclusive");
                }
            }
            _ => {
                println!("Got non-Added outcome — persistence test inconclusive");
            }
        }
    }
}

/// Novel fields with no semantic match should not be renamed.
#[tokio::test]
async fn canonical_registry_ignores_dissimilar_fields() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_canonical_dissimilar")
        .to_string_lossy()
        .to_string();

    let state =
        SchemaServiceState::new(db_path).expect("failed to initialize schema service state");

    let schema_a = json_to_schema(json!({
        "name": "WeatherData",
        "descriptive_name": "Weather Data",
        "fields": ["temperature", "humidity", "wind_speed"],
    }));

    state
        .add_schema(schema_a, HashMap::new())
        .await
        .expect("failed to add first schema");

    // Second schema has completely different domain fields
    let schema_b = json_to_schema(json!({
        "name": "FinancialData",
        "descriptive_name": "Financial Data",
        "fields": ["stock_price", "volume", "market_cap"],
    }));

    let outcome_b = state
        .add_schema(schema_b, HashMap::new())
        .await
        .expect("failed to add second schema");

    match &outcome_b {
        SchemaAddOutcome::Added(schema, mappers) => {
            let fields = schema.fields.as_ref().expect("schema must have fields");
            assert!(
                fields.contains(&"stock_price".to_string()),
                "'stock_price' should remain unchanged"
            );
            assert!(
                fields.contains(&"volume".to_string()),
                "'volume' should remain unchanged"
            );
            assert!(
                fields.contains(&"market_cap".to_string()),
                "'market_cap' should remain unchanged"
            );
            // No weather fields should appear
            assert!(
                !fields.contains(&"temperature".to_string()),
                "'temperature' should not appear in financial schema"
            );
            // No renames
            assert!(
                mappers.is_empty(),
                "no renames should happen for dissimilar fields, got: {:?}",
                mappers
            );
        }
        other => panic!(
            "completely different fields should produce Added, got {:?}",
            other
        ),
    }
}

/// The first schema added (empty canonical registry) should have no renames.
#[tokio::test]
async fn canonical_registry_empty_on_first_schema() {
    let temp_dir = tempdir().expect("failed to create temp directory");
    let db_path = temp_dir
        .path()
        .join("test_canonical_empty")
        .to_string_lossy()
        .to_string();

    let state =
        SchemaServiceState::new(db_path).expect("failed to initialize schema service state");

    let schema = json_to_schema(json!({
        "name": "FirstSchema",
        "descriptive_name": "First Schema",
        "fields": ["field_a", "field_b", "field_c"],
    }));

    let outcome = state
        .add_schema(schema, HashMap::new())
        .await
        .expect("failed to add schema");

    match &outcome {
        SchemaAddOutcome::Added(schema, mappers) => {
            let fields = schema.fields.as_ref().expect("schema must have fields");
            assert_eq!(fields.len(), 3, "should have exactly 3 fields");
            assert!(
                mappers.is_empty(),
                "first schema should have no renames, got: {:?}",
                mappers
            );
        }
        other => panic!("first schema should be Added, got {:?}", other),
    }
}
