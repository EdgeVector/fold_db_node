//! Validates that serialized handler responses match their OpenAPI schema.
//!
//! The drift check in CI (`.github/workflows/ci-tests.yml`) verifies that
//! the spec and the regenerated `openapi.ts` are in sync. That catches
//! drift between Rust source and the spec, but does NOT catch divergence
//! between the spec and the actual JSON wire shape: utoipa's `ToSchema`
//! derive can produce a schema that differs from what `serde` actually
//! emits at runtime, especially when `#[serde(rename)]`,
//! `#[serde(flatten)]`, `#[serde(skip_serializing_if = ...)]`, or custom
//! serializers come into play.
//!
//! These tests close that loop: for each registered type, build a real
//! instance, serialize it via `serde`, and validate the resulting JSON
//! against the OpenAPI-declared schema using a JSON Schema validator.
//! If they disagree, the test fails — exactly the contract the rest of
//! the API typegen unification effort assumes.
//!
//! Adding a new type:
//! 1. Construct a representative `&T` via the test helper.
//! 2. Add a `#[test]` calling `assert_roundtrip::<T>(name, &instance)`.
//! 3. The `name` is the OpenAPI component name (matches the type's simple
//!    name unless `#[schema(as = "...")]` overrides it).

use jsonschema::Validator;
use serde::Serialize;
use serde_json::Value;

/// Parses the OpenAPI spec once and resolves the JSON Schema for the
/// named component. Panics with a clear message if the component is
/// missing — that's the most likely failure mode and worth surfacing
/// loudly.
///
/// utoipa emits component refs as `#/components/schemas/Foo`. To make
/// those resolve under a JSON Schema validator, we rewrite each ref to
/// `#/$defs/Foo` and copy the components/schemas map into a top-level
/// `$defs` on the chosen schema. This sidesteps the validator's external
/// reference loader (which would otherwise try to fetch the full OpenAPI
/// document from a URL).
fn schema_for(component_name: &str) -> Validator {
    let openapi_json = fold_db_node::server::openapi::build_openapi();
    let openapi: Value =
        serde_json::from_str(&openapi_json).expect("build_openapi() did not produce valid JSON");

    let schemas = openapi
        .pointer("/components/schemas")
        .and_then(Value::as_object)
        .expect("openapi.json missing /components/schemas")
        .clone();

    let mut schema = schemas
        .get(component_name)
        .unwrap_or_else(|| {
            panic!(
                "component {component_name} not registered in openapi.rs's \
                 components(schemas(...)) macro arg",
            )
        })
        .clone();

    rewrite_refs(&mut schema);
    let mut defs_map = serde_json::Map::new();
    for (name, mut sub_schema) in schemas {
        rewrite_refs(&mut sub_schema);
        defs_map.insert(name, sub_schema);
    }
    if let Some(obj) = schema.as_object_mut() {
        obj.insert("$defs".to_string(), Value::Object(defs_map));
    }

    Validator::new(&schema).expect("compile JSON Schema")
}

/// Walks `value` and rewrites every `"$ref": "#/components/schemas/X"`
/// to `"$ref": "#/$defs/X"`.
fn rewrite_refs(value: &mut Value) {
    match value {
        Value::Object(map) => {
            if let Some(Value::String(s)) = map.get_mut("$ref") {
                if let Some(rest) = s.strip_prefix("#/components/schemas/") {
                    *s = format!("#/$defs/{rest}");
                }
            }
            for v in map.values_mut() {
                rewrite_refs(v);
            }
        }
        Value::Array(items) => {
            for v in items.iter_mut() {
                rewrite_refs(v);
            }
        }
        _ => {}
    }
}

/// Asserts a value's serialized JSON matches the OpenAPI schema declared
/// for `component_name`. Used by every per-type test below.
fn assert_roundtrip<T: Serialize>(component_name: &str, value: &T) {
    let json = serde_json::to_value(value).expect("serialize");
    let validator = schema_for(component_name);
    if let Err(err) = validator.validate(&json) {
        panic!(
            "JSON for {component_name} does not match OpenAPI schema:\n\
             value: {json:#}\n\
             error: {err}",
        );
    }
}

// ────────────────────────────────────────────────────────────────────────
// Per-type tests. Bounded to a few exemplar types for the initial
// scaffold. Phase 4 will add coverage as more types migrate to the spec
// as source-of-truth.
// ────────────────────────────────────────────────────────────────────────

#[test]
fn process_json_response_matches_schema() {
    let instance = fold_db_node::handlers::ingestion::ProcessJsonResponse {
        success: true,
        progress_id: "p-123".to_string(),
        message: "ingestion accepted".to_string(),
    };
    assert_roundtrip("ProcessJsonResponse", &instance);
}

#[test]
fn admin_job_response_matches_schema_started() {
    let instance = fold_db_node::server::routes::admin::AdminJobResponse {
        success: true,
        message: "reset started".to_string(),
        job_id: Some("job-abc".to_string()),
    };
    assert_roundtrip("AdminJobResponse", &instance);
}

#[test]
fn admin_job_response_matches_schema_error() {
    let instance = fold_db_node::server::routes::admin::AdminJobResponse {
        success: false,
        message: "missing api_url".to_string(),
        job_id: None,
    };
    assert_roundtrip("AdminJobResponse", &instance);
}

#[test]
fn node_key_response_matches_schema() {
    let instance = fold_db_node::handlers::system::NodeKeyResponse {
        success: true,
        public_key: "deadbeef".to_string(),
        message: "Node public key retrieved".to_string(),
    };
    assert_roundtrip("NodeKeyResponse", &instance);
}

/// Sanity check: confirms the validator catches mismatch. If the
/// `NodeKeyResponse` schema declares `success: bool` but we feed it a
/// string, validation MUST fail. If this test ever passes, the
/// validator is broken (false-negative) and the rest of the file gives
/// false confidence.
#[test]
fn validator_rejects_known_mismatch() {
    let bogus = serde_json::json!({
        "success": "yes please",
        "public_key": "deadbeef",
        "message": "ok",
    });
    let validator = schema_for("NodeKeyResponse");
    assert!(
        validator.validate(&bogus).is_err(),
        "validator should reject a string in `success: bool`",
    );
}
