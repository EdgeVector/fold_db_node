//! File conversion utilities — CSV, Twitter JS, code metadata extraction, and unified file reading.

use crate::ingestion::error::IngestionError;
use crate::ingestion::smart_folder::scanner::{CODE_EXTS, CONFIG_EXTS};
use crate::ingestion::IngestionResult;
use regex::Regex;
use serde_json::Value;
use std::path::Path;

/// Convert CSV content to JSON array
pub fn csv_to_json(csv_content: &str) -> IngestionResult<String> {
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_reader(csv_content.as_bytes());

    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| IngestionError::InvalidInput(format!("Failed to read CSV headers: {}", e)))?
        .iter()
        .map(|h| h.to_string())
        .collect();

    let mut records: Vec<Value> = Vec::new();

    for result in reader.records() {
        let record = result.map_err(|e| {
            IngestionError::InvalidInput(format!("Failed to read CSV record: {}", e))
        })?;
        let mut obj = serde_json::Map::new();

        for (i, field) in record.iter().enumerate() {
            if let Some(header) = headers.get(i) {
                let value = if let Ok(n) = field.parse::<f64>() {
                    Value::Number(
                        serde_json::Number::from_f64(n)
                            .unwrap_or_else(|| serde_json::Number::from(0)),
                    )
                } else if field == "true" {
                    Value::Bool(true)
                } else if field == "false" {
                    Value::Bool(false)
                } else {
                    Value::String(field.to_string())
                };
                obj.insert(header.clone(), value);
            }
        }

        records.push(Value::Object(obj));
    }

    serde_json::to_string(&records)
        .map_err(|e| IngestionError::InvalidInput(format!("Failed to serialize JSON: {}", e)))
}

/// Convert a Twitter data export `.js` file to JSON.
///
/// Twitter data exports use files like `window.YTD.tweet.part0 = [...]`.
/// This strips the variable assignment prefix and returns the pure JSON.
pub fn twitter_js_to_json(content: &str) -> IngestionResult<String> {
    if let Some(eq_pos) = content.find('=') {
        let json_part = content[eq_pos + 1..].trim();
        // Validate it parses as JSON
        serde_json::from_str::<Value>(json_part).map_err(|e| {
            IngestionError::InvalidInput(format!("Invalid JSON in .js file: {}", e))
        })?;
        Ok(json_part.to_string())
    } else {
        Err(IngestionError::InvalidInput(
            "Not a Twitter data export .js file (no '=' found)".to_string(),
        ))
    }
}

/// Map file extension to language name.
fn ext_to_language(ext: &str) -> &str {
    match ext {
        "rs" => "rust",
        "py" => "python",
        "js" | "jsx" => "javascript",
        "ts" | "tsx" => "typescript",
        "go" => "go",
        "java" => "java",
        "kt" => "kotlin",
        "rb" => "ruby",
        "c" | "h" => "c",
        "cpp" | "cc" | "cxx" | "hpp" => "cpp",
        "cs" => "csharp",
        "swift" => "swift",
        "scala" => "scala",
        "lua" => "lua",
        "r" => "r",
        "pl" => "perl",
        "sh" | "bash" | "zsh" => "shell",
        other => other,
    }
}

/// Format a zero-padded 6-digit line number string.
fn pad_line_number(line: usize) -> String {
    format!("{:06}", line)
}

/// A raw symbol detected by regex before body extraction.
struct RawSymbol {
    name: String,
    kind: String,
    signature: String,
    line_number: usize,
    parent: Option<String>,
    visibility: String,
}

/// Extract visibility modifier from a declaration line.
fn extract_visibility(line: &str, lang: &str) -> String {
    let trimmed = line.trim_start();
    match lang {
        "rust" => {
            if trimmed.starts_with("pub(crate)") {
                "pub(crate)".to_string()
            } else if trimmed.starts_with("pub(super)") {
                "pub(super)".to_string()
            } else if trimmed.starts_with("pub") {
                "pub".to_string()
            } else {
                "private".to_string()
            }
        }
        "python" => {
            // Convention: _name is private, __name is private
            let name_part = trimmed
                .split_whitespace()
                .nth(1)
                .unwrap_or("");
            if name_part.starts_with('_') {
                "private".to_string()
            } else {
                "pub".to_string()
            }
        }
        "go" => {
            // Go: uppercase first letter = exported
            let parts: Vec<&str> = trimmed.split_whitespace().collect();
            let name = if trimmed.starts_with("func") {
                // func Name() or func (r *T) Name()
                parts.iter()
                    .position(|p| p.starts_with('(') || !["func"].contains(p))
                    .and_then(|i| {
                        let p = parts[i];
                        if p.starts_with('(') {
                            // receiver — skip to name after closing paren
                            parts.get(i + 2).or(parts.get(i + 1)).copied()
                        } else {
                            Some(p)
                        }
                    })
                    .unwrap_or("")
            } else {
                parts.get(1).copied().unwrap_or("")
            };
            let first_char = name.chars().next().unwrap_or('a');
            if first_char.is_uppercase() {
                "pub".to_string()
            } else {
                "private".to_string()
            }
        }
        "java" | "kotlin" | "csharp" | "cpp" | "c" => {
            if trimmed.starts_with("public") {
                "pub".to_string()
            } else if trimmed.starts_with("protected") {
                "protected".to_string()
            } else {
                "private".to_string()
            }
        }
        "javascript" | "typescript" => {
            if trimmed.starts_with("export") {
                "pub".to_string()
            } else {
                "private".to_string()
            }
        }
        _ => "pub".to_string(),
    }
}

/// Extract the body of a brace-delimited symbol starting from a given line index.
/// Returns the body text (including the opening line) and the end line index.
fn extract_brace_body(lines: &[&str], start_idx: usize) -> (String, usize) {
    let mut depth: i32 = 0;
    let mut found_open = false;
    let mut body_lines: Vec<&str> = Vec::new();

    for (i, line) in lines.iter().enumerate().skip(start_idx) {
        body_lines.push(line);

        for ch in line.chars() {
            match ch {
                '{' => {
                    depth += 1;
                    found_open = true;
                }
                '}' => {
                    depth -= 1;
                }
                _ => {}
            }
        }

        if found_open && depth <= 0 {
            return (body_lines.join("\n"), i);
        }
    }

    // Never found matching close — return what we have
    (body_lines.join("\n"), lines.len().saturating_sub(1))
}

/// Extract the body of a Python indentation-delimited symbol starting from a given line.
/// The declaration is at `start_idx`; body lines are subsequent lines with greater indentation.
fn extract_indent_body(lines: &[&str], start_idx: usize) -> (String, usize) {
    let decl_line = lines[start_idx];
    let base_indent = decl_line.len() - decl_line.trim_start().len();
    let mut body_lines: Vec<&str> = vec![decl_line];
    let mut end_idx = start_idx;

    for (i, line) in lines.iter().enumerate().skip(start_idx + 1) {
        // Empty lines or lines with greater indentation are part of the body
        if line.trim().is_empty() {
            body_lines.push(line);
            end_idx = i;
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if indent > base_indent {
            body_lines.push(line);
            end_idx = i;
        } else {
            break;
        }
    }

    // Trim trailing empty lines
    while body_lines.last().is_some_and(|l| l.trim().is_empty()) {
        body_lines.pop();
        if end_idx > start_idx {
            end_idx -= 1;
        }
    }

    (body_lines.join("\n"), end_idx)
}

/// Extract per-symbol structured records from source code using regex.
///
/// Returns a JSON array where each element is a symbol record with fields:
/// `name`, `kind`, `signature`, `source_file`, `line_number` (zero-padded),
/// `language`, `parent`, `visibility`, `body`.
pub fn extract_code_metadata(content: &str, file_name: &str, ext: &str) -> Value {
    let language = ext_to_language(ext);
    let lines: Vec<&str> = content.lines().collect();
    let raw_symbols = match language {
        "rust" => extract_rust_symbols(&lines),
        "python" => extract_python_symbols(&lines),
        "javascript" | "typescript" => extract_js_ts_symbols(&lines),
        "go" => extract_go_symbols(&lines),
        "java" | "kotlin" | "csharp" => extract_java_like_symbols(&lines),
        "cpp" | "c" => extract_cpp_symbols(&lines),
        _ => extract_generic_symbols(&lines),
    };

    let mut records: Vec<Value> = Vec::new();

    for sym in &raw_symbols {
        let (body, _end) = if language == "python" {
            extract_indent_body(&lines, sym.line_number.saturating_sub(1))
        } else {
            extract_brace_body(&lines, sym.line_number.saturating_sub(1))
        };

        records.push(serde_json::json!({
            "name": sym.name,
            "kind": sym.kind,
            "signature": sym.signature,
            "source_file": file_name,
            "line_number": pad_line_number(sym.line_number),
            "language": language,
            "parent": sym.parent.as_deref().unwrap_or(""),
            "visibility": sym.visibility,
            "body": body,
        }));
    }

    Value::Array(records)
}

// ---------------------------------------------------------------------------
// Language-specific extractors
// ---------------------------------------------------------------------------

fn extract_rust_symbols(lines: &[&str]) -> Vec<RawSymbol> {
    let fn_re = Regex::new(
        r"^\s*(?:pub(?:\((?:crate|super)\))?\s+)?(?:async\s+)?(?:unsafe\s+)?fn\s+(\w+)",
    ).unwrap();
    let struct_re = Regex::new(
        r"^\s*(?:pub(?:\((?:crate|super)\))?\s+)?struct\s+(\w+)",
    ).unwrap();
    let enum_re = Regex::new(
        r"^\s*(?:pub(?:\((?:crate|super)\))?\s+)?enum\s+(\w+)",
    ).unwrap();
    let trait_re = Regex::new(
        r"^\s*(?:pub(?:\((?:crate|super)\))?\s+)?trait\s+(\w+)",
    ).unwrap();
    let type_re = Regex::new(
        r"^\s*(?:pub(?:\((?:crate|super)\))?\s+)?type\s+(\w+)",
    ).unwrap();
    let const_re = Regex::new(
        r"^\s*(?:pub(?:\((?:crate|super)\))?\s+)?(?:const|static)\s+(\w+)",
    ).unwrap();
    let use_re = Regex::new(r"^\s*(?:pub\s+)?use\s+(.+);").unwrap();
    let impl_re = Regex::new(r"^\s*impl(?:<[^>]*>)?\s+(?:(\w+)\s+for\s+)?(\w+)").unwrap();

    let mut symbols = Vec::new();
    let mut current_impl: Option<String> = None;
    let mut impl_depth: i32 = 0;
    let mut brace_depth: i32 = 0;

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;

        // Track brace depth for impl block context
        for ch in line.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => {
                    brace_depth -= 1;
                    if brace_depth <= impl_depth && current_impl.is_some() {
                        current_impl = None;
                    }
                }
                _ => {}
            }
        }

        // Check for impl block (before fn, so we set parent context)
        if let Some(caps) = impl_re.captures(line) {
            let type_name = caps.get(2).map_or("", |m| m.as_str());
            current_impl = Some(type_name.to_string());
            impl_depth = brace_depth - line.chars().filter(|&c| c == '{').count() as i32;
            continue;
        }

        if let Some(caps) = fn_re.captures(line) {
            let name = caps.get(1).unwrap().as_str();
            symbols.push(RawSymbol {
                name: name.to_string(),
                kind: if current_impl.is_some() { "method" } else { "function" }.to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: current_impl.clone(),
                visibility: extract_visibility(line, "rust"),
            });
        } else if let Some(caps) = struct_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "struct".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "rust"),
            });
        } else if let Some(caps) = enum_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "enum".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "rust"),
            });
        } else if let Some(caps) = trait_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "trait".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "rust"),
            });
        } else if let Some(caps) = type_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "type_alias".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: current_impl.clone(),
                visibility: extract_visibility(line, "rust"),
            });
        } else if let Some(caps) = const_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "constant".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: current_impl.clone(),
                visibility: extract_visibility(line, "rust"),
            });
        } else if let Some(caps) = use_re.captures(line) {
            let path = caps.get(1).unwrap().as_str().trim();
            let name = path.rsplit("::").next().unwrap_or(path);
            symbols.push(RawSymbol {
                name: name.to_string(),
                kind: "import".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "rust"),
            });
        }
    }

    symbols
}

fn extract_python_symbols(lines: &[&str]) -> Vec<RawSymbol> {
    let def_re = Regex::new(r"^(\s*)(?:async\s+)?def\s+(\w+)").unwrap();
    let class_re = Regex::new(r"^(\s*)class\s+(\w+)").unwrap();
    let import_re = Regex::new(r"^\s*(?:from\s+\S+\s+)?import\s+(.+)").unwrap();
    let assign_re = Regex::new(r"^([A-Z][A-Z_0-9]+)\s*=").unwrap();

    let mut symbols = Vec::new();
    let mut current_class: Option<String> = None;
    let mut class_indent: usize = 0;

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;

        // Track class scope via indentation
        if let Some(caps) = class_re.captures(line) {
            let indent = caps.get(1).unwrap().as_str().len();
            let name = caps.get(2).unwrap().as_str();
            current_class = Some(name.to_string());
            class_indent = indent;
            symbols.push(RawSymbol {
                name: name.to_string(),
                kind: "class".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "python"),
            });
            continue;
        }

        if let Some(caps) = def_re.captures(line) {
            let indent = caps.get(1).unwrap().as_str().len();
            let name = caps.get(2).unwrap().as_str();

            // If indented more than class, it's a method
            let parent = if current_class.is_some() && indent > class_indent {
                current_class.clone()
            } else {
                if indent <= class_indent {
                    current_class = None;
                }
                None
            };

            symbols.push(RawSymbol {
                name: name.to_string(),
                kind: if parent.is_some() { "method" } else { "function" }.to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent,
                visibility: extract_visibility(line, "python"),
            });
            continue;
        }

        if let Some(caps) = import_re.captures(line) {
            let imported = caps.get(1).unwrap().as_str().trim();
            let name = imported.split(',').next().unwrap_or(imported).trim();
            // Handle "import X as Y" and "from X import Y"
            let name = name.split_whitespace().next().unwrap_or(name);
            symbols.push(RawSymbol {
                name: name.to_string(),
                kind: "import".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: "pub".to_string(),
            });
            continue;
        }

        if let Some(caps) = assign_re.captures(line) {
            let name = caps.get(1).unwrap().as_str();
            symbols.push(RawSymbol {
                name: name.to_string(),
                kind: "constant".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: "pub".to_string(),
            });
        }

        // Reset class context if we hit a non-empty line at class indent or less
        if current_class.is_some() && !line.trim().is_empty() {
            let indent = line.len() - line.trim_start().len();
            if indent <= class_indent && !line.trim().starts_with('#') && !line.trim().starts_with('@') {
                current_class = None;
            }
        }
    }

    symbols
}

fn extract_js_ts_symbols(lines: &[&str]) -> Vec<RawSymbol> {
    let fn_re = Regex::new(
        r"^\s*(?:export\s+)?(?:default\s+)?(?:async\s+)?function\s+(\w+)",
    ).unwrap();
    let class_re = Regex::new(
        r"^\s*(?:export\s+)?(?:default\s+)?(?:abstract\s+)?class\s+(\w+)",
    ).unwrap();
    let interface_re = Regex::new(
        r"^\s*(?:export\s+)?interface\s+(\w+)",
    ).unwrap();
    let type_re = Regex::new(
        r"^\s*(?:export\s+)?type\s+(\w+)\s*=",
    ).unwrap();
    let const_fn_re = Regex::new(
        r"^\s*(?:export\s+)?(?:const|let|var)\s+(\w+)\s*=\s*(?:async\s+)?\(",
    ).unwrap();
    let arrow_fn_re = Regex::new(
        r"^\s*(?:export\s+)?(?:const|let|var)\s+(\w+)\s*=\s*(?:async\s+)?(?:\([^)]*\)|[\w]+)\s*=>",
    ).unwrap();
    let const_re = Regex::new(
        r"^\s*(?:export\s+)?(?:const|let|var)\s+([A-Z][A-Z_0-9]+)\s*=",
    ).unwrap();
    let import_re = Regex::new(r"^\s*import\s+").unwrap();
    let enum_re = Regex::new(r"^\s*(?:export\s+)?(?:const\s+)?enum\s+(\w+)").unwrap();

    let mut symbols = Vec::new();
    let mut current_class: Option<String> = None;
    let mut class_brace_depth: i32 = 0;
    let mut brace_depth: i32 = 0;

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;

        let prev_depth = brace_depth;
        for ch in line.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }

        if current_class.is_some() && brace_depth <= class_brace_depth {
            current_class = None;
        }

        if let Some(caps) = class_re.captures(line) {
            let name = caps.get(1).unwrap().as_str();
            current_class = Some(name.to_string());
            class_brace_depth = prev_depth;
            symbols.push(RawSymbol {
                name: name.to_string(),
                kind: "class".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "javascript"),
            });
        } else if let Some(caps) = fn_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: if current_class.is_some() { "method" } else { "function" }.to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: current_class.clone(),
                visibility: extract_visibility(line, "javascript"),
            });
        } else if let Some(caps) = interface_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "interface".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "typescript"),
            });
        } else if let Some(caps) = enum_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "enum".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "typescript"),
            });
        } else if let Some(caps) = type_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "type_alias".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "typescript"),
            });
        } else if let Some(caps) = arrow_fn_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "function".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: current_class.clone(),
                visibility: extract_visibility(line, "javascript"),
            });
        } else if let Some(caps) = const_fn_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "function".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: current_class.clone(),
                visibility: extract_visibility(line, "javascript"),
            });
        } else if let Some(caps) = const_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "constant".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "javascript"),
            });
        } else if import_re.is_match(line) {
            // Extract the module path from import statement
            let trimmed = line.trim().trim_end_matches(';');
            let name = trimmed
                .rsplit("from")
                .next()
                .unwrap_or(trimmed)
                .trim()
                .trim_matches(|c| c == '\'' || c == '"');
            symbols.push(RawSymbol {
                name: name.to_string(),
                kind: "import".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: "pub".to_string(),
            });
        }
    }

    symbols
}

fn extract_go_symbols(lines: &[&str]) -> Vec<RawSymbol> {
    let func_re = Regex::new(r"^\s*func\s+(?:\([^)]+\)\s*)?(\w+)").unwrap();
    let type_struct_re = Regex::new(r"^\s*type\s+(\w+)\s+struct\b").unwrap();
    let type_interface_re = Regex::new(r"^\s*type\s+(\w+)\s+interface\b").unwrap();
    let type_alias_re = Regex::new(r"^\s*type\s+(\w+)\s+\w").unwrap();
    let const_re = Regex::new(r"^\s*(?:const|var)\s+(\w+)").unwrap();
    let import_re = Regex::new(r#"^\s*import\s+(?:\(\s*)?(?:"([^"]+)")"#).unwrap();
    let receiver_re = Regex::new(r"^\s*func\s+\((\w+)\s+\*?(\w+)\)\s*(\w+)").unwrap();

    let mut symbols = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;

        if let Some(caps) = receiver_re.captures(line) {
            let parent_type = caps.get(2).unwrap().as_str();
            let method_name = caps.get(3).unwrap().as_str();
            symbols.push(RawSymbol {
                name: method_name.to_string(),
                kind: "method".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: Some(parent_type.to_string()),
                visibility: extract_visibility(line, "go"),
            });
        } else if let Some(caps) = func_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "function".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "go"),
            });
        } else if let Some(caps) = type_struct_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "struct".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "go"),
            });
        } else if let Some(caps) = type_interface_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "interface".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "go"),
            });
        } else if let Some(caps) = type_alias_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "type_alias".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "go"),
            });
        } else if let Some(caps) = const_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "constant".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "go"),
            });
        } else if let Some(caps) = import_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "import".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: "pub".to_string(),
            });
        }
    }

    symbols
}

fn extract_java_like_symbols(lines: &[&str]) -> Vec<RawSymbol> {
    let class_re = Regex::new(
        r"^\s*(?:public\s+|private\s+|protected\s+)?(?:abstract\s+|final\s+)?class\s+(\w+)",
    ).unwrap();
    let interface_re = Regex::new(
        r"^\s*(?:public\s+|private\s+|protected\s+)?interface\s+(\w+)",
    ).unwrap();
    let method_re = Regex::new(
        r"^\s*(?:public\s+|private\s+|protected\s+)?(?:static\s+)?(?:abstract\s+)?(?:final\s+)?(?:synchronized\s+)?(?:\w+(?:<[^>]*>)?)\s+(\w+)\s*\(",
    ).unwrap();
    let import_re = Regex::new(r"^\s*import\s+(.+);").unwrap();
    let enum_re = Regex::new(
        r"^\s*(?:public\s+|private\s+|protected\s+)?enum\s+(\w+)",
    ).unwrap();

    let mut symbols = Vec::new();
    let mut current_class: Option<String> = None;
    let mut class_brace_depth: i32 = 0;
    let mut brace_depth: i32 = 0;

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;

        let prev_depth = brace_depth;
        for ch in line.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }

        if current_class.is_some() && brace_depth <= class_brace_depth {
            current_class = None;
        }

        if let Some(caps) = class_re.captures(line) {
            let name = caps.get(1).unwrap().as_str();
            current_class = Some(name.to_string());
            class_brace_depth = prev_depth;
            symbols.push(RawSymbol {
                name: name.to_string(),
                kind: "class".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "java"),
            });
        } else if let Some(caps) = interface_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "interface".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "java"),
            });
        } else if let Some(caps) = enum_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "enum".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "java"),
            });
        } else if let Some(caps) = method_re.captures(line) {
            let name = caps.get(1).unwrap().as_str();
            // Skip common false positives like control flow keywords
            if !["if", "else", "for", "while", "switch", "catch", "return", "new"].contains(&name) {
                symbols.push(RawSymbol {
                    name: name.to_string(),
                    kind: if current_class.is_some() { "method" } else { "function" }.to_string(),
                    signature: line.trim().to_string(),
                    line_number: line_num,
                    parent: current_class.clone(),
                    visibility: extract_visibility(line, "java"),
                });
            }
        } else if let Some(caps) = import_re.captures(line) {
            let path = caps.get(1).unwrap().as_str().trim();
            let name = path.rsplit('.').next().unwrap_or(path);
            symbols.push(RawSymbol {
                name: name.to_string(),
                kind: "import".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: "pub".to_string(),
            });
        }
    }

    symbols
}

fn extract_cpp_symbols(lines: &[&str]) -> Vec<RawSymbol> {
    let class_re = Regex::new(r"^\s*(?:class|struct)\s+(\w+)").unwrap();
    let fn_re = Regex::new(
        r"^\s*(?:static\s+|virtual\s+|inline\s+)?(?:const\s+)?(?:\w+(?:::\w+)*(?:<[^>]*>)?[*&\s]+)(\w+)\s*\(",
    ).unwrap();
    let include_re = Regex::new(r#"^\s*#include\s+[<"]([^>"]+)[>"]"#).unwrap();
    let enum_re = Regex::new(r"^\s*enum\s+(?:class\s+)?(\w+)").unwrap();

    let mut symbols = Vec::new();
    let mut current_class: Option<String> = None;
    let mut class_brace_depth: i32 = 0;
    let mut brace_depth: i32 = 0;

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;

        let prev_depth = brace_depth;
        for ch in line.chars() {
            match ch {
                '{' => brace_depth += 1,
                '}' => brace_depth -= 1,
                _ => {}
            }
        }

        if current_class.is_some() && brace_depth <= class_brace_depth {
            current_class = None;
        }

        if let Some(caps) = class_re.captures(line) {
            let name = caps.get(1).unwrap().as_str();
            current_class = Some(name.to_string());
            class_brace_depth = prev_depth;
            let kind = if line.trim().starts_with("struct") { "struct" } else { "class" };
            symbols.push(RawSymbol {
                name: name.to_string(),
                kind: kind.to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "cpp"),
            });
        } else if let Some(caps) = enum_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "enum".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: extract_visibility(line, "cpp"),
            });
        } else if let Some(caps) = fn_re.captures(line) {
            let name = caps.get(1).unwrap().as_str();
            if !["if", "else", "for", "while", "switch", "catch", "return", "new", "delete"].contains(&name) {
                symbols.push(RawSymbol {
                    name: name.to_string(),
                    kind: if current_class.is_some() { "method" } else { "function" }.to_string(),
                    signature: line.trim().to_string(),
                    line_number: line_num,
                    parent: current_class.clone(),
                    visibility: extract_visibility(line, "cpp"),
                });
            }
        } else if let Some(caps) = include_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "import".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: "pub".to_string(),
            });
        }
    }

    symbols
}

/// Generic fallback: catch functions/classes with common patterns.
fn extract_generic_symbols(lines: &[&str]) -> Vec<RawSymbol> {
    let fn_re = Regex::new(
        r"^\s*(?:pub\s+)?(?:async\s+)?(?:fn|def|function|func|sub)\s+(?:\([^)]*\)\s*)?(\w+)",
    ).unwrap();
    let class_re = Regex::new(
        r"^\s*(?:pub\s+)?(?:class|struct|trait|enum|interface|type)\s+(\w+)",
    ).unwrap();

    let mut symbols = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        let line_num = idx + 1;

        if let Some(caps) = fn_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "function".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: "pub".to_string(),
            });
        } else if let Some(caps) = class_re.captures(line) {
            symbols.push(RawSymbol {
                name: caps.get(1).unwrap().as_str().to_string(),
                kind: "class".to_string(),
                signature: line.trim().to_string(),
                line_number: line_num,
                parent: None,
                visibility: "pub".to_string(),
            });
        }
    }

    symbols
}

/// Wrap plain-text content (`.txt`, `.md`, config files) as a `Value`.
fn wrap_text_content(content: &str, file_name: &str, ext: &str) -> Value {
    // Derive a human-readable category hint from the file path so the AI
    // can propose a semantic schema name (e.g., "recipes" instead of "txt").
    let category_hint = derive_category_hint(file_name);
    let mut obj = serde_json::json!({
        "content": content,
        "source_file": file_name,
        "file_type": ext
    });
    if let Some(hint) = category_hint {
        obj["category"] = serde_json::json!(hint);
    }
    obj
}

/// Derive a category hint from the file path by looking at parent directory
/// names and the filename itself. Returns None if no useful hint can be derived.
fn derive_category_hint(file_path: &str) -> Option<String> {
    let path = std::path::Path::new(file_path);
    // Use the parent directory name if available (e.g., "recipes/cookies.txt" -> "recipes")
    if let Some(parent) = path.parent().and_then(|p| p.file_name()) {
        let dir = parent.to_string_lossy();
        if !dir.is_empty() && dir != "." && dir != ".." {
            return Some(dir.to_string());
        }
    }
    None
}

/// Returns true if the content looks like a Twitter data export JS file.
fn is_twitter_export_js(content: &str) -> bool {
    let s = content.trim_start();
    s.starts_with("window.YTD.") || s.starts_with("window.__THAR_CONFIG")
}

/// Convert a `.js` file — routes to Twitter export parser or code metadata
/// based on content prefix.
fn js_to_json(content: &str, file_name: &str) -> IngestionResult<Value> {
    if is_twitter_export_js(content) {
        let json_string = twitter_js_to_json(content)?;
        serde_json::from_str(&json_string)
            .map_err(|e| IngestionError::InvalidInput(format!("Failed to parse JSON: {}", e)))
    } else {
        Ok(extract_code_metadata(content, file_name, "js"))
    }
}

/// Parse file content into a JSON `Value` based on the file extension.
fn parse_content_by_ext(content: &str, file_name: &str, ext: &str) -> IngestionResult<Value> {
    match ext {
        "json" => serde_json::from_str(content)
            .map_err(|e| IngestionError::InvalidInput(format!("Failed to parse JSON: {}", e))),
        "js" => js_to_json(content, file_name),
        "csv" => {
            let json_string = csv_to_json(content)?;
            serde_json::from_str(&json_string)
                .map_err(|e| IngestionError::InvalidInput(format!("Failed to parse JSON: {}", e)))
        }
        "txt" | "md" => Ok(wrap_text_content(content, file_name, ext)),
        e if CODE_EXTS.contains(&e) => Ok(extract_code_metadata(content, file_name, e)),
        e if CONFIG_EXTS.contains(&e) => Ok(wrap_text_content(content, file_name, e)),
        _ => Err(IngestionError::InvalidInput(format!(
            "Unsupported file type: {}",
            ext
        ))),
    }
}

/// Read a file and convert it to a JSON Value regardless of format.
///
/// Supported extensions: `.json`, `.js` (Twitter export or code), `.csv`,
/// `.txt`, `.md`, code files, and config files (`.yaml`, `.yml`, `.toml`, `.xml`).
pub fn read_file_as_json(file_path: &Path) -> IngestionResult<Value> {
    let (value, _, _) = read_file_with_hash(file_path)?;
    Ok(value)
}

/// Read a file, compute its SHA256 hash, and convert to JSON.
/// Returns `(json_value, sha256_hex_hash)`.
pub fn read_file_with_hash(file_path: &Path) -> IngestionResult<(Value, String, Vec<u8>)> {
    use sha2::{Digest, Sha256};

    let raw_bytes = std::fs::read(file_path)
        .map_err(|e| IngestionError::InvalidInput(format!("Failed to read file: {}", e)))?;

    let hash_hex = format!("{:x}", Sha256::digest(&raw_bytes));

    let content = std::str::from_utf8(&raw_bytes)
        .map_err(|e| IngestionError::InvalidInput(format!("File is not valid UTF-8: {}", e)))?;

    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| {
            IngestionError::InvalidInput(format!(
                "Failed to derive file name from path: {}",
                file_path.display()
            ))
        })?;

    let value = parse_content_by_ext(content, file_name, &ext)?;

    Ok((value, hash_hex, raw_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::Write;

    // ---- Helper to find a symbol by name in the array ----

    fn find_symbol<'a>(symbols: &'a Value, name: &str) -> Option<&'a Value> {
        symbols.as_array().unwrap().iter().find(|s| s["name"] == name)
    }

    fn find_symbols_by_kind<'a>(symbols: &'a Value, kind: &str) -> Vec<&'a Value> {
        symbols
            .as_array()
            .unwrap()
            .iter()
            .filter(|s| s["kind"] == kind)
            .collect()
    }

    // ---- File reading tests (unchanged behavior) ----

    #[test]
    fn test_read_file_with_hash_json() {
        let mut tmp = tempfile::Builder::new().suffix(".json").tempfile().unwrap();
        let json_content = r#"{"name": "Alice", "age": 30}"#;
        write!(tmp, "{}", json_content).unwrap();

        let (value, hash, raw) = read_file_with_hash(tmp.path()).unwrap();
        assert_eq!(value["name"], "Alice");
        assert_eq!(value["age"], 30);

        let expected_hash = format!("{:x}", Sha256::digest(json_content.as_bytes()));
        assert_eq!(hash, expected_hash);
        assert_eq!(raw, json_content.as_bytes());
    }

    #[test]
    fn test_read_file_with_hash_twitter_js() {
        let mut tmp = tempfile::Builder::new().suffix(".js").tempfile().unwrap();
        let content = r#"window.YTD.tweet.part0 = [{"id": "123", "text": "hello"}]"#;
        write!(tmp, "{}", content).unwrap();

        let (value, hash, _raw) = read_file_with_hash(tmp.path()).unwrap();
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["id"], "123");

        let expected_hash = format!("{:x}", Sha256::digest(content.as_bytes()));
        assert_eq!(hash, expected_hash);
    }

    #[test]
    fn test_read_file_with_hash_csv() {
        let mut tmp = tempfile::Builder::new().suffix(".csv").tempfile().unwrap();
        let content = "name,age\nAlice,30\nBob,25\n";
        write!(tmp, "{}", content).unwrap();

        let (value, hash, _raw) = read_file_with_hash(tmp.path()).unwrap();
        let arr = value.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["name"], "Alice");

        let expected_hash = format!("{:x}", Sha256::digest(content.as_bytes()));
        assert_eq!(hash, expected_hash);
    }

    #[test]
    fn test_read_file_with_hash_txt() {
        let mut tmp = tempfile::Builder::new().suffix(".txt").tempfile().unwrap();
        let content = "Hello, this is a text file.";
        write!(tmp, "{}", content).unwrap();

        let (value, hash, _raw) = read_file_with_hash(tmp.path()).unwrap();
        assert_eq!(value["content"], content);
        assert_eq!(value["file_type"], "txt");
        assert!(value["source_file"].as_str().unwrap().ends_with(".txt"));

        let expected_hash = format!("{:x}", Sha256::digest(content.as_bytes()));
        assert_eq!(hash, expected_hash);
    }

    #[test]
    fn test_read_file_with_hash_md() {
        let mut tmp = tempfile::Builder::new().suffix(".md").tempfile().unwrap();
        let content = "# Heading\n\nSome markdown content.";
        write!(tmp, "{}", content).unwrap();

        let (value, hash, _raw) = read_file_with_hash(tmp.path()).unwrap();
        assert_eq!(value["content"], content);
        assert_eq!(value["file_type"], "md");
        assert!(value["source_file"].as_str().unwrap().ends_with(".md"));

        let expected_hash = format!("{:x}", Sha256::digest(content.as_bytes()));
        assert_eq!(hash, expected_hash);
    }

    #[test]
    fn test_read_file_with_hash_unsupported_extension() {
        let mut tmp = tempfile::Builder::new().suffix(".xyz").tempfile().unwrap();
        write!(tmp, "some content").unwrap();

        let result = read_file_with_hash(tmp.path());
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("Unsupported file type"));
    }

    #[test]
    fn test_read_file_with_hash_nonexistent_file() {
        let result = read_file_with_hash(Path::new("/tmp/nonexistent_file_abc123.json"));
        assert!(result.is_err());
        let err_msg = format!("{}", result.unwrap_err());
        assert!(err_msg.contains("Failed to read file"));
    }

    // ---- extract_code_metadata: now returns array of symbol records ----

    #[test]
    fn test_extract_returns_array() {
        let content = "pub fn main() {}\n";
        let val = extract_code_metadata(content, "lib.rs", "rs");
        assert!(val.is_array());
    }

    #[test]
    fn test_symbol_record_fields() {
        let content = "pub fn greet(name: &str) {\n    println!(\"Hi {}\", name);\n}\n";
        let val = extract_code_metadata(content, "lib.rs", "rs");
        let sym = find_symbol(&val, "greet").unwrap();

        assert_eq!(sym["kind"], "function");
        assert_eq!(sym["signature"], "pub fn greet(name: &str) {");
        assert_eq!(sym["source_file"], "lib.rs");
        assert_eq!(sym["line_number"], "000001");
        assert_eq!(sym["language"], "rust");
        assert_eq!(sym["visibility"], "pub");
        assert!(sym["body"].as_str().unwrap().contains("println!"));
    }

    #[test]
    fn test_extract_code_metadata_python() {
        let content = r#"import os
from pathlib import Path

MAX_SIZE = 100

class MyClass:
    def __init__(self):
        pass

    def method(self):
        return 42

def foo(x, y):
    return x + y

async def bar():
    pass
"#;
        let val = extract_code_metadata(content, "example.py", "py");
        let arr = val.as_array().unwrap();
        assert!(!arr.is_empty());

        // Check imports
        let imports = find_symbols_by_kind(&val, "import");
        assert!(imports.len() >= 2);

        // Check constant
        let constant = find_symbol(&val, "MAX_SIZE").unwrap();
        assert_eq!(constant["kind"], "constant");

        // Check class
        let class = find_symbol(&val, "MyClass").unwrap();
        assert_eq!(class["kind"], "class");
        assert_eq!(class["language"], "python");

        // Check methods with parent
        let init = find_symbol(&val, "__init__").unwrap();
        assert_eq!(init["kind"], "method");
        assert_eq!(init["parent"], "MyClass");

        let method = find_symbol(&val, "method").unwrap();
        assert_eq!(method["kind"], "method");
        assert_eq!(method["parent"], "MyClass");

        // Check top-level functions
        let foo = find_symbol(&val, "foo").unwrap();
        assert_eq!(foo["kind"], "function");
        assert_eq!(foo["parent"], "");

        let bar = find_symbol(&val, "bar").unwrap();
        assert_eq!(bar["kind"], "function");
    }

    #[test]
    fn test_extract_code_metadata_rust() {
        let content = r#"use std::collections::HashMap;

pub struct Greeter {
    name: String,
}

impl Greeter {
    pub fn new(name: String) -> Self {
        Self { name }
    }

    fn private_helper(&self) -> bool {
        true
    }
}

pub async fn greet(name: &str) {
    println!("Hello, {}", name);
}

enum Color {
    Red,
    Blue,
}

pub const MAX: usize = 100;

pub type Result<T> = std::result::Result<T, Error>;
"#;
        let val = extract_code_metadata(content, "lib.rs", "rs");

        // Import
        let import = find_symbol(&val, "HashMap").unwrap();
        assert_eq!(import["kind"], "import");

        // Struct
        let greeter = find_symbol(&val, "Greeter").unwrap();
        assert_eq!(greeter["kind"], "struct");
        assert_eq!(greeter["visibility"], "pub");
        assert!(greeter["body"].as_str().unwrap().contains("name: String"));

        // Methods with parent
        let new_fn = find_symbol(&val, "new").unwrap();
        assert_eq!(new_fn["kind"], "method");
        assert_eq!(new_fn["parent"], "Greeter");
        assert_eq!(new_fn["visibility"], "pub");

        let helper = find_symbol(&val, "private_helper").unwrap();
        assert_eq!(helper["kind"], "method");
        assert_eq!(helper["parent"], "Greeter");
        assert_eq!(helper["visibility"], "private");

        // Top-level async fn
        let greet = find_symbol(&val, "greet").unwrap();
        assert_eq!(greet["kind"], "function");
        assert_eq!(greet["parent"], "");
        assert_eq!(greet["visibility"], "pub");

        // Enum
        let color = find_symbol(&val, "Color").unwrap();
        assert_eq!(color["kind"], "enum");

        // Constant
        let max = find_symbol(&val, "MAX").unwrap();
        assert_eq!(max["kind"], "constant");

        // Type alias
        let result = find_symbol(&val, "Result").unwrap();
        assert_eq!(result["kind"], "type_alias");
    }

    #[test]
    fn test_extract_code_metadata_javascript() {
        let content = r#"import { useState } from 'react';

export const MAX_RETRIES = 5;

export function greet(name) {
    console.log("Hello " + name);
}

const helper = (x) => x + 1;

class App {
    constructor() {}
}
"#;
        let val = extract_code_metadata(content, "app.js", "js");

        // Import
        let imports = find_symbols_by_kind(&val, "import");
        assert!(!imports.is_empty());

        // Constant
        let max = find_symbol(&val, "MAX_RETRIES").unwrap();
        assert_eq!(max["kind"], "constant");

        // Function
        let greet = find_symbol(&val, "greet").unwrap();
        assert_eq!(greet["kind"], "function");
        assert_eq!(greet["visibility"], "pub"); // exported

        // Arrow function
        let helper = find_symbol(&val, "helper").unwrap();
        assert_eq!(helper["kind"], "function");

        // Class
        let app = find_symbol(&val, "App").unwrap();
        assert_eq!(app["kind"], "class");
    }

    #[test]
    fn test_extract_code_metadata_go() {
        let content = r#"package main

import "fmt"

type Server struct {
    Port int
}

func main() {
    fmt.Println("hello")
}

func (s *Server) Start() {
    fmt.Println("starting")
}
"#;
        let val = extract_code_metadata(content, "main.go", "go");

        // Struct
        let server = find_symbol(&val, "Server").unwrap();
        assert_eq!(server["kind"], "struct");
        assert_eq!(server["visibility"], "pub"); // uppercase = exported

        // Top-level function
        let main_fn = find_symbol(&val, "main").unwrap();
        assert_eq!(main_fn["kind"], "function");
        assert_eq!(main_fn["visibility"], "private"); // lowercase

        // Method with receiver
        let start = find_symbol(&val, "Start").unwrap();
        assert_eq!(start["kind"], "method");
        assert_eq!(start["parent"], "Server");
        assert_eq!(start["visibility"], "pub");
    }

    #[test]
    fn test_extract_code_metadata_java() {
        let content = r#"import java.util.List;

public class UserService {
    private String name;

    public void createUser(String name) {
        this.name = name;
    }

    protected List<String> getUsers() {
        return null;
    }
}
"#;
        let val = extract_code_metadata(content, "UserService.java", "java");

        let import = find_symbol(&val, "List").unwrap();
        assert_eq!(import["kind"], "import");

        let class = find_symbol(&val, "UserService").unwrap();
        assert_eq!(class["kind"], "class");
        assert_eq!(class["visibility"], "pub");

        let create = find_symbol(&val, "createUser").unwrap();
        assert_eq!(create["kind"], "method");
        assert_eq!(create["parent"], "UserService");
        assert_eq!(create["visibility"], "pub");

        let get = find_symbol(&val, "getUsers").unwrap();
        assert_eq!(get["kind"], "method");
        assert_eq!(get["visibility"], "protected");
    }

    #[test]
    fn test_extract_code_metadata_cpp() {
        let content = r#"#include <iostream>
#include "mylib.h"

class Widget {
    void draw() {
        std::cout << "drawing" << std::endl;
    }
};

int main() {
    Widget w;
    return 0;
}
"#;
        let val = extract_code_metadata(content, "main.cpp", "cpp");

        let includes = find_symbols_by_kind(&val, "import");
        assert_eq!(includes.len(), 2);

        let widget = find_symbol(&val, "Widget").unwrap();
        assert_eq!(widget["kind"], "class");

        let main_fn = find_symbol(&val, "main").unwrap();
        assert_eq!(main_fn["kind"], "function");
    }

    #[test]
    fn test_line_number_zero_padding() {
        let mut content = String::new();
        for i in 0..100 {
            content.push_str(&format!("// line {}\n", i));
        }
        content.push_str("pub fn late_function() {\n}\n");

        let val = extract_code_metadata(&content, "big.rs", "rs");
        let late = find_symbol(&val, "late_function").unwrap();
        assert_eq!(late["line_number"], "000101");
    }

    #[test]
    fn test_body_extraction_rust() {
        let content = r#"fn add(a: i32, b: i32) -> i32 {
    let result = a + b;
    result
}
"#;
        let val = extract_code_metadata(content, "math.rs", "rs");
        let add = find_symbol(&val, "add").unwrap();
        let body = add["body"].as_str().unwrap();
        assert!(body.contains("let result = a + b"));
        assert!(body.contains("result"));
        assert!(body.ends_with('}'));
    }

    #[test]
    fn test_body_extraction_python() {
        let content = r#"def greet(name):
    msg = f"Hello {name}"
    print(msg)
    return msg

def other():
    pass
"#;
        let val = extract_code_metadata(content, "greet.py", "py");
        let greet = find_symbol(&val, "greet").unwrap();
        let body = greet["body"].as_str().unwrap();
        assert!(body.contains("msg = f\"Hello {name}\""));
        assert!(body.contains("return msg"));
        // Should NOT include the next function
        assert!(!body.contains("def other"));
    }

    #[test]
    fn test_extract_code_metadata_empty_file() {
        let val = extract_code_metadata("", "empty.py", "py");
        assert!(val.is_array());
        assert!(val.as_array().unwrap().is_empty());
    }

    #[test]
    fn test_extract_code_metadata_typescript_interface_and_enum() {
        let content = r#"export interface User {
    name: string;
    age: number;
}

export type UserId = string;

export enum Status {
    Active,
    Inactive,
}
"#;
        let val = extract_code_metadata(content, "types.ts", "ts");

        let user = find_symbol(&val, "User").unwrap();
        assert_eq!(user["kind"], "interface");
        assert_eq!(user["language"], "typescript");

        let user_id = find_symbol(&val, "UserId").unwrap();
        assert_eq!(user_id["kind"], "type_alias");

        let status = find_symbol(&val, "Status").unwrap();
        assert_eq!(status["kind"], "enum");
    }

    #[test]
    fn test_ext_to_language() {
        assert_eq!(ext_to_language("rs"), "rust");
        assert_eq!(ext_to_language("py"), "python");
        assert_eq!(ext_to_language("js"), "javascript");
        assert_eq!(ext_to_language("ts"), "typescript");
        assert_eq!(ext_to_language("go"), "go");
        assert_eq!(ext_to_language("java"), "java");
        assert_eq!(ext_to_language("cpp"), "cpp");
        assert_eq!(ext_to_language("sh"), "shell");
        assert_eq!(ext_to_language("unknown"), "unknown");
    }

    // ---- Twitter/JS routing tests ----

    #[test]
    fn test_js_twitter_format_detected_by_prefix() {
        let content = r#"window.YTD.tweet.part0 = [{"id": "1"}]"#;
        let val = js_to_json(content, "tweets.js").unwrap();
        assert!(val.is_array());
        assert_eq!(val[0]["id"], "1");
    }

    #[test]
    fn test_js_non_twitter_gets_symbol_array() {
        let content = "function hello() { return 42; }\n";
        let val = js_to_json(content, "app.js").unwrap();
        assert!(val.is_array());
        let sym = find_symbol(&val, "hello").unwrap();
        assert_eq!(sym["kind"], "function");
        assert_eq!(sym["source_file"], "app.js");
    }

    #[test]
    fn test_js_with_equals_not_twitter_gets_symbol_array() {
        let content = "const data = [1, 2, 3];\nfunction process() {}\n";
        let val = js_to_json(content, "utils.js").unwrap();
        assert!(val.is_array());
        let sym = find_symbol(&val, "process").unwrap();
        assert_eq!(sym["kind"], "function");
    }

    // ---- File-level integration tests ----

    #[test]
    fn test_read_file_with_hash_code_file() {
        let mut tmp = tempfile::Builder::new().suffix(".py").tempfile().unwrap();
        let content = "def greet():\n    print('hi')\n";
        write!(tmp, "{}", content).unwrap();

        let (value, _hash, _raw) = read_file_with_hash(tmp.path()).unwrap();
        assert!(value.is_array());
        let sym = find_symbol(&value, "greet").unwrap();
        assert_eq!(sym["kind"], "function");
        assert_eq!(sym["language"], "python");
    }

    #[test]
    fn test_read_file_with_hash_config_file() {
        let mut tmp = tempfile::Builder::new().suffix(".yaml").tempfile().unwrap();
        let content = "name: test\nversion: 1.0\n";
        write!(tmp, "{}", content).unwrap();

        let (value, _hash, _raw) = read_file_with_hash(tmp.path()).unwrap();
        assert_eq!(value["content"], content);
        assert_eq!(value["file_type"], "yaml");
    }

    #[test]
    fn test_read_file_as_json_code_file() {
        let mut tmp = tempfile::Builder::new().suffix(".rs").tempfile().unwrap();
        let content = "pub fn main() {\n    println!(\"hello\");\n}\n";
        write!(tmp, "{}", content).unwrap();

        let value = read_file_as_json(tmp.path()).unwrap();
        assert!(value.is_array());
        let sym = find_symbol(&value, "main").unwrap();
        assert_eq!(sym["kind"], "function");
        assert_eq!(sym["visibility"], "pub");
    }

    #[test]
    fn test_read_file_as_json_config_file() {
        let mut tmp = tempfile::Builder::new().suffix(".toml").tempfile().unwrap();
        let content = "[package]\nname = \"test\"\n";
        write!(tmp, "{}", content).unwrap();

        let value = read_file_as_json(tmp.path()).unwrap();
        assert_eq!(value["content"], content);
        assert_eq!(value["file_type"], "toml");
    }

    #[test]
    fn test_read_file_with_hash_js_non_twitter() {
        let mut tmp = tempfile::Builder::new().suffix(".js").tempfile().unwrap();
        let content = "function setup() { return true; }\n";
        write!(tmp, "{}", content).unwrap();

        let (value, _hash, _raw) = read_file_with_hash(tmp.path()).unwrap();
        assert!(value.is_array());
        let sym = find_symbol(&value, "setup").unwrap();
        assert_eq!(sym["kind"], "function");
    }

    #[test]
    fn test_rust_visibility_variants() {
        let content = r#"pub fn public_fn() {}
pub(crate) fn crate_fn() {}
fn private_fn() {}
"#;
        let val = extract_code_metadata(content, "vis.rs", "rs");

        let pub_fn = find_symbol(&val, "public_fn").unwrap();
        assert_eq!(pub_fn["visibility"], "pub");

        let crate_fn = find_symbol(&val, "crate_fn").unwrap();
        assert_eq!(crate_fn["visibility"], "pub(crate)");

        let priv_fn = find_symbol(&val, "private_fn").unwrap();
        assert_eq!(priv_fn["visibility"], "private");
    }

    #[test]
    fn test_python_nested_methods_have_correct_parent() {
        let content = r#"class Animal:
    def speak(self):
        return "..."

    def move(self):
        return "walking"

class Vehicle:
    def start(self):
        return "vroom"
"#;
        let val = extract_code_metadata(content, "models.py", "py");

        let speak = find_symbol(&val, "speak").unwrap();
        assert_eq!(speak["parent"], "Animal");

        // move is a built-in but valid method name
        let mv = find_symbol(&val, "move").unwrap();
        assert_eq!(mv["parent"], "Animal");

        let start = find_symbol(&val, "start").unwrap();
        assert_eq!(start["parent"], "Vehicle");
    }

    #[test]
    fn test_generic_language_fallback() {
        let content = "sub process_data {\n    my $x = 1;\n}\n";
        let val = extract_code_metadata(content, "script.pl", "pl");
        let sym = find_symbol(&val, "process_data").unwrap();
        assert_eq!(sym["kind"], "function");
        assert_eq!(sym["language"], "perl");
    }
}
