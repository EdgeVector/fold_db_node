//! Integration tests for the `/api/trust-domains/*` HTTP handlers.
//!
//! Calls the framework-agnostic handler functions in `handlers::trust`
//! directly so the test exercises the same code path the routes use,
//! without spinning up actix.

use fold_db::access::AccessTier;
use fold_db::security::Ed25519KeyPair;
use fold_db_node::fold_node::{FoldNode, NodeConfig};
use fold_db_node::handlers::trust as trust_handlers;
use tempfile::TempDir;

async fn setup_node() -> (FoldNode, String, TempDir) {
    let temp_dir = TempDir::new().expect("tempdir");
    let path = temp_dir.path().to_path_buf();
    let config_dir = path.join("config");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    let keypair = Ed25519KeyPair::generate().unwrap();
    let pub_key = keypair.public_key_base64();
    let config = NodeConfig::new(path)
        .with_schema_service_url("test://mock")
        .with_identity(&pub_key, &keypair.secret_key_base64())
        .with_config_dir(config_dir);
    let node = FoldNode::new(config).await.unwrap();
    (node, pub_key, temp_dir)
}

#[tokio::test]
async fn add_trust_domain_grant_creates_domain_and_grant() {
    let (node, _self_key, _tmp) = setup_node().await;
    let user_hash = "test_user".to_string();

    let contact_key = Ed25519KeyPair::generate().unwrap().public_key_base64();
    let req = trust_handlers::TrustDomainAddRequest {
        public_key: contact_key.clone(),
        tier: AccessTier::Trusted,
    };

    trust_handlers::add_trust_domain_grant("financial", &req, &user_hash, &node)
        .await
        .expect("add succeeds");

    let resp = trust_handlers::list_trust_domains(&user_hash, &node)
        .await
        .expect("list succeeds");
    let body = resp.data.expect("response body present");

    let financial = body
        .domains
        .iter()
        .find(|d| d.domain == "financial")
        .expect("financial domain present");
    assert_eq!(financial.grants.len(), 1);
    assert_eq!(financial.grants[0].public_key, contact_key);
    assert_eq!(financial.grants[0].tier, AccessTier::Trusted);
}

#[tokio::test]
async fn remove_trust_domain_grant_revokes_only_target_key() {
    let (node, _self_key, _tmp) = setup_node().await;
    let user_hash = "test_user".to_string();

    let key_a = Ed25519KeyPair::generate().unwrap().public_key_base64();
    let key_b = Ed25519KeyPair::generate().unwrap().public_key_base64();

    for key in [&key_a, &key_b] {
        trust_handlers::add_trust_domain_grant(
            "medical",
            &trust_handlers::TrustDomainAddRequest {
                public_key: key.clone(),
                tier: AccessTier::Inner,
            },
            &user_hash,
            &node,
        )
        .await
        .expect("grant succeeds");
    }

    trust_handlers::remove_trust_domain_grant("medical", &key_a, &user_hash, &node)
        .await
        .expect("revoke succeeds");

    let resp = trust_handlers::list_trust_domains(&user_hash, &node)
        .await
        .expect("list succeeds");
    let body = resp.data.expect("response body present");
    let medical = body
        .domains
        .iter()
        .find(|d| d.domain == "medical")
        .expect("medical domain present");
    assert_eq!(medical.grants.len(), 1);
    assert_eq!(medical.grants[0].public_key, key_b);
}

#[tokio::test]
async fn add_trust_domain_grant_rejects_empty_inputs() {
    let (node, _self_key, _tmp) = setup_node().await;
    let user_hash = "test_user".to_string();
    let key = Ed25519KeyPair::generate().unwrap().public_key_base64();

    let empty_domain = trust_handlers::add_trust_domain_grant(
        "",
        &trust_handlers::TrustDomainAddRequest {
            public_key: key.clone(),
            tier: AccessTier::Trusted,
        },
        &user_hash,
        &node,
    )
    .await;
    assert!(empty_domain.is_err(), "empty domain must be rejected");

    let empty_key = trust_handlers::add_trust_domain_grant(
        "personal",
        &trust_handlers::TrustDomainAddRequest {
            public_key: "   ".to_string(),
            tier: AccessTier::Trusted,
        },
        &user_hash,
        &node,
    )
    .await;
    assert!(empty_key.is_err(), "empty public_key must be rejected");
}

#[tokio::test]
async fn list_trust_domains_includes_multiple_domains_with_independent_grants() {
    let (node, _self_key, _tmp) = setup_node().await;
    let user_hash = "test_user".to_string();

    let doctor = Ed25519KeyPair::generate().unwrap().public_key_base64();
    let accountant = Ed25519KeyPair::generate().unwrap().public_key_base64();

    trust_handlers::add_trust_domain_grant(
        "medical",
        &trust_handlers::TrustDomainAddRequest {
            public_key: doctor.clone(),
            tier: AccessTier::Inner,
        },
        &user_hash,
        &node,
    )
    .await
    .unwrap();
    trust_handlers::add_trust_domain_grant(
        "financial",
        &trust_handlers::TrustDomainAddRequest {
            public_key: accountant.clone(),
            tier: AccessTier::Trusted,
        },
        &user_hash,
        &node,
    )
    .await
    .unwrap();

    let resp = trust_handlers::list_trust_domains(&user_hash, &node)
        .await
        .unwrap();
    let body = resp.data.expect("response body present");

    let medical = body
        .domains
        .iter()
        .find(|d| d.domain == "medical")
        .expect("medical present");
    let financial = body
        .domains
        .iter()
        .find(|d| d.domain == "financial")
        .expect("financial present");

    assert_eq!(medical.grants.len(), 1);
    assert_eq!(medical.grants[0].public_key, doctor);
    assert_eq!(medical.grants[0].tier, AccessTier::Inner);
    assert_eq!(financial.grants.len(), 1);
    assert_eq!(financial.grants[0].public_key, accountant);
    assert_eq!(financial.grants[0].tier, AccessTier::Trusted);

    // Trust in financial does not leak into medical (or vice versa).
    assert!(medical.grants.iter().all(|g| g.public_key != accountant));
    assert!(financial.grants.iter().all(|g| g.public_key != doctor));
}
