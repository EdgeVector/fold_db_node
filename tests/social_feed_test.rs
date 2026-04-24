use fold_db::atom::MoleculeRange;
use fold_db::db_operations::MoleculeData;
use fold_db::schema::types::field::{Field, FieldVariant};
use fold_db::schema::types::key_value::KeyValue;
use fold_db::schema::types::mutation::Mutation;
use fold_db::security::Ed25519KeyPair;
use fold_db::MutationType;
use fold_db_node::fold_node::config::NodeConfig;
use fold_db_node::fold_node::FoldNode;
use fold_db_node::fold_node::OperationProcessor;
use fold_db_node::handlers::feed::{FeedRequest, FeedResponse};
use serde_json::json;
use std::collections::HashMap;
use tempfile::TempDir;

/// Helper: create a FoldNode with a temp directory and mock schema service.
async fn setup_node() -> (FoldNode, TempDir) {
    let temp_dir = TempDir::new().expect("Failed to create temp dir");
    let temp_db_path = temp_dir.path().to_str().unwrap();

    let keypair = fold_db::security::Ed25519KeyPair::generate().unwrap();
    let config = NodeConfig::new(temp_db_path.into())
        .with_schema_service_url("test://mock")
        .with_seed_identity(fold_db_node::identity::identity_from_keypair(&keypair));
    let node = FoldNode::new(config)
        .await
        .expect("Failed to create FoldNode");

    (node, temp_dir)
}

/// Helper: load the Photo schema into the node's database with public access policies.
async fn load_photo_schema(node: &FoldNode) {
    use fold_db::access::types::{AccessTier, FieldAccessPolicy};

    let schema_path = std::env::current_dir()
        .expect("Failed to get current directory")
        .join("tests/schemas_for_testing")
        .join("Photo.json");

    let fold_db = node.get_fold_db().expect("Failed to get FoldDB");
    fold_db
        .load_schema_from_file(&schema_path)
        .await
        .expect("Failed to load Photo schema");

    // Set all fields to public-read so feed tests work
    let mut schema = fold_db
        .schema_manager()
        .get_schema("Photo")
        .await
        .expect("get schema")
        .expect("Photo schema");

    let public_policy = FieldAccessPolicy {
        min_read_tier: AccessTier::Public,
        min_write_tier: AccessTier::Owner,
        ..Default::default()
    };

    let field_names: Vec<String> = schema.runtime_fields.keys().cloned().collect();
    for name in &field_names {
        if let Some(field) = schema.runtime_fields.get_mut(name) {
            field.common_mut().access_policy = Some(public_policy.clone());
        }
    }

    fold_db
        .schema_manager()
        .update_schema(&schema)
        .await
        .expect("Failed to set public policies on Photo schema");
}

/// Helper: insert a photo record. Writes via the normal mutation path
/// (which signs with the node's own signer). Use [`set_authors`] after all
/// inserts to re-sign per-key AtomEntries with per-friend keypairs.
async fn insert_photo(
    processor: &OperationProcessor,
    timestamp: &str,
    photo_url: &str,
    caption: &str,
    author_name: &str,
) {
    let mut fields = HashMap::new();
    fields.insert("photo_url".to_string(), json!(photo_url));
    fields.insert("caption".to_string(), json!(caption));
    fields.insert("author_name".to_string(), json!(author_name));
    fields.insert("timestamp".to_string(), json!(timestamp));

    let mutation = Mutation::new(
        "Photo".to_string(),
        fields,
        KeyValue::new(None, Some(timestamp.to_string())),
        String::new(),
        MutationType::Create,
    );

    processor
        .execute_mutation_op(mutation)
        .await
        .expect("Failed to insert photo");
}

/// After all [`insert_photo`] calls, rewrite each field's molecule so that
/// the AtomEntry at each `range_key` is re-signed with the associated
/// keypair. This simulates the post-sync state where AtomEntries from
/// other nodes carry their original writer_pubkey, which is the only way
/// to get a non-node-owner writer_pubkey in a single-node test (the
/// mutation path always signs with the node's own signer).
///
/// Run this exactly once per test *after* every `insert_photo`, because
/// the mutation path persists the in-memory cached schema after every
/// mutation — running it mid-way would be clobbered by the next insert.
async fn set_authors(node: &FoldNode, schema_name: &str, entries: &[(&str, &Ed25519KeyPair)]) {
    let db = node.get_fold_db().expect("get_fold_db");
    let mut schema = db
        .schema_manager()
        .get_schema(schema_name)
        .await
        .expect("get_schema")
        .expect("schema not found");

    let mut modified: Vec<(String, MoleculeData)> = Vec::new();
    for field in schema.runtime_fields.values_mut() {
        field.refresh_from_db(db.db_ops()).await;

        let FieldVariant::Range(rf) = field else {
            panic!("set_authors only supports Range fields");
        };
        let mut molecule: MoleculeRange = rf.base.molecule.clone().expect("molecule not loaded");

        for (range_key, author_kp) in entries {
            let existing_atom_uuid = molecule
                .get_atom_entry(range_key)
                .unwrap_or_else(|| panic!("atom entry missing for range_key={range_key}"))
                .atom_uuid
                .clone();
            molecule.set_atom_uuid((*range_key).to_string(), existing_atom_uuid, author_kp);
        }

        modified.push((molecule.uuid().to_string(), MoleculeData::Range(molecule)));
    }

    db.db_ops()
        .atoms()
        .batch_store_molecules(modified, None)
        .await
        .expect("batch_store_molecules");
}

/// Extract the FeedResponse data from handler result.
fn unwrap_feed(
    result: Result<
        fold_db_node::handlers::response::ApiResponse<FeedResponse>,
        fold_db_node::handlers::response::HandlerError,
    >,
) -> FeedResponse {
    let api_response = result.expect("Feed handler returned error");
    api_response.data.expect("Feed response missing data")
}

#[tokio::test(flavor = "multi_thread")]
async fn test_basic_feed_returns_friends_photos_sorted_desc() {
    let (node, _tmp) = setup_node().await;
    load_photo_schema(&node).await;
    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));

    let friend_a = Ed25519KeyPair::generate().unwrap();
    let friend_b = Ed25519KeyPair::generate().unwrap();

    insert_photo(
        &processor,
        "2026-03-01T10:00:00Z",
        "https://example.com/a1.jpg",
        "Morning view",
        "Alice",
    )
    .await;
    insert_photo(
        &processor,
        "2026-03-02T15:00:00Z",
        "https://example.com/b1.jpg",
        "Sunset photo",
        "Bob",
    )
    .await;
    insert_photo(
        &processor,
        "2026-03-03T08:00:00Z",
        "https://example.com/a2.jpg",
        "Breakfast",
        "Alice",
    )
    .await;

    set_authors(
        &node,
        "Photo",
        &[
            ("2026-03-01T10:00:00Z", &friend_a),
            ("2026-03-02T15:00:00Z", &friend_b),
            ("2026-03-03T08:00:00Z", &friend_a),
        ],
    )
    .await;

    let friend_a_pub = friend_a.public_key_base64();
    let friend_b_pub = friend_b.public_key_base64();

    let request = FeedRequest {
        schema_name: Some("Photo".to_string()),
        friend_hashes: vec![friend_a_pub.clone(), friend_b_pub.clone()],
        limit: None,
    };

    let feed =
        unwrap_feed(fold_db_node::handlers::feed::get_feed(request, "test_user", &node).await);

    assert_eq!(feed.total, 3, "Should return all 3 photos from friends");
    assert_eq!(feed.items.len(), 3);

    let timestamps: Vec<&str> = feed
        .items
        .iter()
        .map(|item| item["timestamp"].as_str().unwrap())
        .collect();
    assert_eq!(
        timestamps,
        vec![
            "2026-03-03T08:00:00Z",
            "2026-03-02T15:00:00Z",
            "2026-03-01T10:00:00Z"
        ],
        "Items should be sorted newest first"
    );

    assert_eq!(feed.items[0]["author"].as_str().unwrap(), friend_a_pub);
    assert_eq!(feed.items[1]["author"].as_str().unwrap(), friend_b_pub);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_feed_filters_out_non_friends() {
    let (node, _tmp) = setup_node().await;
    load_photo_schema(&node).await;
    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));

    let friend_a = Ed25519KeyPair::generate().unwrap();
    let friend_b = Ed25519KeyPair::generate().unwrap();
    let stranger = Ed25519KeyPair::generate().unwrap();

    insert_photo(
        &processor,
        "2026-03-01T10:00:00Z",
        "https://example.com/a1.jpg",
        "Friend photo",
        "Alice",
    )
    .await;
    insert_photo(
        &processor,
        "2026-03-02T12:00:00Z",
        "https://example.com/s1.jpg",
        "Stranger photo",
        "Eve",
    )
    .await;
    insert_photo(
        &processor,
        "2026-03-03T14:00:00Z",
        "https://example.com/b1.jpg",
        "Another friend",
        "Bob",
    )
    .await;

    set_authors(
        &node,
        "Photo",
        &[
            ("2026-03-01T10:00:00Z", &friend_a),
            ("2026-03-02T12:00:00Z", &stranger),
            ("2026-03-03T14:00:00Z", &friend_b),
        ],
    )
    .await;

    let stranger_pub = stranger.public_key_base64();

    let request = FeedRequest {
        schema_name: Some("Photo".to_string()),
        friend_hashes: vec![friend_a.public_key_base64(), friend_b.public_key_base64()],
        limit: None,
    };

    let feed =
        unwrap_feed(fold_db_node::handlers::feed::get_feed(request, "test_user", &node).await);

    assert_eq!(feed.total, 2, "Should exclude stranger's photo");

    let authors: Vec<&str> = feed
        .items
        .iter()
        .map(|item| item["author"].as_str().unwrap())
        .collect();
    assert!(
        !authors.contains(&stranger_pub.as_str()),
        "Stranger should not appear in feed"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_empty_friends_returns_empty_feed() {
    let (node, _tmp) = setup_node().await;
    load_photo_schema(&node).await;
    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));

    insert_photo(
        &processor,
        "2026-03-01T10:00:00Z",
        "https://example.com/x.jpg",
        "Some photo",
        "Someone",
    )
    .await;

    let request = FeedRequest {
        schema_name: Some("Photo".to_string()),
        friend_hashes: vec![],
        limit: None,
    };

    let feed =
        unwrap_feed(fold_db_node::handlers::feed::get_feed(request, "test_user", &node).await);

    assert_eq!(feed.total, 0);
    assert!(feed.items.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_feed_respects_limit() {
    let (node, _tmp) = setup_node().await;
    load_photo_schema(&node).await;
    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));

    let friend_a = Ed25519KeyPair::generate().unwrap();

    let timestamps: Vec<String> = (1..=5)
        .map(|i| format!("2026-03-{:02}T10:00:00Z", i))
        .collect();
    for (i, ts) in timestamps.iter().enumerate() {
        insert_photo(
            &processor,
            ts,
            &format!("https://example.com/photo{}.jpg", i + 1),
            &format!("Photo {}", i + 1),
            "Alice",
        )
        .await;
    }

    let entries: Vec<(&str, &Ed25519KeyPair)> = timestamps
        .iter()
        .map(|ts| (ts.as_str(), &friend_a))
        .collect();
    set_authors(&node, "Photo", &entries).await;

    let request = FeedRequest {
        schema_name: Some("Photo".to_string()),
        friend_hashes: vec![friend_a.public_key_base64()],
        limit: Some(2),
    };

    let feed =
        unwrap_feed(fold_db_node::handlers::feed::get_feed(request, "test_user", &node).await);

    assert_eq!(feed.total, 5, "Total should reflect all matching items");
    assert_eq!(
        feed.items.len(),
        2,
        "Should return only 2 items due to limit"
    );

    assert_eq!(
        feed.items[0]["timestamp"].as_str().unwrap(),
        "2026-03-05T10:00:00Z"
    );
    assert_eq!(
        feed.items[1]["timestamp"].as_str().unwrap(),
        "2026-03-04T10:00:00Z"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_feed_strips_non_public_fields() {
    let (node, _tmp) = setup_node().await;
    load_photo_schema(&node).await;

    // Set one field's access policy to owner-only
    {
        use fold_db::access::types::FieldAccessPolicy;
        use fold_db::access::AccessTier;

        let db = node.get_fold_db().expect("Failed to get FoldDB");
        let mut schema = db
            .schema_manager()
            .get_schema("Photo")
            .await
            .expect("Failed to get Photo schema")
            .expect("Photo schema not found");

        if let Some(field) = schema.runtime_fields.get_mut("caption") {
            field.common_mut().access_policy = Some(FieldAccessPolicy {
                min_read_tier: AccessTier::Owner,
                min_write_tier: AccessTier::Owner,
                ..Default::default()
            });
        }

        db.schema_manager()
            .update_schema(&schema)
            .await
            .expect("Failed to update schema with access policy");
    }

    let processor = OperationProcessor::new(std::sync::Arc::new(node.clone()));

    let friend_a = Ed25519KeyPair::generate().unwrap();
    insert_photo(
        &processor,
        "2026-03-01T10:00:00Z",
        "https://example.com/a1.jpg",
        "Secret caption",
        "Alice",
    )
    .await;
    set_authors(&node, "Photo", &[("2026-03-01T10:00:00Z", &friend_a)]).await;

    let request = FeedRequest {
        schema_name: Some("Photo".to_string()),
        friend_hashes: vec![friend_a.public_key_base64()],
        limit: None,
    };

    let feed =
        unwrap_feed(fold_db_node::handlers::feed::get_feed(request, "test_user", &node).await);

    assert_eq!(feed.total, 1);
    let fields = feed.items[0]["fields"].as_object().unwrap();

    assert!(
        !fields.contains_key("caption"),
        "Owner-only field 'caption' should be stripped from feed"
    );

    assert!(
        fields.contains_key("photo_url"),
        "Public field 'photo_url' should be present"
    );
    assert!(
        fields.contains_key("author_name"),
        "Public field 'author_name' should be present"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_feed_nonexistent_schema_returns_empty() {
    let (node, _tmp) = setup_node().await;

    let request = FeedRequest {
        schema_name: Some("NonExistent".to_string()),
        friend_hashes: vec!["friend_a".to_string()],
        limit: None,
    };

    let result = fold_db_node::handlers::feed::get_feed(request, "test_user", &node).await;

    let response = result.expect("Should succeed with empty results");
    let data = response.data.expect("Should have data");
    assert_eq!(data.items.len(), 0);
    assert_eq!(data.total, 0);
}
