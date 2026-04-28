//! Shared smart-folder scan and ingestion logic.
//!
//! These functions are framework-agnostic and used by both
//! HTTP handlers in `server::routes::smart_folder` and the CLI (`folddb`).

pub mod batch;
pub mod classify;
pub mod scanner;
pub mod types;

use crate::ingestion::IngestionResult;
use futures::stream::{self, StreamExt};
use std::collections::HashMap;
use std::path::Path;

// Re-export from sibling modules so external callers can still use
// `smart_folder::read_file_as_json`, etc.
pub use super::file_handling::conversion::{
    csv_to_json, read_file_as_json, read_file_with_hash, twitter_js_to_json,
};
pub use scanner::*;

// Re-export types and classification functions so callers using
// `smart_folder::FileRecommendation`, `smart_folder::estimate_file_cost`, etc. still work.
pub use classify::{
    apply_heuristic_filtering, call_llm_for_file_analysis, create_adjust_prompt,
    create_smart_folder_prompt, merge_adjust_results, parse_llm_file_recommendations,
};
pub(crate) use types::file_size_bytes;
pub use types::{
    estimate_file_cost, FileRecommendation, ScanProgressFn, SmartFolderScanResponse,
    SmartFolderSummary,
};

use classify::classify_image_directories;

// ---- Scan orchestration ----

/// Perform a smart folder scan: directory walk, LLM classification, recommendations.
///
/// This is the core logic shared between the HTTP handler and the CLI.
/// If `service` is `None`, an `IngestionService` is created from the environment.
pub async fn perform_smart_folder_scan(
    folder_path: &Path,
    max_depth: usize,
    max_files: usize,
    service: Option<&crate::ingestion::ingestion_service::IngestionService>,
    node: Option<&crate::fold_node::FoldNode>,
) -> IngestionResult<SmartFolderScanResponse> {
    perform_smart_folder_scan_with_progress(folder_path, max_depth, max_files, service, node, None)
        .await
}

pub async fn perform_smart_folder_scan_with_progress(
    folder_path: &Path,
    max_depth: usize,
    max_files: usize,
    service: Option<&crate::ingestion::ingestion_service::IngestionService>,
    node: Option<&crate::fold_node::FoldNode>,
    on_progress: Option<&ScanProgressFn>,
) -> IngestionResult<SmartFolderScanResponse> {
    let report = |pct: u8, msg: String| {
        if let Some(f) = &on_progress {
            f(pct, msg);
        }
    };

    report(5, "Listing files...".into());
    let scan = scan_directory_tree_with_context(folder_path, max_depth, max_files)?;

    if scan.file_paths.is_empty() {
        report(100, "No files found.".into());
        return Ok(SmartFolderScanResponse {
            success: true,
            total_files: 0,
            recommended_files: vec![],
            skipped_files: vec![],
            summary: HashMap::new(),
            total_estimated_cost: 0.0,
            scan_truncated: scan.truncated,
            max_depth_used: max_depth,
            max_files_used: max_files,
        });
    }

    report(
        15,
        format!(
            "Found {} candidate files (only ingestible extensions collected).",
            scan.file_paths.len()
        ),
    );

    // The scanner whitelist already filtered to ingestible extensions, so all
    // paths here are candidates for LLM classification.
    let mut llm_candidates: Vec<String> = scan.file_paths.clone();

    // --- Image directory classification: filter non-personal image directories ---
    // Group image files by parent directory, then use LLM to classify which
    // directories contain personal images vs. asset/scaffolding images.
    let image_dir_skipped =
        classify_image_directories(&mut llm_candidates, folder_path, service, &report).await?;

    tracing::info!(
            target: "fold_node::ingestion",
        "File classification: {} candidates for dedup check",
        llm_candidates.len(),
    );

    // --- Dedup check: remove already-ingested files before AI classification ---
    let pub_key = node.map(|n| n.get_node_public_key().to_string());
    let mut already_ingested_recs: Vec<FileRecommendation> = Vec::new();

    if let (Some(ref pk), Some(n)) = (&pub_key, node) {
        report(
            20,
            format!(
                "Checking {} files for previously ingested (concurrent)...",
                llm_candidates.len(),
            ),
        );

        // Check dedup concurrently — up to 16 at a time (mixed CPU hash + async DB lookup)
        let dedup_results: Vec<(String, bool, u64)> = stream::iter(llm_candidates)
            .map(|path| async {
                let full_path = folder_path.join(&path);
                if let Ok(hash) = compute_file_hash(&full_path) {
                    if n.is_file_ingested(pk, &hash).await.is_some() {
                        let size = file_size_bytes(Path::new(&path), folder_path);
                        return (path, true, size);
                    }
                }
                (path, false, 0)
            })
            .buffer_unordered(16)
            .collect()
            .await;

        let mut remaining = Vec::new();
        for (path, ingested, size) in dedup_results {
            if ingested {
                already_ingested_recs.push(FileRecommendation {
                    path,
                    should_ingest: false,
                    category: "already_ingested".to_string(),
                    reason: "Already ingested".to_string(),
                    file_size_bytes: size,
                    estimated_cost: 0.0,
                    already_ingested: true,
                });
            } else {
                remaining.push(path);
            }
        }
        llm_candidates = remaining;

        tracing::info!(
            target: "fold_node::ingestion",
            "Dedup check: {} already ingested, {} remaining for LLM",
            already_ingested_recs.len(),
            llm_candidates.len(),
        );
    }

    report(
        25,
        format!(
            "Classifying {} files ({} already ingested)...",
            llm_candidates.len(),
            already_ingested_recs.len(),
        ),
    );

    // Two-pass classification: heuristics first (instant), LLM only for ambiguous files.
    // This avoids sending hundreds of obviously-classifiable files to a slow Ollama call.
    let llm_recs = if llm_candidates.is_empty() {
        Vec::new()
    } else {
        let heuristic_results = apply_heuristic_filtering(&llm_candidates);

        // Files the heuristic couldn't classify (category == "unknown") need LLM help
        let mut resolved: Vec<FileRecommendation> = Vec::new();
        let mut ambiguous: Vec<String> = Vec::new();
        for rec in heuristic_results {
            if rec.category == "unknown" {
                ambiguous.push(rec.path);
            } else {
                resolved.push(rec);
            }
        }

        tracing::info!(
            target: "fold_node::ingestion",
            "Heuristic pass: {} classified, {} ambiguous files need LLM",
            resolved.len(),
            ambiguous.len(),
        );

        if !ambiguous.is_empty() {
            // Try LLM classification for ambiguous files; fall back to heuristics if unavailable.
            let ambiguous_recs = if let Some(svc) = service {
                report(
                    30,
                    format!("Classifying {} ambiguous files with AI...", ambiguous.len(),),
                );
                let tree_display = build_directory_tree_string(&ambiguous);
                let prompt = create_smart_folder_prompt(&tree_display, &ambiguous);
                match call_llm_for_file_analysis(&prompt, svc).await {
                    Ok(response) => match parse_llm_file_recommendations(&response, &ambiguous) {
                        Ok(recs) => recs,
                        Err(e) => {
                            tracing::warn!(
                            target: "fold_node::ingestion",
                                                    "LLM classification response unparseable: {}. Falling back to heuristics.",
                                                    e
                                                );
                            apply_heuristic_filtering(&ambiguous)
                        }
                    },
                    Err(e) => {
                        tracing::warn!(
                        target: "fold_node::ingestion",
                                        "LLM classification unavailable: {}. Falling back to heuristics.",
                                        e
                                    );
                        apply_heuristic_filtering(&ambiguous)
                    }
                }
            } else {
                report(
                    30,
                    format!(
                        "Classifying {} ambiguous files with heuristics (no AI service)...",
                        ambiguous.len(),
                    ),
                );
                apply_heuristic_filtering(&ambiguous)
            };
            resolved.extend(ambiguous_recs);
        }

        resolved
    };

    report(80, "Computing costs and finalizing...".into());

    let recommendations: Vec<FileRecommendation> = llm_recs;

    // Split into recommended and skipped, build summary, compute costs
    let mut recommended_files = Vec::new();
    let mut skipped_files = Vec::new();
    let mut total_estimated_cost = 0.0;
    let mut summary: SmartFolderSummary = HashMap::new();

    // Add already-ingested files to skipped list and summary
    if !already_ingested_recs.is_empty() {
        *summary.entry("already_ingested".to_string()).or_insert(0) += already_ingested_recs.len();
        skipped_files.extend(already_ingested_recs);
    }

    // Add image-directory-skipped files to skipped list and summary
    if !image_dir_skipped.is_empty() {
        *summary.entry("non_personal_media".to_string()).or_insert(0) += image_dir_skipped.len();
        skipped_files.extend(image_dir_skipped);
    }

    let rec_count = recommendations.len();
    for (idx, mut rec) in recommendations.into_iter().enumerate() {
        // Report incremental progress every 5 files (80% → 95%)
        if rec_count > 0 && idx % 5 == 0 {
            let pct = (80 + idx * 15 / rec_count).min(95) as u8;
            report(pct, format!("Computing costs ({}/{})...", idx, rec_count));
        }

        // Populate file size and cost estimate (local providers are free)
        let rel_path = Path::new(&rec.path);
        rec.file_size_bytes = file_size_bytes(rel_path, folder_path);
        let is_local = service.is_some_and(|s| s.is_local_provider());
        rec.estimated_cost = if is_local {
            0.0
        } else {
            estimate_file_cost(rel_path, folder_path)
        };

        if rec.should_ingest {
            *summary.entry(rec.category.clone()).or_insert(0) += 1;
            total_estimated_cost += rec.estimated_cost;
            recommended_files.push(rec);
        } else {
            *summary.entry(rec.category.clone()).or_insert(0) += 1;
            skipped_files.push(rec);
        }
    }

    // Don't report 100% here — the caller sets JobStatus::Completed after we return.
    // Reporting 100% via the fire-and-forget spawned callback races with the
    // caller's completion save and can overwrite Completed back to Running.
    report(
        99,
        format!(
            "Finalizing... {} to ingest, {} skipped.",
            recommended_files.len(),
            skipped_files.len(),
        ),
    );

    Ok(SmartFolderScanResponse {
        success: true,
        total_files: scan.file_paths.len(),
        recommended_files,
        skipped_files,
        summary,
        total_estimated_cost,
        scan_truncated: scan.truncated,
        max_depth_used: max_depth,
        max_files_used: max_files,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn test_twitter_js_to_json_valid() {
        let input = r#"window.YTD.tweet.part0 = [{"id":"123","text":"hello"}]"#;
        let result = twitter_js_to_json(input).unwrap();
        let parsed: Value = serde_json::from_str(&result).unwrap();
        assert!(parsed.is_array());
        assert_eq!(parsed[0]["id"], "123");
    }

    #[test]
    fn test_twitter_js_to_json_no_equals() {
        let input = r#"{"id":"123"}"#;
        let result = twitter_js_to_json(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_twitter_js_to_json_invalid_json() {
        let input = "window.YTD.tweet.part0 = not valid json";
        let result = twitter_js_to_json(input);
        assert!(result.is_err());
    }

    #[test]
    fn test_csv_to_json_basic() {
        let csv_content = "name,age,active\nAlice,30,true\nBob,25,false";
        let result = csv_to_json(csv_content).unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0]["name"], "Alice");
        assert_eq!(parsed[0]["age"], 30.0);
        assert_eq!(parsed[0]["active"], true);
        assert_eq!(parsed[1]["name"], "Bob");
        assert_eq!(parsed[1]["age"], 25.0);
        assert_eq!(parsed[1]["active"], false);
    }

    #[test]
    fn test_csv_to_json_empty() {
        let csv_content = "name,age";
        let result = csv_to_json(csv_content).unwrap();
        let parsed: Vec<Value> = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed.len(), 0);
    }

    #[test]
    fn test_extract_json_direct_array() {
        let input = r#"[{"path":"a.json","should_ingest":true,"category":"personal_data","reason":"test"}]"#;
        let result = crate::ingestion::ai::helpers::extract_json_from_response(input).unwrap();
        assert!(result.starts_with('['));
    }

    #[test]
    fn test_extract_json_from_markdown() {
        let input = "Here is the result:\n```json\n[{\"path\":\"a.json\"}]\n```\nDone.";
        let result = crate::ingestion::ai::helpers::extract_json_from_response(input).unwrap();
        assert!(result.starts_with('['));
    }

    #[test]
    fn test_read_file_as_json_unsupported() {
        let result = read_file_as_json(Path::new("/tmp/test.xyz"));
        assert!(result.is_err());
    }

    // ---- build_directory_tree_string tests ----

    #[test]
    fn test_tree_string_flat_files() {
        let paths = vec!["a.txt".to_string(), "b.pdf".to_string()];
        let tree = build_directory_tree_string(&paths);
        assert!(tree.contains("a.txt"));
        assert!(tree.contains("b.pdf"));
    }

    #[test]
    fn test_tree_string_nested_dirs() {
        let paths = vec![
            "Photos/vacation/IMG_001.jpg".to_string(),
            "Photos/vacation/IMG_002.jpg".to_string(),
            "Bank of America/statement.pdf".to_string(),
        ];
        let tree = build_directory_tree_string(&paths);
        assert!(tree.contains("Photos/"));
        assert!(tree.contains("vacation/"));
        assert!(tree.contains("IMG_001.jpg"));
        assert!(tree.contains("Bank of America/"));
        assert!(tree.contains("statement.pdf"));
    }

    #[test]
    fn test_tree_string_empty() {
        let paths: Vec<String> = vec![];
        let tree = build_directory_tree_string(&paths);
        assert!(tree.is_empty());
    }

    // ---- scan_directory_tree_with_context tests ----

    #[test]
    fn test_scan_with_context_returns_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/notes.txt"), "hello").unwrap();
        std::fs::write(root.join("photo.jpg"), "fake jpg").unwrap();
        std::fs::write(root.join("program.exe"), "fake exe").unwrap();

        let result = scan_directory_tree_with_context(root, 10, 50000).unwrap();

        // Both ingestible files appear in file_paths
        assert_eq!(result.file_paths.len(), 2);
        assert!(result.file_paths.contains(&"docs/notes.txt".to_string()));
        assert!(result.file_paths.contains(&"photo.jpg".to_string()));
        // Non-ingestible file is in skipped_files
        assert_eq!(result.skipped_files.len(), 1);
        assert!(result.skipped_files.contains(&"program.exe".to_string()));
        // Skipped file appears in tree display with [skipped] marker
        assert!(result.tree_display.contains("program.exe [skipped]"));
        assert!(!result.truncated);
        assert!(!result.tree_display.is_empty());
        assert!(result.tree_display.contains("docs/"));
    }

    #[test]
    fn test_scan_with_context_truncation() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        for i in 0..5 {
            std::fs::write(root.join(format!("file_{}.txt", i)), "data").unwrap();
        }

        let result = scan_directory_tree_with_context(root, 10, 3).unwrap();

        assert_eq!(result.file_paths.len(), 3);
        assert!(result.truncated);
    }

    // ---- scan_directory_recursive tests ----

    #[test]
    fn test_scan_includes_git_repo_subdirectory() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Create a subdirectory that looks like a git repo
        let repo_dir = root.join("my_project");
        std::fs::create_dir_all(repo_dir.join(".git")).unwrap();
        std::fs::write(repo_dir.join("main.rs"), "fn main() {}").unwrap();

        // Create a normal file at the root level
        std::fs::write(root.join("notes.txt"), "hello").unwrap();

        let files = scan_directory_tree(root, 10, 50000).unwrap();

        // Should find both files — git repos are not excluded
        assert!(files.contains(&"notes.txt".to_string()));
        assert!(files.contains(&"my_project/main.rs".to_string()));
    }

    #[test]
    fn test_scan_includes_coding_project_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Node.js project (package.json)
        let node_dir = root.join("my_website");
        std::fs::create_dir_all(&node_dir).unwrap();
        std::fs::write(node_dir.join("package.json"), r#"{"name":"test"}"#).unwrap();
        std::fs::write(node_dir.join("index.js"), "console.log('hi')").unwrap();

        // Rust project (Cargo.toml)
        let rust_dir = root.join("rust_cli");
        std::fs::create_dir_all(rust_dir.join("src")).unwrap();
        std::fs::write(rust_dir.join("Cargo.toml"), "[package]\nname = \"test\"").unwrap();
        std::fs::write(rust_dir.join("src/main.rs"), "fn main() {}").unwrap();

        // Python project (pyproject.toml)
        let py_dir = root.join("data_analysis");
        std::fs::create_dir_all(&py_dir).unwrap();
        std::fs::write(py_dir.join("pyproject.toml"), "[project]\nname = \"test\"").unwrap();
        std::fs::write(py_dir.join("analysis.py"), "import pandas").unwrap();

        // Normal personal file
        std::fs::write(root.join("notes.txt"), "my notes").unwrap();

        let files = scan_directory_tree(root, 10, 50000).unwrap();

        // Should find personal files AND coding project files with ingestible extensions
        assert!(files.contains(&"notes.txt".to_string()));
        assert!(files.contains(&"my_website/index.js".to_string()));
        assert!(files.contains(&"rust_cli/src/main.rs".to_string()));
        assert!(files.contains(&"data_analysis/analysis.py".to_string()));
        assert!(files.contains(&"my_website/package.json".to_string()));
        assert!(files.contains(&"rust_cli/Cargo.toml".to_string()));
        assert!(files.contains(&"data_analysis/pyproject.toml".to_string()));
    }

    #[test]
    fn test_scan_skips_hidden_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();

        // Hidden directories (dotfiles) are still skipped
        for dir_name in &[".idea", ".vscode", ".next"] {
            let dir = root.join(dir_name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("junk.txt"), "junk").unwrap();
        }

        // Non-hidden directories are included
        let visible_dir = root.join("vendor");
        std::fs::create_dir_all(&visible_dir).unwrap();
        std::fs::write(visible_dir.join("lib.rs"), "// vendor code").unwrap();

        // Create a normal file
        std::fs::write(root.join("personal.txt"), "my data").unwrap();

        let files = scan_directory_tree(root, 10, 50000).unwrap();

        // Should find personal.txt and vendor/lib.rs but NOT hidden dir files
        assert!(files.contains(&"personal.txt".to_string()));
        assert!(files.contains(&"vendor/lib.rs".to_string()));
        assert!(!files.iter().any(|f| f.contains(".idea")));
        assert!(!files.iter().any(|f| f.contains(".vscode")));
    }
}
