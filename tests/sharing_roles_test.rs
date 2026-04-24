//! End-to-end integration tests for sharing roles, domain-aware trust,
//! and the sharing audit system.

use fold_db::access::types::FieldAccessPolicy;
use fold_db::access::AccessTier;
use fold_db::schema::types::declarative_schemas::DeclarativeSchemaDefinition;
use fold_db::schema::types::field::Field;
use fold_db::schema::types::key_config::KeyConfig;
use fold_db::schema::types::schema::DeclarativeSchemaType as SchemaType;
use fold_db::schema::SchemaState;
use fold_db::security::Ed25519KeyPair;
use fold_db_node::fold_node::{FoldNode, NodeConfig, OperationProcessor};
use fold_db_node::trust::contact_book::ContactBook;

use std::collections::HashMap;
use std::path::PathBuf;
use tempfile::TempDir;

/// Each test gets its own temp directory. The `config_dir` on `NodeConfig`
/// routes contact book / sharing roles to that directory — no global
/// `FOLDDB_HOME` env var needed, so tests can run in parallel safely.
async fn setup_node() -> (OperationProcessor, FoldNode, String, PathBuf, TempDir) {
    let temp_dir = TempDir::new().expect("tempdir");
    let path = temp_dir.path().to_path_buf();
    let config_dir = path.join("config");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    let keypair = Ed25519KeyPair::generate().unwrap();
    let pub_key = keypair.public_key_base64();
    let config = NodeConfig::new(path)
        .with_schema_service_url("test://mock")
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair))
        .with_config_dir(config_dir.clone());
    let node = FoldNode::new(config).await.unwrap();
    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));
    (processor, node, pub_key, config_dir, temp_dir)
}

async fn load_schema_with_policy(
    node: &FoldNode,
    name: &str,
    trust_domain: &str,
    min_read_tier: AccessTier,
) {
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
    schema.trust_domain = Some(trust_domain.to_string());
    schema.populate_runtime_fields().unwrap();

    // Set access policy on each field
    for (_field_name, field) in schema.runtime_fields.iter_mut() {
        field.common_mut().access_policy = Some(FieldAccessPolicy {
            trust_domain: trust_domain.to_string(),
            min_read_tier,
            min_write_tier: AccessTier::Owner,
            ..Default::default()
        });
    }

    let db = node.get_fold_db().unwrap();
    db.schema_manager()
        .load_schema_internal(schema)
        .await
        .unwrap();
    db.schema_manager()
        .set_schema_state(name, SchemaState::Approved)
        .await
        .unwrap();
}

#[tokio::test]
async fn test_role_assignment_grants_domain_trust() {
    let (op, node, _pub_key, _config_dir, _tmp) = setup_node().await;

    // Create a contact (simulating an accepted trust invite)
    let contact_key = Ed25519KeyPair::generate().unwrap().public_key_base64();

    // Grant initial trust in personal domain
    op.grant_trust(&contact_key, AccessTier::Trusted)
        .await
        .unwrap();

    // Add to contact book
    let db_ = node.get_fold_db().unwrap();
    let mut book = ContactBook::load(&db_).await.unwrap_or_default();
    book.upsert_contact(fold_db_node::trust::contact_book::Contact {
        public_key: contact_key.clone(),
        display_name: "Test Doctor".to_string(),
        contact_hint: None,
        direction: fold_db_node::trust::contact_book::TrustDirection::Outgoing,
        connected_at: chrono::Utc::now(),
        pseudonym: None,
        messaging_pseudonym: None,
        messaging_public_key: None,
        identity_pseudonym: None,
        revoked: false,
        roles: HashMap::new(),
    });
    book.save(&db_).await.unwrap();

    // Assign "doctor" role
    op.assign_role_to_contact(&contact_key, "doctor")
        .await
        .unwrap();

    // Verify: trust should be granted in medical domain
    let db = node.get_fold_db().unwrap();
    let medical_map = db
        .db_ops()
        .load_trust_map_for_domain("medical")
        .await
        .unwrap();
    let tier = medical_map.get(&contact_key);
    assert_eq!(
        tier,
        Some(&AccessTier::Inner),
        "Doctor role should grant Inner tier in medical domain"
    );

    // Verify: contact book should have the role recorded
    let db_ = node.get_fold_db().unwrap();
    let book = ContactBook::load(&db_).await.unwrap();
    let contact = book.get(&contact_key).unwrap();
    assert_eq!(
        contact.roles.get("medical").map(|s| s.as_str()),
        Some("doctor")
    );
}

#[tokio::test]
async fn test_role_removal_revokes_domain_trust() {
    let (op, node, _pub_key, _config_dir, _tmp) = setup_node().await;

    let contact_key = Ed25519KeyPair::generate().unwrap().public_key_base64();
    op.grant_trust(&contact_key, AccessTier::Trusted)
        .await
        .unwrap();

    let db_ = node.get_fold_db().unwrap();
    let mut book = ContactBook::load(&db_).await.unwrap_or_default();
    book.upsert_contact(fold_db_node::trust::contact_book::Contact {
        public_key: contact_key.clone(),
        display_name: "Trainer".to_string(),
        contact_hint: None,
        direction: fold_db_node::trust::contact_book::TrustDirection::Outgoing,
        connected_at: chrono::Utc::now(),
        pseudonym: None,
        messaging_pseudonym: None,
        messaging_public_key: None,
        identity_pseudonym: None,
        revoked: false,
        roles: HashMap::new(),
    });
    book.save(&db_).await.unwrap();

    // Assign then remove
    op.assign_role_to_contact(&contact_key, "trainer")
        .await
        .unwrap();
    op.remove_role_from_contact(&contact_key, "health")
        .await
        .unwrap();

    // Verify: trust should be revoked in health domain
    let db = node.get_fold_db().unwrap();
    let health_map = db
        .db_ops()
        .load_trust_map_for_domain("health")
        .await
        .unwrap();
    assert_eq!(
        health_map.get(&contact_key),
        None,
        "Trust should be revoked after role removal"
    );

    // Verify: role removed from contact book
    let db_ = node.get_fold_db().unwrap();
    let book = ContactBook::load(&db_).await.unwrap();
    let contact = book.get(&contact_key).unwrap();
    assert!(!contact.roles.contains_key("health"));
}

#[tokio::test]
async fn test_sharing_audit_with_domain_policies() {
    let (op, node, _pub_key, _config_dir, _tmp) = setup_node().await;

    // Create schemas with different domain policies
    load_schema_with_policy(&node, "PersonalNotes", "personal", AccessTier::Trusted).await;
    load_schema_with_policy(&node, "HealthLog", "health", AccessTier::Trusted).await;
    load_schema_with_policy(&node, "MedicalRecords", "medical", AccessTier::Inner).await;

    // Create a contact with friend role (personal domain, Trusted tier)
    let friend_key = Ed25519KeyPair::generate().unwrap().public_key_base64();
    op.grant_trust(&friend_key, AccessTier::Trusted)
        .await
        .unwrap();
    let db_ = node.get_fold_db().unwrap();
    let mut book = ContactBook::load(&db_).await.unwrap_or_default();
    book.upsert_contact(fold_db_node::trust::contact_book::Contact {
        public_key: friend_key.clone(),
        display_name: "Bob".to_string(),
        contact_hint: None,
        direction: fold_db_node::trust::contact_book::TrustDirection::Outgoing,
        connected_at: chrono::Utc::now(),
        pseudonym: None,
        messaging_pseudonym: None,
        messaging_public_key: None,
        identity_pseudonym: None,
        revoked: false,
        roles: HashMap::new(),
    });
    book.save(&db_).await.unwrap();

    // Assign friend role (personal domain, distance 3)
    op.assign_role_to_contact(&friend_key, "friend")
        .await
        .unwrap();

    // Audit: Bob should see PersonalNotes (personal/3) but not health or medical
    let audit = op.audit_contact_access(&friend_key).await.unwrap();
    assert_eq!(audit.contact_display_name, "Bob");

    let personal_schema = audit
        .accessible_schemas
        .iter()
        .find(|s| s.schema_name == "PersonalNotes");
    assert!(personal_schema.is_some(), "Friend should see PersonalNotes");
    assert_eq!(personal_schema.unwrap().readable_fields.len(), 3);

    let health_schema = audit
        .accessible_schemas
        .iter()
        .find(|s| s.schema_name == "HealthLog");
    assert!(
        health_schema.is_none(),
        "Friend should NOT see HealthLog (not in health domain)"
    );

    let medical_schema = audit
        .accessible_schemas
        .iter()
        .find(|s| s.schema_name == "MedicalRecords");
    assert!(
        medical_schema.is_none(),
        "Friend should NOT see MedicalRecords (not in medical domain)"
    );

    // Now also assign doctor role
    op.assign_role_to_contact(&friend_key, "doctor")
        .await
        .unwrap();

    // Audit again: Bob should now also see MedicalRecords
    let audit2 = op.audit_contact_access(&friend_key).await.unwrap();

    let medical_schema = audit2
        .accessible_schemas
        .iter()
        .find(|s| s.schema_name == "MedicalRecords");
    assert!(
        medical_schema.is_some(),
        "After doctor role, should see MedicalRecords"
    );

    // But still not HealthLog (doctor is medical domain, not health)
    let health_schema = audit2
        .accessible_schemas
        .iter()
        .find(|s| s.schema_name == "HealthLog");
    assert!(
        health_schema.is_none(),
        "Doctor role is medical, not health — HealthLog still hidden"
    );
}

#[tokio::test]
async fn test_multiple_roles_across_domains() {
    let (op, node, _pub_key, _config_dir, _tmp) = setup_node().await;

    load_schema_with_policy(&node, "Notes", "personal", AccessTier::Trusted).await;
    load_schema_with_policy(&node, "Fitness", "health", AccessTier::Trusted).await;
    load_schema_with_policy(&node, "Taxes", "financial", AccessTier::Inner).await;

    let contact_key = Ed25519KeyPair::generate().unwrap().public_key_base64();
    let db_ = node.get_fold_db().unwrap();
    let mut book = ContactBook::load(&db_).await.unwrap_or_default();
    book.upsert_contact(fold_db_node::trust::contact_book::Contact {
        public_key: contact_key.clone(),
        display_name: "Multi-Role".to_string(),
        contact_hint: None,
        direction: fold_db_node::trust::contact_book::TrustDirection::Outgoing,
        connected_at: chrono::Utc::now(),
        pseudonym: None,
        messaging_pseudonym: None,
        messaging_public_key: None,
        identity_pseudonym: None,
        revoked: false,
        roles: HashMap::new(),
    });
    book.save(&db_).await.unwrap();

    // Assign roles across multiple domains
    op.assign_role_to_contact(&contact_key, "friend")
        .await
        .unwrap();
    op.assign_role_to_contact(&contact_key, "trainer")
        .await
        .unwrap();
    op.assign_role_to_contact(&contact_key, "financial_advisor")
        .await
        .unwrap();

    // Audit: should see all three schemas
    let audit = op.audit_contact_access(&contact_key).await.unwrap();
    assert_eq!(audit.accessible_schemas.len(), 3);
    assert_eq!(audit.domain_tiers.len(), 3);
    assert!(audit.domain_tiers.contains_key("personal"));
    assert!(audit.domain_tiers.contains_key("health"));
    assert!(audit.domain_tiers.contains_key("financial"));
}

#[tokio::test]
async fn test_sharing_roles_config() {
    use fold_db::access::AccessTier;
    use fold_db_node::trust::sharing_roles::SharingRoleConfig;

    let config = SharingRoleConfig::default();

    // Verify default roles exist
    let doctor = config.get_role("doctor").unwrap();
    assert_eq!(doctor.domain, "medical");
    assert_eq!(doctor.tier, AccessTier::Inner);

    let friend = config.get_role("friend").unwrap();
    assert_eq!(friend.domain, "personal");
    assert_eq!(friend.tier, AccessTier::Trusted);

    // Verify roles_for_domain
    let personal_roles = config.roles_for_domain("personal");
    assert_eq!(personal_roles.len(), 3); // close_friend, friend, acquaintance
}
