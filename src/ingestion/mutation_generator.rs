//! Mutation generator for creating mutations from AI responses and JSON data

use crate::ingestion::IngestionResult;
use chrono::Utc;
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use fold_db::schema::types::{KeyValue, Mutation};
use fold_db::MutationType;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Timestamp format for fallback range keys — matches the normalized date
/// format used by `key_extraction::try_normalize_date`.
const RANGE_KEY_TIMESTAMP_FMT: &str = "%Y-%m-%d %H:%M:%S";

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
                let field_name = schema_field.rsplit('.').next().unwrap_or(schema_field);

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

        // If neither hash nor range key was extracted, the AI's key proposal
        // didn't match the actual data (common with decomposed array items where
        // the parent's key field isn't carried into each child record). Fall back
        // to a deterministic content hash so the record is still ingested and
        // deduplication works based on field values.
        let key_value = if key_value.hash.is_none() && key_value.range.is_none() {
            let mut sorted_keys: Vec<&String> = mapped_fields.keys().collect();
            sorted_keys.sort();
            let mut hasher = Sha256::new();
            hasher.update(schema_name.as_bytes());
            for k in &sorted_keys {
                hasher.update(k.as_bytes());
                hasher.update(mapped_fields[*k].to_string().as_bytes());
            }
            let content_hash = format!("{:x}", hasher.finalize());
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Key fields not found in data for schema '{}', using content hash '{}' as key",
                schema_name,
                &content_hash[..12]
            );
            // HashRange schemas require both hash and range — use a timestamp so
            // records written with the same content hash are still distinguishable.
            KeyValue::new(
                Some(content_hash),
                Some(Utc::now().format(RANGE_KEY_TIMESTAMP_FMT).to_string()),
            )
        } else {
            key_value
        };

        // If hash was extracted but range is missing (e.g., null range field),
        // provide a timestamp fallback so HashRange mutations are still indexed.
        let key_value = if key_value.hash.is_some() && key_value.range.is_none() {
            let ts = Utc::now().format(RANGE_KEY_TIMESTAMP_FMT).to_string();
            log_feature!(
                LogFeature::Ingestion,
                warn,
                "Range key missing for schema '{}', using timestamp '{}' as fallback",
                schema_name,
                ts
            );
            KeyValue::new(key_value.hash, Some(ts))
        } else {
            key_value
        };

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
    fn test_generate_mutations_falls_back_to_content_hash_when_keys_missing() {
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
        )
        .expect("should succeed with content hash fallback");

        assert_eq!(result.len(), 1);
        assert!(
            result[0].key_value.hash.is_some(),
            "Should have a content-hash key"
        );
        assert!(
            result[0].key_value.range.is_some(),
            "Should have a timestamp range key when both keys are missing"
        );
        assert_eq!(result[0].fields_and_values.len(), 2);
    }

    #[test]
    fn test_generate_mutations_provides_range_fallback_when_only_range_missing() {
        // Simulates the PDF bug: hash extracted from data, but range field was null
        let mut keys_and_values = HashMap::new();
        keys_and_values.insert("hash_field".to_string(), "doc-title-hash".to_string());
        // range_field intentionally absent — mimics null `created` date from PDF

        let mut fields_and_values = HashMap::new();
        fields_and_values.insert("title".to_string(), json!("My Document"));
        fields_and_values.insert("content".to_string(), json!("Some content"));

        let mut mappers = HashMap::new();
        mappers.insert("title".to_string(), "DocSchema.title".to_string());
        mappers.insert("content".to_string(), "DocSchema.content".to_string());

        let result = generate_mutations(
            "DocSchema",
            &keys_and_values,
            &fields_and_values,
            &mappers,
            "test-key".to_string(),
            None,
            None,
        )
        .expect("should succeed with range key fallback");

        assert_eq!(result.len(), 1);
        assert_eq!(
            result[0].key_value.hash.as_deref(),
            Some("doc-title-hash"),
            "Hash key should be preserved from input"
        );
        assert!(
            result[0].key_value.range.is_some(),
            "Should have a timestamp fallback range key"
        );
        // Verify the fallback range is a valid timestamp in our expected format
        let range = result[0].key_value.range.as_ref().unwrap();
        chrono::NaiveDateTime::parse_from_str(range, RANGE_KEY_TIMESTAMP_FMT)
            .expect("Fallback range key should be a valid timestamp");
    }
}
