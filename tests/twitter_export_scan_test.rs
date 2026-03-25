//! Tests for SmartFolder scanning of Twitter/X data exports.
//!
//! Two tests:
//!
//! 1. `test_twitter_export_scanner_includes_all_ingestible` — always runs, no API key.
//!    Builds a synthetic Twitter-export directory (data/*.js + media files + fonts)
//!    and calls `scan_directory_tree_with_context` directly.
//!    Verifies that all ingestible files (including images) are discovered,
//!    and that non-ingestible files (video, audio, fonts) are in skipped_files.
//!
//! 2. `test_twitter_export_llm_scan` — ignored (requires ANTHROPIC_API_KEY).
//!    Scans a realistic Twitter export with AI classification and asserts that
//!    all personal-data .js files are recommended for ingestion.  Accepts either
//!    a real archive via `TWITTER_EXPORT_PATH` env var, or a self-contained
//!    temp-dir fixture built from the committed `tests/fixtures/tweets.js`.
//!
//! Run the LLM test with:
//!   cargo test --test twitter_export_scan_test -- --ignored --nocapture

use fold_db_node::ingestion::ingestion_service::IngestionService;
use fold_db_node::ingestion::smart_folder::scanner::{
    is_ingestible_file, scan_directory_tree_with_context,
};
use fold_db_node::ingestion::smart_folder::{perform_smart_folder_scan, read_file_with_hash};

use std::path::Path;
use tempfile::TempDir;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Create a file with `content` at `root/rel_path`, creating parent dirs.
fn write_fixture(root: &Path, rel_path: &str, content: &[u8]) {
    let full = root.join(rel_path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(&full, content).unwrap();
}

/// A minimal valid Twitter-export JS file wrapping `payload` as JSON.
fn twitter_js(var_name: &str, payload: &str) -> String {
    format!("window.YTD.{}.part0 = {}", var_name, payload)
}

/// Build a realistic Twitter-export directory structure in `root`.
///
/// Returns the list of `.js` data-file names (relative to root) that should
/// appear in the scan — i.e. every ingestible file we created.
fn build_twitter_fixture(root: &Path) -> Vec<String> {
    // Personal-data .js files
    let js_files: &[(&str, &str, &str)] = &[
        (
            "data/account.js",
            "account",
            r#"[{"account":{"username":"testuser","email":"test@example.com","accountId":"12345","createdAt":"2014-01-01T00:00:00.000Z"}}]"#,
        ),
        (
            "data/tweets.js",
            "tweet",
            r#"[{"tweet":{"id":"1","full_text":"Hello world","created_at":"Mon Jan 01 00:00:00 +0000 2024"}}]"#,
        ),
        (
            "data/follower.js",
            "follower",
            r#"[{"follower":{"accountId":"67890","userLink":"https://twitter.com/intent/user?user_id=67890"}}]"#,
        ),
        (
            "data/following.js",
            "following",
            r#"[{"following":{"accountId":"11111","userLink":"https://twitter.com/intent/user?user_id=11111"}}]"#,
        ),
        (
            "data/direct-messages.js",
            "direct_messages",
            r#"[{"dmConversation":{"conversationId":"12345-67890","messages":[{"messageCreate":{"id":"999","senderId":"12345","text":"Hi!","createdAt":"2023-01-01T00:00:00.000Z"}}]}}]"#,
        ),
        (
            "data/like.js",
            "like",
            r#"[{"like":{"tweetId":"9999","fullText":"liked tweet"}}]"#,
        ),
        (
            "data/profile.js",
            "profile",
            r#"[{"profile":{"description":{"bio":"Software engineer"},"avatarMediaUrl":"https://example.com/img.jpg"}}]"#,
        ),
        (
            "data/block.js",
            "block",
            r#"[{"blocking":{"accountId":"22222","userLink":"https://twitter.com/intent/user?user_id=22222"}}]"#,
        ),
        (
            "data/mute.js",
            "mute",
            r#"[{"muting":{"accountId":"33333","userLink":"https://twitter.com/intent/user?user_id=33333"}}]"#,
        ),
        (
            "data/screen-name-change.js",
            "screen_name_change",
            r#"[{"screenNameChange":{"changedAt":"2020-01-01T00:00:00.000Z","changedFrom":"oldname","changedTo":"testuser"}}]"#,
        ),
    ];

    for (rel, var, payload) in js_files {
        write_fixture(root, rel, twitter_js(var, payload).as_bytes());
    }

    // manifest.js uses a different prefix but is still `window.X = {...}`
    write_fixture(
        root,
        "data/manifest.js",
        br#"window.__THAR_CONFIG = {"userInfo":{"accountId":"12345","userName":"testuser"},"archiveInfo":{"sizeBytes":"5000000","generationDate":"2024-10-22"}}"#,
    );

    // README is a text file — should appear in the scan
    write_fixture(
        root,
        "data/README.txt",
        b"This is your Twitter data archive.\n",
    );

    // ── Image files (now ingestible) ────────────────────────────────────────

    // tweets_media/ (jpg) — ingestible images
    for i in 0..50 {
        write_fixture(
            root,
            &format!("data/tweets_media/photo_{}.jpg", i),
            b"\xff\xd8\xff",
        );
    }

    // profile_media/
    write_fixture(root, "data/profile_media/avatar.jpg", b"\xff\xd8\xff");

    // assets/images/ — twemoji SVGs (ingestible)
    for i in 0..100 {
        write_fixture(
            root,
            &format!("assets/images/twemoji/v/latest/svg/emoji_{}.svg", i),
            b"<svg/>",
        );
    }
    write_fixture(root, "assets/images/groupAvatar.svg", b"<svg/>");

    // ── Non-ingestible files (video, audio, fonts, ico) ──────────────────

    write_fixture(
        root,
        "data/direct_messages_media/video.mp4",
        b"\x00\x00\x00",
    );
    write_fixture(root, "assets/fonts/chirp-regular.woff2", b"\x00wOF2");
    write_fixture(root, "assets/fonts/chirp-bold.ttf", b"\x00\x01\x00\x00");
    write_fixture(root, "assets/images/favicon.ico", b"\x00\x00\x01\x00");

    // Return the names of JS/TXT data files that SHOULD appear in scan results
    let mut expected: Vec<String> = js_files.iter().map(|(rel, _, _)| rel.to_string()).collect();
    expected.push("data/manifest.js".to_string());
    expected.push("data/README.txt".to_string());
    expected
}

// ── Test 1: scanner-level, no API key ────────────────────────────────────────

#[test]
fn test_twitter_export_scanner_includes_all_ingestible() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path();
    let expected_data_files = build_twitter_fixture(root);

    // max_files large enough for all ingestible files (data + images)
    let result = scan_directory_tree_with_context(root, 10, 500).unwrap();

    eprintln!(
        "Found {} ingestible, {} skipped:",
        result.file_paths.len(),
        result.skipped_files.len()
    );
    for f in &result.file_paths {
        eprintln!("  {}", f);
    }

    // Every data file must appear in scan results.
    for expected in &expected_data_files {
        assert!(
            result.file_paths.contains(expected),
            "Expected '{}' in scan results but it was missing.",
            expected
        );
    }

    // Images (jpg, svg) should also be in file_paths (now ingestible).
    assert!(
        result.file_paths.iter().any(|f| f.ends_with(".jpg")),
        "JPG files should be ingestible"
    );
    assert!(
        result.file_paths.iter().any(|f| f.ends_with(".svg")),
        "SVG files should be ingestible"
    );

    // Non-ingestible files (mp4, woff2, ttf, ico) should be in skipped_files.
    assert!(
        result.skipped_files.iter().any(|f| f.ends_with(".mp4")),
        "MP4 should be skipped"
    );
    assert!(
        result.skipped_files.iter().any(|f| f.ends_with(".woff2")),
        "WOFF2 should be skipped"
    );
    assert!(
        result.skipped_files.iter().any(|f| f.ends_with(".ico")),
        "ICO should be skipped"
    );

    // All file_paths must have ingestible extensions.
    for path in &result.file_paths {
        assert!(
            is_ingestible_file(path),
            "File '{}' is not ingestible but appeared in file_paths.",
            path
        );
    }

    // Skipped files should appear in tree display with [skipped] marker.
    assert!(
        result.tree_display.contains("[skipped]"),
        "Tree display should show skipped files"
    );

    eprintln!(
        "PASS: {} ingestible files found, {} skipped.",
        result.file_paths.len(),
        result.skipped_files.len()
    );
}

// ── Test 2: full LLM scan, requires API key ──────────────────────────────────

/// Key personal-data files that the LLM must classify as worth ingesting.
/// These are unambiguously personal regardless of LLM temperature.
const MUST_INGEST: &[&str] = &[
    "tweets.js",
    "account.js",
    "direct-messages.js",
    "follower.js",
    "following.js",
    "profile.js",
    "like.js",
];

#[actix_web::test]
#[ignore] // Requires ANTHROPIC_API_KEY
async fn test_twitter_export_llm_scan() {
    std::env::var("ANTHROPIC_API_KEY").expect("ANTHROPIC_API_KEY must be set to run this test");

    // ── Resolve scan directory ────────────────────────────────────────────────

    // Default: use the committed fixture at tests/fixtures/twitter_export/.
    // Override with TWITTER_EXPORT_PATH to scan a different archive.
    let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/twitter_export");

    let override_path = std::env::var("TWITTER_EXPORT_PATH").ok();
    let scan_path: &Path = match &override_path {
        Some(p) => Path::new(p.as_str()),
        None => &fixture_dir,
    };

    eprintln!("Scanning: {}", scan_path.display());

    // ── Scan ─────────────────────────────────────────────────────────────────

    let ingestion_service =
        IngestionService::from_env().expect("Failed to create IngestionService");

    let scan = perform_smart_folder_scan(
        scan_path,
        10,  // max_depth
        500, // max_files — generous; media is now filtered before counting
        Some(&ingestion_service),
        None, // no node needed for classification
    )
    .await
    .expect("SmartFolder scan failed");

    eprintln!(
        "Scan complete: total={} recommended={} skipped={} truncated={}",
        scan.total_files,
        scan.recommended_files.len(),
        scan.skipped_files.len(),
        scan.scan_truncated,
    );
    for f in &scan.recommended_files {
        eprintln!("  RECOMMEND: {} ({})", f.path, f.category);
    }
    for f in &scan.skipped_files {
        eprintln!("  SKIP:      {} ({})", f.path, f.category);
    }

    assert!(scan.success, "Scan must succeed");
    assert!(
        !scan.scan_truncated,
        "Scan must not be truncated at max_files=500"
    );

    // ── All results must have whitelisted (ingestible) extensions ──────────────

    for rec in scan
        .recommended_files
        .iter()
        .chain(scan.skipped_files.iter())
    {
        assert!(
            is_ingestible_file(&rec.path),
            "File '{}' is not in the ingestible whitelist — \
             only whitelisted extensions should appear in scan results.",
            rec.path
        );
    }

    // ── All MUST_INGEST files must be recommended ─────────────────────────────

    let recommended_names: Vec<String> = scan
        .recommended_files
        .iter()
        .map(|f| {
            Path::new(&f.path)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string()
        })
        .collect();

    let mut missing_count = 0usize;
    for must in MUST_INGEST {
        if recommended_names.iter().any(|n| n == must) {
            eprintln!("  PASS: {} is recommended", must);
        } else {
            eprintln!("  FAIL: {} is NOT in recommended_files", must);
            missing_count += 1;
        }
    }

    assert_eq!(
        missing_count, 0,
        "{} personal-data files were not recommended for ingestion (see FAIL lines above). \
         The LLM should classify Twitter account/tweet/DM/follower data as worth ingesting.",
        missing_count
    );

    // ── All recommended .js files must be parseable by read_file_with_hash ───

    let mut parse_failures = 0usize;
    for rec in &scan.recommended_files {
        if !rec.path.ends_with(".js") {
            continue;
        }
        let full_path = scan_path.join(&rec.path);
        match read_file_with_hash(&full_path) {
            Ok(_) => eprintln!("  PARSEABLE: {}", rec.path),
            Err(e) => {
                eprintln!("  PARSE ERROR: {} — {}", rec.path, e);
                parse_failures += 1;
            }
        }
    }

    assert_eq!(
        parse_failures, 0,
        "{} recommended .js files could not be parsed by read_file_with_hash. \
         Every file the LLM recommends must be ingestible.",
        parse_failures
    );

    eprintln!(
        "\nPASS: {} recommended, 0 parse failures, 0 media files leaked.",
        scan.recommended_files.len()
    );
}
