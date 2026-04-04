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

/// Flatten a JSON object's nested objects into dot-notation keys.
///
/// Nested objects like `{"budget": {"flights": 1500, "hotel": 800}}` become
/// `{"budget.flights": 1500, "budget.hotel": 800}`. Non-object values (strings,
/// numbers, booleans, arrays, nulls) are kept as-is. This ensures the flattened
/// keys match the dot-notation paths the AI sees in the structure skeleton.
pub fn flatten_json_object(obj: &serde_json::Map<String, Value>) -> HashMap<String, Value> {
    let mut result = HashMap::new();
    flatten_recursive(obj, "", &mut result);
    result
}

fn flatten_recursive(
    obj: &serde_json::Map<String, Value>,
    prefix: &str,
    out: &mut HashMap<String, Value>,
) {
    for (key, value) in obj {
        let path = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{}.{}", prefix, key)
        };

        match value {
            Value::Object(nested) => {
                flatten_recursive(nested, &path, out);
            }
            _ => {
                out.insert(path, value.clone());
            }
        }
    }
}

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
        // Apply mappers to transform JSON field names to schema field names.
        // First pass: detect collisions (multiple JSON fields → same schema field).
        // Keep the mapping whose JSON key exactly matches the schema field name
        // (identity mapping), since it's almost certainly the correct one.
        let mut result = HashMap::new();
        let mut sources: HashMap<String, String> = HashMap::new(); // schema_field → json_field

        for (json_field, schema_field) in mutation_mappers {
            if let Some(value) = fields_and_values.get(json_field) {
                let field_name = schema_field.rsplit('.').next().unwrap_or(schema_field);

                if let Some(prev_json) = sources.get(field_name) {
                    // Collision: two JSON fields map to the same schema field.
                    // Prefer the identity mapping (json_field == field_name).
                    if json_field == field_name {
                        log_feature!(
                            LogFeature::Ingestion, warn,
                            "Mapper collision: '{}' and '{}' both map to '{}' — keeping identity mapping '{}'",
                            prev_json, json_field, field_name, json_field
                        );
                        result.insert(field_name.to_string(), value.clone());
                        sources.insert(field_name.to_string(), json_field.to_string());
                    } else {
                        log_feature!(
                            LogFeature::Ingestion, warn,
                            "Mapper collision: '{}' and '{}' both map to '{}' — keeping earlier mapping from '{}'",
                            prev_json, json_field, field_name, prev_json
                        );
                        // Don't overwrite — keep the first (or identity) mapping
                    }
                } else {
                    result.insert(field_name.to_string(), value.clone());
                    sources.insert(field_name.to_string(), json_field.to_string());
                }

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
    fn test_flatten_json_object_basic() {
        let obj: serde_json::Map<String, Value> = serde_json::from_value(json!({
            "name": "Vacation",
            "budget_breakdown": {
                "flights": 1500,
                "accommodation": 1500,
                "food": 500
            },
            "travel_dates": {
                "departure": "2026-06-01",
                "return": "2026-06-15"
            },
            "total_budget": 5000
        }))
        .unwrap();

        let flat = flatten_json_object(&obj);

        assert_eq!(flat.get("name"), Some(&json!("Vacation")));
        assert_eq!(flat.get("total_budget"), Some(&json!(5000)));
        assert_eq!(flat.get("budget_breakdown.flights"), Some(&json!(1500)));
        assert_eq!(
            flat.get("budget_breakdown.accommodation"),
            Some(&json!(1500))
        );
        assert_eq!(flat.get("budget_breakdown.food"), Some(&json!(500)));
        assert_eq!(
            flat.get("travel_dates.departure"),
            Some(&json!("2026-06-01"))
        );
        assert_eq!(flat.get("travel_dates.return"), Some(&json!("2026-06-15")));
        // Nested object keys themselves should NOT appear
        assert!(!flat.contains_key("budget_breakdown"));
        assert!(!flat.contains_key("travel_dates"));
    }

    #[test]
    fn test_flatten_deeply_nested() {
        let obj: serde_json::Map<String, Value> = serde_json::from_value(json!({
            "a": { "b": { "c": 42 } }
        }))
        .unwrap();

        let flat = flatten_json_object(&obj);
        assert_eq!(flat.get("a.b.c"), Some(&json!(42)));
        assert_eq!(flat.len(), 1);
    }

    #[test]
    fn test_flatten_preserves_arrays() {
        let obj: serde_json::Map<String, Value> = serde_json::from_value(json!({
            "tags": ["travel", "vacation"],
            "nested": { "items": [1, 2, 3] }
        }))
        .unwrap();

        let flat = flatten_json_object(&obj);
        assert_eq!(flat.get("tags"), Some(&json!(["travel", "vacation"])));
        assert_eq!(flat.get("nested.items"), Some(&json!([1, 2, 3])));
    }

    #[test]
    fn test_generate_mutations_with_nested_objects() {
        let mut keys_and_values = HashMap::new();
        keys_and_values.insert("hash_field".to_string(), "Vacation".to_string());
        keys_and_values.insert("range_field".to_string(), "2026-06-01".to_string());

        // Simulate flattened fields (as generate_mutations_for_item now produces)
        let mut fields_and_values = HashMap::new();
        fields_and_values.insert("name".to_string(), json!("Vacation"));
        fields_and_values.insert("budget_breakdown.flights".to_string(), json!(1500));
        fields_and_values.insert("budget_breakdown.accommodation".to_string(), json!(1500));
        fields_and_values.insert("travel_dates.departure".to_string(), json!("2026-06-01"));
        fields_and_values.insert("travel_dates.return".to_string(), json!("2026-06-15"));

        let mut mappers = HashMap::new();
        mappers.insert("name".to_string(), "TripSchema.name".to_string());
        mappers.insert(
            "budget_breakdown.flights".to_string(),
            "TripSchema.flights_budget".to_string(),
        );
        mappers.insert(
            "budget_breakdown.accommodation".to_string(),
            "TripSchema.accommodation_budget".to_string(),
        );
        mappers.insert(
            "travel_dates.departure".to_string(),
            "TripSchema.departure_date".to_string(),
        );
        mappers.insert(
            "travel_dates.return".to_string(),
            "TripSchema.return_date".to_string(),
        );

        let result = generate_mutations(
            "TripSchema",
            &keys_and_values,
            &fields_and_values,
            &mappers,
            "test-key".to_string(),
            None,
            None,
        )
        .unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].fields_and_values.len(), 5);
        assert_eq!(
            result[0].fields_and_values.get("flights_budget"),
            Some(&json!(1500))
        );
        assert_eq!(
            result[0].fields_and_values.get("departure_date"),
            Some(&json!("2026-06-01"))
        );
    }

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
