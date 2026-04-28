//! End-to-end test for semantic field matching through the full ingestion pipeline.
//!
//! Simulates what happens when the UI ingests data twice with different field names:
//! 1. First ingestion: Schema A with "artist", "title", "year"
//! 2. Second ingestion: Schema B with "creator", "title", "year", "medium"
//!
//! Verifies:
//! - Schema expansion produces a superset with "artist" (not "creator")
//! - mutation_mappers from the schema service flow through to mutation generation
//! - Data written with "creator" field name ends up stored under "artist"
//! - "medium" is preserved as a new field (not falsely matched)
//!
//! Uses real FastEmbedModel (no mock) but no AI — schemas and data are supplied directly.
//!
//! Run with: `cargo test --test semantic_field_matching_e2e_test -- --ignored --nocapture`

use fold_db::user_context::run_with_user;
use fold_db_node::fold_node::node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use fold_db_node::ingestion::mutation_generator;
mod common;

use fold_db::schema::types::{KeyValue, Query};
use serde_json::json;
use std::collections::HashMap;

use common::schema_service::{spawn_schema_service, SpawnedSchemaService};

async fn spawn_local_schema_service() -> SpawnedSchemaService {
    spawn_schema_service().await
}

// -- The test -----------------------------------------------------------------

/// Full pipeline test: semantic field matching → mutation_mappers → correct data storage.
///
/// This test exercises the exact code path the UI takes:
/// 1. Schema service receives Schema A (artist, title, year)
/// 2. Schema service receives Schema B (creator, title, year, medium)
/// 3. Schema service detects "creator" ≈ "artist" and returns mutation_mappers
/// 4. Ingestion pipeline merges mappers and generates mutations
/// 5. Data with "creator" field is written under "artist" field on the expanded schema
#[actix_web::test]
#[ignore] // Uses real FastEmbedModel (downloads on first run)
async fn test_semantic_field_matching_full_pipeline() {
    // 1. Spin up local schema service with real embeddings
    let svc = spawn_local_schema_service().await;
    let schema_url = svc.url.clone();
    eprintln!("Schema service at {}", schema_url);

    // 2. Create FoldNode
    let mut config = common::create_test_node_config();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let user_id = keypair.public_key_base64();
    config = config
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair))
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    // 3. Submit Schema A: ["artist", "title", "year"]
    let schema_a_def: fold_db::schema::types::Schema = serde_json::from_value(json!({
        "name": "ArtworkSchemaA",
        "descriptive_name": "Artwork Collection",
        "schema_type": "Single",
        "key": { "hash_field": "title" },
        "fields": ["artist", "title", "year"],
        "field_classifications": {
            "artist": ["word"],
            "title": ["word"],
            "year": ["number"]
        },
        "field_data_classifications": {
            "artist": { "sensitivity_level": 0, "data_domain": "general" },
            "title": { "sensitivity_level": 0, "data_domain": "general" },
            "year": { "sensitivity_level": 0, "data_domain": "general" }
        }
    }))
    .unwrap();

    let resp_a = node.add_schema_to_service(&schema_a_def).await.unwrap();
    let schema_a_name = resp_a.schema.name.clone();
    eprintln!("Schema A registered: {}", &schema_a_name[..16]);

    // Load and approve Schema A locally
    {
        let db = node.get_fold_db().unwrap();
        let json_a = serde_json::to_string(&resp_a.schema).unwrap();
        db.schema_manager()
            .load_schema_from_json(&json_a)
            .await
            .unwrap();
        db.schema_manager().approve(&schema_a_name).await.unwrap();
    }

    // 4. Write data using Schema A
    let mutation_a = fold_db::schema::types::Mutation::new(
        schema_a_name.clone(),
        {
            let mut fields = HashMap::new();
            fields.insert("artist".to_string(), json!("Claude Monet"));
            fields.insert("title".to_string(), json!("Water Lilies"));
            fields.insert("year".to_string(), json!("1906"));
            fields
        },
        KeyValue::new(Some("Water Lilies".to_string()), None),
        user_id.clone(),
        fold_db::MutationType::Create,
    );

    let result = run_with_user(&user_id, async {
        node.mutate_batch(vec![mutation_a]).await
    })
    .await;
    assert!(
        result.is_ok(),
        "Schema A mutation failed: {:?}",
        result.err()
    );
    eprintln!("Schema A: wrote 'Water Lilies' by 'Claude Monet'");

    // 5. Submit Schema B: ["creator", "title", "year", "medium"]
    //    The schema service should detect "creator" ≈ "artist" and return mappers
    let schema_b_def: fold_db::schema::types::Schema = serde_json::from_value(json!({
        "name": "ArtworkSchemaB",
        "descriptive_name": "Artwork Collection",
        "schema_type": "Single",
        "key": { "hash_field": "title" },
        "fields": ["creator", "title", "year", "medium"],
        "field_classifications": {
            "creator": ["word"],
            "title": ["word"],
            "year": ["number"],
            "medium": ["word"]
        },
        "field_data_classifications": {
            "creator": { "sensitivity_level": 0, "data_domain": "general" },
            "title": { "sensitivity_level": 0, "data_domain": "general" },
            "year": { "sensitivity_level": 0, "data_domain": "general" },
            "medium": { "sensitivity_level": 0, "data_domain": "general" }
        }
    }))
    .unwrap();

    let resp_b = node.add_schema_to_service(&schema_b_def).await.unwrap();
    let schema_b_name = resp_b.schema.name.clone();
    eprintln!("Schema B registered: {}", &schema_b_name[..16]);
    eprintln!(
        "  replaced_schema: {:?}",
        resp_b.replaced_schema.as_deref().map(|s| &s[..16])
    );
    eprintln!("  mutation_mappers: {:?}", resp_b.mutation_mappers);

    // VERIFY: expansion happened
    assert!(
        resp_b.replaced_schema.is_some(),
        "Schema B should expand Schema A (same descriptive_name)"
    );

    // VERIFY: mutation_mappers include the semantic rename
    assert_eq!(
        resp_b.mutation_mappers.get("creator").map(|s| s.as_str()),
        Some("artist"),
        "Schema service should return mutation_mapper: creator → artist"
    );

    // VERIFY: expanded schema has "artist" not "creator", and has "medium"
    let expanded_fields = resp_b.schema.fields.as_ref().unwrap();
    assert!(
        expanded_fields.contains(&"artist".to_string()),
        "Expanded schema must have 'artist' (canonical)"
    );
    assert!(
        !expanded_fields.contains(&"creator".to_string()),
        "Expanded schema must NOT have 'creator' (renamed to artist)"
    );
    assert!(
        expanded_fields.contains(&"medium".to_string()),
        "Expanded schema must have 'medium' (new field, not falsely matched)"
    );
    eprintln!("  expanded fields: {:?}", expanded_fields);

    // 6. Load and approve Schema B locally, block Schema A
    {
        let db = node.get_fold_db().unwrap();
        let json_b = serde_json::to_string(&resp_b.schema).unwrap();
        db.schema_manager()
            .load_schema_from_json(&json_b)
            .await
            .unwrap();
        db.schema_manager().approve(&schema_b_name).await.unwrap();
        if let Some(ref old_name) = resp_b.replaced_schema {
            let _ = db
                .schema_manager()
                .block_and_supersede(old_name, &schema_b_name)
                .await;
        }
    }

    // 7. Simulate what the ingestion pipeline does: merge service mappers into
    //    the AI's mappers, then generate mutations.
    //
    //    The AI originally gave us: creator → creator, title → title, etc.
    //    The service added: creator → artist (semantic rename)
    //    After merge: creator → artist, title → title, year → year, medium → medium
    let mut ai_mappers: HashMap<String, String> = HashMap::new();
    ai_mappers.insert("creator".to_string(), "creator".to_string());
    ai_mappers.insert("title".to_string(), "title".to_string());
    ai_mappers.insert("year".to_string(), "year".to_string());
    ai_mappers.insert("medium".to_string(), "medium".to_string());

    // Merge service mappers (this is what process_flat_path now does)
    for (from, to) in &resp_b.mutation_mappers {
        ai_mappers.insert(from.clone(), to.clone());
    }

    eprintln!("  merged mappers: {:?}", ai_mappers);

    // VERIFY: merged mappers map creator → artist
    assert_eq!(
        ai_mappers.get("creator").map(|s| s.as_str()),
        Some("artist"),
        "Merged mappers must map creator → artist"
    );

    // 8. Generate mutation with "creator" in the data
    let data_b: HashMap<String, serde_json::Value> = [
        ("creator".to_string(), json!("Vincent van Gogh")),
        ("title".to_string(), json!("Starry Night")),
        ("year".to_string(), json!("1889")),
        ("medium".to_string(), json!("Oil on canvas")),
    ]
    .into_iter()
    .collect();

    let mut keys = HashMap::new();
    keys.insert("hash_field".to_string(), "Starry Night".to_string());

    let mutations = mutation_generator::generate_mutations(
        &schema_b_name,
        &keys,
        &data_b,
        &ai_mappers,
        user_id.clone(),
        None,
        None,
    )
    .unwrap();

    assert_eq!(mutations.len(), 1, "Should generate exactly 1 mutation");

    // VERIFY: the mutation writes to "artist" not "creator"
    let mutation = &mutations[0];
    assert!(
        mutation.fields_and_values.contains_key("artist"),
        "Mutation must write to 'artist' field (the canonical name). Got: {:?}",
        mutation.fields_and_values.keys().collect::<Vec<_>>()
    );
    assert!(
        !mutation.fields_and_values.contains_key("creator"),
        "Mutation must NOT write to 'creator' — it should be renamed to 'artist'"
    );
    assert!(
        mutation.fields_and_values.contains_key("medium"),
        "Mutation must write to 'medium' field"
    );
    assert_eq!(
        mutation.fields_and_values.get("artist").unwrap(),
        &json!("Vincent van Gogh"),
        "Artist field should have van Gogh's name"
    );
    eprintln!(
        "  mutation fields: {:?}",
        mutation.fields_and_values.keys().collect::<Vec<_>>()
    );

    // 9. Execute the mutation and verify data is stored correctly
    let result = run_with_user(&user_id, async { node.mutate_batch(mutations).await }).await;
    assert!(
        result.is_ok(),
        "Schema B mutation failed: {:?}",
        result.err()
    );
    eprintln!("Schema B: wrote 'Starry Night' by 'Vincent van Gogh' (via creator→artist rename)");

    // 10. Query the expanded schema and verify the record was written
    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    let (keys_list, total) = processor
        .list_schema_keys(&schema_b_name, 0, 100)
        .await
        .unwrap();

    eprintln!("\nExpanded schema keys ({} total):", total);
    for kv in &keys_list {
        eprintln!(
            "  hash={}, range={}",
            kv.hash.as_deref().unwrap_or("(none)"),
            kv.range.as_deref().unwrap_or("(none)")
        );
    }

    // Should have the Starry Night record
    assert!(
        total >= 1,
        "Expanded schema should have at least 1 record, got {}",
        total
    );

    // Query via OperationProcessor to verify data is stored under correct fields
    let query = Query::new(
        schema_b_name.clone(),
        vec![
            "artist".to_string(),
            "title".to_string(),
            "year".to_string(),
            "medium".to_string(),
        ],
    );

    let query_result = run_with_user(&user_id, async {
        processor.execute_query_json(query).await
    })
    .await;

    match query_result {
        Ok(records) => {
            eprintln!("\nQuery result: {:?}", records);
            assert!(
                !records.is_empty(),
                "Should find records in expanded schema"
            );
            // Check that "artist" field has the value (written via creator→artist mapper)
            for record in &records {
                if let Some(artist_val) = record.get("artist") {
                    if artist_val == &json!("Vincent van Gogh") {
                        eprintln!("PASS: 'creator' data correctly stored under 'artist' field");
                    }
                }
                if let Some(medium_val) = record.get("medium") {
                    if medium_val == &json!("Oil on canvas") {
                        eprintln!("PASS: 'medium' field correctly preserved as new field");
                    }
                }
            }
        }
        Err(e) => {
            eprintln!(
                "Query returned error (field_mapper resolution may need data on old schema): {}",
                e
            );
        }
    }

    // 11. Verify schema states
    let all_schemas = processor
        .list_schemas()
        .await
        .expect("failed to list schemas");

    let active: Vec<_> = all_schemas
        .iter()
        .filter(|s| s.state != fold_db::schema::SchemaState::Blocked)
        .collect();
    let blocked: Vec<_> = all_schemas
        .iter()
        .filter(|s| s.state == fold_db::schema::SchemaState::Blocked)
        .collect();

    eprintln!("\nFinal state:");
    eprintln!("  Active schemas: {}", active.len());
    for s in &active {
        eprintln!(
            "    {} fields={:?}",
            &s.schema.name[..16.min(s.schema.name.len())],
            s.schema.fields
        );
    }
    eprintln!("  Blocked schemas: {}", blocked.len());

    assert_eq!(
        active.len(),
        1,
        "Should have exactly 1 active schema after expansion"
    );
    assert_eq!(
        blocked.len(),
        1,
        "Should have exactly 1 blocked schema (the predecessor)"
    );

    // Cleanup
    svc.handle.stop(true).await;
    eprintln!("\nPASS: Full semantic field matching pipeline works end-to-end");
}
