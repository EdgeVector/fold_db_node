use chrono::Utc;
use fold_db::schema::SchemaError;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::trust::contact_book::{Contact, ContactBook, TrustDirection};
use crate::trust::identity_card::IdentityCard;
use crate::trust::trust_invite::TrustInvite;
use crate::utils::paths::folddb_home;

use super::OperationProcessor;

/// Track consumed invite nonces to prevent replay. Persisted to disk.
static CONSUMED_NONCES: Mutex<Option<HashSet<String>>> = Mutex::new(None);

const CONSUMED_NONCES_FILE: &str = "config/consumed_nonces.json";

fn load_consumed_nonces() -> HashSet<String> {
    let mut guard = CONSUMED_NONCES.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(ref set) = *guard {
        return set.clone();
    }
    let path = folddb_home()
        .map(|h| h.join(CONSUMED_NONCES_FILE))
        .unwrap_or_else(|_| PathBuf::from(CONSUMED_NONCES_FILE));
    let set = if path.exists() {
        std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        HashSet::new()
    };
    *guard = Some(set.clone());
    set
}

fn save_consumed_nonce(nonce: &str) {
    let mut guard = CONSUMED_NONCES.lock().unwrap_or_else(|p| p.into_inner());
    let set = guard.get_or_insert_with(HashSet::new);
    set.insert(nonce.to_string());
    let path = folddb_home()
        .map(|h| h.join(CONSUMED_NONCES_FILE))
        .unwrap_or_else(|_| PathBuf::from(CONSUMED_NONCES_FILE));
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, serde_json::to_string(set).unwrap_or_default());
}

/// Identity card and contact book operations.
impl OperationProcessor {
    // ===== Identity Card =====

    /// Get the current identity card (or None if not set).
    pub fn get_identity_card(&self) -> Result<Option<IdentityCard>, SchemaError> {
        IdentityCard::load()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load identity card: {e}")))
    }

    /// Set or update the identity card (file + Sled).
    pub fn set_identity_card(
        &self,
        display_name: String,
        contact_hint: Option<String>,
        birthday: Option<String>,
    ) -> Result<(), SchemaError> {
        let card = IdentityCard::new(display_name, contact_hint, birthday);
        // Save to file (backward compat)
        card.save()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to save identity card: {e}")))?;

        // Also save to Sled config store (best-effort)
        if let Ok(folddb_home) = folddb_home() {
            let data_path = folddb_home.join("data");
            {
                let pool = std::sync::Arc::new(fold_db::storage::SledPool::new(data_path));
                if let Ok(store) = fold_db::NodeConfigStore::new(pool) {
                    if let Err(e) = card.save_to_sled(&store) {
                        log::warn!("Failed to save identity card to Sled: {}", e);
                    }
                }
            }
        }

        Ok(())
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

        let invite = TrustInvite::create(private_key, public_key, &identity, proposed_distance)
            .map_err(|e| SchemaError::InvalidData(format!("Failed to create trust invite: {e}")))?;

        // Record in sent invites
        if let Ok(mut store) = crate::trust::sent_invites::SentInviteStore::load() {
            store.record(crate::trust::sent_invites::SentInvite {
                nonce: invite.nonce.clone(),
                recipient_hint: "unknown".to_string(),
                proposed_distance,
                created_at: invite.created_at,
                status: crate::trust::sent_invites::SentInviteStatus::Pending,
            });
            let _ = store.save();
        }

        Ok(invite)
    }

    /// Accept a trust invite: verify signature, add to trust graph and contact book.
    /// If `trust_back` is true, also creates a reciprocal invite.
    pub async fn accept_trust_invite(
        &self,
        invite: &TrustInvite,
        accept_distance: Option<u64>,
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

        // Replay prevention: check nonce hasn't been consumed
        let consumed = load_consumed_nonces();
        if consumed.contains(&invite.nonce) {
            return Err(SchemaError::PermissionDenied(
                "Trust invite has already been used (replay detected)".to_string(),
            ));
        }

        // Check if this invite was previously declined
        if let Ok(declined) = crate::trust::declined_invites::DeclinedInviteStore::load() {
            if declined.is_declined(&invite.nonce) {
                return Err(SchemaError::InvalidData(
                    "This invite was previously declined. Remove the decline first to accept."
                        .to_string(),
                ));
            }
        }

        // Validate accept_distance: must be >= 1 (distance 0 is owner-only)
        let distance = accept_distance.unwrap_or(invite.proposed_distance);
        if distance == 0 {
            return Err(SchemaError::InvalidData(
                "Trust distance must be >= 1 (distance 0 is reserved for the owner)".to_string(),
            ));
        }

        // Add to trust graph
        self.grant_trust(&invite.sender_pub_key, distance).await?;

        // Mark nonce as consumed
        save_consumed_nonce(&invite.nonce);

        // Determine direction: check if we previously sent an invite to this sender.
        // If yes, this is a reciprocal accept → mutual trust.
        // Also mark the matching sent invite as accepted.
        let mut is_mutual = false;
        if let Ok(mut sent) = crate::trust::sent_invites::SentInviteStore::load() {
            // Check if we have any sent invite (pending or not) — the sender
            // is responding to our invite, making this mutual
            let has_sent = sent.invites.iter().any(|_| true);
            if has_sent {
                is_mutual = true;
                // Mark the most recent pending invite as accepted
                for inv in sent.invites.iter_mut().rev() {
                    if inv.status == crate::trust::sent_invites::SentInviteStatus::Pending {
                        inv.status = crate::trust::sent_invites::SentInviteStatus::Accepted;
                        break;
                    }
                }
                let _ = sent.save();
            }
        }
        // Also check if sender is already in our contacts (re-accept scenario)
        let existing_book = ContactBook::load()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load contact book: {e}")))?;
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

        let contact = Contact {
            public_key: invite.sender_pub_key.clone(),
            display_name: invite.sender_identity.display_name.clone(),
            contact_hint: invite.sender_identity.contact_hint.clone(),
            trust_distance: distance,
            direction,
            connected_at: Utc::now(),
            pseudonym: None,
            messaging_pseudonym: None,
            messaging_public_key: None,
            revoked: false,
            roles: std::collections::HashMap::new(),
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
