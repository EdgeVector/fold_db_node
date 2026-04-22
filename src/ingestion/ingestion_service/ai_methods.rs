//! AI interaction methods for IngestionService.

use super::IngestionService;
use crate::ingestion::{AISchemaResponse, IngestionError, IngestionResult, Role};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde_json::Value;

impl IngestionService {
    /// Call the underlying AI API with a raw prompt string, tagged as
    /// `Role::IngestionText`. Uses the cached backend from construction.
    ///
    /// For other roles (smart_folder classifier, discovery interests,
    /// mutation agent) call [`Self::call_ai_raw_as`] — it builds a
    /// role-tagged backend on demand so per-role metrics stay accurate.
    pub async fn call_ai_raw(&self, prompt: &str) -> IngestionResult<String> {
        let detail = self.init_error.as_deref().unwrap_or("unknown reason");
        let resolved = self.config.resolve(Role::IngestionText);
        self.backend
            .as_ref()
            .ok_or_else(|| {
                IngestionError::configuration_error(format!(
                    "{:?} backend not initialized ({})",
                    resolved.provider, detail
                ))
            })?
            .call(prompt)
            .await
    }

    /// Call the underlying AI API with a role tag. Builds a fresh
    /// role-tagged backend per call — overhead is a small reqwest::Client
    /// construction relative to the LLM round-trip. Records metrics against
    /// the shared store so `/api/ingestion/stats` counts this role
    /// correctly.
    ///
    /// Returns an error if the resolved provider for `role` can't be
    /// initialised (missing Anthropic key, empty Ollama URL, etc.).
    pub async fn call_ai_raw_as(&self, role: Role, prompt: &str) -> IngestionResult<String> {
        // Fast path: the default IngestionText backend is already cached.
        if role == Role::IngestionText {
            return self.call_ai_raw(prompt).await;
        }
        let (backend, err) = self
            .config
            .build_backend_with_metrics(role, self.metrics.clone());
        match backend {
            Some(b) => b.call(prompt).await,
            None => Err(IngestionError::configuration_error(format!(
                "Role::{:?} backend not initialised ({})",
                role,
                err.unwrap_or_else(|| "unknown reason".to_string())
            ))),
        }
    }

    /// Get AI schema recommendation with validation retries.
    ///
    /// Builds the prompt once, then retries the AI call if response parsing fails
    /// (e.g., malformed JSON, missing required fields). Network-level retries are
    /// handled separately inside `call_ai_raw`.
    pub(super) async fn get_ai_recommendation(
        &self,
        json_data: &Value,
    ) -> IngestionResult<AISchemaResponse> {
        use crate::ingestion::ai::helpers::{analyze_and_build_prompt, parse_ai_response};

        let base_prompt = analyze_and_build_prompt(json_data)?;
        let max_validation_attempts = self.config.max_retries.clamp(1, 3);
        let mut last_error = None;
        // Overall deadline prevents retry sequences from accumulating indefinitely.
        // Individual calls have their own timeout, but retries + backoff can exceed it.
        let deadline =
            std::time::Instant::now() + std::time::Duration::from_secs(self.config.timeout_seconds);

        for attempt in 1..=max_validation_attempts {
            if std::time::Instant::now() > deadline {
                return Err(IngestionError::ai_response_validation_error(format!(
                    "AI recommendation deadline exceeded ({}s) after {} attempts",
                    self.config.timeout_seconds,
                    attempt - 1
                )));
            }

            // On retries, append the previous validation error so the model
            // can correct its output instead of repeating the same mistake.
            let prompt = match &last_error {
                Some(err) if attempt > 1 => format!(
                    "{}\n\nYour previous response was rejected: {}. Fix this issue.",
                    base_prompt, err
                ),
                _ => base_prompt.clone(),
            };

            let raw_response = self.call_ai_raw(&prompt).await?;

            match parse_ai_response(&raw_response) {
                Ok(response) => return Ok(response),
                Err(e) => {
                    log_feature!(
                        LogFeature::Ingestion,
                        warn,
                        "AI response validation failed on attempt {}/{}: {}",
                        attempt,
                        max_validation_attempts,
                        e
                    );
                    last_error = Some(e);

                    if attempt < max_validation_attempts {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            IngestionError::ai_response_validation_error(
                "All AI attempts returned invalid responses",
            )
        }))
    }

    /// Second AI pass: fill in missing field_descriptions on the schema.
    ///
    /// If the initial schema proposal is missing field_descriptions, this method
    /// calls the AI with a focused prompt that only asks for descriptions, given
    /// the JSON data and field names. This is more reliable than expecting the
    /// schema proposal prompt to always produce descriptions.
    pub(super) async fn fill_missing_field_descriptions(
        &self,
        ai_response: &mut AISchemaResponse,
        json_data: &Value,
    ) -> IngestionResult<()> {
        let schema_def = match ai_response.new_schemas.as_mut() {
            Some(def) => def,
            None => return Ok(()),
        };

        let fields: Vec<String> = schema_def
            .get("fields")
            .and_then(|f| f.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        if fields.is_empty() {
            return Ok(());
        }

        let existing_descriptions = schema_def
            .get("field_descriptions")
            .and_then(|v| v.as_object())
            .cloned()
            .unwrap_or_default();

        let missing: Vec<&String> = fields
            .iter()
            .filter(|f| !existing_descriptions.contains_key(f.as_str()))
            .collect();

        if missing.is_empty() {
            return Ok(());
        }

        log_feature!(
            LogFeature::Ingestion,
            info,
            "Schema missing field_descriptions for {:?}, calling AI for descriptions",
            missing
        );

        // Build a compact sample for the prompt — truncate long string fields
        // (e.g. full markdown bodies) to avoid Ollama timeouts.
        let raw_sample = if let Some(array) = json_data.as_array() {
            serde_json::json!(array.iter().take(2).collect::<Vec<_>>())
        } else {
            json_data.clone()
        };
        let sample = crate::ingestion::ai::helpers::truncate_long_strings(&raw_sample);

        let prompt = fold_db::llm_registry::prompts::ingestion::FIELD_DESCRIPTIONS_PROMPT
            .replace(
                "{sample}",
                &serde_json::to_string_pretty(&sample).unwrap_or_default(),
            )
            .replace("{fields}", &format!("{:?}", missing));

        match self.call_ai_raw(&prompt).await {
            Ok(raw_response) => {
                match crate::ingestion::ai::helpers::extract_json_from_response(&raw_response) {
                    Ok(json_str) => {
                        if let Ok(descriptions) =
                            serde_json::from_str::<serde_json::Map<String, Value>>(&json_str)
                        {
                            let schema_obj = schema_def.as_object_mut().ok_or_else(|| {
                                IngestionError::ai_response_validation_error(
                                    "new_schemas must be a JSON object to backfill field_descriptions",
                                )
                            })?;
                            let fd = schema_obj
                                .entry("field_descriptions")
                                .or_insert_with(|| Value::Object(serde_json::Map::new()));
                            if let Some(fd_obj) = fd.as_object_mut() {
                                for (field, desc) in descriptions {
                                    if desc.is_string() {
                                        fd_obj.entry(&field).or_insert(desc);
                                    }
                                }
                            }
                            log_feature!(
                                LogFeature::Ingestion,
                                info,
                                "AI provided field descriptions for missing fields"
                            );
                        }
                    }
                    Err(e) => {
                        log_feature!(
                            LogFeature::Ingestion,
                            warn,
                            "Failed to parse field descriptions AI response: {}",
                            e
                        );
                    }
                }
            }
            Err(e) => {
                log_feature!(
                    LogFeature::Ingestion,
                    warn,
                    "Failed to get field descriptions from AI: {}",
                    e
                );
            }
        }

        Ok(())
    }
}
