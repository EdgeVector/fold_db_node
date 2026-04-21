use fold_db::error::{FoldDbError, FoldDbResult};
use fold_db::schema::types::Schema;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;

use fold_db::schema_service::types::{
    AddViewRequest, AddViewResponse, BatchSchemaReuseRequest, BatchSchemaReuseResponse,
    SchemaLookupEntry, StoredView,
};

/// Internal error wrapper used by retryable schema-service calls.
///
/// The retry layer only retries transient failures (connect errors, timeouts,
/// 5xx responses). Permanent failures (4xx, 409 CONFLICT, deserialization
/// errors) fail fast on the first attempt.
#[derive(Debug)]
enum RetryError {
    Transient(FoldDbError),
    Permanent(FoldDbError),
}

/// Classify a `reqwest::Error` from `.send()` as transient or permanent.
///
/// Note: `reqwest::Error` from `.send()` covers connect/timeout/body-stream
/// errors but NOT HTTP status — status classification is handled separately
/// by `status_is_retryable`.
fn reqwest_error_is_retryable(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request()
}

/// Classify an HTTP status code as retryable (5xx) or permanent (4xx).
fn status_is_retryable(status: StatusCode) -> bool {
    status.is_server_error()
}

/// Retry an async schema-service operation up to 3 times with exponential
/// backoff on transient failures.
///
/// The operation must distinguish transient from permanent failures by
/// returning `RetryError::Transient` or `RetryError::Permanent`. Permanent
/// errors (4xx, 409 CONFLICT, deserialization) fail fast without retry.
async fn with_retries<F, Fut, T>(mut op: F) -> FoldDbResult<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, RetryError>>,
{
    const MAX_ATTEMPTS: u32 = 3;
    let mut last_err: Option<FoldDbError> = None;
    for attempt in 0..MAX_ATTEMPTS {
        match op().await {
            Ok(v) => return Ok(v),
            Err(RetryError::Permanent(e)) => return Err(e),
            Err(RetryError::Transient(e)) => {
                log::warn!(
                    "schema service call failed (attempt {}/{}): {}",
                    attempt + 1,
                    MAX_ATTEMPTS,
                    e
                );
                last_err = Some(e);
                if attempt + 1 < MAX_ATTEMPTS {
                    backoff_delay(attempt).await;
                }
            }
        }
    }
    Err(last_err.expect("retry loop must have produced at least one error"))
}

/// Exponential backoff delay: 250ms, 1s, 4s, then 4s cap.
async fn backoff_delay(attempt: u32) {
    let ms = match attempt {
        0 => 250,
        1 => 1000,
        _ => 4000,
    };
    tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
}

/// Client for communicating with the schema service
#[derive(Clone)]
pub struct SchemaServiceClient {
    base_url: String,
    client: reqwest::Client,
}

/// Request structure for adding a schema with mutation mappers
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AddSchemaRequest {
    schema: Schema,
    mutation_mappers: HashMap<String, String>,
}

/// Response structure for adding a schema with mutation mappers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddSchemaResponse {
    pub schema: Schema,
    pub mutation_mappers: HashMap<String, String>,
    /// When a schema expansion occurred, this contains the old schema name that was replaced.
    #[serde(default)]
    pub replaced_schema: Option<String>,
}

impl SchemaServiceClient {
    /// Create a new schema service client
    pub fn new(schema_service_url: &str) -> Self {
        // Create client with timeout to prevent hanging.
        // Schema creation can involve LLM field classification (Anthropic Haiku or Ollama),
        // which takes 5-60s per field under load — allow enough headroom.
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .connect_timeout(std::time::Duration::from_secs(10))
            .no_proxy()
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());

        Self {
            base_url: schema_service_url.to_string(),
            client,
        }
    }

    /// Add a schema definition to the schema service.
    ///
    /// Retries up to 3 times on transient failures (connect errors, timeouts,
    /// 5xx responses). Does NOT retry on 4xx responses including 409 CONFLICT
    /// or on deserialization failures.
    pub async fn add_schema(
        &self,
        schema: &Schema,
        mutation_mappers: HashMap<String, String>,
    ) -> FoldDbResult<AddSchemaResponse> {
        let url = format!("{}/v1/schemas", self.base_url);
        let request = AddSchemaRequest {
            schema: schema.clone(),
            mutation_mappers,
        };

        with_retries(|| async {
            let response = self
                .client
                .post(&url)
                .json(&request)
                .send()
                .await
                .map_err(|error| {
                    let retryable = reqwest_error_is_retryable(&error);
                    let wrapped = FoldDbError::Config(format!(
                        "Failed to submit schema to schema service at {}: {}. Is the schema service running?",
                        url, error
                    ));
                    if retryable {
                        RetryError::Transient(wrapped)
                    } else {
                        RetryError::Permanent(wrapped)
                    }
                })?;

            let status = response.status();

            if status == StatusCode::CREATED || status == StatusCode::OK {
                return response
                    .json::<AddSchemaResponse>()
                    .await
                    .map_err(|error| {
                        // Deserialization of a 2xx body is a permanent failure —
                        // retrying won't change the server's response shape.
                        RetryError::Permanent(FoldDbError::Config(format!(
                            "Failed to parse schema response: {}",
                            error
                        )))
                    });
            }

            if status == StatusCode::CONFLICT {
                // CONFLICT should never happen — the schema service always either
                // returns an existing schema, expands, or creates new. Treat it
                // as a permanent error so we don't waste time retrying.
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "<empty>".to_string());
                return Err(RetryError::Permanent(FoldDbError::Config(format!(
                    "Schema service returned unexpected CONFLICT (409): {}. \
                     The schema service should always return Added, AlreadyExists, or Expanded.",
                    body
                ))));
            }

            let retryable = status_is_retryable(status);
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<empty>".to_string());
            let wrapped = FoldDbError::Config(format!(
                "Schema service add schema failed with status {}: {}",
                status, body
            ));
            if retryable {
                Err(RetryError::Transient(wrapped))
            } else {
                Err(RetryError::Permanent(wrapped))
            }
        })
        .await
    }

    /// Send a GET request and deserialize the JSON response.
    ///
    /// Retries up to 3 times on transient failures (connect errors, timeouts,
    /// 5xx responses). Does NOT retry on 4xx responses or deserialization
    /// failures.
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        url: &str,
        context: &str,
    ) -> FoldDbResult<T> {
        with_retries(|| async {
            let response = self.client.get(url).send().await.map_err(|e| {
                let retryable = reqwest_error_is_retryable(&e);
                let wrapped = FoldDbError::Config(format!("Failed to fetch {}: {}", context, e));
                if retryable {
                    RetryError::Transient(wrapped)
                } else {
                    RetryError::Permanent(wrapped)
                }
            })?;
            let status = response.status();
            if !status.is_success() {
                let wrapped = FoldDbError::Config(format!(
                    "Schema service returned error for {}: {}",
                    context, status
                ));
                return Err(if status_is_retryable(status) {
                    RetryError::Transient(wrapped)
                } else {
                    RetryError::Permanent(wrapped)
                });
            }
            response.json().await.map_err(|e| {
                RetryError::Permanent(FoldDbError::Config(format!(
                    "Failed to parse {} response: {}",
                    context, e
                )))
            })
        })
        .await
    }

    /// Extract a schema name from a JSON value that may be a string, `{"name": ...}`, or `{"schema": {"name": ...}}`.
    fn extract_schema_name(v: serde_json::Value) -> Option<String> {
        v.as_str().map(|s| s.to_string()).or_else(|| {
            let obj = v.as_object()?;
            obj.get("name")
                .and_then(|n| n.as_str())
                .or_else(|| {
                    obj.get("schema")
                        .and_then(|s| s.get("name"))
                        .and_then(|n| n.as_str())
                })
                .map(|s| s.to_string())
        })
    }

    /// List all available schemas from the schema service
    pub async fn list_schemas(&self) -> FoldDbResult<Vec<String>> {
        #[derive(Deserialize)]
        struct SchemasListResponse {
            schemas: Vec<serde_json::Value>,
        }

        let url = format!("{}/v1/schemas", self.base_url);
        let resp: SchemasListResponse = self.get_json(&url, "schemas").await?;
        Ok(resp
            .schemas
            .into_iter()
            .filter_map(Self::extract_schema_name)
            .collect())
    }

    /// Get all available schemas with their full definitions from the schema service
    pub async fn get_available_schemas(&self) -> FoldDbResult<Vec<Schema>> {
        #[derive(Deserialize)]
        struct AvailableSchemasResponse {
            schemas: Vec<Schema>,
        }

        let url = format!("{}/v1/schemas/available", self.base_url);
        let resp: AvailableSchemasResponse = self.get_json(&url, "available schemas").await?;
        Ok(resp.schemas)
    }

    /// Get a specific schema definition from the schema service
    pub async fn get_schema(&self, name: &str) -> FoldDbResult<Schema> {
        let url = format!("{}/v1/schema/{}", self.base_url, name);
        self.get_json(&url, &format!("schema '{}'", name)).await
    }

    /// Batch check whether proposed schemas can reuse existing ones.
    ///
    /// Retries up to 3 times on transient failures.
    pub async fn batch_check_schema_reuse(
        &self,
        entries: &[SchemaLookupEntry],
    ) -> FoldDbResult<BatchSchemaReuseResponse> {
        let url = format!("{}/v1/schemas/batch-check-reuse", self.base_url);
        let request = BatchSchemaReuseRequest {
            schemas: entries.to_vec(),
        };

        with_retries(|| async {
            let response = self
                .client
                .post(&url)
                .json(&request)
                .send()
                .await
                .map_err(|e| {
                    let retryable = reqwest_error_is_retryable(&e);
                    let wrapped = FoldDbError::Config(format!(
                        "Failed to batch check schema reuse at {}: {}",
                        url, e
                    ));
                    if retryable {
                        RetryError::Transient(wrapped)
                    } else {
                        RetryError::Permanent(wrapped)
                    }
                })?;

            let status = response.status();
            if !status.is_success() {
                let retryable = status_is_retryable(status);
                let body = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "<empty>".to_string());
                let wrapped = FoldDbError::Config(format!(
                    "Batch schema reuse check failed (status {}): {}",
                    status, body
                ));
                return Err(if retryable {
                    RetryError::Transient(wrapped)
                } else {
                    RetryError::Permanent(wrapped)
                });
            }

            response
                .json::<BatchSchemaReuseResponse>()
                .await
                .map_err(|e| {
                    RetryError::Permanent(FoldDbError::Config(format!(
                        "Failed to parse batch schema reuse response: {}",
                        e
                    )))
                })
        })
        .await
    }

    /// Register a view with the global schema service.
    pub async fn add_view(&self, request: &AddViewRequest) -> FoldDbResult<AddViewResponse> {
        let url = format!("{}/v1/views", self.base_url);

        let response = self
            .client
            .post(&url)
            .json(request)
            .send()
            .await
            .map_err(|error| {
                FoldDbError::Config(format!(
                    "Failed to submit view to schema service at {}: {}. Is the schema service running?",
                    url, error
                ))
            })?;

        let status = response.status();

        if status == StatusCode::CREATED || status == StatusCode::OK {
            return response.json::<AddViewResponse>().await.map_err(|error| {
                FoldDbError::Config(format!("Failed to parse view response: {}", error))
            });
        }

        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "<empty>".to_string());
        Err(FoldDbError::Config(format!(
            "Schema service add view failed with status {}: {}",
            status, body
        )))
    }

    /// List all views from the global schema service.
    pub async fn list_views(&self) -> FoldDbResult<Vec<String>> {
        #[derive(Deserialize)]
        struct ViewsListResponse {
            views: Vec<String>,
        }

        let url = format!("{}/v1/views", self.base_url);
        let resp: ViewsListResponse = self.get_json(&url, "views").await?;
        Ok(resp.views)
    }

    /// Get all views with their full definitions.
    pub async fn get_available_views(&self) -> FoldDbResult<Vec<StoredView>> {
        #[derive(Deserialize)]
        struct AvailableViewsResponse {
            views: Vec<StoredView>,
        }

        let url = format!("{}/v1/views/available", self.base_url);
        let resp: AvailableViewsResponse = self.get_json(&url, "available views").await?;
        Ok(resp.views)
    }

    /// Get a specific view by name.
    pub async fn get_view(&self, name: &str) -> FoldDbResult<StoredView> {
        let url = format!("{}/v1/view/{}", self.base_url, name);
        self.get_json(&url, &format!("view '{}'", name)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::{rt::time::sleep, web, App, HttpResponse, HttpServer};
    use fold_db::schema::types::SchemaType;
    use fold_db::schema_service::state::SchemaServiceState;
    use fold_db::schema_service::types::{ErrorResponse, SchemaAddOutcome};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::tempdir;

    // ----- Retry wrapper unit tests -----

    #[actix_web::test]
    async fn with_retries_returns_ok_on_first_attempt() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_op = calls.clone();
        let result: FoldDbResult<u32> = with_retries(move || {
            let calls = calls_for_op.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok::<u32, RetryError>(42u32)
            }
        })
        .await;

        assert_eq!(result.unwrap(), 42);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[actix_web::test]
    async fn with_retries_retries_transient_then_succeeds() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_op = calls.clone();
        let result: FoldDbResult<&'static str> = with_retries(move || {
            let calls = calls_for_op.clone();
            async move {
                let n = calls.fetch_add(1, Ordering::SeqCst);
                if n < 2 {
                    Err(RetryError::Transient(FoldDbError::Config(
                        "simulated timeout".into(),
                    )))
                } else {
                    Ok("ok")
                }
            }
        })
        .await;

        assert_eq!(result.unwrap(), "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[actix_web::test]
    async fn with_retries_does_not_retry_permanent() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_op = calls.clone();
        let result: FoldDbResult<()> = with_retries(move || {
            let calls = calls_for_op.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(RetryError::Permanent(FoldDbError::Config(
                    "409 CONFLICT".into(),
                )))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("CONFLICT"), "got: {}", msg);
    }

    #[actix_web::test]
    async fn with_retries_exhausts_after_max_attempts() {
        let calls = Arc::new(AtomicU32::new(0));
        let calls_for_op = calls.clone();
        let result: FoldDbResult<()> = with_retries(move || {
            let calls = calls_for_op.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Err::<(), _>(RetryError::Transient(FoldDbError::Config(
                    "503 Service Unavailable".into(),
                )))
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn status_is_retryable_classifies_correctly() {
        assert!(status_is_retryable(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(status_is_retryable(StatusCode::BAD_GATEWAY));
        assert!(status_is_retryable(StatusCode::SERVICE_UNAVAILABLE));
        assert!(status_is_retryable(StatusCode::GATEWAY_TIMEOUT));

        assert!(!status_is_retryable(StatusCode::BAD_REQUEST));
        assert!(!status_is_retryable(StatusCode::UNAUTHORIZED));
        assert!(!status_is_retryable(StatusCode::FORBIDDEN));
        assert!(!status_is_retryable(StatusCode::NOT_FOUND));
        assert!(!status_is_retryable(StatusCode::CONFLICT));
        assert!(!status_is_retryable(StatusCode::OK));
    }

    // ----- Integration: retry against a flaky mock server -----

    /// Spawn a mock POST /v1/schemas endpoint that returns 500 for the first
    /// `fail_count` requests, then a valid AddSchemaResponse thereafter.
    /// Returns (base_url, handle, request_counter).
    async fn spawn_flaky_add_schema(
        fail_count: u32,
    ) -> (String, actix_web::dev::ServerHandle, Arc<AtomicU32>) {
        let counter = Arc::new(AtomicU32::new(0));
        let counter_for_handler = counter.clone();

        let listener =
            TcpListener::bind(("127.0.0.1", 0)).expect("failed to bind flaky test listener");
        let port = listener.local_addr().unwrap().port();

        let server = HttpServer::new(move || {
            let counter = counter_for_handler.clone();
            App::new().route(
                "/v1/schemas",
                web::post().to(move |_payload: web::Json<serde_json::Value>| {
                    let counter = counter.clone();
                    async move {
                        let n = counter.fetch_add(1, Ordering::SeqCst);
                        if n < fail_count {
                            HttpResponse::InternalServerError().body("flaky")
                        } else {
                            let mut schema = Schema::new(
                                "flaky_schema".to_string(),
                                SchemaType::Single,
                                None,
                                Some(vec!["id".to_string()]),
                                None,
                                None,
                            );
                            schema.descriptive_name = Some("Flaky Schema".to_string());
                            HttpResponse::Created().json(AddSchemaResponse {
                                schema,
                                mutation_mappers: HashMap::new(),
                                replaced_schema: None,
                            })
                        }
                    }
                }),
            )
        })
        .listen(listener)
        .expect("failed to listen for flaky test server")
        .run();

        let handle = server.handle();
        actix_web::rt::spawn(server);
        sleep(Duration::from_millis(50)).await;

        (format!("http://127.0.0.1:{}", port), handle, counter)
    }

    #[actix_web::test]
    async fn add_schema_retries_on_5xx_and_eventually_succeeds() {
        // Fail twice with 500, succeed on third try.
        let (base_url, handle, counter) = spawn_flaky_add_schema(2).await;

        let client = SchemaServiceClient::new(&base_url);
        let mut schema = Schema::new(
            "probe".to_string(),
            SchemaType::Single,
            None,
            Some(vec!["id".to_string()]),
            None,
            None,
        );
        schema.descriptive_name = Some("Probe".to_string());

        let response = client
            .add_schema(&schema, HashMap::new())
            .await
            .expect("retry should succeed after 2 failures");

        assert_eq!(response.schema.name, "flaky_schema");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            3,
            "should have made 3 attempts"
        );

        handle.stop(true).await;
    }

    #[actix_web::test]
    async fn add_schema_gives_up_after_exhausting_retries() {
        // Always fail with 500.
        let (base_url, handle, counter) = spawn_flaky_add_schema(u32::MAX).await;

        let client = SchemaServiceClient::new(&base_url);
        let mut schema = Schema::new(
            "probe2".to_string(),
            SchemaType::Single,
            None,
            Some(vec!["id".to_string()]),
            None,
            None,
        );
        schema.descriptive_name = Some("Probe2".to_string());

        let result = client.add_schema(&schema, HashMap::new()).await;
        assert!(result.is_err(), "should fail after exhausting retries");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            3,
            "should have made exactly 3 attempts"
        );

        handle.stop(true).await;
    }

    async fn spawn_schema_service(
        state: SchemaServiceState,
    ) -> (String, actix_web::dev::ServerHandle) {
        let server_state = state.clone();

        let listener = TcpListener::bind(("127.0.0.1", 0))
            .expect("failed to bind schema service test listener");
        let bound_address = listener
            .local_addr()
            .expect("failed to read schema service test listener address");

        let server = HttpServer::new(move || {
            let state = server_state.clone();
            App::new().app_data(web::Data::new(state)).service(
                web::scope("/v1").route(
                    "/schemas",
                    web::post().to(
                        |payload: web::Json<AddSchemaRequest>,
                         state: web::Data<SchemaServiceState>| async move {
                            let request = payload.into_inner();
                            match state
                                .add_schema(request.schema, request.mutation_mappers)
                                .await
                            {
                                Ok(SchemaAddOutcome::Added(schema, mutation_mappers)) => {
                                    HttpResponse::Created().json(AddSchemaResponse {
                                        schema,
                                        mutation_mappers,
                                        replaced_schema: None,
                                    })
                                }
                                Ok(SchemaAddOutcome::AlreadyExists(schema, mutation_mappers)) => {
                                    HttpResponse::Ok().json(AddSchemaResponse {
                                        schema,
                                        mutation_mappers,
                                        replaced_schema: None,
                                    })
                                }
                                Ok(SchemaAddOutcome::Expanded(
                                    old_name,
                                    schema,
                                    mutation_mappers,
                                )) => HttpResponse::Created().json(AddSchemaResponse {
                                    schema,
                                    mutation_mappers,
                                    replaced_schema: Some(old_name),
                                }),
                                Err(error) => HttpResponse::BadRequest().json(ErrorResponse {
                                    error: format!("Failed to add schema: {}", error),
                                }),
                            }
                        },
                    ),
                ),
            )
        })
        .listen(listener)
        .expect("failed to listen for test schema service")
        .run();

        let address = bound_address;
        let handle = server.handle();
        actix_web::rt::spawn(server);
        sleep(Duration::from_millis(50)).await;

        (format!("http://{}", address), handle)
    }

    #[actix_web::test]
    async fn add_schema_succeeds() {
        let temp_dir = tempdir().expect("failed to create tempdir");
        let db_path = temp_dir
            .path()
            .join("test_schema_db")
            .to_string_lossy()
            .to_string();
        let state =
            SchemaServiceState::new(db_path).expect("failed to create schema service state");

        let (base_url, handle) = spawn_schema_service(state).await;

        let client = SchemaServiceClient::new(&base_url);
        let mut schema = Schema::new(
            "TestSchema".to_string(),
            SchemaType::Single,
            None,
            Some(vec!["id".to_string()]),
            None,
            None,
        );
        schema.descriptive_name = Some("Test Schema".to_string());
        schema
            .field_classifications
            .insert("id".to_string(), vec!["word".to_string()]);
        schema
            .field_descriptions
            .insert("id".to_string(), "unique identifier".to_string());
        schema.field_data_classifications.insert(
            "id".to_string(),
            fold_db::schema::types::DataClassification::new(0, "general").unwrap(),
        );

        let response = client
            .add_schema(&schema, HashMap::new())
            .await
            .expect("schema addition should succeed");

        // Schema name is now the identity hash (hash of descriptive_name + fields)
        // The readable name lives in descriptive_name
        assert_eq!(
            response.schema.descriptive_name,
            Some("Test Schema".to_string())
        );
        assert!(
            !response.schema.name.is_empty(),
            "schema name should be set"
        );

        handle.stop(true).await;
    }

    #[actix_web::test]
    async fn list_schemas_handles_objects() {
        let temp_dir = tempdir().expect("failed to create tempdir");
        let _db_path = temp_dir
            .path()
            .join("test_schema_list_db")
            .to_string_lossy()
            .to_string();

        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("failed to bind");
        let port = listener.local_addr().unwrap().port();

        let server = HttpServer::new(|| {
            App::new().route(
                "/v1/schemas",
                web::get().to(|| async {
                    HttpResponse::Ok().json(serde_json::json!({
                        "schemas": [
                            "simple_string_schema",
                            {"name": "object_schema", "state": "approved"},
                            {"schema": {"name": "nested_schema"}}
                        ]
                    }))
                }),
            )
        })
        .listen(listener)
        .expect("failed to listen")
        .run();

        let handle = server.handle();
        actix_web::rt::spawn(server);
        sleep(Duration::from_millis(50)).await;

        let client = SchemaServiceClient::new(&format!("http://127.0.0.1:{}", port));
        let schemas = client.list_schemas().await.expect("failed to list schemas");

        assert!(schemas.contains(&"simple_string_schema".to_string()));
        assert!(schemas.contains(&"object_schema".to_string()));
        assert!(schemas.contains(&"nested_schema".to_string()));
        assert_eq!(schemas.len(), 3);

        handle.stop(true).await;
    }
}
