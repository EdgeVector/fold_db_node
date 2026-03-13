//! Mutation generator for creating mutations from AI responses and JSON data

use crate::ingestion::IngestionResult;
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::schema::types::{KeyValue, Mutation};
use fold_db::MutationType;
use serde_json::Value;
use std::collections::HashMap;

/// Generate mutations from JSON data and mutation mappers
pub fn generate_mutations(
    schema_name: &str,
    keys_and_values: &HashMap<String, String>,
    fields_and_values: &HashMap<String, Value>,
    mutation_mappers: &HashMap<String, String>,
    pub_key: String,
    source_file_name: Option<String>,
    metadata: Option<HashMap<String, String>>,
) -> IngestionResult<Vec<Mutation>> {
    log_feature!(
        LogFeature::Ingestion,
        info,
        "Generating mutations for schema '{}' with {} mappers, {} input fields",
        schema_name,
        mutation_mappers.len(),
        fields_and_values.len()
    );

    let mut mutations = Vec::new();

    // Apply mutation mappers to transform JSON fields to schema fields
    let mapped_fields = if mutation_mappers.is_empty() {
        // If no mappers provided, use fields as-is (backward compatibility)
        log_feature!(
            LogFeature::Ingestion,
            info,
            "No mutation mappers provided, using all {} fields directly",
            fields_and_values.len()
        );
        fields_and_values.clone()
    } else {
        // Apply mappers to transform JSON field names to schema field names
        let mut result = HashMap::new();
        for (json_field, schema_field) in mutation_mappers {
            if let Some(value) = fields_and_values.get(json_field) {
                // Extract just the field name from schema path (e.g., "UserSchema.name" -> "name")
                let field_name = if schema_field.contains('.') {
                    schema_field.rsplit('.').next().unwrap_or(schema_field)
                } else {
                    schema_field.as_str()
                };

                result.insert(field_name.to_string(), value.clone());
                log_feature!(
                    LogFeature::Ingestion,
                    debug,
                    "Mapped field: {} -> {}",
                    json_field,
                    field_name
                );
            } else {
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
                    "Mapper references missing JSON field: {}",
                    json_field
                );
            }
        }

        log_feature!(
            LogFeature::Ingestion,
            info,
            "Applied mutation mappers: {} JSON fields -> {} schema fields",
            fields_and_values.len(),
            result.len()
        );
        result
    };

    // If we have fields to mutate, create a mutation
    if !mapped_fields.is_empty() {
        // Build KeyValue from keys
        let key_value = KeyValue::new(
            keys_and_values.get("hash_field").cloned(),
            keys_and_values.get("range_field").cloned(),
        );

        // Key fields MUST be present in the data. If neither hash nor range key
        // was extracted, the schema's key config doesn't match the actual data —
        // this is a bug in the AI schema proposal or the key extraction logic.
        // Fail hard instead of silently creating records with content hashes as keys.
        if key_value.hash.is_none() && key_value.range.is_none() {
            return Err(crate::ingestion::IngestionError::SchemaCreationError(format!(
                "Key fields not found in data for schema '{}'. \
                 The schema's key configuration does not match the ingested data. \
                 Mapped fields: {:?}",
                schema_name,
                mapped_fields.keys().collect::<Vec<_>>()
            )));
        }

        let mut mutation = Mutation::new(
            schema_name.to_string(),
            mapped_fields,
            key_value,
            pub_key,
            MutationType::Create,
        );

        if let Some(filename) = source_file_name {
            mutation = mutation.with_source_file_name(filename);
        }

        if let Some(meta) = metadata {
            mutation = mutation.with_metadata(meta);
        }

        mutations.push(mutation);
        log_feature!(
            LogFeature::Ingestion,
            info,
            "Created mutation with {} fields",
            mutations[0].fields_and_values.len()
        );
    } else {
        log_feature!(
            LogFeature::Ingestion,
            warn,
            "No valid field mappings found, no mutations generated"
        );
    }

    Ok(mutations)
}


#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_generate_mutations() {
        let mut keys_and_values = HashMap::new();
        keys_and_values.insert("hash_field".to_string(), "hash_key".to_string());
        keys_and_values.insert("range_field".to_string(), "range_key".to_string());

        let mut fields_and_values = HashMap::new();
        fields_and_values.insert("name".to_string(), json!("John"));
        fields_and_values.insert("age".to_string(), json!(30));

        let mut mappers = HashMap::new();
        mappers.insert("name".to_string(), "UserSchema.name".to_string());
        mappers.insert("age".to_string(), "UserSchema.age".to_string());

        let result = generate_mutations(
            "UserSchema",
            &keys_and_values,
            &fields_and_values,
            &mappers,
            "test-key".to_string(),
            None,
            None,
        )
        .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].fields_and_values.len(), 2);
    }

    #[test]
    fn test_generate_mutations_errors_when_keys_missing() {
        // Empty keys_and_values simulates missing key fields in data
        let keys_and_values = HashMap::new();

        let mut fields_and_values = HashMap::new();
        fields_and_values.insert("name".to_string(), json!("Alice"));
        fields_and_values.insert("phone".to_string(), json!("555-0101"));

        let mut mappers = HashMap::new();
        mappers.insert("name".to_string(), "ContactSchema.name".to_string());
        mappers.insert("phone".to_string(), "ContactSchema.phone".to_string());

        let result = generate_mutations(
            "ContactSchema",
            &keys_and_values,
            &fields_and_values,
            &mappers,
            "test-key".to_string(),
            None,
            None,
        );

        assert!(
            result.is_err(),
            "Should error when key fields are missing from data"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("Key fields not found"),
            "Error should mention missing key fields, got: {}",
            err
        );
    }
}
