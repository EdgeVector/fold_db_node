//! Tests for discovery-trust integration: ConnectionPayload serialization
//! and Contact::from_discovery() creation.

use fold_db_node::discovery::connection::{ConnectionPayload, IdentityCardPayload};
use fold_db_node::trust::contact_book::{Contact, TrustDirection};

#[test]
fn test_connection_payload_with_identity_card() {
    let payload = ConnectionPayload {
        message_type: "accept".to_string(),
        message: "hello".to_string(),
        sender_public_key: "pk1".to_string(),
        sender_pseudonym: "ps1".to_string(),
        reply_public_key: "rpk1".to_string(),
        identity_card: Some(IdentityCardPayload {
            display_name: "Alice".to_string(),
            contact_hint: Some("alice@example.com".to_string()),
            node_public_key: "node_pk_alice".to_string(),
        }),
        preferred_role: None,
    };
    let json = serde_json::to_string(&payload).unwrap();
    let deserialized: ConnectionPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.message_type, "accept");
    assert_eq!(deserialized.message, "hello");
    assert_eq!(deserialized.sender_public_key, "pk1");
    assert_eq!(deserialized.sender_pseudonym, "ps1");
    assert_eq!(deserialized.reply_public_key, "rpk1");
    let card = deserialized.identity_card.unwrap();
    assert_eq!(card.display_name, "Alice");
    assert_eq!(card.contact_hint, Some("alice@example.com".to_string()));
    assert_eq!(card.node_public_key, "node_pk_alice");
}

#[test]
fn test_connection_payload_backward_compat() {
    // Old payload without identity_card field — must still deserialize
    let json = r#"{
        "message_type": "accept",
        "message": "hi",
        "sender_public_key": "pk",
        "sender_pseudonym": "ps",
        "reply_public_key": "rpk"
    }"#;
    let payload: ConnectionPayload = serde_json::from_str(json).unwrap();
    assert_eq!(payload.message_type, "accept");
    assert_eq!(payload.message, "hi");
    assert_eq!(payload.sender_public_key, "pk");
    assert_eq!(payload.sender_pseudonym, "ps");
    assert_eq!(payload.reply_public_key, "rpk");
    assert!(payload.identity_card.is_none());
    assert!(payload.preferred_role.is_none());
}

#[test]
fn test_contact_from_discovery_incoming() {
    let contact = Contact::from_discovery(
        "pub_key_123".to_string(),
        "Bob".to_string(),
        Some("bob@example.com".to_string()),
        TrustDirection::Incoming,
        Some("pseudo-uuid".to_string()),
        Some("reply_pk".to_string()),
        "personal".to_string(),
        "acquaintance".to_string(),
    );
    assert_eq!(contact.public_key, "pub_key_123");
    assert_eq!(contact.display_name, "Bob");
    assert_eq!(contact.contact_hint, Some("bob@example.com".to_string()));
    assert_eq!(contact.direction, TrustDirection::Incoming);
    assert_eq!(contact.pseudonym, Some("pseudo-uuid".to_string()));
    assert_eq!(contact.messaging_public_key, Some("reply_pk".to_string()));
    assert_eq!(
        contact.roles.get("personal"),
        Some(&"acquaintance".to_string())
    );
    assert!(!contact.revoked);
}

#[test]
fn test_contact_from_discovery_outgoing() {
    let contact = Contact::from_discovery(
        "pub_key_456".to_string(),
        "Alice".to_string(),
        None,
        TrustDirection::Outgoing,
        None,
        None,
        "health".to_string(),
        "trainer".to_string(),
    );
    assert_eq!(contact.public_key, "pub_key_456");
    assert_eq!(contact.display_name, "Alice");
    assert_eq!(contact.direction, TrustDirection::Outgoing);
    assert_eq!(contact.roles.get("health"), Some(&"trainer".to_string()));
    assert!(contact.contact_hint.is_none());
    assert!(contact.pseudonym.is_none());
    assert!(contact.messaging_public_key.is_none());
    assert!(!contact.revoked);
}
