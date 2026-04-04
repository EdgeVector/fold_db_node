//! AI interaction methods for IngestionService.

use super::IngestionService;
use crate::ingestion::{AISchemaResponse, IngestionError, IngestionResult};
use fold_db::log_feature;
use fold_db::logging::features::LogFeature;
use serde_json::Value;

impl IngestionService {
    /// Call the underlying AI API with a raw prompt string.
    ///
    /// This is the low-level API used by smart_folder scanning and other
    /// components that need raw AI text completion without schema parsing.
    pub async fn call_ai_raw(&self, prompt: &str) -> IngestionResult<String> {
        let detail = self.init_error.as_deref().unwrap_or("unknown reason");
        self.backend
            .as_ref()
            .ok_or_else(|| {
                IngestionError::configuration_error(format!(
                    "{:?} backend not initialized ({})",
                    self.config.provider, detail
                ))
            })?
            .call(prompt)
            .await
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

        for attempt in 1..=max_validation_attempts {
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
                            let fd = schema_def
                                .as_object_mut()
                                .unwrap()
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
