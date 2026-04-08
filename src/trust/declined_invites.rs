//! Declined invites — tracks invites the user chose to decline.
//!
//! Stored at `$FOLDDB_HOME/config/declined_invites.json`. Local-only.
//! Prevents the same invite from being shown again and allows the user
//! to review past decisions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::utils::paths::folddb_home;

const DECLINED_INVITES_FILE: &str = "config/declined_invites.json";

/// A record of a declined trust invite.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeclinedInvite {
    /// The sender's Ed25519 public key.
    pub sender_pub_key: String,
    /// The sender's display name from the invite.
    pub sender_display_name: String,
    /// The sender's contact hint.
    pub sender_contact_hint: Option<String>,
    /// The proposed trust distance.
    pub proposed_distance: u64,
    /// When the invite was declined.
    pub declined_at: DateTime<Utc>,
    /// The invite's nonce (to identify duplicates).
    pub nonce: String,
}

/// All declined invites for this node.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DeclinedInviteStore {
    pub invites: Vec<DeclinedInvite>,
}

impl DeclinedInviteStore {
    fn file_path() -> Result<PathBuf, String> {
        Ok(folddb_home()?.join(DECLINED_INVITES_FILE))
    }

    pub fn load() -> Result<Self, String> {
        Self::load_from(&Self::file_path()?)
    }

    pub fn load_from(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read declined invites: {e}"))?;
        serde_json::from_str(&data).map_err(|e| format!("Failed to parse declined invites: {e}"))
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
            .map_err(|e| format!("Failed to serialize declined invites: {e}"))?;
        std::fs::write(path, data).map_err(|e| format!("Failed to write declined invites: {e}"))
    }

    /// Record a declined invite.
    pub fn decline(&mut self, invite: DeclinedInvite) {
        // Don't duplicate — check by nonce
        if !self.invites.iter().any(|i| i.nonce == invite.nonce) {
            self.invites.push(invite);
        }
    }

    /// Check if an invite (by nonce) was previously declined.
    pub fn is_declined(&self, nonce: &str) -> bool {
        self.invites.iter().any(|i| i.nonce == nonce)
    }

    /// Remove a decline record (user changed their mind).
    pub fn undecline(&mut self, nonce: &str) -> bool {
        let len = self.invites.len();
        self.invites.retain(|i| i.nonce != nonce);
        self.invites.len() < len
    }
}
