use chrono::Utc;
use fold_db::schema::SchemaError;

use crate::trust::contact_book::{Contact, TrustDirection};
use crate::trust::identity_card::IdentityCard;
use crate::trust::sharing_roles::SharingRoleConfig;
use crate::trust::trust_invite::TrustInvite;
use crate::user_profile::UserProfileStore;

use super::OperationProcessor;

/// Sub-prefix under `UserProfileStore` for consumed invite nonces.
/// Each nonce is a separate record — we never list them all, we only
/// check containment for replay defense.
const CONSUMED_NONCES_PREFIX: &str = "invites/consumed/";

fn consumed_nonce_key(nonce: &str) -> String {
    format!("{CONSUMED_NONCES_PREFIX}{nonce}")
}

/// Identity card and contact book operations.
impl OperationProcessor {
    // ===== Identity Card =====

    /// Storage sub-key inside `UserProfileStore` (lives under the synced
    /// `user_profile/` prefix of the `metadata` namespace).
    const IDENTITY_CARD_KEY: &'static str = "identity_card";

    fn user_profile(&self) -> Result<UserProfileStore, SchemaError> {
        let fold_db = self
            .node
            .get_fold_db()
            .map_err(|e| SchemaError::InvalidData(format!("FoldDB not available: {e}")))?;
        Ok(UserProfileStore::from_db(&fold_db))
    }

    /// Get the current identity card (or None if not set).
    ///
    /// Reads from the synced user-profile store. Writes to this store
    /// propagate to every device restored from the same mnemonic via the
    /// sync log.
    pub async fn get_identity_card(&self) -> Result<Option<IdentityCard>, SchemaError> {
        self.user_profile()?.get(Self::IDENTITY_CARD_KEY).await
    }

    /// Set or update the identity card.
    pub async fn set_identity_card(
        &self,
        display_name: String,
        contact_hint: Option<String>,
        birthday: Option<String>,
    ) -> Result<(), SchemaError> {
        let card = IdentityCard::new(display_name, contact_hint, birthday);
        self.user_profile()?
            .put(Self::IDENTITY_CARD_KEY, &card)
            .await
    }

    // ===== Contact Book =====

    /// List all active contacts.
    pub async fn list_contacts(&self) -> Result<Vec<Contact>, SchemaError> {
        let book = self.load_contact_book().await?;
        Ok(book.active_contacts().into_iter().cloned().collect())
    }

    /// Get a specific contact by public key.
    pub async fn get_contact(&self, public_key: &str) -> Result<Option<Contact>, SchemaError> {
        let book = self.load_contact_book().await?;
        Ok(book.get(public_key).filter(|c| !c.revoked).cloned())
    }

    // ===== Consumed invite nonces (replay defense) =====

    async fn is_nonce_consumed(&self, nonce: &str) -> Result<bool, SchemaError> {
        let store = self.user_profile()?;
        let found: Option<serde_json::Value> = store.get(&consumed_nonce_key(nonce)).await?;
        Ok(found.is_some())
    }

    async fn mark_nonce_consumed(&self, nonce: &str) -> Result<(), SchemaError> {
        self.user_profile()?
            .put(&consumed_nonce_key(nonce), &serde_json::json!({}))
            .await
    }

    // ===== Trust Invites =====

    /// Create a signed trust invite token for direct sharing.
    /// The `proposed_role` is the role name (e.g., "friend", "doctor") to propose.
    pub async fn create_trust_invite(
        &self,
        proposed_role: &str,
    ) -> Result<TrustInvite, SchemaError> {
        let db = self
            .node
            .get_fold_db()
            .map_err(|e| SchemaError::InvalidData(format!("FoldDB not available: {e}")))?;
        let identity = IdentityCard::load(&db).await?.ok_or_else(|| {
            SchemaError::InvalidData(
                "Identity card not set. Please set your display name first.".to_string(),
            )
        })?;

        let private_key = self.node.get_node_private_key();
        let public_key = self.node.get_node_public_key();

        let invite = TrustInvite::create(private_key, public_key, &identity, proposed_role)
            .map_err(|e| SchemaError::InvalidData(format!("Failed to create trust invite: {e}")))?;

        // Record in sent invites — synced so every device sees it.
        let mut sent = crate::trust::sent_invites::SentInviteStore::load(&db)
            .await
            .unwrap_or_default();
        sent.record(crate::trust::sent_invites::SentInvite {
            nonce: invite.nonce.clone(),
            recipient_hint: "unknown".to_string(),
            proposed_role: proposed_role.to_string(),
            created_at: invite.created_at,
            status: crate::trust::sent_invites::SentInviteStatus::Pending,
        });
        if let Err(e) = sent.save(&db).await {
            tracing::warn!("create_trust_invite: failed to save sent invite: {e}");
        }

        Ok(invite)
    }

    /// Accept a trust invite: verify signature, look up role, add to trust map and contact book.
    /// If `accept_role` is provided, use that role instead of the proposed_role from the invite.
    /// If `trust_back` is true, also creates a reciprocal invite.
    pub async fn accept_trust_invite(
        &self,
        invite: &TrustInvite,
        accept_role: Option<&str>,
        trust_back: bool,
    ) -> Result<Option<TrustInvite>, SchemaError> {
        // Verify the invite signature and expiry
        let valid = invite
            .verify()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to verify invite: {e}")))?;
        if !valid {
            return Err(SchemaError::PermissionDenied(
                "Trust invite signature verification failed".to_string(),
            ));
        }

        let db = self
            .node
            .get_fold_db()
            .map_err(|e| SchemaError::InvalidData(format!("FoldDB not available: {e}")))?;

        // Replay prevention: check nonce hasn't been consumed
        if self.is_nonce_consumed(&invite.nonce).await? {
            return Err(SchemaError::PermissionDenied(
                "Trust invite has already been used (replay detected)".to_string(),
            ));
        }

        // Check if this invite was previously declined
        let declined = crate::trust::declined_invites::DeclinedInviteStore::load(&db)
            .await
            .unwrap_or_default();
        if declined.is_declined(&invite.nonce) {
            return Err(SchemaError::InvalidData(
                "This invite was previously declined. Remove the decline first to accept."
                    .to_string(),
            ));
        }

        // Resolve the role name: accept_role overrides the proposed_role from the invite
        let role_name = accept_role.unwrap_or(&invite.proposed_role);

        // Look up the role in SharingRoleConfig to get domain and tier
        let config = SharingRoleConfig::load(&db).await?;
        let role = config
            .get_role(role_name)
            .ok_or_else(|| SchemaError::InvalidData(format!("Unknown role: {role_name}")))?;

        // Grant trust in the role's domain at the role's tier
        self.grant_trust_for_domain(&invite.sender_pub_key, &role.domain, role.tier)
            .await?;

        // Mark nonce as consumed
        self.mark_nonce_consumed(&invite.nonce).await?;

        // Determine direction: check if we previously sent an invite to this sender.
        // If yes, this is a reciprocal accept -> mutual trust.
        // Also mark the matching sent invite as accepted.
        let mut is_mutual = false;
        let mut sent = crate::trust::sent_invites::SentInviteStore::load(&db)
            .await
            .unwrap_or_default();
        let has_pending = sent
            .invites
            .iter()
            .any(|i| i.status == crate::trust::sent_invites::SentInviteStatus::Pending);
        if has_pending {
            for inv in sent.invites.iter_mut().rev() {
                if inv.status == crate::trust::sent_invites::SentInviteStatus::Pending {
                    inv.status = crate::trust::sent_invites::SentInviteStatus::Accepted;
                    break;
                }
            }
            if let Err(e) = sent.save(&db).await {
                tracing::warn!("accept_trust_invite: failed to save sent invites: {e}");
            }
        }
        // Also check if sender is already in our contacts (re-accept scenario)
        let existing_book = self.load_contact_book().await?;
        if existing_book
            .get(&invite.sender_pub_key)
            .filter(|c| !c.revoked)
            .is_some()
        {
            is_mutual = true;
        }
        let direction = if is_mutual {
            TrustDirection::Mutual
        } else {
            TrustDirection::Outgoing
        };

        let mut roles = std::collections::HashMap::new();
        roles.insert(role.domain.clone(), role_name.to_string());

        let contact = Contact {
            public_key: invite.sender_pub_key.clone(),
            display_name: invite.sender_identity.display_name.clone(),
            contact_hint: invite.sender_identity.contact_hint.clone(),
            direction,
            connected_at: Utc::now(),
            pseudonym: None,
            messaging_pseudonym: None,
            messaging_public_key: None,
            identity_pseudonym: None,
            revoked: false,
            roles,
        };

        let mut book = self.load_contact_book().await?;
        book.upsert_contact(contact);
        self.save_contact_book(&book).await?;

        // Create reciprocal invite if requested
        if trust_back {
            let reciprocal = self.create_trust_invite(role_name).await?;
            Ok(Some(reciprocal))
        } else {
            Ok(None)
        }
    }

    /// Revoke trust for a contact: remove from ALL trust domains and mark revoked in contact book.
    pub async fn revoke_contact(&self, public_key: &str) -> Result<(), SchemaError> {
        // Revoke from ALL domains — not just personal
        let domains = self.list_trust_domains().await?;
        for domain in &domains {
            self.revoke_trust_for_domain(public_key, domain).await?;
        }

        // Mark revoked in contact book
        let mut book = self.load_contact_book().await?;
        book.revoke(public_key);
        self.save_contact_book(&book).await?;

        Ok(())
    }
}
