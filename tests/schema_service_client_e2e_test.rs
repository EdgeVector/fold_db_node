//! End-to-end integration tests for the `schema_service_client` crate as
//! consumed by fold_db_node.
//!
//! Locks in the Phase 3 T2 migration (fold_db_node PR #630) by exercising
//! the client re-exported at `fold_db_node::fold_node::SchemaServiceClient`
//! against an in-process `SchemaServiceState` mounted under the real
//! production `/v1/*` route table (see
//! `schema_service_server_http::configure_routes`). Any wire-contract drift
//! between the client crate and the server crate fails here rather than at
//! runtime against the deployed Lambda.
//!
//! Existing tests in this suite either drive `SchemaServiceState` directly
//! (unit-level, no HTTP) or drive a whole `FoldNode` (integration-level,
//! schema-service traffic is incidental). This file fills the gap in the
//! middle: the thin HTTP client as the migration exposes it.

mod common;

use common::schema_service::spawn_schema_service;
use fold_db::schema::types::data_classification::DataClassification;
use fold_db::schema::types::{KeyConfig, Schema, SchemaType};
use fold_db_node::fold_node::SchemaServiceClient;
use schema_service_core::types::SchemaLookupEntry;
use std::collections::HashMap;

/// Build a minimally-valid schema with the fields the server requires
/// (descriptive_name, field_descriptions, field_data_classifications).
/// The name is set to the computed identity_hash, matching the convention
/// used throughout the existing schema-service test suite.
fn build_schema(descriptive_name: &str, fields: &[&str]) -> Schema {
    let field_names: Vec<String> = fields.iter().map(|f| f.to_string()).collect();
    let mut schema = Schema::new(
        String::new(),
        SchemaType::HashRange,
        Some(KeyConfig::new(Some("id".to_string()), None)),
        Some(field_names.clone()),
        None,
        None,
    );
    schema.descriptive_name = Some(descriptive_name.to_string());
    for f in &field_names {
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

#[actix_web::test]
async fn add_schema_creates_and_round_trips_via_get_schema() {
    let svc = spawn_schema_service().await;
    let client = SchemaServiceClient::new(&svc.url);

    let schema = build_schema("Client E2E Alpha", &["id", "body", "created_at"]);
    let response = client
        .add_schema(&schema, HashMap::new())
        .await
        .expect("add_schema via client should succeed");

    assert!(
        response.replaced_schema.is_none(),
        "first submit must not expand anything"
    );

    let stored_name = response.schema.name.clone();
    let fetched = client
        .get_schema(&stored_name)
        .await
        .expect("get_schema for newly-added schema should succeed");

    assert_eq!(fetched.schema.name, stored_name);
    let fields = fetched
        .schema
        .fields
        .as_ref()
        .expect("fetched schema has fields");
    for expected in ["id", "body", "created_at"] {
        assert!(
            fields.contains(&expected.to_string()),
            "field `{}` missing from round-tripped schema: {:?}",
            expected,
            fields
        );
    }
    assert_eq!(
        fetched.schema.descriptive_name.as_deref(),
        Some("Client E2E Alpha"),
        "descriptive_name must round-trip via /v1/schema/{{name}}"
    );
    assert!(
        !fetched.system,
        "user-added schemas must surface system=false"
    );
}

#[actix_web::test]
async fn add_schema_appears_in_list_schemas_and_available_schemas() {
    let svc = spawn_schema_service().await;
    let client = SchemaServiceClient::new(&svc.url);

    let before_names = client.list_schemas().await.expect("list_schemas pre-add");
    let before_defs = client
        .get_available_schemas()
        .await
        .expect("get_available_schemas pre-add");
    assert_eq!(
        before_names.len(),
        before_defs.len(),
        "names and defs endpoints should agree on count"
    );

    let schema = build_schema("Client E2E Listable", &["id", "value"]);
    let response = client.add_schema(&schema, HashMap::new()).await.unwrap();
    let stored_name = response.schema.name.clone();

    let after_names = client.list_schemas().await.expect("list_schemas post-add");
    assert_eq!(
        after_names.len(),
        before_names.len() + 1,
        "list_schemas should grow by exactly 1"
    );
    assert!(
        after_names.contains(&stored_name),
        "list_schemas missing the just-added schema: {:?}",
        after_names
    );

    let defs = client.get_available_schemas().await.unwrap();
    let found = defs
        .iter()
        .find(|s| s.schema.name == stored_name)
        .expect("available_schemas must include the just-added schema");
    let fields = found.schema.fields.as_ref().unwrap();
    assert!(fields.contains(&"id".to_string()));
    assert!(fields.contains(&"value".to_string()));
    assert!(
        !found.system,
        "user-added schemas must surface system=false in available_schemas"
    );
}

#[actix_web::test]
async fn duplicate_submission_via_client_is_deduplicated() {
    let svc = spawn_schema_service().await;
    let client = SchemaServiceClient::new(&svc.url);

    let schema = build_schema("Client E2E Idempotent", &["id", "content"]);
    let first = client.add_schema(&schema, HashMap::new()).await.unwrap();

    // Submit the exact same schema again; the service should return the
    // same identity-hashed schema name with no expansion.
    let second = client.add_schema(&schema, HashMap::new()).await.unwrap();

    assert!(
        second.replaced_schema.is_none(),
        "dedup path must not carry a replaced_schema"
    );
    assert_eq!(
        first.schema.name, second.schema.name,
        "dedup must return the same identity-hashed name both times"
    );

    let names = client.list_schemas().await.unwrap();
    let matching = names.iter().filter(|n| **n == first.schema.name).count();
    assert_eq!(
        matching, 1,
        "duplicate submission must not produce a second list entry"
    );
}

#[actix_web::test]
async fn expansion_submission_reports_replaced_schema() {
    let svc = spawn_schema_service().await;
    let client = SchemaServiceClient::new(&svc.url);

    let base = build_schema("Client E2E Expansion", &["id", "title"]);
    let base_response = client.add_schema(&base, HashMap::new()).await.unwrap();
    let base_name = base_response.schema.name.clone();
    assert!(base_response.replaced_schema.is_none());

    // Same descriptive_name, add one field → server should expand the
    // existing schema and report the old name in replaced_schema.
    let expanded = build_schema("Client E2E Expansion", &["id", "title", "summary"]);
    let expand_response = client.add_schema(&expanded, HashMap::new()).await.unwrap();

    assert_eq!(
        expand_response.replaced_schema.as_deref(),
        Some(base_name.as_str()),
        "expansion response must name the replaced schema"
    );
    let fields = expand_response
        .schema
        .fields
        .as_ref()
        .expect("expanded schema has fields");
    for expected in ["id", "title", "summary"] {
        assert!(
            fields.contains(&expected.to_string()),
            "expanded schema missing field `{}`: {:?}",
            expected,
            fields
        );
    }

    // The old schema stays fetchable — the node relies on fetching it by
    // name while applying field_mappers during expansion (see
    // `schema_expansion_fresh_db_test`). The contract we lock in here is
    // only that the expanded response carries the correct old name and
    // that the new schema is distinct from the old one.
    assert_ne!(
        expand_response.schema.name, base_name,
        "expansion must produce a new identity-hashed schema distinct from the old one"
    );
    let old = client
        .get_schema(&base_name)
        .await
        .expect("old schema should still be fetchable by the node during expansion apply");
    assert_eq!(old.schema.name, base_name);
}

#[actix_web::test]
async fn batch_check_schema_reuse_matches_only_existing_descriptive_names() {
    let svc = spawn_schema_service().await;
    let client = SchemaServiceClient::new(&svc.url);

    let schema = build_schema("Client E2E Reuse Target", &["id", "label"]);
    client.add_schema(&schema, HashMap::new()).await.unwrap();

    let probes = vec![
        SchemaLookupEntry {
            descriptive_name: "Client E2E Reuse Target".to_string(),
            fields: vec!["id".to_string(), "label".to_string()],
        },
        SchemaLookupEntry {
            descriptive_name: "ZZZ Unrelated Descriptor ZZZ".to_string(),
            fields: vec!["id".to_string()],
        },
    ];

    let response = client.batch_check_schema_reuse(&probes).await.unwrap();

    let hit = response
        .matches
        .get("Client E2E Reuse Target")
        .expect("existing descriptive_name should produce a match");
    assert_eq!(hit.matched_descriptive_name, "Client E2E Reuse Target");
    assert!(
        !response
            .matches
            .contains_key("ZZZ Unrelated Descriptor ZZZ"),
        "unmatched descriptive_name must be absent from the response map, got {:?}",
        response.matches.keys().collect::<Vec<_>>()
    );
}

#[actix_web::test]
async fn client_against_unreachable_url_surfaces_error() {
    // Port 1 on loopback is not listening — the client should exhaust its
    // retry budget and return an error instead of hanging.
    let client = SchemaServiceClient::new("http://127.0.0.1:1");

    let result = client.list_schemas().await;
    assert!(
        result.is_err(),
        "list_schemas against an unreachable server must return an error"
    );
}

#[actix_web::test]
async fn get_schema_missing_returns_permanent_error() {
    let svc = spawn_schema_service().await;
    let client = SchemaServiceClient::new(&svc.url);

    let result = client.get_schema("schema_that_does_not_exist_12345").await;
    assert!(
        result.is_err(),
        "get_schema for an unknown name must return an error, got {:?}",
        result.ok().map(|s| s.schema.name)
    );
}
