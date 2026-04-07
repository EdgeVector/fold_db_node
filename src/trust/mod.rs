//! Trust identity and contact management.
//!
//! This module handles:
//! - **Identity Card**: Local-only display name + contact hint, never synced
//! - **Contact Book**: Maps public keys to human-readable identity info
//! - **Trust Invites**: Signed payloads for establishing trust between nodes

pub mod contact_book;
pub mod identity_card;
pub mod sharing_roles;
pub mod trust_invite;
