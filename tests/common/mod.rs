use fold_db::schema::types::{KeyConfig, KeyValue, Mutation, MutationType};
use fold_db_node::fold_node::config::NodeConfig;
use std::collections::HashMap;

pub mod schema_service;

#[allow(dead_code)]
pub fn create_test_node_config() -> NodeConfig {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_path_buf();

    // Isolate config directory so tests don't read the host machine's saved UI state
    // (e.g., if the user selected Ollama in the app, tests expecting Anthropic shouldn't fail)
    std::env::set_var("FOLD_CONFIG_DIR", path.join("config"));

    NodeConfig::new(path)
}

/// Generate a valid Ed25519 keypair for tests, returned as base64 strings
/// `(private_key_b64, public_key_b64)`. The unified identity path in
/// `FoldNode::new` feeds the private key seed through
/// `ed25519-compact::KeyPair::from_seed`, which panics on an all-zero seed,
/// so tests must use a real random keypair instead of a hardcoded fixture.
#[allow(dead_code)]
pub fn test_identity_b64() -> (String, String) {
    use base64::Engine;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key = signing_key.verifying_key();
    let b64 = base64::engine::general_purpose::STANDARD;
    (
        b64.encode(signing_key.to_bytes()),
        b64.encode(verifying_key.to_bytes()),
    )
}

#[allow(dead_code)]
pub fn create_test_mutation(
    schema_json: &serde_json::Value,
    mutation_json: serde_json::Value,
) -> Mutation {
    let key_config: KeyConfig = serde_json::from_value(schema_json["key"].clone())
        .expect("Failed to parse KeyConfig from schema");

    let schema_name = mutation_json["schema_name"]
        .as_str()
        .expect("Missing schema_name")
        .to_string();

    let pub_key = mutation_json["pub_key"]
        .as_str()
        .unwrap_or("default_key")
        .to_string();

    let fields_and_values: HashMap<String, serde_json::Value> =
        serde_json::from_value(mutation_json["fields_and_values"].clone())
            .expect("Failed to parse fields_and_values");

    let key_value = KeyValue::from_mutation(&fields_and_values, &key_config);

    // Allow overriding mutation type from JSON, default to Update
    let mutation_type = if let Some(type_str) = mutation_json["mutation_type"].as_str() {
        match type_str {
            "Create" => MutationType::Create,
            "Update" => MutationType::Update,
            "Delete" => MutationType::Delete,
            _ => MutationType::Update,
        }
    } else {
        MutationType::Update
    };

    let mut mutation = Mutation::new(
        schema_name,
        fields_and_values,
        key_value,
        pub_key,
        mutation_type,
    );

    if let Some(uuid) = mutation_json["uuid"].as_str() {
        mutation.uuid = uuid.to_string();
    }

    mutation
}
