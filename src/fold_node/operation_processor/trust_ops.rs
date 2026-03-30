use fold_db::access::{AuditAction, AuditEvent};
use fold_db::schema::SchemaError;

use super::OperationProcessor;

/// Helper to convert FoldDbError to SchemaError for trust operation return types.
fn to_schema_err(e: fold_db::error::FoldDbError) -> SchemaError {
    SchemaError::InvalidData(e.to_string())
}

/// Trust and access-control operations.
///
/// Trust graph and audit log operations are backed by fold_db's access module.
/// Field policies, capabilities, and payment gates are not yet implemented.
impl OperationProcessor {
    pub async fn load_trust_graph(&self) -> Result<serde_json::Value, SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let graph = db.db_ops.load_trust_graph().await?;
        serde_json::to_value(&graph).map_err(|e| {
            SchemaError::InvalidData(format!("Failed to serialize trust graph: {e}"))
        })
    }

    pub async fn grant_trust(
        &self,
        user_public_key: &str,
        distance: u64,
    ) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let owner = self.node.get_node_public_key().to_string();
        let mut graph = db.db_ops.load_trust_graph().await?;
        graph.assign_trust(&owner, user_public_key, distance);
        db.db_ops.store_trust_graph(&graph).await?;
        let event = AuditEvent::trust_event(
            user_public_key,
            AuditAction::TrustGrant {
                user_id: user_public_key.to_string(),
                distance,
            },
        );
        db.db_ops.append_audit_event(event).await?;
        Ok(())
    }

    pub async fn revoke_trust(&self, user_public_key: &str) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let owner = self.node.get_node_public_key().to_string();
        let mut graph = db.db_ops.load_trust_graph().await?;
        graph.revoke_trust(&owner, user_public_key);
        db.db_ops.store_trust_graph(&graph).await?;
        let event = AuditEvent::trust_event(
            user_public_key,
            AuditAction::TrustRevoke {
                user_id: user_public_key.to_string(),
            },
        );
        db.db_ops.append_audit_event(event).await?;
        Ok(())
    }

    pub async fn set_trust_override(
        &self,
        user_public_key: &str,
        distance: u64,
    ) -> Result<(), SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let owner = self.node.get_node_public_key().to_string();
        let mut graph = db.db_ops.load_trust_graph().await?;
        graph.set_override(&owner, user_public_key, distance);
        db.db_ops.store_trust_graph(&graph).await?;
        Ok(())
    }

    pub async fn resolve_trust_distance(
        &self,
        user_public_key: &str,
    ) -> Result<Option<u64>, SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let owner = self.node.get_node_public_key().to_string();
        let graph = db.db_ops.load_trust_graph().await?;
        Ok(graph.resolve(user_public_key, &owner))
    }

    pub async fn list_trust_grants(&self) -> Result<Vec<(String, u64)>, SchemaError> {
        let db = self.get_db().await.map_err(to_schema_err)?;
        let owner = self.node.get_node_public_key().to_string();
        let graph = db.db_ops.load_trust_graph().await?;
        Ok(graph.assignments_from(&owner))
    }

    pub async fn set_field_access_policy(
        &self,
        _schema_name: &str,
        _field_name: &str,
        _policy: serde_json::Value,
    ) -> Result<(), SchemaError> {
        Err(SchemaError::InvalidData(
            "Not yet implemented in this node".to_string(),
        ))
    }

    pub async fn get_all_field_policies(
        &self,
        _schema_name: &str,
    ) -> Result<std::collections::HashMap<String, Option<serde_json::Value>>, SchemaError> {
        Err(SchemaError::InvalidData(
            "Not yet implemented in this node".to_string(),
        ))
    }

    pub async fn get_field_access_policy(
        &self,
        _schema_name: &str,
        _field_name: &str,
    ) -> Result<Option<serde_json::Value>, SchemaError> {
        Err(SchemaError::InvalidData(
            "Not yet implemented in this node".to_string(),
        ))
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
        serde_json::to_value(recent).map_err(|e| {
            SchemaError::InvalidData(format!("Failed to serialize audit log: {e}"))
        })
    }
}
