use fold_db::access::{AuditAction, AuditEvent, TrustTier};
use fold_db::schema::types::field::Field;
use fold_db::schema::SchemaError;

use crate::trust::contact_book::ContactBook;
use crate::trust::sharing_audit::{AccessibleSchema, SharingAuditResult};
use crate::trust::sharing_roles::SharingRoleConfig;

use super::OperationProcessor;

/// Helper to convert FoldDbError to SchemaError for trust operation return types.
fn to_schema_err(e: fold_db::error::FoldDbError) -> SchemaError {
    SchemaError::InvalidData(e.to_string())
}

/// Trust and access-control operations.
///
/// Trust map and audit log operations are backed by fold_db's access module.
/// Field policies, capabilities, and payment gates are not yet implemented.
impl OperationProcessor {
    pub async fn load_trust_maps(&self) -> Result<serde_json::Value, SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let domains = db.db_ops.list_trust_domains().await?;
        let mut result = serde_json::Map::new();
        for domain in &domains {
            let map = db.db_ops.load_trust_map_for_domain(domain).await?;
            let value = serde_json::to_value(&map).map_err(|e| {
                SchemaError::InvalidData(format!("Failed to serialize trust map: {e}"))
            })?;
            result.insert(domain.clone(), value);
        }
        Ok(serde_json::Value::Object(result))
    }

    pub async fn grant_trust(
        &self,
        user_public_key: &str,
        tier: TrustTier,
    ) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let mut map = db.db_ops.load_trust_map().await?;
        map.insert(user_public_key.to_string(), tier);
        db.db_ops.store_trust_map(&map).await?;
        let event = AuditEvent::trust_event(
            user_public_key,
            AuditAction::TrustGrant {
                user_id: user_public_key.to_string(),
                tier,
            },
        );
        db.db_ops.append_audit_event(event).await?;
        Ok(())
    }

    pub async fn revoke_trust(&self, user_public_key: &str) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let mut map = db.db_ops.load_trust_map().await?;
        map.remove(user_public_key);
        db.db_ops.store_trust_map(&map).await?;
        let event = AuditEvent::trust_event(
            user_public_key,
            AuditAction::TrustRevoke {
                user_id: user_public_key.to_string(),
            },
        );
        db.db_ops.append_audit_event(event).await?;
        Ok(())
    }

    pub async fn resolve_trust_tier(
        &self,
        user_public_key: &str,
    ) -> Result<Option<TrustTier>, SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let map = db.db_ops.load_trust_map().await?;
        Ok(map.get(user_public_key).copied())
    }

    pub async fn list_trust_grants(&self) -> Result<Vec<(String, TrustTier)>, SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let map = db.db_ops.load_trust_map().await?;
        Ok(map.into_iter().collect())
    }

    // ===== Domain-aware trust operations =====

    /// Grant trust in a specific domain.
    pub async fn grant_trust_for_domain(
        &self,
        user_public_key: &str,
        domain: &str,
        tier: TrustTier,
    ) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let mut map = db.db_ops.load_trust_map_for_domain(domain).await?;
        map.insert(user_public_key.to_string(), tier);
        db.db_ops.store_trust_map_for_domain(domain, &map).await?;
        let event = AuditEvent::trust_event(
            user_public_key,
            AuditAction::TrustGrant {
                user_id: user_public_key.to_string(),
                tier,
            },
        );
        db.db_ops.append_audit_event(event).await?;
        Ok(())
    }

    /// Revoke trust from a specific domain.
    pub async fn revoke_trust_for_domain(
        &self,
        user_public_key: &str,
        domain: &str,
    ) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let mut map = db.db_ops.load_trust_map_for_domain(domain).await?;
        map.remove(user_public_key);
        db.db_ops.store_trust_map_for_domain(domain, &map).await?;
        let event = AuditEvent::trust_event(
            user_public_key,
            AuditAction::TrustRevoke {
                user_id: user_public_key.to_string(),
            },
        );
        db.db_ops.append_audit_event(event).await?;
        Ok(())
    }

    /// List all trust domains that have stored maps.
    pub async fn list_trust_domains(&self) -> Result<Vec<String>, SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        db.db_ops.list_trust_domains().await
    }

    // ===== Role-based trust operations =====

    /// Resolve the contact book file path from the node's config directory.
    fn contact_book_path(&self) -> Result<std::path::PathBuf, SchemaError> {
        let config_dir = self
            .node
            .get_config_dir()
            .map_err(|e| SchemaError::InvalidData(format!("Cannot resolve config dir: {e}")))?;
        Ok(config_dir.join("contact_book.json"))
    }

    /// Resolve the sharing roles file path from the node's config directory.
    pub(super) fn sharing_roles_path(&self) -> Result<std::path::PathBuf, SchemaError> {
        let config_dir = self
            .node
            .get_config_dir()
            .map_err(|e| SchemaError::InvalidData(format!("Cannot resolve config dir: {e}")))?;
        Ok(config_dir.join("sharing_roles.json"))
    }

    /// Assign a sharing role to a contact. Translates the role to a domain-specific
    /// trust grant and records the role on the contact.
    pub async fn assign_role_to_contact(
        &self,
        public_key: &str,
        role_name: &str,
    ) -> Result<(), SchemaError> {
        let roles_path = self.sharing_roles_path()?;
        let config = SharingRoleConfig::load_from(&roles_path)
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load roles: {e}")))?;
        let role = config
            .get_role(role_name)
            .ok_or_else(|| SchemaError::InvalidData(format!("Unknown role: {role_name}")))?;

        // Grant trust in the role's domain at the role's tier
        self.grant_trust_for_domain(public_key, &role.domain, role.tier)
            .await?;

        // Update contact book with role assignment
        let book_path = self.contact_book_path()?;
        let mut book = ContactBook::load_from(&book_path)
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load contacts: {e}")))?;
        if let Some(contact) = book.contacts.get_mut(public_key) {
            contact
                .roles
                .insert(role.domain.clone(), role_name.to_string());
        }
        book.save_to(&book_path)
            .map_err(|e| SchemaError::InvalidData(format!("Failed to save contacts: {e}")))?;

        Ok(())
    }

    /// Remove a role from a contact in a specific domain. Revokes trust in that domain.
    pub async fn remove_role_from_contact(
        &self,
        public_key: &str,
        domain: &str,
    ) -> Result<(), SchemaError> {
        self.revoke_trust_for_domain(public_key, domain).await?;

        let book_path = self.contact_book_path()?;
        let mut book = ContactBook::load_from(&book_path)
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load contacts: {e}")))?;
        if let Some(contact) = book.contacts.get_mut(public_key) {
            contact.roles.remove(domain);
        }
        book.save_to(&book_path)
            .map_err(|e| SchemaError::InvalidData(format!("Failed to save contacts: {e}")))?;

        Ok(())
    }

    // ===== Sharing Audit =====

    /// Compute what a contact can access across all schemas and domains.
    /// Returns readable/writable fields per schema based on the contact's
    /// trust tiers in each domain vs. field access policies.
    pub async fn audit_contact_access(
        &self,
        public_key: &str,
    ) -> Result<SharingAuditResult, SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;

        // 1. Resolve tiers across all domains
        let domains = db.db_ops.list_trust_domains().await?;
        let mut domain_tiers = std::collections::HashMap::new();
        for domain in &domains {
            let map = db.db_ops.load_trust_map_for_domain(domain).await?;
            if let Some(&tier) = map.get(public_key) {
                domain_tiers.insert(domain.clone(), tier);
            }
        }

        // 2. Get contact info
        let book_path = self.contact_book_path()?;
        let book = ContactBook::load_from(&book_path)
            .map_err(|e| SchemaError::InvalidData(format!("Failed to load contacts: {e}")))?;
        let contact = book.get(public_key).filter(|c| !c.revoked);
        let display_name = contact.map(|c| c.display_name.clone()).unwrap_or_default();
        let domain_roles = contact.map(|c| c.roles.clone()).unwrap_or_default();

        // 3. For each approved schema, check which fields are accessible
        let schemas = db
            .schema_manager
            .get_schemas_with_states()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to list schemas: {e}")))?;
        let mut accessible_schemas = Vec::new();
        let mut total_readable = 0usize;
        let mut total_writable = 0usize;

        for sws in &schemas {
            if sws.state != fold_db::schema::SchemaState::Approved {
                continue;
            }
            let schema = &sws.schema;

            // Determine the schema's trust domain
            let schema_domain = schema.trust_domain.as_deref().unwrap_or("personal");

            let mut readable = Vec::new();
            let mut writable = Vec::new();

            for (field_name, field) in &schema.runtime_fields {
                if let Some(policy) = &field.common().access_policy {
                    let domain = &policy.trust_domain;
                    if let Some(&tier) = domain_tiers.get(domain) {
                        if tier >= policy.min_read_tier {
                            readable.push(field_name.clone());
                        }
                        if tier >= policy.min_write_tier {
                            writable.push(field_name.clone());
                        }
                    }
                } else {
                    // No policy = legacy = granted (for now)
                    readable.push(field_name.clone());
                    writable.push(field_name.clone());
                }
            }

            if !readable.is_empty() {
                total_readable += readable.len();
                total_writable += writable.len();
                let total_fields = schema.runtime_fields.len();
                accessible_schemas.push(AccessibleSchema {
                    schema_name: schema.name.clone(),
                    descriptive_name: schema.descriptive_name.clone(),
                    trust_domain: schema_domain.to_string(),
                    readable_fields: readable,
                    writable_fields: writable,
                    total_fields,
                });
            }
        }

        Ok(SharingAuditResult {
            contact_public_key: public_key.to_string(),
            contact_display_name: display_name,
            domain_tiers,
            domain_roles,
            accessible_schemas,
            total_readable,
            total_writable,
        })
    }

    /// Get an overview of the node's sharing posture: how many schemas per domain,
    /// how many contacts have access, total exposed fields.
    pub async fn sharing_posture(&self) -> Result<serde_json::Value, SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;

        // Count schemas per trust domain
        let schemas = db
            .schema_manager
            .get_schemas_with_states()
            .map_err(|e| SchemaError::InvalidData(format!("Failed to list schemas: {e}")))?;

        let mut domain_schemas: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let mut total_policy_fields = 0usize;
        let mut total_unprotected_fields = 0usize;

        for sws in &schemas {
            if sws.state != fold_db::schema::SchemaState::Approved {
                continue;
            }
            let domain = sws.schema.trust_domain.as_deref().unwrap_or("personal");
            *domain_schemas.entry(domain.to_string()).or_insert(0) += 1;

            for field in sws.schema.runtime_fields.values() {
                if field.common().access_policy.is_some() {
                    total_policy_fields += 1;
                } else {
                    total_unprotected_fields += 1;
                }
            }
        }

        // Count contacts per domain
        let domains = db.db_ops.list_trust_domains().await?;
        let mut domain_contacts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        for domain in &domains {
            let map = db.db_ops.load_trust_map_for_domain(domain).await?;
            if !map.is_empty() {
                domain_contacts.insert(domain.clone(), map.len());
            }
        }

        Ok(serde_json::json!({
            "domains": domains,
            "schemas_per_domain": domain_schemas,
            "contacts_per_domain": domain_contacts,
            "total_policy_fields": total_policy_fields,
            "total_unprotected_fields": total_unprotected_fields,
        }))
    }

    // ===== Field access policy operations =====

    /// Set the access policy on a specific field. Persists to disk.
    pub async fn set_field_access_policy(
        &self,
        schema_name: &str,
        field_name: &str,
        policy: serde_json::Value,
    ) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let mut schema = db
            .schema_manager
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| SchemaError::InvalidData(format!("Schema '{schema_name}' not found")))?;

        let field = schema.runtime_fields.get_mut(field_name).ok_or_else(|| {
            SchemaError::InvalidData(format!("Field '{field_name}' not found in '{schema_name}'"))
        })?;

        let parsed: fold_db::access::FieldAccessPolicy = serde_json::from_value(policy)
            .map_err(|e| SchemaError::InvalidData(format!("Invalid access policy: {e}")))?;

        // Set on runtime field (immediate effect)
        field.common_mut().access_policy = Some(parsed.clone());
        // Also persist in field_access_policies (survives restart)
        schema
            .field_access_policies
            .insert(field_name.to_string(), parsed);
        db.schema_manager.update_schema(&schema).await?;
        Ok(())
    }

    /// Get access policies for all fields in a schema.
    pub async fn get_all_field_policies(
        &self,
        schema_name: &str,
    ) -> Result<std::collections::HashMap<String, Option<serde_json::Value>>, SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let schema = db
            .schema_manager
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| SchemaError::InvalidData(format!("Schema '{schema_name}' not found")))?;

        let mut policies = std::collections::HashMap::new();
        for (field_name, field) in &schema.runtime_fields {
            let policy_json = field
                .common()
                .access_policy
                .as_ref()
                .and_then(|p| serde_json::to_value(p).ok());
            policies.insert(field_name.clone(), policy_json);
        }
        Ok(policies)
    }

    /// Get the access policy for a specific field.
    pub async fn get_field_access_policy(
        &self,
        schema_name: &str,
        field_name: &str,
    ) -> Result<Option<serde_json::Value>, SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let schema = db
            .schema_manager
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| SchemaError::InvalidData(format!("Schema '{schema_name}' not found")))?;

        let field = schema.runtime_fields.get(field_name).ok_or_else(|| {
            SchemaError::InvalidData(format!("Field '{field_name}' not found in '{schema_name}'"))
        })?;

        Ok(field
            .common()
            .access_policy
            .as_ref()
            .and_then(|p| serde_json::to_value(p).ok()))
    }

    /// Apply classification-based default access policies to fields.
    /// Uses DataClassification::default_trust_tier() and default_trust_domain() directly.
    /// If `force` is true, overwrites existing policies (reset to defaults).
    pub async fn apply_classification_defaults_with_force(
        &self,
        schema_name: &str,
        force: bool,
    ) -> Result<usize, SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let mut schema = db
            .schema_manager
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| SchemaError::InvalidData(format!("Schema '{schema_name}' not found")))?;

        let schema_domain = schema.trust_domain.clone();
        let mut applied = 0usize;
        let field_names: Vec<String> = schema.runtime_fields.keys().cloned().collect();

        for field_name in &field_names {
            if !force {
                if schema.field_access_policies.contains_key(field_name) {
                    continue;
                }
                let field = schema.runtime_fields.get(field_name).unwrap();
                if field.common().access_policy.is_some() {
                    continue;
                }
            }

            let classification = schema.field_data_classifications.get(field_name);
            let policy = if let Some(cls) = classification {
                fold_db::access::FieldAccessPolicy {
                    trust_domain: cls.default_trust_domain().to_string(),
                    min_read_tier: cls.default_trust_tier(),
                    min_write_tier: TrustTier::Owner,
                    capabilities: vec![],
                }
            } else {
                let domain = schema_domain.as_deref().unwrap_or("personal");
                fold_db::access::FieldAccessPolicy {
                    trust_domain: domain.to_string(),
                    min_read_tier: TrustTier::Inner, // moderate default
                    min_write_tier: TrustTier::Owner,
                    capabilities: vec![],
                }
            };

            if let Some(field_mut) = schema.runtime_fields.get_mut(field_name) {
                field_mut.common_mut().access_policy = Some(policy.clone());
                schema
                    .field_access_policies
                    .insert(field_name.clone(), policy);
                applied += 1;
            }
        }

        if applied > 0 {
            db.schema_manager.update_schema(&schema).await?;
        }

        Ok(applied)
    }

    /// Apply classification defaults without overwriting existing policies.
    pub async fn apply_classification_defaults(
        &self,
        schema_name: &str,
    ) -> Result<usize, SchemaError> {
        self.apply_classification_defaults_with_force(schema_name, false)
            .await
    }

    pub async fn issue_capability(
        &self,
        _schema_name: &str,
        _field_name: &str,
        _constraint: serde_json::Value,
    ) -> Result<(), SchemaError> {
        Err(SchemaError::InvalidData(
            "Not yet implemented in this node".to_string(),
        ))
    }

    pub async fn revoke_capability(
        &self,
        _schema_name: &str,
        _field_name: &str,
        _public_key: &str,
        _kind: serde_json::Value,
    ) -> Result<bool, SchemaError> {
        Err(SchemaError::InvalidData(
            "Not yet implemented in this node".to_string(),
        ))
    }

    pub async fn list_capabilities(
        &self,
        _schema_name: &str,
        _field_name: &str,
    ) -> Result<Vec<serde_json::Value>, SchemaError> {
        Err(SchemaError::InvalidData(
            "Not yet implemented in this node".to_string(),
        ))
    }

    pub async fn set_payment_gate(
        &self,
        _schema_name: &str,
        _gate: serde_json::Value,
    ) -> Result<(), SchemaError> {
        Err(SchemaError::InvalidData(
            "Not yet implemented in this node".to_string(),
        ))
    }

    pub async fn get_payment_gate(
        &self,
        _schema_name: &str,
    ) -> Result<Option<serde_json::Value>, SchemaError> {
        Err(SchemaError::InvalidData(
            "Not yet implemented in this node".to_string(),
        ))
    }

    pub async fn remove_payment_gate(&self, _schema_name: &str) -> Result<bool, SchemaError> {
        Err(SchemaError::InvalidData(
            "Not yet implemented in this node".to_string(),
        ))
    }

    pub async fn get_audit_log(&self, limit: usize) -> Result<serde_json::Value, SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let log = db.db_ops.load_audit_log().await?;
        let recent = log.recent(limit);
        serde_json::to_value(recent)
            .map_err(|e| SchemaError::InvalidData(format!("Failed to serialize audit log: {e}")))
    }
}
