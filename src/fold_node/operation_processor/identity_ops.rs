use chrono::Utc;
use fold_db::schema::SchemaError;

use crate::trust::contact_book::{Contact, ContactBook, TrustDirection};
use crate::trust::identity_card::IdentityCard;
use crate::trust::trust_invite::TrustInvite;

use super::OperationProcessor;

/// Identity card and contact book operations.
impl OperationProcessor {
    // ===== Identity Card =====

    /// Get the current identity card (or None if not set).
    pub fn get_identity_card(&self) -> Result<Option<IdentityCard>, SchemaError> {
        IdentityCard::load()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load identity card: {e}")))
    }

    /// Set or update the identity card.
    pub fn set_identity_card(
        &self,
        display_name: String,
        contact_hint: Option<String>,
    ) -> Result<(), SchemaError> {
        let card = IdentityCard::new(display_name, contact_hint);
        card.save()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to save identity card: {e}")))
    }

    // ===== Contact Book =====

    /// List all active contacts.
    pub fn list_contacts(&self) -> Result<Vec<Contact>, SchemaError> {
        let book = ContactBook::load()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load contact book: {e}")))?;
        Ok(book.active_contacts().into_iter().cloned().collect())
    }

    /// Get a specific contact by public key.
    pub fn get_contact(&self, public_key: &str) -> Result<Option<Contact>, SchemaError> {
        let book = ContactBook::load()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load contact book: {e}")))?;
        Ok(book.get(public_key).filter(|c| !c.revoked).cloned())
    }

    // ===== Trust Invites =====

    /// Create a signed trust invite token for direct sharing.
    pub fn create_trust_invite(&self, proposed_distance: u64) -> Result<TrustInvite, SchemaError> {
        let identity = IdentityCard::load()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load identity card: {e}")))?
            .ok_or_else(|| {
                SchemaError::InvalidData(
                    "Identity card not set. Please set your display name first.".to_string(),
                )
            })?;

        let private_key = self.node.get_node_private_key();
        let public_key = self.node.get_node_public_key();

        TrustInvite::create(private_key, public_key, &identity, proposed_distance)
            .map_err(|e| SchemaError::InvalidData(format!("Failed to create trust invite: {e}")))
    }

    /// Accept a trust invite: verify signature, add to trust graph and contact book.
    /// If `trust_back` is true, also creates a reciprocal invite.
    pub async fn accept_trust_invite(
        &self,
        invite: &TrustInvite,
        accept_distance: Option<u64>,
        trust_back: bool,
    ) -> Result<Option<TrustInvite>, SchemaError> {
        // Verify the invite signature
        let valid = invite
            .verify()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to verify invite: {e}")))?;
        if !valid {
            return Err(SchemaError::PermissionDenied(
                "Trust invite signature verification failed".to_string(),
            ));
        }

        let distance = accept_distance.unwrap_or(invite.proposed_distance);

        // Add to trust graph
        self.grant_trust(&invite.sender_pub_key, distance).await?;

        // Add to contact book
        let direction = if trust_back {
            TrustDirection::Mutual
        } else {
            TrustDirection::Incoming
        };

        let contact = Contact {
            public_key: invite.sender_pub_key.clone(),
            display_name: invite.sender_identity.display_name.clone(),
            contact_hint: invite.sender_identity.contact_hint.clone(),
            trust_distance: distance,
            direction,
            connected_at: Utc::now(),
            pseudonym: None,
            revoked: false,
        };

        let mut book = ContactBook::load()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load contact book: {e}")))?;
        book.upsert_contact(contact);
        book.save()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to save contact book: {e}")))?;

        // Create reciprocal invite if requested
        if trust_back {
            let reciprocal = self.create_trust_invite(distance)?;
            Ok(Some(reciprocal))
        } else {
            Ok(None)
        }
    }

    /// Revoke trust for a contact: remove from trust graph and mark revoked in contact book.
    pub async fn revoke_contact(&self, public_key: &str) -> Result<(), SchemaError> {
        // Revoke in trust graph
        self.revoke_trust(public_key).await?;

        // Mark revoked in contact book
        let mut book = ContactBook::load()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load contact book: {e}")))?;
        book.revoke(public_key);
        book.save()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to save contact book: {e}")))?;

        Ok(())
    }
}
