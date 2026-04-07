//! Comprehensive end-to-end org lifecycle integration test.
//!
//! Exercises: org creation → invite generation → member join → schema loading →
//! data mutations with org context → data isolation → member removal → org purge.
//!
//! Tests at the OperationProcessor + org_ops level (same code path as HTTP
//! handlers, bypassing Exemem cloud calls which need real credentials).

use fold_db::access::AccessContext;
use fold_db::org::operations as org_ops;
use fold_db::schema::types::key_config::KeyConfig;
use fold_db::schema::types::operations::{MutationType, Query};
use fold_db::schema::types::schema::DeclarativeSchemaType as SchemaType;
use fold_db::schema::types::{
    declarative_schemas::DeclarativeSchemaDefinition, KeyValue, Mutation,
};
use fold_db::schema::SchemaState;
use fold_db::security::Ed25519KeyPair;
use fold_db_node::fold_node::{FoldNode, NodeConfig, OperationProcessor};
use serde_json::json;
use std::collections::HashMap;
use tempfile::tempdir;

/// Helper: set up a FoldNode + OperationProcessor in a temp directory.
async fn setup_node() -> (OperationProcessor, FoldNode, String) {
    let temp_dir = tempdir().unwrap();
    let keypair = Ed25519KeyPair::generate().unwrap();
    let pub_key = keypair.public_key_base64();
    let config = NodeConfig::new(temp_dir.into_path())
        .with_schema_service_url("test://mock")
        .with_identity(&pub_key, &keypair.secret_key_base64());
    let node = FoldNode::new(config).await.unwrap();
    let processor = OperationProcessor::new(node.clone());
    (processor, node, pub_key)
}

/// Helper: load and approve a schema with optional org_hash.
async fn load_schema(node: &FoldNode, name: &str, org_hash: Option<&str>) {
    let mut schema = DeclarativeSchemaDefinition::new(
        name.to_string(),
        SchemaType::HashRange,
        Some(KeyConfig {
            hash_field: Some("title".to_string()),
            range_field: Some("date".to_string()),
        }),
        Some(vec![
            "title".to_string(),
            "body".to_string(),
            "date".to_string(),
        ]),
        None,
        None,
    );
    schema
        .field_classifications
        .insert("title".to_string(), vec!["word".to_string()]);
    schema
        .field_classifications
        .insert("body".to_string(), vec!["word".to_string()]);
    schema
        .field_classifications
        .insert("date".to_string(), vec!["date".to_string()]);
    if let Some(hash) = org_hash {
        schema.org_hash = Some(hash.to_string());
    }
    schema.populate_runtime_fields().unwrap();

    let db = node.get_fold_db().await.unwrap();
    db.schema_manager
        .load_schema_internal(schema)
        .await
        .unwrap();
    db.schema_manager
        .set_schema_state(name, SchemaState::Approved)
        .await
        .unwrap();
}

/// Helper: execute a mutation via OperationProcessor.
async fn write_record(
    op: &OperationProcessor,
    schema: &str,
    title: &str,
    date: &str,
    body: &str,
    pub_key: &str,
) {
    let mut fields = HashMap::new();
    fields.insert("title".to_string(), json!(title));
    fields.insert("body".to_string(), json!(body));
    fields.insert("date".to_string(), json!(date));

    op.execute_mutation_op(Mutation::new(
        schema.to_string(),
        fields,
        KeyValue::new(Some(title.to_string()), Some(date.to_string())),
        pub_key.to_string(),
        MutationType::Create,
    ))
    .await
    .unwrap();
}

/// Helper: query all records from a schema.
async fn query_all(op: &OperationProcessor, schema: &str) -> Vec<serde_json::Value> {
    let query = Query::new(
        schema.to_string(),
        vec!["title".to_string(), "body".to_string(), "date".to_string()],
    );
    op.execute_query_json(query).await.unwrap()
}

// ===== Tests =====

#[tokio::test]
async fn test_full_org_lifecycle() {
    let (op, node, admin_key) = setup_node().await;

    // 1. Create org
    let sled_db = node
        .get_fold_db()
        .await
        .unwrap()
        .sled_db()
        .cloned()
        .unwrap();
    let membership = org_ops::create_org(&sled_db, "Test Org", &admin_key, "admin").unwrap();
    let org_hash = membership.org_hash.clone();

    assert_eq!(membership.org_name, "Test Org");
    assert!(membership
        .members
        .iter()
        .any(|m| m.node_public_key == admin_key));

    // 2. Generate invite
    let invite = org_ops::generate_invite(&sled_db, &org_hash).unwrap();
    assert_eq!(invite.org_name, "Test Org");
    assert_eq!(invite.org_hash, org_hash);

    // 3. List orgs
    let orgs = org_ops::list_orgs(&sled_db).unwrap();
    assert_eq!(orgs.len(), 1);
    assert_eq!(orgs[0].org_name, "Test Org");

    // 4. Load org-scoped schema
    load_schema(&node, "OrgNotes", Some(&org_hash)).await;

    // 5. Write data to org schema
    write_record(
        &op,
        "OrgNotes",
        "Meeting",
        "2026-04-07",
        "Discussed roadmap",
        &admin_key,
    )
    .await;
    write_record(
        &op,
        "OrgNotes",
        "Standup",
        "2026-04-08",
        "Quick sync",
        &admin_key,
    )
    .await;

    // 6. Verify data is queryable
    let results = query_all(&op, "OrgNotes").await;
    assert_eq!(results.len(), 2, "Should have 2 org records");

    // 7. Verify data is accessible with owner context
    let db = node.get_fold_db().await.unwrap();
    let access = AccessContext::owner(&admin_key);
    let query = Query::new(
        "OrgNotes".to_string(),
        vec!["title".to_string(), "body".to_string()],
    );
    let access_results = db
        .query_executor
        .query_with_access(query, &access, None)
        .await
        .unwrap();
    assert_eq!(access_results.len(), 2, "Owner should see both records");
}

#[tokio::test]
async fn test_org_and_personal_data_isolation() {
    let (op, node, pub_key) = setup_node().await;

    // Create org
    let sled_db = node
        .get_fold_db()
        .await
        .unwrap()
        .sled_db()
        .cloned()
        .unwrap();
    let membership = org_ops::create_org(&sled_db, "Work Org", &pub_key, "admin").unwrap();
    let org_hash = membership.org_hash.clone();

    // Load personal schema (no org_hash)
    load_schema(&node, "PersonalNotes", None).await;
    // Load org schema (with org_hash)
    load_schema(&node, "WorkNotes", Some(&org_hash)).await;

    // Write personal data
    write_record(
        &op,
        "PersonalNotes",
        "Diary",
        "2026-04-07",
        "Personal stuff",
        &pub_key,
    )
    .await;

    // Write org data
    write_record(
        &op,
        "WorkNotes",
        "Sprint",
        "2026-04-07",
        "Work stuff",
        &pub_key,
    )
    .await;

    // Query personal — should only see personal data
    let personal = query_all(&op, "PersonalNotes").await;
    assert_eq!(personal.len(), 1);
    assert_eq!(personal[0]["fields"]["title"], json!("Diary"));

    // Query org — should only see org data
    let org = query_all(&op, "WorkNotes").await;
    assert_eq!(org.len(), 1);
    assert_eq!(org[0]["fields"]["title"], json!("Sprint"));
}

#[tokio::test]
async fn test_org_member_add_and_invite_generation() {
    let (_, node, admin_key) = setup_node().await;

    let sled_db = node
        .get_fold_db()
        .await
        .unwrap()
        .sled_db()
        .cloned()
        .unwrap();
    let membership = org_ops::create_org(&sled_db, "Team", &admin_key, "admin").unwrap();
    let org_hash = membership.org_hash.clone();

    // Generate a second keypair for the new member
    let member_keypair = Ed25519KeyPair::generate().unwrap();
    let member_key = member_keypair.public_key_base64();

    // Add member
    let member_info = fold_db::org::OrgMemberInfo {
        node_public_key: member_key.clone(),
        display_name: "Bob".to_string(),
        added_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        added_by: admin_key.clone(),
    };
    org_ops::add_member(&sled_db, &org_hash, member_info).unwrap();

    // Verify member is listed
    let org = org_ops::get_org(&sled_db, &org_hash).unwrap().unwrap();
    assert_eq!(org.members.len(), 2);
    assert!(org.members.iter().any(|m| m.node_public_key == member_key));
    assert!(org.members.iter().any(|m| m.display_name == "Bob"));

    // Generate invite bundle — should contain both members
    let invite = org_ops::generate_invite(&sled_db, &org_hash).unwrap();
    assert_eq!(invite.members.len(), 2);
}

#[tokio::test]
async fn test_org_invite_join_flow() {
    // Node A creates org
    let (_, node_a, key_a) = setup_node().await;
    let sled_a = node_a
        .get_fold_db()
        .await
        .unwrap()
        .sled_db()
        .cloned()
        .unwrap();
    let membership_a = org_ops::create_org(&sled_a, "SharedOrg", &key_a, "Alice").unwrap();
    let org_hash = membership_a.org_hash.clone();

    // Generate invite
    let invite = org_ops::generate_invite(&sled_a, &org_hash).unwrap();

    // Node B joins using the invite
    let (_, node_b, key_b) = setup_node().await;
    let sled_b = node_b
        .get_fold_db()
        .await
        .unwrap()
        .sled_db()
        .cloned()
        .unwrap();
    let membership_b = org_ops::join_org(&sled_b, &invite, &key_b, "Bob").unwrap();

    assert_eq!(membership_b.org_name, "SharedOrg");
    assert_eq!(membership_b.org_hash, org_hash);

    // Node B should see itself in the member list
    assert!(membership_b
        .members
        .iter()
        .any(|m| m.node_public_key == key_b));

    // Both nodes should list the org
    let orgs_a = org_ops::list_orgs(&sled_a).unwrap();
    let orgs_b = org_ops::list_orgs(&sled_b).unwrap();
    assert_eq!(orgs_a.len(), 1);
    assert_eq!(orgs_b.len(), 1);
}

#[tokio::test]
async fn test_org_member_removal() {
    let (_, node, admin_key) = setup_node().await;
    let sled_db = node
        .get_fold_db()
        .await
        .unwrap()
        .sled_db()
        .cloned()
        .unwrap();
    let membership = org_ops::create_org(&sled_db, "RemoveTest", &admin_key, "admin").unwrap();
    let org_hash = membership.org_hash.clone();

    // Add a member
    let member_keypair = Ed25519KeyPair::generate().unwrap();
    let member_key = member_keypair.public_key_base64();
    let member_info = fold_db::org::OrgMemberInfo {
        node_public_key: member_key.clone(),
        display_name: "Charlie".to_string(),
        added_at: 0,
        added_by: admin_key.clone(),
    };
    org_ops::add_member(&sled_db, &org_hash, member_info).unwrap();

    // Verify 2 members
    let org = org_ops::get_org(&sled_db, &org_hash).unwrap().unwrap();
    assert_eq!(org.members.len(), 2);

    // Remove the member
    org_ops::remove_member(&sled_db, &org_hash, &member_key).unwrap();

    // Verify 1 member remaining
    let org = org_ops::get_org(&sled_db, &org_hash).unwrap().unwrap();
    assert_eq!(org.members.len(), 1);
    assert!(!org.members.iter().any(|m| m.node_public_key == member_key));
}

#[tokio::test]
async fn test_org_delete() {
    let (_, node, admin_key) = setup_node().await;
    let sled_db = node
        .get_fold_db()
        .await
        .unwrap()
        .sled_db()
        .cloned()
        .unwrap();
    let membership = org_ops::create_org(&sled_db, "DeleteTest", &admin_key, "admin").unwrap();
    let org_hash = membership.org_hash.clone();

    // Verify org exists
    assert!(org_ops::get_org(&sled_db, &org_hash).unwrap().is_some());

    // Delete org
    org_ops::delete_org(&sled_db, &org_hash).unwrap();

    // Verify org is gone
    assert!(org_ops::get_org(&sled_db, &org_hash).unwrap().is_none());
    assert_eq!(org_ops::list_orgs(&sled_db).unwrap().len(), 0);
}

#[tokio::test]
async fn test_multiple_orgs_coexist() {
    let (op, node, pub_key) = setup_node().await;
    let sled_db = node
        .get_fold_db()
        .await
        .unwrap()
        .sled_db()
        .cloned()
        .unwrap();

    // Create two orgs
    let org1 = org_ops::create_org(&sled_db, "Org Alpha", &pub_key, "admin").unwrap();
    let org2 = org_ops::create_org(&sled_db, "Org Beta", &pub_key, "admin").unwrap();

    // Load schemas for each
    load_schema(&node, "AlphaDocs", Some(&org1.org_hash)).await;
    load_schema(&node, "BetaDocs", Some(&org2.org_hash)).await;

    // Write data to each
    write_record(
        &op,
        "AlphaDocs",
        "Alpha1",
        "2026-01-01",
        "Alpha data",
        &pub_key,
    )
    .await;
    write_record(
        &op,
        "BetaDocs",
        "Beta1",
        "2026-01-01",
        "Beta data",
        &pub_key,
    )
    .await;

    // Query each — verify isolation
    let alpha = query_all(&op, "AlphaDocs").await;
    let beta = query_all(&op, "BetaDocs").await;

    assert_eq!(alpha.len(), 1);
    assert_eq!(beta.len(), 1);
    assert_eq!(alpha[0]["fields"]["body"], json!("Alpha data"));
    assert_eq!(beta[0]["fields"]["body"], json!("Beta data"));

    // List orgs — should have 2
    let orgs = org_ops::list_orgs(&sled_db).unwrap();
    assert_eq!(orgs.len(), 2);
}
