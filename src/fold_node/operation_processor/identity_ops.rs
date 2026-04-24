use chrono::Utc;
use fold_db::schema::SchemaError;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::trust::contact_book::{Contact, ContactBook, TrustDirection};
use crate::trust::identity_card::IdentityCard;
use crate::trust::sharing_roles::SharingRoleConfig;
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

    /// Storage key inside the synced `metadata` namespace.
    const IDENTITY_CARD_KEY: &'static str = "identity_card";

    /// Get the current identity card (or None if not set).
    ///
    /// Reads from the synced `metadata` namespace first — this is what lets a
    /// restored device inherit the owner's card without re-entering it. If
    /// the synced store is empty, fall back to the legacy per-device JSON
    /// file at `$FOLDDB_HOME/config/identity_card.json` and promote it into
    /// the synced store on the next `set_identity_card`.
    pub async fn get_identity_card(&self) -> Result<Option<IdentityCard>, SchemaError> {
        use fold_db::storage::traits::TypedStore;
        use fold_db::storage::TypedKvStore;

        let fold_db = self
            .node
            .get_fold_db()
            .map_err(|e| SchemaError::InvalidData(format!("FoldDB not available: {e}")))?;
        let raw = fold_db.db_ops().metadata().raw_metadata_kv();
        let typed: TypedKvStore<dyn fold_db::storage::traits::KvStore> = TypedKvStore::new(raw);
        match typed
            .get_item::<IdentityCard>(Self::IDENTITY_CARD_KEY)
            .await
        {
            Ok(Some(card)) => return Ok(Some(card)),
            Ok(None) => {}
            Err(e) => {
                log::warn!(
                    "identity_card: synced-store read failed ({e}); falling back to legacy JSON"
                );
            }
        }

        // Legacy fallback: per-device JSON file. Kept readable so a client
        // that upgraded before ever calling `set_identity_card` on the new
        // code path still sees its card.
        IdentityCard::load()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load identity card: {e}")))
    }

    /// Set or update the identity card.
    ///
    /// Writes to the synced `metadata` namespace — writes propagate to every
    /// other device restored from the same mnemonic via the sync log. Also
    /// still writes the legacy JSON file so older code paths (and rollbacks)
    /// continue to see the card locally.
    pub async fn set_identity_card(
        &self,
        display_name: String,
        contact_hint: Option<String>,
        birthday: Option<String>,
    ) -> Result<(), SchemaError> {
        use fold_db::storage::traits::TypedStore;
        use fold_db::storage::TypedKvStore;

        let card = IdentityCard::new(display_name, contact_hint, birthday);

        // Save to the synced metadata namespace — this is the authoritative
        // multi-device source of truth. Writes here flow through the
        // `SyncingKvStore` and propagate to peer devices on the same
        // personal prefix.
        let fold_db = self
            .node
            .get_fold_db()
            .map_err(|e| SchemaError::InvalidData(format!("FoldDB not available: {e}")))?;
        let raw = fold_db.db_ops().metadata().raw_metadata_kv();
        let typed: TypedKvStore<dyn fold_db::storage::traits::KvStore> = TypedKvStore::new(raw);
        typed
            .put_item(Self::IDENTITY_CARD_KEY, &card)
            .await
            .map_err(|e| {
                SchemaError::InvalidData(format!("Failed to save identity card (synced): {e}"))
            })?;

        // Legacy mirror on disk — still written so old reads (and any
        // external inspection tool) keep working. One release after
        // everything reads from the synced store we can drop this.
        if let Err(e) = card.save() {
            log::warn!("identity_card: legacy JSON write failed (non-fatal): {e}");
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
    /// The `proposed_role` is the role name (e.g., "friend", "doctor") to propose.
    pub fn create_trust_invite(&self, proposed_role: &str) -> Result<TrustInvite, SchemaError> {
        let identity = IdentityCard::load()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load identity card: {e}")))?
            .ok_or_else(|| {
                SchemaError::InvalidData(
                    "Identity card not set. Please set your display name first.".to_string(),
                )
            })?;

        let private_key = self.node.get_node_private_key();
        let public_key = self.node.get_node_public_key();

        let invite = TrustInvite::create(private_key, public_key, &identity, proposed_role)
            .map_err(|e| SchemaError::InvalidData(format!("Failed to create trust invite: {e}")))?;

        // Record in sent invites
        if let Ok(mut store) = crate::trust::sent_invites::SentInviteStore::load() {
            store.record(crate::trust::sent_invites::SentInvite {
                nonce: invite.nonce.clone(),
                recipient_hint: "unknown".to_string(),
                proposed_role: proposed_role.to_string(),
                created_at: invite.created_at,
                status: crate::trust::sent_invites::SentInviteStatus::Pending,
            });
            let _ = store.save();
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

        // Resolve the role name: accept_role overrides the proposed_role from the invite
        let role_name = accept_role.unwrap_or(&invite.proposed_role);

        // Look up the role in SharingRoleConfig to get domain and tier
        let roles_path = self.sharing_roles_path()?;
        let config = SharingRoleConfig::load_from(&roles_path)
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load roles: {e}")))?;
        let role = config
            .get_role(role_name)
            .ok_or_else(|| SchemaError::InvalidData(format!("Unknown role: {role_name}")))?;

        // Grant trust in the role's domain at the role's tier
        self.grant_trust_for_domain(&invite.sender_pub_key, &role.domain, role.tier)
            .await?;

        // Mark nonce as consumed
        save_consumed_nonce(&invite.nonce);

        // Determine direction: check if we previously sent an invite to this sender.
        // If yes, this is a reciprocal accept -> mutual trust.
        // Also mark the matching sent invite as accepted.
        let mut is_mutual = false;
        if let Ok(mut sent) = crate::trust::sent_invites::SentInviteStore::load() {
            // Mark matching pending invite as accepted. We detect mutual trust
            // by finding a sent invite whose nonce was consumed by the sender,
            // but since we don't have cross-node nonce data, we fall back to
            // checking if the sender is already in our contact book (below).
            // Here we just mark the most recent pending invite as accepted.
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

        let mut book = ContactBook::load()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load contact book: {e}")))?;
        book.upsert_contact(contact);
        book.save()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to save contact book: {e}")))?;

        // Create reciprocal invite if requested
        if trust_back {
            let reciprocal = self.create_trust_invite(role_name)?;
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
        let mut book = ContactBook::load()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load contact book: {e}")))?;
        book.revoke(public_key);
        book.save()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to save contact book: {e}")))?;

        Ok(())
    }
}
