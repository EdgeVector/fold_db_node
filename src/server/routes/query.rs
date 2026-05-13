use crate::fold_node::OperationProcessor;
use crate::handlers::query as query_handlers;
// Imported so the bare `QueryResponse` token in the `#[utoipa::path]`
// annotation resolves to the same type registered via `components(schemas(...))`.
#[allow(unused_imports)]
use crate::handlers::query::QueryResponse;
use crate::handlers::schema_resolution::{resolve_schema_name, SchemaResolution};
use crate::server::http_server::AppState;
use crate::server::routes::{
    handler_error_to_response, handler_result_to_response, node_or_return,
};
use actix_web::{web, HttpResponse, Responder};
use fold_db::schema::types::operations::{Operation, Query};
use fold_db::schema::types::Schema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Run the descriptive_name resolver and convert an `Ambiguous` outcome into
/// a 409 [`HttpResponse`] with a structured body listing every candidate
/// canonical hash, so the caller can pin a future query to one of them.
/// `Canonical` returns the canonical name (or the input verbatim when no
/// match was found — downstream then emits its own "not found" error,
/// preserving prior behaviour).
///
/// Both `execute_query` and `execute_mutation` need this exact mapping; the
/// helper exists to keep the body shape and status code in lockstep between
/// the two routes.
async fn resolve_or_conflict_response(
    processor: &OperationProcessor,
    requested: &str,
) -> Result<String, HttpResponse> {
    match resolve_schema_name(processor, requested).await {
        Ok(SchemaResolution::Canonical(name)) => Ok(name),
        Ok(SchemaResolution::Ambiguous { input, candidates }) => {
            tracing::info!(
                target: "fold_node::http_server",
                schema_name = %input,
                candidates = ?candidates,
                "rejecting ambiguous descriptive_name with 409"
            );
            Err(HttpResponse::Conflict().json(json!({
                "ok": false,
                "error": "ambiguous_schema_name",
                "message": format!(
                    "descriptive_name '{}' matches {} approved schemas; pin by canonical hash",
                    input,
                    candidates.len(),
                ),
                "schema_name": input,
                "ambiguous_schemas": candidates,
            })))
        }
        Err(e) => Err(handler_error_to_response(e)),
    }
}

/// Collect every queryable field name the user could already see via
/// `GET /api/schema/{name}` — plain fields, transform-field keys, and
/// reference-field keys. This is the same surface the 400 response
/// advertises in `available_fields`, so the error message can't leak any
/// field name the caller doesn't already have access to.
fn queryable_field_names(schema: &Schema) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Some(plain) = schema.fields.as_ref() {
        names.extend(plain.iter().cloned());
    }
    if let Some(transforms) = schema.transform_fields.as_ref() {
        names.extend(transforms.keys().cloned());
    }
    names.extend(schema.ref_fields.keys().cloned());
    names.sort();
    names.dedup();
    names
}

/// Diff `requested` against the schema's queryable surface. Returns the
/// (unknown, available) pair when at least one requested field isn't on
/// the schema; `None` when every requested name is legal.
///
/// An empty `requested` list is treated as "all fields" (used by view
/// queries) and short-circuits to `None`.
fn find_unknown_fields(
    schema: &Schema,
    requested: &[String],
) -> Option<(Vec<String>, Vec<String>)> {
    if requested.is_empty() {
        return None;
    }
    let available = queryable_field_names(schema);
    let available_set: std::collections::HashSet<&str> =
        available.iter().map(String::as_str).collect();
    let unknown: Vec<String> = requested
        .iter()
        .filter(|f| !available_set.contains(f.as_str()))
        .cloned()
        .collect();
    if unknown.is_empty() {
        None
    } else {
        Some((unknown, available))
    }
}

/// Collect every writable field name on `schema`. Mirror of
/// [`queryable_field_names`] for the mutation side, but tighter:
/// `transform_fields` are computed at read time and rejected as mutation
/// targets; only plain `fields` and `ref_fields` keys (which a caller can
/// legitimately mutate, even though the value shape is a reference object)
/// are included. Value-shape validation for refs is intentionally out of
/// scope here — this gate only refuses unknown field NAMES, not bad shapes.
fn mutation_writable_field_names(schema: &Schema) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Some(plain) = schema.fields.as_ref() {
        names.extend(plain.iter().cloned());
    }
    names.extend(schema.ref_fields.keys().cloned());
    names.sort();
    names.dedup();
    names
}

/// Diff the keys of `fields_and_values` against the schema's writable
/// surface. Same return contract as [`find_unknown_fields`]: `Some` when
/// at least one key is unknown, `None` when every key is legal or the
/// map is empty (no field names to check).
fn mutation_unknown_fields(
    schema: &Schema,
    fields_and_values: &std::collections::HashMap<String, Value>,
) -> Option<(Vec<String>, Vec<String>)> {
    if fields_and_values.is_empty() {
        return None;
    }
    let available = mutation_writable_field_names(schema);
    let available_set: std::collections::HashSet<&str> =
        available.iter().map(String::as_str).collect();
    let mut unknown: Vec<String> = fields_and_values
        .keys()
        .filter(|f| !available_set.contains(f.as_str()))
        .cloned()
        .collect();
    if unknown.is_empty() {
        None
    } else {
        // HashMap iteration order is non-deterministic; sort so the error
        // payload is stable across runs (and so tests can pin order).
        unknown.sort();
        Some((unknown, available))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MutationResponse {
    pub mutation_id: String,
}

/// Execute a query.
///
/// Body is a [`Query`]-shaped JSON object with two optional pagination
/// siblings: `limit` (default 100, max 1000) and `offset` (default 0). They
/// are stripped from the body before deserialising into `Query`, which has
/// `#[serde(deny_unknown_fields)]` and no native pagination fields. The
/// response carries `total_count`, `returned_count`, `limit`, `offset`, and
/// `has_more` so callers can detect truncation.
#[utoipa::path(
    post,
    path = "/api/query",
    tag = "query",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Page of query results plus pagination metadata", body = QueryResponse),
        (status = 400, description = "Bad request"),
        (status = 409, description = "Ambiguous descriptive_name; body lists candidate canonical hashes in `ambiguous_schemas`"),
        (status = 500, description = "Server error")
    )
)]
pub async fn execute_query(body: web::Json<Value>, state: web::Data<AppState>) -> impl Responder {
    let mut body = body.into_inner();
    let (limit, offset) = match body.as_object_mut() {
        Some(obj) => (
            obj.remove("limit")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize),
            obj.remove("offset")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize),
        ),
        None => (None, None),
    };

    let mut query_inner: Query = match serde_json::from_value(body) {
        Ok(q) => q,
        Err(e) => {
            tracing::warn!(
                target: "fold_node::http_server",
                "execute_query: failed to parse query body: {}",
                e
            );
            return HttpResponse::BadRequest()
                .json(json!({"error": format!("Invalid query body: {}", e)}));
        }
    };

    tracing::info!(
            target: "fold_node::http_server",
        "execute_query: schema={}, fields={:?}, filter={:?}, limit={:?}, offset={:?}",
        query_inner.schema_name,
        query_inner.fields,
        query_inner.filter,
        limit,
        offset,
    );

    let (user_hash, node) = node_or_return!(state);

    // Resolve descriptive_name → canonical hash before any downstream
    // bookkeeping. A 2+-Approved-schemas-with-the-same-descriptive_name
    // collision becomes a 409 here, not a silent pick that routes the
    // query at one of several schemas — see
    // [`crate::handlers::schema_resolution`].
    let processor = OperationProcessor::from_ref(&node);
    match resolve_or_conflict_response(&processor, &query_inner.schema_name).await {
        Ok(canonical) => query_inner.schema_name = canonical,
        Err(response) => return response,
    }

    // Loud unknown-field validation: today the resolver silently drops fields
    // that aren't on the schema, so a typo (`title` vs `summary`) is
    // indistinguishable from "schema is empty". When the target resolves to a
    // known schema, diff the requested fields against its public surface and
    // 400 with the legal field list. Targets that don't resolve as schemas
    // (views, unknown names) fall through so the resolver's own 404 still
    // wins.
    if let Ok(Some(schema_with_state)) = processor.get_schema(&query_inner.schema_name).await {
        if let Some((unknown, available)) =
            find_unknown_fields(&schema_with_state.schema, &query_inner.fields)
        {
            let quoted_unknown = unknown
                .iter()
                .map(|f| format!("'{}'", f))
                .collect::<Vec<_>>()
                .join(", ");
            let plural = if unknown.len() == 1 { "" } else { "s" };
            let available_summary = if available.is_empty() {
                "<none>".to_string()
            } else {
                available.join(", ")
            };
            let message = format!(
                "Field{plural} {quoted_unknown} not on schema '{}'. Available: {}",
                query_inner.schema_name, available_summary,
            );
            tracing::info!(
                target: "fold_node::http_server",
                schema = %query_inner.schema_name,
                unknown_fields = ?unknown,
                "execute_query: rejecting unknown fields"
            );
            return HttpResponse::BadRequest().json(json!({
                "ok": false,
                "error": "unknown_fields",
                "message": message,
                "schema_name": query_inner.schema_name,
                "unknown_fields": unknown,
                "available_fields": available,
            }));
        }
    }

    match query_handlers::execute_query(query_inner, limit, offset, &user_hash, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => {
            tracing::error!(
            target: "fold_node::http_server", "Query failed: {}", e);
            handler_error_to_response(e)
        }
    }
}

/// Execute a mutation.
#[utoipa::path(
    post,
    path = "/api/mutation",
    tag = "query",
    request_body = serde_json::Value,
    responses(
        (status = 200, description = "Mutation accepted", body = MutationResponse),
        (status = 400, description = "Bad request"),
        (status = 409, description = "Ambiguous descriptive_name; body lists candidate canonical hashes in `ambiguous_schemas`"),
        (status = 500, description = "Server error")
    )
)]
pub async fn execute_mutation(
    mutation_data: web::Json<Value>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (mut schema, fields_and_values, key_value, mutation_type) =
        match serde_json::from_value::<Operation>(mutation_data.into_inner()) {
            Ok(Operation::Mutation {
                schema,
                fields_and_values,
                key_value,
                mutation_type,
                source_file_name: _,
            }) => {
                tracing::info!(
                target: "fold_node::http_server",
                        "Parsed mutation: schema={}, type={:?}, fields={}",
                        schema,
                        mutation_type,
                        fields_and_values.len()
                    );
                (schema, fields_and_values, key_value, mutation_type)
            }
            Err(e) => {
                tracing::error!(
                target: "fold_node::http_server",
                        "Failed to parse mutation: {}",
                        e
                    );
                return HttpResponse::BadRequest()
                    .json(json!({"error": format!("Failed to parse mutation: {}", e)}));
            }
        };

    let (user_hash, node) = node_or_return!(state);

    // Resolve descriptive_name → canonical hash for mutation symmetry with
    // execute_query — 2+ Approved schemas sharing a descriptive_name become a
    // 409 here, not a silent pick that could write the molecule into the
    // wrong schema.
    let processor = OperationProcessor::from_ref(&node);
    match resolve_or_conflict_response(&processor, &schema).await {
        Ok(canonical) => schema = canonical,
        Err(response) => return response,
    }

    // Loud unknown-field validation, parallel to execute_query above. Without
    // this gate the mutation pipeline silently writes unknown field names into
    // the molecule — a typo in `fields_and_values` is indistinguishable from
    // a successful write and the data is lost. When the target resolves to a
    // known schema, diff the supplied field keys against its writable surface
    // (plain fields + ref_fields keys; transform_fields are computed and not
    // writable) and 400 with the legal field list. Targets that don't resolve
    // as schemas fall through so the resolver's own error wins, matching the
    // query-side contract.
    if let Ok(Some(schema_with_state)) = processor.get_schema(&schema).await {
        if let Some((unknown, available)) =
            mutation_unknown_fields(&schema_with_state.schema, &fields_and_values)
        {
            let quoted_unknown = unknown
                .iter()
                .map(|f| format!("'{}'", f))
                .collect::<Vec<_>>()
                .join(", ");
            let plural = if unknown.len() == 1 { "" } else { "s" };
            let available_summary = if available.is_empty() {
                "<none>".to_string()
            } else {
                available.join(", ")
            };
            let message = format!(
                "Field{plural} {quoted_unknown} not writable on schema '{}'. Available: {}",
                schema, available_summary,
            );
            tracing::info!(
                target: "fold_node::http_server",
                schema = %schema,
                unknown_fields = ?unknown,
                "execute_mutation: rejecting unknown fields"
            );
            return HttpResponse::BadRequest().json(json!({
                "ok": false,
                "error": "unknown_fields",
                "message": message,
                "schema_name": schema,
                "unknown_fields": unknown,
                "available_fields": available,
            }));
        }
    }

    match crate::handlers::mutation::execute_mutation_from_components(
        schema,
        fields_and_values,
        key_value,
        mutation_type,
        &user_hash,
        &node,
    )
    .await
    {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => {
            tracing::error!(
            target: "fold_node::http_server", "Mutation failed: {}", e);
            handler_error_to_response(e)
        }
    }
}

/// Search the native word index for a term.
#[utoipa::path(
    get,
    path = "/api/native-index/search",
    tag = "query",
    params(
        ("term" = String, Query, description = "Search term for native word index"),
        ("include_internal" = Option<bool>, Query, description = "Include internal/bookkeeping schemas (Mention, MentionBySource, ExtractionStatus, IngestionError, TriggerFiring, ai_conversations, ExtractionRule). Defaults to false."),
    ),
    responses(
        // body = [serde_json::Value]: utoipa emits the $ref using the
        // full Rust path (`fold_db.db_operations.IndexResult`) but
        // components(schemas(...)) registers by simple name
        // (`IndexResult`), so the reference can't resolve. Surface as
        // opaque array until upstream registration is reconciled (gbrain
        // projects/api-typegen-unification Phase 3).
        (status = 200, description = "Array of native index results", body = [serde_json::Value]),
        (status = 400, description = "Bad request"),
        (status = 500, description = "Server error")
    )
)]
pub async fn native_index_search(
    query: web::Query<std::collections::HashMap<String, String>>,
    state: web::Data<AppState>,
) -> impl Responder {
    let term = match query.get("term") {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            tracing::warn!(
            target: "fold_node::http_server",
                "native_index_search: missing or empty term"
            );
            return HttpResponse::BadRequest()
                .json(json!({"error": "Missing required 'term' query parameter"}));
        }
    };

    let include_internal = query
        .get("include_internal")
        .map(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);

    let (user_hash, node) = node_or_return!(state);

    tracing::info!(
            target: "fold_node::http_server",
        "native_index_search: term='{}', user='{}', include_internal={}",
        term,
        user_hash,
        include_internal
    );

    match query_handlers::native_index_search(&term, include_internal, &user_hash, &node).await {
        Ok(response) => HttpResponse::Ok().json(response),
        Err(e) => {
            tracing::error!(
            target: "fold_node::http_server",
                "native_index_search failed: {}",
                e
            );
            handler_error_to_response(e)
        }
    }
}

/// Get indexing status
#[utoipa::path(
    get,
    path = "/api/indexing/status",
    tag = "system",
    responses(
        (status = 200, description = "Current indexing status", body = IndexingStatus),
        (status = 500, description = "Server error")
    )
)]
pub async fn get_indexing_status(state: web::Data<AppState>) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);

    match crate::handlers::system::get_indexing_status(&user_hash, &node).await {
        Ok(response) => {
            HttpResponse::Ok().json(response.data.map(|d| d.status).unwrap_or(json!(null)))
        }
        Err(e) => handler_error_to_response(e),
    }
}

/// Get mutation history for a molecule.
#[utoipa::path(
    get,
    path = "/api/history/{molecule_uuid}",
    tag = "query",
    params(
        ("molecule_uuid" = String, Path, description = "Molecule UUID")
    ),
    responses(
        (status = 200, description = "Molecule mutation history"),
        (status = 500, description = "Server error")
    )
)]
pub async fn get_molecule_history(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let molecule_uuid = path.into_inner();
    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(
        query_handlers::get_molecule_history(&molecule_uuid, &user_hash, &node).await,
    )
}

/// Get atom content by UUID.
#[utoipa::path(
    get,
    path = "/api/atom/{atom_uuid}",
    tag = "query",
    params(
        ("atom_uuid" = String, Path, description = "Atom UUID")
    ),
    responses(
        (status = 200, description = "Atom content"),
        (status = 404, description = "Atom not found"),
        (status = 500, description = "Server error")
    )
)]
pub async fn get_atom_content(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let atom_uuid = path.into_inner();
    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(
        query_handlers::get_atom_content(&atom_uuid, &user_hash, &node).await,
    )
}

/// Get process results for a progress_id (actual stored keys from ingestion mutations).
pub async fn get_process_results(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let progress_id = path.into_inner();
    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(
        query_handlers::get_process_results(&progress_id, &user_hash, &node).await,
    )
}

/// Optional query parameter for filtering conflicts by molecule.
#[derive(Debug, Deserialize)]
pub struct ConflictQuery {
    pub molecule_uuid: Option<String>,
}

/// List unresolved sync conflicts.
#[utoipa::path(
    get,
    path = "/api/conflicts",
    tag = "query",
    params(
        ("molecule_uuid" = Option<String>, Query, description = "Filter by molecule UUID")
    ),
    responses(
        (status = 200, description = "List of unresolved sync conflicts"),
        (status = 500, description = "Server error")
    )
)]
pub async fn get_conflicts(
    query: web::Query<ConflictQuery>,
    state: web::Data<AppState>,
) -> impl Responder {
    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(
        query_handlers::get_conflicts(query.molecule_uuid.as_deref(), &user_hash, &node).await,
    )
}

/// Resolve (acknowledge) a sync conflict.
#[utoipa::path(
    post,
    path = "/api/conflicts/{conflict_id}/resolve",
    tag = "query",
    params(
        ("conflict_id" = String, Path, description = "Conflict ID to resolve")
    ),
    responses(
        (status = 200, description = "Conflict resolved"),
        (status = 404, description = "Conflict not found"),
        (status = 500, description = "Server error")
    )
)]
pub async fn resolve_conflict(
    path: web::Path<String>,
    state: web::Data<AppState>,
) -> impl Responder {
    let conflict_id = path.into_inner();
    let (user_hash, node) = node_or_return!(state);

    handler_result_to_response(
        query_handlers::resolve_conflict(&conflict_id, &user_hash, &node).await,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::routes::common::test_helpers::create_test_state;
    use actix_web::body::MessageBody;
    use actix_web::http::StatusCode;
    use actix_web::test as actix_test;
    use fold_db::schema::types::declarative_schemas::DeclarativeSchemaDefinition;
    use fold_db::schema::types::key_config::KeyConfig;
    use fold_db::schema::types::schema::DeclarativeSchemaType;
    use fold_db::schema::SchemaState;
    use std::collections::HashMap;
    use tempfile::tempdir;

    fn schema_with_fields(name: &str, fields: Vec<String>) -> Schema {
        DeclarativeSchemaDefinition::new(
            name.to_string(),
            DeclarativeSchemaType::Single,
            None,
            Some(fields),
            None,
            None,
        )
    }

    #[test]
    fn queryable_field_names_unions_fields_transforms_and_refs() {
        let mut schema =
            schema_with_fields("S", vec!["summary".to_string(), "start_time".to_string()]);
        let mut transforms = HashMap::new();
        transforms.insert("computed".to_string(), "expr".to_string());
        schema.transform_fields = Some(transforms);
        schema
            .ref_fields
            .insert("author".to_string(), "Person".to_string());

        let names = queryable_field_names(&schema);
        assert_eq!(
            names,
            vec![
                "author".to_string(),
                "computed".to_string(),
                "start_time".to_string(),
                "summary".to_string(),
            ]
        );
    }

    #[test]
    fn find_unknown_fields_short_circuits_on_empty_request() {
        let schema = schema_with_fields("S", vec!["a".to_string()]);
        assert!(find_unknown_fields(&schema, &[]).is_none());
    }

    #[test]
    fn find_unknown_fields_returns_none_when_all_known() {
        let schema = schema_with_fields("S", vec!["a".to_string(), "b".to_string()]);
        assert!(find_unknown_fields(&schema, &["a".to_string()]).is_none());
        assert!(find_unknown_fields(&schema, &["a".to_string(), "b".to_string()]).is_none());
    }

    #[test]
    fn find_unknown_fields_enumerates_only_unknowns_on_mixed_request() {
        let schema = schema_with_fields(
            "AppleCalendar",
            vec!["summary".to_string(), "start_time".to_string()],
        );
        let (unknown, available) = find_unknown_fields(
            &schema,
            &[
                "summary".to_string(),
                "title".to_string(),
                "start".to_string(),
            ],
        )
        .expect("should report unknowns");
        assert_eq!(unknown, vec!["title".to_string(), "start".to_string()]);
        assert_eq!(
            available,
            vec!["start_time".to_string(), "summary".to_string()]
        );
    }

    /// Drain an actix-web response body to a JSON value. Test-only.
    async fn body_json<B: MessageBody + 'static>(
        resp: actix_web::HttpResponse<B>,
    ) -> serde_json::Value {
        let body = resp.into_body();
        let bytes = actix_web::body::to_bytes(body).await.unwrap_or_default();
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }

    async fn load_apple_calendar_schema(state: &web::Data<AppState>) {
        let node = state.node_manager.get_node("test_user").await.unwrap();
        let mut schema = DeclarativeSchemaDefinition::new(
            "AppleCalendar".to_string(),
            DeclarativeSchemaType::HashRange,
            Some(KeyConfig {
                hash_field: Some("summary".to_string()),
                range_field: Some("start_time".to_string()),
            }),
            Some(vec![
                "summary".to_string(),
                "start_time".to_string(),
                "end_time".to_string(),
                "location".to_string(),
            ]),
            None,
            None,
        );
        schema.populate_runtime_fields().unwrap();
        let db = node.get_fold_db().unwrap();
        db.schema_manager()
            .load_schema_internal(schema)
            .await
            .unwrap();
        db.schema_manager()
            .set_schema_state("AppleCalendar", SchemaState::Approved)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn execute_query_rejects_unknown_field_with_400_payload() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        load_apple_calendar_schema(&state).await;

        fold_db::user_context::run_with_user("test_user", async move {
            let query = Query::new(
                "AppleCalendar".to_string(),
                vec!["title".to_string(), "start".to_string(), "end".to_string()],
            );
            let req = actix_test::TestRequest::default().to_http_request();
            let body = serde_json::to_value(&query).unwrap();
            let resp = execute_query(web::Json(body), state).await.respond_to(&req);
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

            let body = body_json(resp).await;
            assert_eq!(body["ok"], serde_json::json!(false));
            assert_eq!(body["error"], serde_json::json!("unknown_fields"));
            assert_eq!(body["schema_name"], serde_json::json!("AppleCalendar"));
            let unknown: Vec<String> = serde_json::from_value(body["unknown_fields"].clone())
                .expect("unknown_fields should be a string array");
            assert_eq!(
                unknown,
                vec!["title".to_string(), "start".to_string(), "end".to_string()]
            );
            let available: Vec<String> = serde_json::from_value(body["available_fields"].clone())
                .expect("available_fields should be a string array");
            assert!(available.contains(&"summary".to_string()));
            assert!(available.contains(&"start_time".to_string()));
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("AppleCalendar") && message.contains("summary"),
                "message should name schema and list available fields: {}",
                message
            );
        })
        .await;
    }

    #[tokio::test]
    async fn execute_query_rejects_mixed_request_without_partial_success() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        load_apple_calendar_schema(&state).await;

        fold_db::user_context::run_with_user("test_user", async move {
            let query = Query::new(
                "AppleCalendar".to_string(),
                vec!["summary".to_string(), "title".to_string()],
            );
            let req = actix_test::TestRequest::default().to_http_request();
            let body = serde_json::to_value(&query).unwrap();
            let resp = execute_query(web::Json(body), state).await.respond_to(&req);
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

            let body = body_json(resp).await;
            assert_eq!(body["error"], serde_json::json!("unknown_fields"));
            let unknown: Vec<String> = serde_json::from_value(body["unknown_fields"].clone())
                .expect("unknown_fields should be a string array");
            assert_eq!(unknown, vec!["title".to_string()]);
        })
        .await;
    }

    #[tokio::test]
    async fn execute_query_accepts_valid_fields() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        load_apple_calendar_schema(&state).await;

        fold_db::user_context::run_with_user("test_user", async move {
            let query = Query::new(
                "AppleCalendar".to_string(),
                vec!["summary".to_string(), "start_time".to_string()],
            );
            let req = actix_test::TestRequest::default().to_http_request();
            let body = serde_json::to_value(&query).unwrap();
            let resp = execute_query(web::Json(body), state).await.respond_to(&req);
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "valid field names should not trigger the unknown-fields gate"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn execute_query_does_not_block_when_schema_unresolved() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        // Intentionally do not load AppleCalendar. The unknown-fields gate
        // must let this through so the resolver's own "not found as schema
        // or view" path wins. The resolver currently surfaces that as a 400
        // with the resolver's own error string — our gate must NOT shadow
        // that with an unknown_fields payload.

        fold_db::user_context::run_with_user("test_user", async move {
            let query = Query::new("MissingSchema".to_string(), vec!["whatever".to_string()]);
            let req = actix_test::TestRequest::default().to_http_request();
            let body = serde_json::to_value(&query).unwrap();
            let resp = execute_query(web::Json(body), state).await.respond_to(&req);
            let status = resp.status();
            let body = body_json(resp).await;
            assert_ne!(
                body["error"].as_str().unwrap_or_default(),
                "unknown_fields",
                "missing schema must not be reported as unknown_fields (status was {}, body {})",
                status,
                body
            );
        })
        .await;
    }

    // ----- mutation_writable_field_names / mutation_unknown_fields -----

    #[test]
    fn mutation_writable_field_names_excludes_transform_fields() {
        let mut schema =
            schema_with_fields("S", vec!["summary".to_string(), "start_time".to_string()]);
        let mut transforms = HashMap::new();
        transforms.insert("computed".to_string(), "expr".to_string());
        schema.transform_fields = Some(transforms);
        schema
            .ref_fields
            .insert("author".to_string(), "Person".to_string());

        let names = mutation_writable_field_names(&schema);
        // 'computed' is a transform_field — must NOT appear in the writable
        // surface even though it's queryable.
        assert_eq!(
            names,
            vec![
                "author".to_string(),
                "start_time".to_string(),
                "summary".to_string(),
            ]
        );
    }

    #[test]
    fn mutation_unknown_fields_short_circuits_on_empty_map() {
        let schema = schema_with_fields("S", vec!["a".to_string()]);
        assert!(mutation_unknown_fields(&schema, &HashMap::new()).is_none());
    }

    #[test]
    fn mutation_unknown_fields_returns_none_when_all_known() {
        let schema = schema_with_fields("S", vec!["a".to_string(), "b".to_string()]);
        let mut m = HashMap::new();
        m.insert("a".to_string(), serde_json::json!(1));
        m.insert("b".to_string(), serde_json::json!(2));
        assert!(mutation_unknown_fields(&schema, &m).is_none());
    }

    #[test]
    fn mutation_unknown_fields_enumerates_only_unknowns_sorted() {
        let schema = schema_with_fields(
            "AppleCalendar",
            vec!["summary".to_string(), "start_time".to_string()],
        );
        let mut m = HashMap::new();
        m.insert("summary".to_string(), serde_json::json!("hi"));
        m.insert("title".to_string(), serde_json::json!("typo"));
        m.insert("start".to_string(), serde_json::json!("typo"));
        let (unknown, available) =
            mutation_unknown_fields(&schema, &m).expect("should report unknowns");
        // HashMap iteration is non-deterministic; the helper sorts so the
        // 400 payload is stable.
        assert_eq!(unknown, vec!["start".to_string(), "title".to_string()]);
        assert_eq!(
            available,
            vec!["start_time".to_string(), "summary".to_string()]
        );
    }

    #[test]
    fn mutation_unknown_fields_rejects_transform_field_as_writable() {
        let mut schema = schema_with_fields("S", vec!["plain".to_string()]);
        let mut transforms = HashMap::new();
        transforms.insert("computed".to_string(), "expr".to_string());
        schema.transform_fields = Some(transforms);

        let mut m = HashMap::new();
        m.insert("computed".to_string(), serde_json::json!("x"));
        let (unknown, available) =
            mutation_unknown_fields(&schema, &m).expect("transform_field must be unwritable");
        assert_eq!(unknown, vec!["computed".to_string()]);
        assert_eq!(available, vec!["plain".to_string()]);
    }

    // ----- execute_mutation integration tests -----

    fn mutation_body(
        schema: &str,
        mutation_type: &str,
        fields_and_values: serde_json::Value,
    ) -> serde_json::Value {
        serde_json::json!({
            "type": "mutation",
            "schema": schema,
            "fields_and_values": fields_and_values,
            "key_value": { "hash_key": null, "range_key": null },
            "mutation_type": mutation_type,
        })
    }

    #[tokio::test]
    async fn execute_mutation_rejects_unknown_field_with_400_payload() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        load_apple_calendar_schema(&state).await;

        fold_db::user_context::run_with_user("test_user", async move {
            let body = mutation_body(
                "AppleCalendar",
                "create",
                serde_json::json!({
                    "title": "Standup",
                    "start": "2026-05-12T09:00:00Z",
                }),
            );
            let req = actix_test::TestRequest::default().to_http_request();
            let resp = execute_mutation(web::Json(body), state)
                .await
                .respond_to(&req);
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

            let body = body_json(resp).await;
            assert_eq!(body["ok"], serde_json::json!(false));
            assert_eq!(body["error"], serde_json::json!("unknown_fields"));
            assert_eq!(body["schema_name"], serde_json::json!("AppleCalendar"));
            let mut unknown: Vec<String> = serde_json::from_value(body["unknown_fields"].clone())
                .expect("unknown_fields should be a string array");
            unknown.sort();
            assert_eq!(unknown, vec!["start".to_string(), "title".to_string()]);
            let available: Vec<String> = serde_json::from_value(body["available_fields"].clone())
                .expect("available_fields should be a string array");
            assert!(available.contains(&"summary".to_string()));
            assert!(available.contains(&"start_time".to_string()));
            let message = body["message"].as_str().unwrap_or_default();
            assert!(
                message.contains("AppleCalendar") && message.contains("summary"),
                "message should name schema and list available fields: {}",
                message
            );
        })
        .await;
    }

    #[tokio::test]
    async fn execute_mutation_rejects_mixed_request_only_unknowns_listed() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        load_apple_calendar_schema(&state).await;

        fold_db::user_context::run_with_user("test_user", async move {
            let body = mutation_body(
                "AppleCalendar",
                "create",
                serde_json::json!({
                    "summary": "Standup",
                    "title": "typo",
                }),
            );
            let req = actix_test::TestRequest::default().to_http_request();
            let resp = execute_mutation(web::Json(body), state)
                .await
                .respond_to(&req);
            assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

            let body = body_json(resp).await;
            assert_eq!(body["error"], serde_json::json!("unknown_fields"));
            let unknown: Vec<String> = serde_json::from_value(body["unknown_fields"].clone())
                .expect("unknown_fields should be a string array");
            // Only the unknown name appears; the legal key isn't echoed
            // back in the error.
            assert_eq!(unknown, vec!["title".to_string()]);
        })
        .await;
    }

    #[tokio::test]
    async fn execute_mutation_does_not_block_when_schema_unresolved() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        // Intentionally do not load AppleCalendar. The gate must NOT shadow
        // the downstream "schema not found" error with an unknown_fields
        // payload — same contract as the query side.

        fold_db::user_context::run_with_user("test_user", async move {
            let body = mutation_body(
                "MissingSchema",
                "create",
                serde_json::json!({ "whatever": "x" }),
            );
            let req = actix_test::TestRequest::default().to_http_request();
            let resp = execute_mutation(web::Json(body), state)
                .await
                .respond_to(&req);
            let status = resp.status();
            let body = body_json(resp).await;
            assert_ne!(
                body["error"].as_str().unwrap_or_default(),
                "unknown_fields",
                "missing schema must not be reported as unknown_fields (status was {}, body {})",
                status,
                body
            );
        })
        .await;
    }

    #[tokio::test]
    async fn execute_mutation_empty_fields_falls_through() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        load_apple_calendar_schema(&state).await;

        // An empty fields_and_values map has no names to check, so the gate
        // must short-circuit and let the downstream handler decide (e.g. a
        // Delete operation legitimately carries no fields).
        fold_db::user_context::run_with_user("test_user", async move {
            let body = mutation_body("AppleCalendar", "delete", serde_json::json!({}));
            let req = actix_test::TestRequest::default().to_http_request();
            let resp = execute_mutation(web::Json(body), state)
                .await
                .respond_to(&req);
            let status = resp.status();
            let body = body_json(resp).await;
            assert_ne!(
                body["error"].as_str().unwrap_or_default(),
                "unknown_fields",
                "empty fields_and_values must not trigger unknown_fields (status was {}, body {})",
                status,
                body
            );
        })
        .await;
    }

    // ----- descriptive_name resolution at the route layer -----

    /// Load one Approved HashRange schema whose canonical `name` differs from
    /// its `descriptive_name`, mimicking how the schema service rewrites
    /// user-ingested schemas (canonical = identity hash, descriptive = human
    /// label like "Contacts"). Used to drive the route-layer descriptive_name
    /// tests.
    async fn load_named_schema(
        state: &web::Data<AppState>,
        canonical: &str,
        descriptive: &str,
        fields: &[&str],
        hash_field: &str,
    ) {
        let node = state.node_manager.get_node("test_user").await.unwrap();
        let mut schema = DeclarativeSchemaDefinition::new(
            canonical.to_string(),
            DeclarativeSchemaType::HashRange,
            Some(KeyConfig {
                hash_field: Some(hash_field.to_string()),
                range_field: Some("_rk".to_string()),
            }),
            Some(
                fields
                    .iter()
                    .map(|f| f.to_string())
                    .chain(std::iter::once("_rk".to_string()))
                    .collect(),
            ),
            None,
            None,
        );
        schema.descriptive_name = Some(descriptive.to_string());
        schema.populate_runtime_fields().unwrap();
        let db = node.get_fold_db().unwrap();
        db.schema_manager()
            .load_schema_internal(schema)
            .await
            .unwrap();
        db.schema_manager()
            .set_schema_state(canonical, SchemaState::Approved)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn execute_query_accepts_descriptive_name_with_200() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        // Canonical name is a synthetic 64-hex identity-hash; descriptive
        // name is the human label that READMEs, the UI, and `folddb query
        // <NAME>` all use.
        load_named_schema(
            &state,
            "fe331affcd23486a170a2bfb56555e114f7c2371a346b5fe58d2177746f831e3",
            "Contacts",
            &["full_name"],
            "full_name",
        )
        .await;

        fold_db::user_context::run_with_user("test_user", async move {
            let body = serde_json::json!({
                "schema_name": "Contacts",
                "fields": ["full_name"],
            });
            let req = actix_test::TestRequest::default().to_http_request();
            let resp = execute_query(web::Json(body), state).await.respond_to(&req);
            let status = resp.status();
            let body = body_json(resp).await;
            assert_eq!(
                status,
                StatusCode::OK,
                "descriptive_name must resolve to canonical hash and return 200; body was {}",
                body
            );
        })
        .await;
    }

    #[tokio::test]
    async fn execute_query_returns_409_on_ambiguous_descriptive_name() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        // Two Approved schemas share the descriptive_name "Contacts" — the
        // real-world failure mode that motivated this 409 (see the running
        // prod with 2x Contacts, 2x CalendarEvent, 2x Photography).
        let hash_a = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let hash_b = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        load_named_schema(&state, hash_a, "Contacts", &["full_name"], "full_name").await;
        load_named_schema(&state, hash_b, "Contacts", &["full_name"], "full_name").await;

        fold_db::user_context::run_with_user("test_user", async move {
            let body = serde_json::json!({
                "schema_name": "Contacts",
                "fields": ["full_name"],
            });
            let req = actix_test::TestRequest::default().to_http_request();
            let resp = execute_query(web::Json(body), state).await.respond_to(&req);
            assert_eq!(
                resp.status(),
                StatusCode::CONFLICT,
                "two Approved schemas sharing a descriptive_name must surface 409, not 200 or 500"
            );

            let body = body_json(resp).await;
            assert_eq!(body["ok"], serde_json::json!(false));
            assert_eq!(body["error"], serde_json::json!("ambiguous_schema_name"));
            assert_eq!(body["schema_name"], serde_json::json!("Contacts"));
            let mut candidates: Vec<String> =
                serde_json::from_value(body["ambiguous_schemas"].clone())
                    .expect("ambiguous_schemas should be a string array");
            candidates.sort();
            assert_eq!(
                candidates,
                vec![hash_a.to_string(), hash_b.to_string()],
                "every Approved canonical hash must appear so the caller can pin one"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn execute_mutation_accepts_descriptive_name_with_200() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        load_named_schema(
            &state,
            "fe331affcd23486a170a2bfb56555e114f7c2371a346b5fe58d2177746f831e3",
            "Contacts",
            &["full_name"],
            "full_name",
        )
        .await;

        fold_db::user_context::run_with_user("test_user", async move {
            let body = mutation_body(
                "Contacts",
                "create",
                serde_json::json!({ "full_name": "Ada Lovelace", "_rk": "ada" }),
            );
            let req = actix_test::TestRequest::default().to_http_request();
            let resp = execute_mutation(web::Json(body), state)
                .await
                .respond_to(&req);
            assert_eq!(
                resp.status(),
                StatusCode::OK,
                "descriptive_name on mutation must resolve to canonical and return 200"
            );
        })
        .await;
    }

    #[tokio::test]
    async fn execute_mutation_returns_409_on_ambiguous_descriptive_name() {
        let temp_dir = tempdir().unwrap();
        let state = create_test_state(&temp_dir).await;
        let hash_a = "1111111111111111111111111111111111111111111111111111111111111111";
        let hash_b = "2222222222222222222222222222222222222222222222222222222222222222";
        load_named_schema(&state, hash_a, "Contacts", &["full_name"], "full_name").await;
        load_named_schema(&state, hash_b, "Contacts", &["full_name"], "full_name").await;

        fold_db::user_context::run_with_user("test_user", async move {
            let body = mutation_body(
                "Contacts",
                "create",
                serde_json::json!({ "full_name": "Grace Hopper", "_rk": "grace" }),
            );
            let req = actix_test::TestRequest::default().to_http_request();
            let resp = execute_mutation(web::Json(body), state)
                .await
                .respond_to(&req);
            assert_eq!(
                resp.status(),
                StatusCode::CONFLICT,
                "ambiguous descriptive_name on mutation must surface 409, not write into one"
            );

            let body = body_json(resp).await;
            assert_eq!(body["error"], serde_json::json!("ambiguous_schema_name"));
            let mut candidates: Vec<String> =
                serde_json::from_value(body["ambiguous_schemas"].clone())
                    .expect("ambiguous_schemas should be a string array");
            candidates.sort();
            assert_eq!(candidates, vec![hash_a.to_string(), hash_b.to_string()]);
        })
        .await;
    }
}
