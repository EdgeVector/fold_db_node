use actix_web::{error::JsonPayloadError, HttpRequest, HttpResponse};
use serde_json::json;

/// Canonical `503 node_not_provisioned` response.
///
/// Returned whenever a request reaches a handler that needs the local
/// node identity but the bootstrap flow has not yet run (the
/// `node_identity` Sled tree is empty). The UI / CLI use the `next`
/// hint to route the user back through `POST /api/setup/bootstrap`
/// instead of treating this as a generic 500.
///
/// The body shape is part of the API contract — keep it stable across
/// every call site (currently `routes::common::get_node_for_user` and
/// `routes::config::auto_identity`).
pub fn node_not_provisioned_response() -> HttpResponse {
    HttpResponse::ServiceUnavailable().json(json!({
        "error": "node_not_provisioned",
        "next": "POST /api/setup/bootstrap",
    }))
}

/// Custom error handler for JSON deserialization errors.
///
/// This function is registered with Actix Web to handle errors that occur
/// when deserializing JSON payloads from requests. It logs the specific
/// error and returns a user-friendly JSON response.
///
/// We split `Deserialize` errors by `serde_json::error::Category`:
/// - `Syntax` / `Eof` → the bytes aren't valid JSON. Surface as
///   `INVALID_JSON_SYNTAX` / `"Invalid JSON format"` so callers know to
///   check quoting / escaping / encoding.
/// - `Data` → the JSON parsed but didn't match the target type (missing
///   field, wrong field name, type mismatch, unknown variant). Surface as
///   `INVALID_REQUEST_BODY` / `"Invalid request body"` so callers know
///   to check their schema, not their JSON syntax.
pub fn json_error_handler(err: JsonPayloadError, req: &HttpRequest) -> actix_web::Error {
    let detail = err.to_string();
    let path = req.path().to_string();
    let method = req.method().to_string();

    tracing::error!(
        "JSON payload error: \"{}\" for {} request to {}",
        detail,
        method,
        path
    );

    let response = match &err {
        JsonPayloadError::Deserialize(serde_err) => match serde_err.classify() {
            serde_json::error::Category::Data => HttpResponse::BadRequest().json(json!({
                "success": false,
                "error": "Invalid request body",
                "detail": serde_err.to_string(),
                "code": "INVALID_REQUEST_BODY",
            })),
            _ => HttpResponse::BadRequest().json(json!({
                "success": false,
                "error": "Invalid JSON format",
                "detail": serde_err.to_string(),
                "code": "INVALID_JSON_SYNTAX",
            })),
        },
        _ => HttpResponse::BadRequest().json(json!({
            "success": false,
            "error": "Invalid request payload",
            "detail": detail,
        })),
    };

    actix_web::error::InternalError::from_response(err, response).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::body::to_bytes;
    use actix_web::test::TestRequest;
    use serde::Deserialize;

    async fn extract_body(err: actix_web::Error) -> serde_json::Value {
        let resp = err.error_response();
        assert_eq!(resp.status(), actix_web::http::StatusCode::BAD_REQUEST);
        let body = to_bytes(resp.into_body()).await.expect("body bytes");
        serde_json::from_slice(&body).expect("response body is JSON")
    }

    fn syntax_error() -> serde_json::Error {
        serde_json::from_str::<serde_json::Value>("{not json")
            .expect_err("invalid syntax should fail")
    }

    fn data_error_missing_field() -> serde_json::Error {
        // The `folder_path` field is never read — its purpose is to make
        // serde produce a "missing field `folder_path`" error that the
        // assertion below pattern-matches. Don't rename or remove.
        #[derive(Debug, Deserialize)]
        #[allow(dead_code)]
        struct ScanRequest {
            folder_path: String,
        }
        serde_json::from_str::<ScanRequest>(r#"{"path":"/x"}"#)
            .expect_err("missing field should fail")
    }

    #[actix_web::test]
    async fn syntax_error_maps_to_invalid_json_syntax() {
        let err = JsonPayloadError::Deserialize(syntax_error());
        let req = TestRequest::default().to_http_request();
        let body = extract_body(json_error_handler(err, &req)).await;

        assert_eq!(body["error"], "Invalid JSON format");
        assert_eq!(body["code"], "INVALID_JSON_SYNTAX");
        assert_eq!(body["success"], false);
        assert!(body["detail"].as_str().is_some());
    }

    #[actix_web::test]
    async fn missing_field_maps_to_invalid_request_body() {
        let err = JsonPayloadError::Deserialize(data_error_missing_field());
        let req = TestRequest::default().to_http_request();
        let body = extract_body(json_error_handler(err, &req)).await;

        assert_eq!(body["error"], "Invalid request body");
        assert_eq!(body["code"], "INVALID_REQUEST_BODY");
        assert_eq!(body["success"], false);
        let detail = body["detail"].as_str().expect("detail string");
        assert!(
            detail.contains("missing field") && detail.contains("folder_path"),
            "detail should preserve serde's missing-field message, got: {detail}"
        );
    }
}
