use fold_db::schema::SchemaError;

use super::OperationProcessor;

/// Trust and access-control operations.
///
/// The access-control system (trust graph, field policies, capabilities,
/// payment gates, audit log) was removed from fold_db. These methods are
/// retained as stubs so that HTTP routes compile but return clear errors.
impl OperationProcessor {
    pub async fn load_trust_graph(&self) -> Result<serde_json::Value, SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn grant_trust(
        &self,
        _user_public_key: &str,
        _distance: u64,
    ) -> Result<(), SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn revoke_trust(&self, _user_public_key: &str) -> Result<(), SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn set_trust_override(
        &self,
        _user_public_key: &str,
        _distance: u64,
    ) -> Result<(), SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn resolve_trust_distance(
        &self,
        _user_public_key: &str,
    ) -> Result<Option<u64>, SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn list_trust_grants(&self) -> Result<Vec<(String, u64)>, SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn set_field_access_policy(
        &self,
        _schema_name: &str,
        _field_name: &str,
        _policy: serde_json::Value,
    ) -> Result<(), SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn get_all_field_policies(
        &self,
        _schema_name: &str,
    ) -> Result<std::collections::HashMap<String, Option<serde_json::Value>>, SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn get_field_access_policy(
        &self,
        _schema_name: &str,
        _field_name: &str,
    ) -> Result<Option<serde_json::Value>, SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn issue_capability(
        &self,
        _schema_name: &str,
        _field_name: &str,
        _constraint: serde_json::Value,
    ) -> Result<(), SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
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
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn list_capabilities(
        &self,
        _schema_name: &str,
        _field_name: &str,
    ) -> Result<Vec<serde_json::Value>, SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn set_payment_gate(
        &self,
        _schema_name: &str,
        _gate: serde_json::Value,
    ) -> Result<(), SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn get_payment_gate(
        &self,
        _schema_name: &str,
    ) -> Result<Option<serde_json::Value>, SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn remove_payment_gate(&self, _schema_name: &str) -> Result<bool, SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }

    pub async fn get_audit_log(&self, _limit: usize) -> Result<serde_json::Value, SchemaError> {
        Err(SchemaError::InvalidData(
            "Access control has been removed from fold_db".to_string(),
        ))
    }
}
