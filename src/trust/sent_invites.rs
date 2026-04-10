//! Sent invites — tracks invites this node has created and shared.
//!
//! Stored at `$FOLDDB_HOME/config/sent_invites.json`. Local-only.
//! Allows Alice to see which invites are pending, accepted, or expired.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::utils::paths::folddb_home;

const SENT_INVITES_FILE: &str = "config/sent_invites.json";

/// Status of a sent invite.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SentInviteStatus {
    Pending,
    Accepted,
    Expired,
}

/// A record of a sent trust invite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SentInvite {
    /// The invite's nonce (unique identifier).
    pub nonce: String,
    /// Who we sent it to (if known — may be "unknown" for link shares).
    pub recipient_hint: String,
    /// Proposed role name.
    pub proposed_role: String,
    /// When the invite was created.
    pub created_at: DateTime<Utc>,
    /// Current status.
    pub status: SentInviteStatus,
}

/// All sent invites for this node.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SentInviteStore {
    pub invites: Vec<SentInvite>,
}

impl SentInviteStore {
    fn file_path() -> Result<PathBuf, String> {
        Ok(folddb_home()?.join(SENT_INVITES_FILE))
    }

    pub fn load() -> Result<Self, String> {
        Self::load_from(&Self::file_path()?)
    }

    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read sent invites: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("Failed to parse sent invites: {e}"))
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::file_path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create config dir: {e}"))?;
        }
        let data = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize sent invites: {e}"))?;
        std::fs::write(path, data).map_err(|e| format!("Failed to write sent invites: {e}"))
    }

    /// Record a sent invite.
    pub fn record(&mut self, invite: SentInvite) {
        if !self.invites.iter().any(|i| i.nonce == invite.nonce) {
            self.invites.push(invite);
        }
    }

    /// Mark an invite as accepted (when we see the sender in our contacts).
    pub fn mark_accepted(&mut self, nonce: &str) {
        if let Some(inv) = self.invites.iter_mut().find(|i| i.nonce == nonce) {
            inv.status = SentInviteStatus::Accepted;
        }
    }

    /// Get pending invites.
    pub fn pending(&self) -> Vec<&SentInvite> {
        self.invites
            .iter()
            .filter(|i| i.status == SentInviteStatus::Pending)
            .collect()
    }
}
