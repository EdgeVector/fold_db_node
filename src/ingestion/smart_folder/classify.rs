//! LLM-based file classification, image-directory classification, and heuristic fallback.

use crate::ingestion::error::IngestionError;
use crate::ingestion::IngestionResult;
use super::scanner::IMAGE_EXTS;
use super::types::{FileRecommendation, file_size_bytes};
use std::collections::{HashMap, HashSet};
use std::path::Path;

// ---- Image directory classification ----

/// Group image files by parent directory and use the LLM to classify
/// which directories contain personal images vs. asset/scaffolding images.
///
/// Returns a list of `FileRecommendation`s for image files that were removed
/// from `llm_candidates` (non-personal image directories). The caller should
/// add these to the skipped files list.
///
/// If no LLM service is available, returns an empty vec (all images stay as candidates).
pub(crate) async fn classify_image_directories(
    llm_candidates: &mut Vec<String>,
    folder_path: &Path,
    service: Option<&crate::ingestion::ingestion_service::IngestionService>,
    report: &(dyn Fn(u8, String) + Send + Sync),
) -> IngestionResult<Vec<FileRecommendation>> {
    let image_exts: HashSet<&str> = IMAGE_EXTS.iter().copied().collect();

    // Group image files by parent directory
    let mut image_dirs: HashMap<String, Vec<String>> = HashMap::new();
    for path in llm_candidates.iter() {
        let ext = Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();
        if image_exts.contains(ext.as_str()) {
            let parent = Path::new(path)
                .parent()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            image_dirs.entry(parent).or_default().push(path.clone());
        }
    }

    if image_dirs.is_empty() {
        return Ok(Vec::new());
    }

    report(16, format!(
        "Found {} image directories with {} total images — classifying...",
        image_dirs.len(),
        image_dirs.values().map(|v| v.len()).sum::<usize>(),
    ));

    // Try LLM classification; fall back to keeping all images if unavailable
    let svc = match service {
        Some(s) => s,
        None => {
            match crate::ingestion::ingestion_service::IngestionService::from_env() {
                Ok(_) => {
                    // We can't hold an owned service across await, just skip classification
                    log::info!("No ingestion service provided for image classification, keeping all images");
                    return Ok(Vec::new());
                }
                Err(_) => {
                    log::info!("No LLM available for image directory classification, keeping all images");
                    return Ok(Vec::new());
                }
            }
        }
    };

    // Build a prompt listing each image directory with file count and sample filenames
    let prompt = create_image_directory_prompt(&image_dirs, folder_path);

    let llm_response = match call_llm_for_file_analysis(&prompt, svc).await {
        Ok(r) => r,
        Err(e) => {
            log::warn!("LLM unavailable for image directory classification: {}. Keeping all images.", e);
            return Ok(Vec::new());
        }
    };

    // Parse response: expect JSON object mapping directory → "personal" or "asset"
    let non_personal_dirs = parse_image_directory_response(&llm_response, &image_dirs);

    // Remove non-personal image files from llm_candidates and build skip records
    let mut skipped_recs = Vec::new();
    let mut remove_set: HashSet<String> = HashSet::new();

    for dir in &non_personal_dirs {
        if let Some(files) = image_dirs.get(dir) {
            for f in files {
                remove_set.insert(f.clone());
                skipped_recs.push(FileRecommendation {
                    path: f.clone(),
                    should_ingest: false,
                    category: "non_personal_media".to_string(),
                    reason: format!("Image directory '{}' classified as non-personal assets", dir),
                    file_size_bytes: file_size_bytes(Path::new(f), folder_path),
                    estimated_cost: 0.0,
                    already_ingested: false,
                });
            }
        }
    }

    if !skipped_recs.is_empty() {
        llm_candidates.retain(|p| !remove_set.contains(p));
        fold_db::log_feature!(
            fold_db::logging::features::LogFeature::Ingestion,
            info,
            "Image directory classification: {} images in {} non-personal dirs removed",
            skipped_recs.len(),
            non_personal_dirs.len(),
        );
    }

    Ok(skipped_recs)
}

/// Build an LLM prompt to classify image directories as personal or asset.
fn create_image_directory_prompt(
    image_dirs: &HashMap<String, Vec<String>>,
    _folder_path: &Path,
) -> String {
    let mut dir_lines = Vec::new();
    let mut sorted_dirs: Vec<_> = image_dirs.iter().collect();
    sorted_dirs.sort_by_key(|(dir, _)| (*dir).clone());

    for (dir, files) in &sorted_dirs {
        let display_dir = if dir.is_empty() { "(root)" } else { dir.as_str() };
        // Show up to 5 sample filenames
        let samples: Vec<&str> = files.iter().take(5).map(|f| {
            Path::new(f.as_str())
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(f.as_str())
        }).collect();
        let sample_str = samples.join(", ");
        let more = if files.len() > 5 {
            format!(" (+{} more)", files.len() - 5)
        } else {
            String::new()
        };
        dir_lines.push(format!("- {}: {} files [{}{}]", display_dir, files.len(), sample_str, more));
    }

    format!(
        r#"You are classifying IMAGE DIRECTORIES to determine if they contain personal images or non-personal asset images.

IMAGE DIRECTORIES (with file counts and sample filenames):
{}

For each directory, classify it as either:
- "personal" — user photos, screenshots, personal artwork, scanned documents, camera images
- "asset" — UI assets, emoji/icon collections, website graphics, app resources, stock images, thumbnails

GUIDELINES:
- Directories named like "tweets_media", "profile_media", "photos", "camera", "screenshots" → personal
- Directories named like "twemoji", "emoji", "icons", "assets/images", "thumbnails", "sprites" → asset
- Directories with few large files (photos) → likely personal
- Directories with many small files (icons, emoji) → likely asset
- When in doubt, classify as "personal" (better to include than exclude)

Respond with a JSON object mapping each directory path to "personal" or "asset":
```json
{{
  "directory/path": "personal",
  "assets/images/twemoji": "asset"
}}
```

Only return the JSON object, no other text."#,
        dir_lines.join("\n")
    )
}

/// Parse the LLM response for image directory classification.
/// Returns the set of directory paths classified as non-personal (asset).
fn parse_image_directory_response(
    response: &str,
    image_dirs: &HashMap<String, Vec<String>>,
) -> Vec<String> {
    let json_str = match crate::ingestion::ai::helpers::extract_json_from_response(response) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("Failed to extract JSON from image directory response: {}", e);
            return Vec::new();
        }
    };

    let parsed: HashMap<String, String> = match serde_json::from_str(&json_str) {
        Ok(m) => m,
        Err(e) => {
            log::warn!("Failed to parse image directory JSON: {}", e);
            return Vec::new();
        }
    };

    let valid_dirs: HashSet<&String> = image_dirs.keys().collect();

    parsed
        .into_iter()
        .filter(|(dir, classification)| {
            valid_dirs.contains(dir) && classification.to_lowercase() == "asset"
        })
        .map(|(dir, _)| dir)
        .collect()
}

// ---- Scan result adjustment via natural language ----

/// Build an LLM prompt for adjusting scan results based on a user instruction.
pub fn create_adjust_prompt(
    instruction: &str,
    recommended: &[FileRecommendation],
    skipped: &[FileRecommendation],
) -> String {
    let mut rec_lines = Vec::new();
    for f in recommended {
        rec_lines.push(format!(
            "  {{\"path\": \"{}\", \"should_ingest\": true, \"category\": \"{}\", \"reason\": \"{}\"}}",
            f.path, f.category, f.reason
        ));
    }
    let mut skip_lines = Vec::new();
    for f in skipped {
        if !f.already_ingested {
            skip_lines.push(format!(
                "  {{\"path\": \"{}\", \"should_ingest\": false, \"category\": \"{}\", \"reason\": \"{}\"}}",
                f.path, f.category, f.reason
            ));
        }
    }

    format!(
        r#"You are adjusting file ingestion recommendations based on the user's instruction.

USER INSTRUCTION: "{instruction}"

CURRENT FILES TO INGEST:
[
{rec_list}
]

CURRENT SKIPPED FILES:
[
{skip_list}
]

Apply the user's instruction to reclassify files. For example:
- "include all work files" → move work-category files from skipped to should_ingest=true
- "skip all images" → move image files from recommended to should_ingest=false
- "include everything" → set all files to should_ingest=true

CATEGORIES:
- personal_data: Personal documents, notes, journals, financial records, health data, creative work
- media: Images, videos, audio that are user-created content
- config: Application configs, settings files
- website_scaffolding: HTML templates, CSS, JS bundles, emoji assets
- work: Work/corporate files, professional documents
- unknown: Cannot determine

Respond with a JSON array of ALL files (both recommended and skipped) with updated classifications:
```json
[
  {{"path": "file/path.ext", "should_ingest": true, "category": "personal_data", "reason": "Brief reason"}},
  ...
]
```

Only return the JSON array, no other text."#,
        rec_list = rec_lines.join(",\n"),
        skip_list = skip_lines.join(",\n"),
    )
}

/// Merge LLM adjustment results with existing file metadata (sizes, costs, etc.).
/// Returns the full list of files with updated should_ingest/category/reason
/// but preserving file_size_bytes, estimated_cost, and already_ingested from originals.
pub fn merge_adjust_results(
    originals: &[FileRecommendation],
    llm_updates: &[FileRecommendation],
) -> Vec<FileRecommendation> {
    let update_map: HashMap<&str, &FileRecommendation> =
        llm_updates.iter().map(|f| (f.path.as_str(), f)).collect();

    originals
        .iter()
        .map(|orig| {
            if let Some(updated) = update_map.get(orig.path.as_str()) {
                FileRecommendation {
                    path: orig.path.clone(),
                    should_ingest: updated.should_ingest,
                    category: updated.category.clone(),
                    reason: updated.reason.clone(),
                    file_size_bytes: orig.file_size_bytes,
                    estimated_cost: orig.estimated_cost,
                    already_ingested: orig.already_ingested,
                }
            } else {
                orig.clone()
            }
        })
        .collect()
}

// ---- LLM-based file classification and heuristic fallback ----

/// Create the LLM prompt for file analysis with directory tree context.
///
/// The prompt includes the full directory tree so the LLM can reason about
/// what folders represent (e.g. a .gif inside a "Bank of America" HTML save
/// is scaffolding, not personal media).
pub fn create_smart_folder_prompt(tree_display: &str, file_paths: &[String]) -> String {
    let files_list = file_paths.join("\n");

    format!(
        r#"You are classifying files in a user's personal folder for ingestion into their personal database.

DIRECTORY TREE (for context — understand what each folder represents):
{tree_display}

FILES TO CLASSIFY:
{files_list}

For each file path listed in FILES TO CLASSIFY, determine:
1. Should it be ingested into the user's personal database?
2. What category does it belong to?

IMPORTANT: Use the directory tree to understand context. For example:
- A .gif inside a "Bank of America" saved HTML page is website scaffolding, NOT personal media
- A .js file inside a Twitter data export IS personal data
- A .css or .html file inside a saved webpage folder is scaffolding
- A .pdf in a "Statements" folder IS personal financial data
- Source code files (.py, .rs, .js) in a code project folder are NOT personal data
- But a .py notebook in a "Research" folder might be personal work

CATEGORIES:
- personal_data: Personal documents, notes, journals, financial records, health data, creative work, personal projects
- media: Images, videos, audio that are user-created content (NOT UI assets or website graphics)
- config: Application configs, settings files, dotfiles
- website_scaffolding: HTML templates, CSS, JS bundles, emoji assets, fonts, saved webpage resources
- work: Work/corporate files, professional documents
- unknown: Cannot determine

SKIP CRITERIA (should_ingest = false):
- Website scaffolding (CSS, JS bundles, images that are part of saved web pages)
- Application config files
- Source code (unless it's personal creative work)
- Cache and temporary files
- Downloaded installers/archives

INGEST CRITERIA (should_ingest = true):
- Personal documents (letters, notes, journals)
- Photos and videos (user-created, not UI assets)
- Messages and chat logs
- Financial records (statements, budgets, tax documents)
- Health data
- Creative work (writing, art, music)
- Data exports from services (Twitter, Facebook, Google Takeout, etc.)
- Personal work output (reports, presentations, research notes)

When in doubt, set should_ingest to false.

Respond with a JSON array of objects:
```json
[
  {{"path": "file/path.ext", "should_ingest": true, "category": "personal_data", "reason": "Brief reason"}},
  ...
]
```

Only return the JSON array, no other text."#
    )
}

/// Call the LLM for file analysis using the provided IngestionService
pub async fn call_llm_for_file_analysis(
    prompt: &str,
    service: &crate::ingestion::ingestion_service::IngestionService,
) -> IngestionResult<String> {
    service.call_ai_raw(prompt).await
}

/// Parse LLM response into file recommendations
pub fn parse_llm_file_recommendations(
    response: &str,
    file_tree: &[String],
) -> IngestionResult<Vec<FileRecommendation>> {
    let json_str = crate::ingestion::ai::helpers::extract_json_from_response(response)?;

    let parsed: Vec<FileRecommendation> = serde_json::from_str(&json_str)
        .map_err(|e| IngestionError::InvalidInput(format!("Failed to parse JSON: {}", e)))?;

    // Validate that paths exist in our file tree
    let file_set: HashSet<&str> = file_tree.iter().map(|s| s.as_str()).collect();

    let valid_recs: Vec<FileRecommendation> = parsed
        .into_iter()
        .filter(|rec| file_set.contains(rec.path.as_str()))
        .collect();

    Ok(valid_recs)
}

/// Apply conservative heuristic-based filtering when LLM fails.
/// When in doubt, marks files as should_ingest = false.
pub fn apply_heuristic_filtering(file_tree: &[String]) -> Vec<FileRecommendation> {
    file_tree
        .iter()
        .map(|path| {
            let lower = path.to_lowercase();
            let ext = Path::new(path)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            // Strong personal data signals (documents with well-known personal formats)
            let is_personal_doc = matches!(
                ext.as_str(),
                "doc" | "docx" | "pdf" | "rtf" | "odt" | "pages"
                    | "xlsx" | "xls" | "csv" | "ods" | "numbers"
                    | "pptx" | "ppt" | "odp" | "key"
                    | "eml" | "mbox" | "vcf"
            );

            // Strong media signals
            let is_media = matches!(
                ext.as_str(),
                "jpg" | "jpeg" | "png" | "gif" | "heic" | "heif" | "webp" | "bmp" | "tiff"
                    | "raw" | "cr2" | "nef" | "arw"
                    | "mp4" | "mov" | "avi" | "mkv" | "m4v" | "wmv"
                    | "mp3" | "wav" | "flac" | "aac" | "m4a" | "ogg" | "wma"
            );

            // Data export patterns (high confidence personal data)
            let is_data_export = lower.contains("export")
                || lower.contains("backup")
                || lower.contains("takeout");

            let (should_ingest, category, reason) = if is_personal_doc {
                (true, "personal_data", "Personal document file")
            } else if is_media && is_data_export {
                (true, "media", "Media in data export")
            } else if is_data_export {
                (true, "personal_data", "Data export file")
            } else if is_media {
                (true, "media", "Media file")
            } else {
                (false, "unknown", "Could not classify without AI")
            };

            FileRecommendation {
                path: path.clone(),
                should_ingest,
                category: category.to_string(),
                reason: reason.to_string(),
                file_size_bytes: 0,
                estimated_cost: 0.0,
                already_ingested: false,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_llm_valid_json_with_matching_paths() {
        let response = r#"```json
[
  {"path": "docs/notes.txt", "should_ingest": true, "category": "personal_data", "reason": "Personal notes"},
  {"path": "photos/pic.jpg", "should_ingest": true, "category": "media", "reason": "Photo"}
]
```"#;
        let file_tree = vec![
            "docs/notes.txt".to_string(),
            "photos/pic.jpg".to_string(),
        ];
        let result = parse_llm_file_recommendations(response, &file_tree).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].path, "docs/notes.txt");
        assert_eq!(result[1].path, "photos/pic.jpg");
    }

    #[test]
    fn test_parse_llm_hallucinated_paths_filtered() {
        let response = r#"[
  {"path": "docs/notes.txt", "should_ingest": true, "category": "personal_data", "reason": "ok"},
  {"path": "fake/hallucinated.txt", "should_ingest": true, "category": "unknown", "reason": "nope"}
]"#;
        let file_tree = vec!["docs/notes.txt".to_string()];
        let result = parse_llm_file_recommendations(response, &file_tree).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].path, "docs/notes.txt");
    }

    #[test]
    fn test_parse_llm_empty_response_returns_error() {
        let file_tree = vec!["a.txt".to_string()];
        let result = parse_llm_file_recommendations("", &file_tree);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_llm_mixed_valid_invalid_paths() {
        let response = r#"[
  {"path": "a.txt", "should_ingest": true, "category": "personal_data", "reason": "ok"},
  {"path": "b.txt", "should_ingest": false, "category": "unknown", "reason": "nope"},
  {"path": "c.txt", "should_ingest": true, "category": "work", "reason": "work file"}
]"#;
        let file_tree = vec!["a.txt".to_string(), "c.txt".to_string()];
        let result = parse_llm_file_recommendations(response, &file_tree).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].path, "a.txt");
        assert_eq!(result[1].path, "c.txt");
    }

    #[test]
    fn test_parse_llm_empty_file_tree_returns_empty() {
        let response = r#"[
  {"path": "a.txt", "should_ingest": true, "category": "personal_data", "reason": "ok"}
]"#;
        let file_tree: Vec<String> = vec![];
        let result = parse_llm_file_recommendations(response, &file_tree).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_parse_llm_malformed_json_returns_error() {
        let response = r#"This is not JSON at all, just some text."#;
        let file_tree = vec!["a.txt".to_string()];
        let result = parse_llm_file_recommendations(response, &file_tree);
        assert!(result.is_err());
    }

    // ---- create_smart_folder_prompt tests ----

    #[test]
    fn test_prompt_contains_tree_and_file_paths() {
        let tree = "docs/\n  notes.txt\n  report.pdf";
        let files = vec![
            "docs/notes.txt".to_string(),
            "docs/report.pdf".to_string(),
        ];
        let prompt = create_smart_folder_prompt(tree, &files);
        assert!(prompt.contains(tree));
        assert!(prompt.contains("docs/notes.txt"));
        assert!(prompt.contains("docs/report.pdf"));
    }

    #[test]
    fn test_prompt_contains_categories_and_instructions() {
        let prompt = create_smart_folder_prompt("tree", &["f.txt".to_string()]);
        assert!(prompt.contains("personal_data"));
        assert!(prompt.contains("media"));
        assert!(prompt.contains("website_scaffolding"));
        assert!(prompt.contains("should_ingest"));
        assert!(prompt.contains("JSON array"));
    }

    // ---- apply_heuristic_filtering tests ----

    #[test]
    fn test_heuristic_filtering_personal_doc() {
        let files = vec!["reports/q1.pdf".to_string()];
        let recs = apply_heuristic_filtering(&files);
        assert_eq!(recs.len(), 1);
        assert!(recs[0].should_ingest);
        assert_eq!(recs[0].category, "personal_data");
    }

    #[test]
    fn test_heuristic_filtering_data_export() {
        let files = vec!["data/export.json".to_string()];
        let recs = apply_heuristic_filtering(&files);
        assert_eq!(recs.len(), 1);
        assert!(recs[0].should_ingest);
        assert_eq!(recs[0].category, "personal_data");
    }

    #[test]
    fn test_heuristic_filtering_media_without_context() {
        let files = vec!["photos/vacation.jpg".to_string()];
        let recs = apply_heuristic_filtering(&files);
        assert_eq!(recs.len(), 1);
        assert!(recs[0].should_ingest);
        assert_eq!(recs[0].category, "media");
    }

    #[test]
    fn test_heuristic_filtering_media_in_export() {
        // Media in data export paths should be ingested
        let files = vec!["export/photos/vacation.jpg".to_string()];
        let recs = apply_heuristic_filtering(&files);
        assert_eq!(recs.len(), 1);
        assert!(recs[0].should_ingest);
        assert_eq!(recs[0].category, "media");
    }

    #[test]
    fn test_heuristic_filtering_unknown_file() {
        let files = vec!["random/stuff.xyz".to_string()];
        let recs = apply_heuristic_filtering(&files);
        assert_eq!(recs.len(), 1);
        assert!(!recs[0].should_ingest);
    }

    #[test]
    fn test_heuristic_mixed_file_types() {
        let files = vec![
            "report.pdf".to_string(),
            "photo.jpg".to_string(),
            "script.py".to_string(),
            "data.csv".to_string(),
            "export/backup.json".to_string(),
        ];
        let recs = apply_heuristic_filtering(&files);
        assert_eq!(recs.len(), 5);

        // PDF → personal_data, should_ingest
        assert!(recs[0].should_ingest);
        assert_eq!(recs[0].category, "personal_data");

        // JPG → media, should_ingest
        assert!(recs[1].should_ingest);
        assert_eq!(recs[1].category, "media");

        // .py → unknown, should_ingest = false
        assert!(!recs[2].should_ingest);

        // CSV → personal_data, should_ingest
        assert!(recs[3].should_ingest);
        assert_eq!(recs[3].category, "personal_data");

        // backup path → personal_data, should_ingest
        assert!(recs[4].should_ingest);
    }

    #[test]
    fn test_heuristic_case_insensitive_extensions() {
        let files = vec![
            "REPORT.PDF".to_string(),
            "Data.Csv".to_string(),
            "photo.JPG".to_string(),
        ];
        let recs = apply_heuristic_filtering(&files);

        // .PDF → personal_data (extension lowercased internally)
        assert!(recs[0].should_ingest);
        assert_eq!(recs[0].category, "personal_data");

        // .Csv → personal_data
        assert!(recs[1].should_ingest);
        assert_eq!(recs[1].category, "personal_data");

        // .JPG → media, should_ingest
        assert!(recs[2].should_ingest);
        assert_eq!(recs[2].category, "media");
    }

    // ---- image directory classification tests ----

    #[test]
    fn test_parse_image_directory_response_valid() {
        let response = r#"{"photos/vacation": "personal", "assets/images/twemoji/v/latest/svg": "asset", "data/tweets_media": "personal"}"#;
        let mut image_dirs = HashMap::new();
        image_dirs.insert("photos/vacation".to_string(), vec!["photos/vacation/img1.jpg".to_string()]);
        image_dirs.insert("assets/images/twemoji/v/latest/svg".to_string(), vec!["assets/images/twemoji/v/latest/svg/emoji_0.svg".to_string()]);
        image_dirs.insert("data/tweets_media".to_string(), vec!["data/tweets_media/photo_0.jpg".to_string()]);

        let non_personal = parse_image_directory_response(response, &image_dirs);
        assert_eq!(non_personal.len(), 1);
        assert!(non_personal.contains(&"assets/images/twemoji/v/latest/svg".to_string()));
    }

    #[test]
    fn test_parse_image_directory_response_ignores_unknown_dirs() {
        let response = r#"{"unknown/dir": "asset", "photos": "personal"}"#;
        let mut image_dirs = HashMap::new();
        image_dirs.insert("photos".to_string(), vec!["photos/img.jpg".to_string()]);

        let non_personal = parse_image_directory_response(response, &image_dirs);
        assert!(non_personal.is_empty());
    }

    #[test]
    fn test_parse_image_directory_response_malformed() {
        let response = "not json at all";
        let image_dirs = HashMap::new();
        let non_personal = parse_image_directory_response(response, &image_dirs);
        assert!(non_personal.is_empty());
    }

    #[test]
    fn test_create_image_directory_prompt_contains_dirs() {
        let mut image_dirs = HashMap::new();
        image_dirs.insert("photos/vacation".to_string(), vec![
            "photos/vacation/img1.jpg".to_string(),
            "photos/vacation/img2.jpg".to_string(),
        ]);
        image_dirs.insert("assets/icons".to_string(), vec![
            "assets/icons/icon1.svg".to_string(),
        ]);

        let prompt = create_image_directory_prompt(&image_dirs, Path::new("/tmp"));
        assert!(prompt.contains("photos/vacation"));
        assert!(prompt.contains("2 files"));
        assert!(prompt.contains("assets/icons"));
        assert!(prompt.contains("1 files"));
        assert!(prompt.contains("personal"));
        assert!(prompt.contains("asset"));
    }
}
