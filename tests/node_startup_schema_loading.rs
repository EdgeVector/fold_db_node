use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use tempfile::TempDir;

/// Test to verify that FoldNode starts successfully when no schema service is configured
#[tokio::test(flavor = "multi_thread")]
async fn test_node_starts_without_schema_service() {
    // Create a temporary directory for this test
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let test_db_path = temp_dir.path().join("test_db");

    // Create node configuration WITHOUT schema service URL
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(test_db_path.to_path_buf())
        .with_network_listen_address("/ip4/127.0.0.1/tcp/9002")
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair));

    // Attempt to create the node - should succeed without schema service URL
    let result = FoldNode::new(config).await;

    // Verify that node creation succeeds
    assert!(
        result.is_ok(),
        "Node creation should succeed when schema_service_url is None"
    );

    println!("✅ Node correctly starts when schema service is not configured!");
}

/// Test to verify that FoldNode can start with a mock schema service for testing
#[tokio::test(flavor = "multi_thread")]
async fn test_node_new_loads_schemas_for_testing() {
    // Create a temporary directory for this test
    let temp_dir = TempDir::new().expect("Failed to create temp directory");
    let test_db_path = temp_dir.path().join("test_db");

    // Create node configuration with mock schema service URL
    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(test_db_path.to_path_buf())
        .with_network_listen_address("/ip4/127.0.0.1/tcp/9003")
        .with_schema_service_url("test://mock")
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair));

    // Create a new node using FoldNode::new() with mock service
    let node = FoldNode::new(config)
        .await
        .expect("Failed to create FoldNode with mock schema service");

    // Get the fold_db to verify it was created successfully
    let fold_db = node.get_fold_db().expect("Failed to get FoldDB");
    let schema_manager = fold_db.schema_manager();

    // FoldDB auto-registers the built-in TriggerFiring schema on every boot
    // (see fold_db_core::FoldDB::new). The mock schema service skips Phase 1
    // fingerprint registration, so TriggerFiring should be the only schema
    // present.
    let schemas = schema_manager.get_schemas().expect("Failed to get schemas");
    let user_schemas: Vec<_> = schemas
        .keys()
        .filter(|name| name.as_str() != fold_db::triggers::TRIGGER_FIRING_SCHEMA_NAME)
        .collect();
    assert_eq!(
        user_schemas.len(),
        0,
        "No user schemas should be auto-loaded with mock schema service"
    );

    println!("✅ FoldNode correctly starts with mock schema service for testing!");
}
