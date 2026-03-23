use fold_db::access::{
    AuditAction, AuditEvent, AuditLog, CapabilityConstraint, CapabilityKind, FieldAccessPolicy,
    PaymentGate, TrustGraph,
};
use fold_db::schema::types::field::Field;
use fold_db::schema::SchemaError;

use super::OperationProcessor;

impl OperationProcessor {
    /// Load the trust graph from storage.
    pub async fn load_trust_graph(&self) -> Result<TrustGraph, SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;
        db.db_ops.load_trust_graph().await
    }

    /// Grant trust: assign a trust distance from the node owner to a public key.
    pub async fn grant_trust(
        &self,
        user_public_key: &str,
        distance: u64,
    ) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;
        let owner_key = self.get_node_public_key();

        let mut graph = db.db_ops.load_trust_graph().await?;
        graph.assign_trust(&owner_key, user_public_key, distance);
        db.db_ops.store_trust_graph(&graph).await?;

        // Audit
        let event = AuditEvent::trust_event(
            &owner_key,
            AuditAction::TrustGrant {
                user_id: user_public_key.to_string(),
                distance,
            },
        );
        db.db_ops.append_audit_event(event).await?;

        Ok(())
    }

    /// Revoke trust for a public key.
    pub async fn revoke_trust(&self, user_public_key: &str) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;
        let owner_key = self.get_node_public_key();

        let mut graph = db.db_ops.load_trust_graph().await?;
        graph.revoke_trust(&owner_key, user_public_key);
        db.db_ops.store_trust_graph(&graph).await?;

        // Audit
        let event = AuditEvent::trust_event(
            &owner_key,
            AuditAction::TrustRevoke {
                user_id: user_public_key.to_string(),
            },
        );
        db.db_ops.append_audit_event(event).await?;

        Ok(())
    }

    /// Set an explicit distance override for a public key.
    pub async fn set_trust_override(
        &self,
        user_public_key: &str,
        distance: u64,
    ) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;
        let owner_key = self.get_node_public_key();

        let mut graph = db.db_ops.load_trust_graph().await?;
        graph.set_override(&owner_key, user_public_key, distance);
        db.db_ops.store_trust_graph(&graph).await?;

        Ok(())
    }

    /// Resolve the trust distance for a given public key.
    pub async fn resolve_trust_distance(
        &self,
        user_public_key: &str,
    ) -> Result<Option<u64>, SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;
        let owner_key = self.get_node_public_key();

        let graph = db.db_ops.load_trust_graph().await?;
        Ok(graph.resolve(user_public_key, &owner_key))
    }

    /// List all trust assignments from this node's owner.
    pub async fn list_trust_grants(&self) -> Result<Vec<(String, u64)>, SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;
        let owner_key = self.get_node_public_key();

        let graph = db.db_ops.load_trust_graph().await?;
        Ok(graph.assignments_from(&owner_key))
    }

    /// Set the access policy on a specific field of a schema.
    pub async fn set_field_access_policy(
        &self,
        schema_name: &str,
        field_name: &str,
        policy: FieldAccessPolicy,
    ) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;

        let mut schema = db
            .schema_manager
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| {
                SchemaError::NotFound(format!("Schema '{}' not found", schema_name))
            })?;

        let field_variant = schema.runtime_fields.get_mut(field_name).ok_or_else(|| {
            SchemaError::InvalidField(format!(
                "Field '{}' not found in schema '{}'",
                field_name, schema_name
            ))
        })?;

        field_variant.common_mut().access_policy = Some(policy);

        // Persist and reload
        db.db_ops.store_schema(schema_name, &schema).await?;
        db.schema_manager.load_schema_internal(schema).await?;

        Ok(())
    }

    /// Get the access policy for a specific field.
    pub async fn get_field_access_policy(
        &self,
        schema_name: &str,
        field_name: &str,
    ) -> Result<Option<FieldAccessPolicy>, SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;

        let schema = db
            .schema_manager
            .get_schema(schema_name)
            .await?
            .ok_or_else(|| {
                SchemaError::NotFound(format!("Schema '{}' not found", schema_name))
            })?;

        let field_variant = schema.runtime_fields.get(field_name).ok_or_else(|| {
            SchemaError::InvalidField(format!(
                "Field '{}' not found in schema '{}'",
                field_name, schema_name
            ))
        })?;

        Ok(field_variant.common().access_policy.clone())
    }

    // ===== Capability token management =====

    /// Issue a capability token for a field.
    pub async fn issue_capability(
        &self,
        schema_name: &str,
        field_name: &str,
        constraint: CapabilityConstraint,
    ) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;
        db.db_ops
            .store_capability(schema_name, field_name, &constraint)
            .await
    }

    /// Revoke a capability token.
    pub async fn revoke_capability(
        &self,
        schema_name: &str,
        field_name: &str,
        public_key: &str,
        kind: CapabilityKind,
    ) -> Result<bool, SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;
        db.db_ops
            .delete_capability(schema_name, field_name, public_key, kind)
            .await
    }

    /// List all capability tokens for a schema field.
    pub async fn list_capabilities(
        &self,
        schema_name: &str,
        field_name: &str,
    ) -> Result<Vec<CapabilityConstraint>, SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;
        db.db_ops
            .load_capabilities(schema_name, field_name)
            .await
    }

    // ===== Payment gate management =====

    /// Set a payment gate on a schema.
    pub async fn set_payment_gate(
        &self,
        schema_name: &str,
        gate: PaymentGate,
    ) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;
        db.db_ops.store_payment_gate(schema_name, &gate).await
    }

    /// Get the payment gate for a schema.
    pub async fn get_payment_gate(
        &self,
        schema_name: &str,
    ) -> Result<Option<PaymentGate>, SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;
        db.db_ops.load_payment_gate(schema_name).await
    }

    /// Remove the payment gate from a schema.
    pub async fn remove_payment_gate(&self, schema_name: &str) -> Result<bool, SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;
        db.db_ops.delete_payment_gate(schema_name).await
    }

    /// Get recent audit log events.
    pub async fn get_audit_log(&self, limit: usize) -> Result<AuditLog, SchemaError> {
        let db = self.get_db().await.map_err(|e| {
            SchemaError::InvalidData(format!("Failed to get database: {}", e))
        })?;
        let log = db.db_ops.load_audit_log().await?;
        // Return only the recent events if limit is specified
        if limit > 0 && limit < log.total_events() {
            let recent = log.recent(limit).to_vec();
            let mut trimmed = AuditLog::new();
            for event in recent {
                trimmed.record(event);
            }
            return Ok(trimmed);
        }
        Ok(log)
    }
}
