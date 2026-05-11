//! Integration test for the per-process schema registration cache on
//! `FoldNode::add_schema_to_service`.
//!
//! The dogfood Apple-Notes import surfaces the duplicate-add waste this cache
//! eliminates: 132 notes → 14 batches → 14 identical "Apple Notes" add_schema
//! calls, only the first of which does real work. This test pins the
//! short-circuit: after the first call, stopping the schema service must not
//! break the second call.

use fold_db::schema::types::data_classification::DataClassification;
use fold_db::schema::types::{KeyConfig, Schema, SchemaType};
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::node::FoldNode;
use std::collections::HashMap;
use tempfile::TempDir;

mod common;
use common::schema_service::spawn_schema_service;

fn build_schema(fields: Vec<&str>, descriptive_name: &str, hash_field: &str) -> Schema {
    let field_names: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
    let mut field_classifications = HashMap::new();
    for f in &field_names {
        field_classifications.insert(f.clone(), vec!["word".to_string()]);
    }
    let mut schema = Schema::new(
        String::new(),
        SchemaType::Single,
        Some(KeyConfig::new(Some(hash_field.to_string()), None)),
        Some(field_names),
        None,
        None,
    );
    schema.descriptive_name = Some(descriptive_name.to_string());
    schema.field_classifications = field_classifications;
    for f in schema.fields.clone().unwrap_or_default() {
        schema
            .field_descriptions
            .insert(f.clone(), format!("{} field", f));
        schema
            .field_data_classifications
            .insert(f.clone(), DataClassification::new(0, "general").unwrap());
    }
    schema.compute_identity_hash();
    schema.name = schema.get_identity_hash().unwrap().clone();
    schema
}

/// Submitting the same schema twice must only round-trip to the schema
/// service once. We prove "only once" by stopping the server after the
/// first call: if the cache works, the second call still succeeds.
#[actix_web::test]
async fn duplicate_add_schema_hits_cache_no_round_trip() {
    let svc = spawn_schema_service().await;
    let schema_url = svc.url.clone();

    let temp_dir = TempDir::new().unwrap();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(temp_dir.path().to_path_buf())
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair))
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    let schema = build_schema(vec!["title", "body"], "Test Notes", "title");

    let first = node
        .add_schema_to_service(&schema)
        .await
        .expect("first add_schema must succeed against live service");
    let canonical_name = first.schema.name.clone();

    // Stop the schema service. Any real HTTP round-trip after this must fail.
    svc.handle.stop(true).await;

    let second = node
        .add_schema_to_service(&schema)
        .await
        .expect("second add_schema must be served from cache");

    assert_eq!(
        second.schema.name, canonical_name,
        "cached response must return the same canonical schema name"
    );
    assert!(
        second.replaced_schema.is_none(),
        "cached response must not carry a stale replaced_schema marker"
    );
}

/// A schema whose fields change is not the same schema. The identity_hash
/// changes, so the cache key changes, and the second call must round-trip
/// (and therefore fail when the service is stopped).
#[actix_web::test]
async fn changed_fields_bypass_cache() {
    let svc = spawn_schema_service().await;
    let schema_url = svc.url.clone();

    let temp_dir = TempDir::new().unwrap();
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(temp_dir.path().to_path_buf())
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair))
        .with_schema_service_url(&schema_url);
    let node = FoldNode::new(config).await.unwrap();

    let schema_v1 = build_schema(vec!["title", "body"], "Versioned Notes", "title");
    node.add_schema_to_service(&schema_v1).await.unwrap();

    svc.handle.stop(true).await;

    let schema_v2 = build_schema(vec!["title", "body", "tags"], "Versioned Notes", "title");
    let result = node.add_schema_to_service(&schema_v2).await;
    assert!(
        result.is_err(),
        "different fields must produce a different cache key and force a real round-trip"
    );
}
