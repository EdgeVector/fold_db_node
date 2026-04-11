//! Structure analysis utilities for JSON ingestion
//!
//! This module provides utilities to analyze JSON structure and create supersets
//! that capture all possible fields across multiple objects.

use serde_json::Value;

/// Analyzes JSON structure and creates a superset representation
/// that includes all fields found across all top-level elements
pub struct StructureAnalyzer;

impl StructureAnalyzer {
    /// Extract a minimal structure skeleton from JSON data.
    ///
    /// Produces flattened dot-separated paths with all values replaced by `"..."`.
    /// Arrays use `[]` notation and only the first element is examined.
    /// Includes `_meta` with array length when the top-level value is an array.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use serde_json::json;
    /// use fold_db_node::ingestion::structure_analyzer::StructureAnalyzer;
    ///
    /// let data = json!([
    ///     {"name": "Alice", "profile": {"age": 30}},
    ///     {"name": "Bob", "profile": {"email": "bob@example.com"}}
    /// ]);
    ///
    /// let skeleton = StructureAnalyzer::extract_structure_skeleton(&data);
    /// // Result: {"_meta": "array(2 items)", "name": "...", "profile.age": "...", "profile.email": "..."}
    /// ```
    pub fn extract_structure_skeleton(json_data: &Value) -> Value {
        let mut fields = serde_json::Map::new();

        match json_data {
            Value::Array(array) => {
                fields.insert(
                    "_meta".to_string(),
                    Value::String(format!("array({} items)", array.len())),
                );
                if array.is_empty() {
                    return Value::Object(fields);
                }
                // Merge keys from all elements to capture the superset of fields,
                // but only recurse into the first element's nested structures.
                let mut seen_paths: std::collections::HashSet<String> =
                    std::collections::HashSet::new();
                for item in array {
                    if let Some(obj) = item.as_object() {
                        Self::skeleton_object(obj, &mut fields, "", &mut seen_paths);
                    }
                }
            }
            Value::Object(obj) => {
                let mut seen_paths = std::collections::HashSet::new();
                Self::skeleton_object(obj, &mut fields, "", &mut seen_paths);
            }
            other => {
                fields.insert(
                    "value".to_string(),
                    Value::String(Self::json_type_name(other).to_string()),
                );
            }
        }

        Value::Object(fields)
    }

    /// Return the JSON type name for a value (e.g. "string", "number", "boolean", "null").
    fn json_type_name(value: &Value) -> &'static str {
        match value {
            Value::String(_) => "string",
            Value::Number(_) => "number",
            Value::Bool(_) => "boolean",
            Value::Null => "null",
            Value::Object(_) => "object",
            Value::Array(_) => "array",
        }
    }

    /// Recursively flatten an object into dot-separated skeleton paths.
    /// `seen_paths` tracks which paths have already been added so duplicate
    /// array elements don't cause repeated recursion into nested structures.
    fn skeleton_object(
        obj: &serde_json::Map<String, Value>,
        fields: &mut serde_json::Map<String, Value>,
        prefix: &str,
        seen_paths: &mut std::collections::HashSet<String>,
    ) {
        for (key, value) in obj {
            let path = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{}.{}", prefix, key)
            };

            match value {
                Value::Object(nested) => {
                    Self::skeleton_object(nested, fields, &path, seen_paths);
                }
                Value::Array(arr) => {
                    let arr_path = format!("{}[]", path);
                    if arr.is_empty() {
                        fields.insert(arr_path, Value::String("[]".to_string()));
                    } else if arr[0].is_object() {
                        // Only recurse into the first element of object arrays
                        if seen_paths.insert(arr_path.clone()) {
                            if let Some(inner_obj) = arr[0].as_object() {
                                Self::skeleton_object(inner_obj, fields, &arr_path, seen_paths);
                            }
                        }
                    } else {
                        // Primitive array — emit element type, e.g. "[string]", "[number]"
                        let elem_type = Self::json_type_name(&arr[0]);
                        fields.insert(arr_path, Value::String(format!("[{}]", elem_type)));
                    }
                }
                _ => {
                    if !fields.contains_key(&path) {
                        fields.insert(path, Value::String(Self::json_type_name(value).to_string()));
                    }
                }
            }
        }
    }

    /// Get lightweight statistics about the structure analysis.
    ///
    /// Returns the number of top-level elements and unique field names.
    /// Only computes what callers actually need (element count + unique fields).
    pub fn get_analysis_stats(json_data: &Value) -> StructureStats {
        match json_data {
            Value::Array(array) => {
                let mut unique: std::collections::HashSet<&str> =
                    std::collections::HashSet::new();
                for item in array {
                    if let Some(obj) = item.as_object() {
                        for key in obj.keys() {
                            unique.insert(key.as_str());
                        }
                    }
                }
                StructureStats {
                    total_elements: array.len(),
                    unique_fields: unique.len(),
                }
            }
            Value::Object(obj) => StructureStats {
                total_elements: 1,
                unique_fields: obj.len(),
            },
            _ => StructureStats {
                total_elements: 1,
                unique_fields: 1,
            },
        }
    }
}

/// Statistics about structure analysis
#[derive(Debug, Clone)]
pub struct StructureStats {
    /// Total number of elements analyzed
    pub total_elements: usize,
    /// Number of unique fields found
    pub unique_fields: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_skeleton_array_of_objects() {
        let data = json!([
            {"name": "Alice", "age": 30},
            {"name": "Bob", "email": "bob@example.com"},
            {"name": "Charlie", "age": 25, "email": "charlie@example.com"}
        ]);

        let skeleton = StructureAnalyzer::extract_structure_skeleton(&data);
        let obj = skeleton.as_object().unwrap();

        assert_eq!(obj["_meta"], "array(3 items)");
        assert_eq!(obj["name"], "string");
        assert_eq!(obj["age"], "number");
        assert_eq!(obj["email"], "string");
        // No actual values should appear
        assert!(!obj.values().any(|v| v == "Alice" || v == "30"));
    }

    #[test]
    fn test_skeleton_single_object() {
        let data = json!({"name": "Alice", "age": 30, "active": true});

        let skeleton = StructureAnalyzer::extract_structure_skeleton(&data);
        let obj = skeleton.as_object().unwrap();

        assert!(!obj.contains_key("_meta"));
        assert_eq!(obj["name"], "string");
        assert_eq!(obj["age"], "number");
        assert_eq!(obj["active"], "boolean");
    }

    #[test]
    fn test_skeleton_nested_objects() {
        let data = json!([
            {"name": "Alice", "profile": {"age": 30, "dept": "Eng"}},
            {"name": "Bob", "profile": {"email": "bob@test.com"}}
        ]);

        let skeleton = StructureAnalyzer::extract_structure_skeleton(&data);
        let obj = skeleton.as_object().unwrap();

        assert_eq!(obj["_meta"], "array(2 items)");
        assert_eq!(obj["name"], "string");
        assert_eq!(obj["profile.age"], "number");
        assert_eq!(obj["profile.dept"], "string");
        assert_eq!(obj["profile.email"], "string");
    }

    #[test]
    fn test_skeleton_nested_arrays() {
        let data = json!({
            "id": 1,
            "messages": [
                {"text": "hello", "sender": "alice"},
                {"text": "world", "sender": "bob"}
            ]
        });

        let skeleton = StructureAnalyzer::extract_structure_skeleton(&data);
        let obj = skeleton.as_object().unwrap();

        assert_eq!(obj["id"], "number");
        assert_eq!(obj["messages[].text"], "string");
        assert_eq!(obj["messages[].sender"], "string");
    }

    #[test]
    fn test_skeleton_empty_array() {
        let data = json!([]);

        let skeleton = StructureAnalyzer::extract_structure_skeleton(&data);
        let obj = skeleton.as_object().unwrap();

        assert_eq!(obj["_meta"], "array(0 items)");
        assert_eq!(obj.len(), 1);
    }

    #[test]
    fn test_skeleton_primitive_arrays() {
        let data = json!({"tags": ["a", "b", "c"], "scores": [1, 2, 3]});

        let skeleton = StructureAnalyzer::extract_structure_skeleton(&data);
        let obj = skeleton.as_object().unwrap();

        assert_eq!(obj["tags[]"], "[string]");
        assert_eq!(obj["scores[]"], "[number]");
    }
}
