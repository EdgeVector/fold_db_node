//! Sharing audit — "What can Bob see?"
//!
//! Computes which schemas and fields a contact can access based on their
//! trust tiers across all domains and the access policies on fields.

use fold_db::access::AccessTier;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// A schema and which of its fields a contact can read/write.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessibleSchema {
    pub schema_name: String,
    /// Human-readable name (if available).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descriptive_name: Option<String>,
    pub trust_domain: String,
    pub readable_fields: Vec<String>,
    pub writable_fields: Vec<String>,
    /// Total fields in the schema (readable + hidden).
    pub total_fields: usize,
}

/// Result of auditing what a contact can see.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SharingAuditResult {
    pub contact_public_key: String,
    pub contact_display_name: String,
    /// Per-domain trust tiers for this contact.
    pub domain_tiers: HashMap<String, AccessTier>,
    /// Per-domain roles for this contact.
    pub domain_roles: HashMap<String, String>,
    /// Schemas this contact can access (at least one readable field).
    pub accessible_schemas: Vec<AccessibleSchema>,
    /// Total readable fields across all schemas.
    pub total_readable: usize,
    /// Total writable fields across all schemas.
    pub total_writable: usize,
}
