//! Field data classification inference.
//!
//! Determines the (sensitivity_level, data_domain) for a field based on its
//! description and classification tags. The schema service is the sole authority
//! on data classification.
//!
//! Inference strategy:
//! 1. Rule-based: map field_classifications tags to known sensitivity/domain
//! 2. LLM-based: analyze field description with Anthropic API (if ANTHROPIC_API_KEY set)
//! 3. Fallback: (0, "general") if no signal available

use fold_db::schema::types::data_classification::DataClassification;

/// Infer classification from field_classifications tags (fast, no network).
///
/// Maps known tag patterns to sensitivity levels and data domains:
/// ```text
/// Tag              → Level  Domain
/// ─────────────────────────────────
/// name:person      → 3      identity
/// email            → 3      identity
/// phone            → 3      identity
/// username         → 2      identity
/// name:company     → 1      general
/// name:place       → 0      location
/// date             → 0      general
/// hashtag          → 0      general
/// url              → 0      general
/// number           → 0      general
/// word             → 0      general
/// ```
///
/// When multiple tags are present, the highest sensitivity wins.
pub fn classify_from_tags(tags: &[String]) -> Option<DataClassification> {
    if tags.is_empty() {
        return None;
    }

    let mut best_level: u8 = 0;
    let mut best_domain = "general".to_string();
    let mut has_signal = false;

    for tag in tags {
        let (level, domain) = match tag.as_str() {
            "name:person" => (3, "identity"),
            "email" => (3, "identity"),
            "phone" => (3, "identity"),
            "username" => (2, "identity"),
            "name:company" => (1, "general"),
            "name:place" => (0, "location"),
            "date" => (0, "general"),
            "hashtag" => (0, "general"),
            "url" => (0, "general"),
            "number" => (0, "general"),
            "word" => continue, // "word" is too generic to be a signal
            _ => continue,
        };
        has_signal = true;
        // Higher sensitivity wins; at equal level, prefer a specific domain over "general"
        if level > best_level || (level == best_level && best_domain == "general" && domain != "general") {
            best_level = level;
            best_domain = domain.to_string();
        }
    }

    if !has_signal {
        return None;
    }

    DataClassification::new(best_level, best_domain).ok()
}

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

    // Parse the JSON response — try the raw text first, then extract JSON from markdown
    let parsed: Result<DataClassification, _> = serde_json::from_str(text)
        .or_else(|_| {
            // LLM might wrap in ```json ... ```
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

/// Infer classification for a field using all available signals.
/// Priority: caller-provided > tag-based > LLM > fallback (0, "general").
pub async fn infer_classification(
    field_name: &str,
    description: &str,
    tags: Option<&Vec<String>>,
    caller_provided: Option<&DataClassification>,
) -> DataClassification {
    // 1. Caller-provided classification takes priority
    if let Some(c) = caller_provided {
        return c.clone();
    }

    // 2. Try rule-based inference from tags
    if let Some(tags) = tags {
        if let Some(c) = classify_from_tags(tags) {
            return c;
        }
    }

    // 3. Try LLM-based classification from field description
    if let Some(c) = classify_with_llm(field_name, description).await {
        return c;
    }

    // 4. Fallback: Public/General
    DataClassification::new(0, "general").expect("default classification is always valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tag_person_name_is_restricted_identity() {
        let tags = vec!["name:person".to_string(), "word".to_string()];
        let c = classify_from_tags(&tags).unwrap();
        assert_eq!(c.sensitivity_level, 3);
        assert_eq!(c.data_domain, "identity");
    }

    #[test]
    fn tag_email_is_restricted_identity() {
        let tags = vec!["email".to_string()];
        let c = classify_from_tags(&tags).unwrap();
        assert_eq!(c.sensitivity_level, 3);
        assert_eq!(c.data_domain, "identity");
    }

    #[test]
    fn tag_phone_is_restricted_identity() {
        let tags = vec!["phone".to_string()];
        let c = classify_from_tags(&tags).unwrap();
        assert_eq!(c.sensitivity_level, 3);
        assert_eq!(c.data_domain, "identity");
    }

    #[test]
    fn tag_username_is_confidential_identity() {
        let tags = vec!["username".to_string()];
        let c = classify_from_tags(&tags).unwrap();
        assert_eq!(c.sensitivity_level, 2);
        assert_eq!(c.data_domain, "identity");
    }

    #[test]
    fn tag_date_is_public_general() {
        let tags = vec!["date".to_string()];
        let c = classify_from_tags(&tags).unwrap();
        assert_eq!(c.sensitivity_level, 0);
        assert_eq!(c.data_domain, "general");
    }

    #[test]
    fn tag_place_is_public_location() {
        let tags = vec!["name:place".to_string()];
        let c = classify_from_tags(&tags).unwrap();
        assert_eq!(c.sensitivity_level, 0);
        assert_eq!(c.data_domain, "location");
    }

    #[test]
    fn multiple_tags_highest_wins() {
        let tags = vec![
            "word".to_string(),
            "date".to_string(),      // level 0
            "name:person".to_string(), // level 3
        ];
        let c = classify_from_tags(&tags).unwrap();
        assert_eq!(c.sensitivity_level, 3);
        assert_eq!(c.data_domain, "identity");
    }

    #[test]
    fn word_only_returns_none() {
        let tags = vec!["word".to_string()];
        assert!(classify_from_tags(&tags).is_none());
    }

    #[test]
    fn empty_tags_returns_none() {
        let tags: Vec<String> = vec![];
        assert!(classify_from_tags(&tags).is_none());
    }

    #[test]
    fn unknown_tags_return_none() {
        let tags = vec!["custom_tag".to_string()];
        assert!(classify_from_tags(&tags).is_none());
    }

    #[tokio::test]
    async fn infer_uses_caller_provided_first() {
        let caller = DataClassification::new(4, "medical").unwrap();
        let result = infer_classification(
            "diagnosis",
            "patient diagnosis",
            Some(&vec!["word".to_string()]),
            Some(&caller),
        ).await;
        assert_eq!(result.sensitivity_level, 4);
        assert_eq!(result.data_domain, "medical");
    }

    #[tokio::test]
    async fn infer_uses_tags_when_no_caller() {
        let result = infer_classification(
            "email_address",
            "user email address",
            Some(&vec!["email".to_string()]),
            None,
        ).await;
        assert_eq!(result.sensitivity_level, 3);
        assert_eq!(result.data_domain, "identity");
    }

    #[tokio::test]
    async fn infer_falls_back_to_default_without_api_key() {
        // No ANTHROPIC_API_KEY set, no tags with signal → falls back to (0, "general")
        let result = infer_classification(
            "some_field",
            "a miscellaneous field",
            Some(&vec!["word".to_string()]),
            None,
        ).await;
        assert_eq!(result.sensitivity_level, 0);
        assert_eq!(result.data_domain, "general");
    }
}
