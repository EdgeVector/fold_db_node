//! Field sensitivity classification inference.
//!
//! Determines the (sensitivity_level, data_domain) for new canonical fields
//! based on the field's description. The schema service is the sole authority
//! on data classification.
//!
//! Strategy:
//! 1. Existing canonical field → use its classification (handled by caller)
//! 2. New field → LLM call using field description (if ANTHROPIC_API_KEY set)
//! 3. Fallback: (0, "general") if no API key or LLM call fails

use fold_db::schema::types::data_classification::DataClassification;

/// Prompt for LLM-based classification of a single field.
fn build_classification_prompt(field_name: &str, description: &str) -> String {
    format!(
        r#"Classify this database field's data sensitivity. Return ONLY a JSON object with two fields, no explanation.

Field name: "{field_name}"
Description: "{description}"

Sensitivity levels:
0 = Public (freely distributable, no restrictions)
1 = Internal (not sensitive but not for public release)
2 = Confidential (business-sensitive, competitive value)
3 = Restricted (personally identifiable or individually attributable)
4 = Highly Restricted (regulated data: HIPAA, financial records, biometric)

Data domains: "general", "financial", "medical", "identity", "behavioral", "location"

Return format: {{"sensitivity_level": <0-4>, "data_domain": "<domain>"}}"#
    )
}

/// Classify a field using LLM analysis of its description.
/// Returns None if the API key is not set or the call fails.
pub async fn classify_with_llm(
    field_name: &str,
    description: &str,
) -> Option<DataClassification> {
    let api_key = std::env::var("ANTHROPIC_API_KEY").ok()?;
    if api_key.trim().is_empty() {
        return None;
    }

    let prompt = build_classification_prompt(field_name, description);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .no_proxy()
        .build()
        .ok()?;

    let request_body = serde_json::json!({
        "model": "claude-haiku-4-5-20251001",
        "messages": [{"role": "user", "content": prompt}],
        "max_tokens": 100,
        "temperature": 0.0
    });

    let response = client
        .post("https://api.anthropic.com/v1/messages")
        .header("x-api-key", &api_key)
        .header("anthropic-version", "2023-06-01")
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        fold_db::log_feature!(
            fold_db::logging::features::LogFeature::Schema,
            warn,
            "Classification LLM call failed with status {} for field '{}'",
            response.status(),
            field_name
        );
        return None;
    }

    let resp: serde_json::Value = response.json().await.ok()?;
    let text = resp
        .get("content")?
        .as_array()?
        .first()?
        .get("text")?
        .as_str()?;

    // Parse the JSON response — try raw text first, then extract from markdown fence
    let parsed: Result<DataClassification, _> = serde_json::from_str(text).or_else(|_| {
        let trimmed = text.trim();
        let json_str = trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .and_then(|s| s.strip_suffix("```"))
            .unwrap_or(trimmed)
            .trim();
        serde_json::from_str(json_str)
    });

    match parsed {
        Ok(classification) => {
            fold_db::log_feature!(
                fold_db::logging::features::LogFeature::Schema,
                info,
                "LLM classified field '{}' as ({}, {})",
                field_name,
                classification.sensitivity_level,
                classification.data_domain
            );
            Some(classification)
        }
        Err(e) => {
            fold_db::log_feature!(
                fold_db::logging::features::LogFeature::Schema,
                warn,
                "Failed to parse LLM classification for field '{}': {} (raw: {})",
                field_name,
                e,
                text
            );
            None
        }
    }
}

/// Infer classification for a new canonical field.
///
/// ```text
/// caller-provided? ──yes──▶ use it
///       │ no
///       ▼
/// LLM call (ANTHROPIC_API_KEY) ──success──▶ use it
///       │ no key / failure
///       ▼
/// fallback: (0, "general")
/// ```
pub async fn infer_classification(
    field_name: &str,
    description: &str,
    caller_provided: Option<&DataClassification>,
) -> DataClassification {
    // 1. Caller-provided classification takes priority
    if let Some(c) = caller_provided {
        return c.clone();
    }

    // 2. LLM-based classification from field description
    if let Some(c) = classify_with_llm(field_name, description).await {
        return c;
    }

    // 3. Fallback: Public/General
    DataClassification::new(0, "general").expect("default classification is always valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn infer_uses_caller_provided_first() {
        let caller = DataClassification::new(4, "medical").unwrap();
        let result = infer_classification(
            "diagnosis",
            "patient diagnosis",
            Some(&caller),
        )
        .await;
        assert_eq!(result.sensitivity_level, 4);
        assert_eq!(result.data_domain, "medical");
    }

    #[tokio::test]
    async fn infer_falls_back_to_default_without_api_key() {
        // No ANTHROPIC_API_KEY set → falls back to (0, "general")
        let result = infer_classification(
            "some_field",
            "a miscellaneous field",
            None,
        )
        .await;
        assert_eq!(result.sensitivity_level, 0);
        assert_eq!(result.data_domain, "general");
    }
}
