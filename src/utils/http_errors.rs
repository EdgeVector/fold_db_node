use actix_web::{error::JsonPayloadError, HttpRequest, HttpResponse};
use serde_json::json;

/// Custom error handler for JSON deserialization errors.
///
/// This function is registered with Actix Web to handle errors that occur
/// when deserializing JSON payloads from requests. It logs the specific
/// error and returns a user-friendly JSON response.
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
        JsonPayloadError::Deserialize(serde_err) => {
            // Distinguish "JSON didn't parse" (syntax) from "JSON parsed but
            // didn't match the expected shape" (data). The latter is the most
            // common failure mode — wrong/missing field names — and a generic
            // "Invalid JSON format" hides that from the caller.
            let detail = serde_err.to_string();
            let error = match serde_err.classify() {
                serde_json::error::Category::Data => "Invalid request payload",
                _ => "Invalid JSON format",
            };
            HttpResponse::BadRequest().json(json!({
                "success": false,
                "error": format!("{}: {}", error, detail),
                "detail": detail,
            }))
        }
        _ => HttpResponse::BadRequest().json(json!({
            "success": false,
            "error": format!("Invalid request payload: {}", detail),
            "detail": detail,
        })),
    };

    actix_web::error::InternalError::from_response(err, response).into()
}
