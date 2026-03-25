//! Rust → WASM compiler for transform views.
//!
//! Takes a Rust function body (the `transform_impl` function), wraps it in
//! a WASM scaffold with memory management and JSON serialization, compiles it
//! to a `.wasm` module using `cargo build`, and returns the bytes.

use std::path::Path;
use std::process::Command;

/// The WASM scaffold template. The `{transform_body}` placeholder is replaced
/// with the LLM-generated `transform_impl` function body.
const SCAFFOLD_TEMPLATE: &str = r#"
use serde_json::Value;

// ---- Memory management ----
static mut ARENA: Vec<u8> = Vec::new();

#[no_mangle]
pub extern "C" fn alloc(size: i32) -> i32 {
    unsafe {
        ARENA = vec![0u8; size as usize];
        ARENA.as_ptr() as i32
    }
}

// ---- Transform entry point ----
#[no_mangle]
pub extern "C" fn transform(ptr: *const u8, len: i32) -> i64 {
    let input_bytes = unsafe { std::slice::from_raw_parts(ptr, len as usize) };
    let input: Value = match serde_json::from_slice(input_bytes) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    let output = transform_impl(input);
    let output_bytes = match serde_json::to_vec(&output) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    let out_ptr = output_bytes.as_ptr() as i64;
    let out_len = output_bytes.len() as i64;
    std::mem::forget(output_bytes);
    (out_ptr << 32) | out_len
}

// ---- LLM-generated transform ----
TRANSFORM_BODY
"#;

const CARGO_TOML_TEMPLATE: &str = r#"[package]
name = "wasm_transform"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
serde_json = "1"
serde = { version = "1", features = ["derive"] }

[profile.release]
opt-level = "s"
lto = true
"#;

/// Check that `rustc` with `wasm32-unknown-unknown` target is available.
pub fn check_wasm_toolchain() -> Result<(), String> {
    let output = Command::new("rustup")
        .args(["target", "list", "--installed"])
        .output()
        .map_err(|e| format!("Failed to run rustup: {e}. Is rustup installed?"))?;

    let installed = String::from_utf8_lossy(&output.stdout);
    if installed.contains("wasm32-unknown-unknown") {
        Ok(())
    } else {
        Err(
            "wasm32-unknown-unknown target not installed. Run: rustup target add wasm32-unknown-unknown"
                .to_string(),
        )
    }
}

/// Compile a Rust transform function body to WASM bytes.
///
/// `rust_transform` should be the full `fn transform_impl(input: Value) -> Value { ... }`
/// function definition. The compiler wraps it in a scaffold with alloc/transform exports.
pub fn compile_rust_to_wasm(rust_transform: &str) -> Result<Vec<u8>, String> {
    check_wasm_toolchain()?;

    let tmp_dir =
        tempfile::tempdir().map_err(|e| format!("Failed to create temp directory: {e}"))?;

    let project_dir = tmp_dir.path().join("wasm_transform");
    let src_dir = project_dir.join("src");
    std::fs::create_dir_all(&src_dir)
        .map_err(|e| format!("Failed to create project directory: {e}"))?;

    // Write Cargo.toml
    std::fs::write(project_dir.join("Cargo.toml"), CARGO_TOML_TEMPLATE)
        .map_err(|e| format!("Failed to write Cargo.toml: {e}"))?;

    // Write lib.rs with the scaffold + transform body
    let lib_rs = SCAFFOLD_TEMPLATE.replace("TRANSFORM_BODY", rust_transform);
    std::fs::write(src_dir.join("lib.rs"), &lib_rs)
        .map_err(|e| format!("Failed to write lib.rs: {e}"))?;

    log::info!(
        "WASM compiler: building transform in {}",
        project_dir.display()
    );

    // Build with cargo
    let output = Command::new("cargo")
        .args([
            "build",
            "--target",
            "wasm32-unknown-unknown",
            "--release",
            "--manifest-path",
        ])
        .arg(project_dir.join("Cargo.toml"))
        .output()
        .map_err(|e| format!("Failed to run cargo build: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Extract just the error lines for a cleaner message
        let errors: Vec<&str> = stderr.lines().filter(|l| l.contains("error")).collect();
        let error_summary = if errors.is_empty() {
            stderr.to_string()
        } else {
            errors.join("\n")
        };
        return Err(format!("Rust compilation failed:\n{error_summary}"));
    }

    // Read the compiled .wasm file
    let wasm_path = project_dir
        .join("target")
        .join("wasm32-unknown-unknown")
        .join("release")
        .join("wasm_transform.wasm");

    read_wasm_file(&wasm_path)
}

fn read_wasm_file(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("Failed to read compiled WASM file: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_check_wasm_toolchain() {
        // This test will pass if the toolchain is installed, skip otherwise
        if check_wasm_toolchain().is_err() {
            eprintln!("Skipping: wasm32-unknown-unknown not installed");
        }
    }

    #[test]
    fn test_compile_simple_transform() {
        if check_wasm_toolchain().is_err() {
            eprintln!("Skipping: wasm32-unknown-unknown not installed");
            return;
        }

        let rust_code = r#"
fn transform_impl(input: Value) -> Value {
    // Simple passthrough — just return the input as-is wrapped in fields
    serde_json::json!({
        "fields": {
            "result": input
        }
    })
}
"#;

        let wasm_bytes = compile_rust_to_wasm(rust_code).expect("Compilation should succeed");
        assert!(!wasm_bytes.is_empty(), "WASM output should not be empty");
        // WASM magic number: \0asm
        assert_eq!(&wasm_bytes[..4], b"\0asm", "Should be a valid WASM module");
    }

    #[test]
    fn test_compile_invalid_rust_fails() {
        if check_wasm_toolchain().is_err() {
            eprintln!("Skipping: wasm32-unknown-unknown not installed");
            return;
        }

        let bad_code = r#"
fn transform_impl(input: Value) -> Value {
    this is not valid rust
}
"#;

        let result = compile_rust_to_wasm(bad_code);
        assert!(result.is_err(), "Invalid Rust should fail to compile");
    }

    #[test]
    fn test_scaffold_template_is_valid() {
        let body = "fn transform_impl(input: Value) -> Value { input }";
        let rendered = SCAFFOLD_TEMPLATE.replace("TRANSFORM_BODY", body);
        assert!(rendered.contains("fn transform_impl"));
        assert!(rendered.contains("pub extern \"C\" fn alloc"));
        assert!(rendered.contains("pub extern \"C\" fn transform"));
    }
}
