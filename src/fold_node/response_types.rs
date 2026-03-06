//! Type aliases for common data structures

use fold_db::schema::types::field::FieldValue;
use fold_db::schema::types::KeyValue;
use std::collections::HashMap;

/// Type alias for query result maps (field -> key -> value)
pub type QueryResultMap = HashMap<String, HashMap<KeyValue, FieldValue>>;
