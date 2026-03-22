use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::ToolResult;
use aura_reasoner::ToolDefinition;
use std::fs;
use tracing::{debug, instrument};

/// Maximum compiled regex size (bytes) accepted by `search_code`.
const SEARCH_REGEX_SIZE_LIMIT: usize = 1_000_000;

/// Directories automatically skipped during code search.
const SEARCH_SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "__pycache__",
    "dist",
    "build",
    ".next",
    "vendor",
    ".venv",
    "coverage",
    ".tox",
    ".mypy_cache",
];

/// Format a single match with context lines.
fn format_match_with_context(
    relative_path: &str,
    lines: &[&str],
    line_idx: usize,
    context: usize,
) -> String {
    use std::fmt::Write;

    let start = line_idx.saturating_sub(context);
    let end = (line_idx + context + 1).min(lines.len());
    let mut block = format!("{relative_path}:{}", line_idx + 1);
    for (ctx_idx, ctx_line) in lines[start..end].iter().enumerate() {
        let abs_idx = start + ctx_idx;
        let marker = if abs_idx == line_idx { ">" } else { " " };
        let _ = write!(block, "\n{marker} {:>4}|{ctx_line}", abs_idx + 1);
    }
    block
}

/// Build a diagnostic message when `search_code` finds zero matches.
fn zero_match_diagnostic(sandbox: &Sandbox, path: Option<&str>, pattern: &str) -> String {
    use std::fmt::Write;

    let mut msg = String::from("No matches found");
    if let Some(p) = path {
        let resolved = sandbox.resolve(p);
        if resolved.is_err() || !resolved.as_ref().is_ok_and(|r| r.exists()) {
            let _ = write!(msg, ". Note: path '{p}' does not exist");
        }
    }
    if pattern.contains('\\') || pattern.contains('[') || pattern.contains('(') {
        msg.push_str(". Tip: check that special regex characters are escaped correctly");
    }
    msg
}

/// Search for patterns in code.
///
/// Supports a `context_lines` parameter (0–10) that, when > 0, includes
/// surrounding lines with `>` marking each match line.
#[instrument(skip(sandbox), fields(pattern = %pattern))]
pub fn search_code(
    sandbox: &Sandbox,
    pattern: &str,
    path: Option<&str>,
    file_pattern: Option<&str>,
    max_results: usize,
    context_lines: usize,
) -> Result<ToolResult, ToolError> {
    use regex::Regex;
    use walkdir::WalkDir;

    let context_lines = context_lines.min(10);

    let search_root = match path {
        Some(p) => sandbox.resolve_existing(p)?,
        None => sandbox.root().to_path_buf(),
    };

    debug!(?search_root, "Searching code");

    let regex = Regex::new(pattern)
        .map_err(|e| ToolError::InvalidArguments(format!("Invalid regex: {e}")))?;

    if regex.as_str().len() > SEARCH_REGEX_SIZE_LIMIT {
        return Err(ToolError::InvalidArguments(format!(
            "Regex pattern exceeds size limit of {SEARCH_REGEX_SIZE_LIMIT} bytes"
        )));
    }

    let file_pattern_regex = file_pattern
        .map(|p| {
            let regex_pattern = p.replace('.', r"\.").replace('*', ".*").replace('?', ".");
            Regex::new(&format!("^{regex_pattern}$"))
        })
        .transpose()
        .map_err(|e| ToolError::InvalidArguments(format!("Invalid file pattern: {e}")))?;

    let mut results = Vec::new();

    for entry in WalkDir::new(&search_root)
        .follow_links(true)
        .into_iter()
        .filter_entry(|e| {
            if e.file_type().is_dir() {
                let name = e.file_name().to_string_lossy();
                return !SEARCH_SKIP_DIRS.contains(&name.as_ref());
            }
            true
        })
        .filter_map(Result::ok)
    {
        if results.len() >= max_results {
            break;
        }

        let entry_path = entry.path();
        if !entry_path.is_file() {
            continue;
        }

        if let Some(ref fp_regex) = file_pattern_regex {
            let file_name = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            if !fp_regex.is_match(file_name) {
                continue;
            }
        }

        if !is_text_file(entry_path) {
            continue;
        }

        if let Ok(file_content) = fs::read_to_string(entry_path) {
            let lines: Vec<&str> = file_content.lines().collect();
            let relative_path = entry_path
                .strip_prefix(&search_root)
                .unwrap_or(entry_path)
                .to_string_lossy();

            for (line_idx, line) in lines.iter().enumerate() {
                if results.len() >= max_results {
                    break;
                }
                if regex.is_match(line) {
                    if context_lines == 0 {
                        results.push(format!("{relative_path}:{}:{line}", line_idx + 1));
                    } else {
                        results.push(format_match_with_context(
                            &relative_path,
                            &lines,
                            line_idx,
                            context_lines,
                        ));
                    }
                }
            }
        }
    }

    if results.is_empty() {
        let msg = zero_match_diagnostic(sandbox, path, pattern);
        return Ok(
            ToolResult::success("search_code", msg).with_metadata("match_count", "0".to_string())
        );
    }

    let output = results.join("\n");
    Ok(ToolResult::success("search_code", output)
        .with_metadata("match_count", results.len().to_string()))
}

/// Heuristic check for text files based on extension.
fn is_text_file(path: &std::path::Path) -> bool {
    let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    let text_extensions = [
        "rs", "ts", "js", "py", "go", "java", "c", "cpp", "h", "hpp", "md", "txt", "json", "yaml",
        "yml", "toml", "xml", "html", "css", "sql", "sh", "bat", "ps1",
    ];
    text_extensions.contains(&extension) || extension.is_empty()
}

/// `search_code` tool: search for patterns in code.
pub struct SearchCodeTool;

#[async_trait]
impl Tool for SearchCodeTool {
    fn name(&self) -> &str {
        "search_code"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "search_code".into(),
            description: "Search for patterns in code using regex. Useful for finding function definitions, usages, and patterns across files.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Search pattern (regex supported)"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (default: workspace root)"
                    },
                    "file_pattern": {
                        "type": "string",
                        "description": "Glob pattern for files to search (e.g., '*.rs', '*.ts')"
                    },
                    "include": {
                        "type": "string",
                        "description": "Glob pattern for files to search (alias for file_pattern)"
                    },
                    "max_results": {
                        "type": "integer",
                        "description": "Maximum number of results to return (default: 100)"
                    },
                    "context_lines": {
                        "type": "integer",
                        "description": "Number of surrounding lines to show (0-10, default: 0)"
                    }
                },
                "required": ["pattern"]
            }),
            cache_control: None,
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let pattern = args["pattern"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'pattern' argument".into()))?
            .to_string();
        let path = args["path"].as_str().map(String::from);
        let file_pattern = args["include"]
            .as_str()
            .or_else(|| args["file_pattern"].as_str())
            .map(String::from);
        let max_results = args["max_results"]
            .as_u64()
            .map_or(100, |n| usize::try_from(n).unwrap_or(100));
        let context_lines = args["context_lines"]
            .as_u64()
            .map_or(0, |n| usize::try_from(n).unwrap_or(0));
        let sandbox = ctx.sandbox.clone();
        super::spawn_blocking_tool(move || {
            search_code(
                &sandbox,
                &pattern,
                path.as_deref(),
                file_pattern.as_deref(),
                max_results,
                context_lines,
            )
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_sandbox() -> (Sandbox, TempDir) {
        let dir = TempDir::new().unwrap();
        let sandbox = Sandbox::new(dir.path()).unwrap();
        (sandbox, dir)
    }

    #[test]
    fn test_search_code_simple_pattern() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(
            dir.path().join("code.rs"),
            "fn main() { println!(\"hello\"); }",
        )
        .unwrap();

        let result = search_code(&sandbox, "fn main", None, None, 100, 0).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("fn main"));
        assert!(output.contains("code.rs"));
    }

    #[test]
    fn test_search_code_regex_pattern() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("code.rs"), "let x = 42;\nlet y = 123;").unwrap();

        let result = search_code(&sandbox, r"let \w+ = \d+", None, None, 100, 0).unwrap();
        assert!(result.ok);
        assert_eq!(result.metadata.get("match_count").unwrap(), "2");
    }

    #[test]
    fn test_search_code_no_matches() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("code.rs"), "fn main() {}").unwrap();

        let result = search_code(&sandbox, "nonexistent_pattern_xyz", None, None, 100, 0).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("No matches found"));
    }

    #[test]
    fn test_search_code_file_pattern() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("code.rs"), "let rust_var = 1;").unwrap();
        fs::write(dir.path().join("code.ts"), "let ts_var = 2;").unwrap();

        let result = search_code(&sandbox, "let", None, Some("*.rs"), 100, 0).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("rust_var"));
        assert!(!output.contains("ts_var"));
    }

    #[test]
    fn test_search_code_max_results() {
        let (sandbox, dir) = create_test_sandbox();

        let content = (0..20)
            .map(|i| format!("line{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(dir.path().join("many.txt"), content).unwrap();

        let result = search_code(&sandbox, "line", None, None, 5, 0).unwrap();
        assert!(result.ok);
        assert_eq!(result.metadata.get("match_count").unwrap(), "5");
    }

    #[test]
    fn test_search_code_in_subdirectory() {
        let (sandbox, dir) = create_test_sandbox();

        fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        fs::write(dir.path().join("src/nested/code.rs"), "fn nested_fn() {}").unwrap();

        let result = search_code(&sandbox, "nested_fn", Some("src"), None, 100, 0).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("nested_fn"));
    }

    #[test]
    fn test_search_code_invalid_regex() {
        let (sandbox, _dir) = create_test_sandbox();

        let result = search_code(&sandbox, "[invalid(regex", None, None, 100, 0);
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[test]
    fn test_search_code_context_lines() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "alpha\nbeta\ngamma\ndelta\nepsilon\n";
        fs::write(dir.path().join("ctx.txt"), content).unwrap();

        let result = search_code(&sandbox, "gamma", None, None, 100, 1).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        // Context should include surrounding lines
        assert!(output.contains("beta"));
        assert!(output.contains("gamma"));
        assert!(output.contains("delta"));
        // Match line should be marked with >
        assert!(output.contains(">"));
    }

    #[test]
    fn test_search_code_skip_dirs() {
        let (sandbox, dir) = create_test_sandbox();

        fs::create_dir_all(dir.path().join("node_modules")).unwrap();
        fs::write(dir.path().join("node_modules/dep.js"), "let hidden = true;").unwrap();
        fs::create_dir_all(dir.path().join("target")).unwrap();
        fs::write(dir.path().join("target/out.rs"), "let hidden = true;").unwrap();
        fs::write(dir.path().join("visible.rs"), "let visible = true;").unwrap();

        let result = search_code(&sandbox, "let", None, None, 100, 0).unwrap();
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("visible"));
        assert!(!output.contains("hidden"));
    }

    #[test]
    fn test_search_code_complex_regex_lookahead_character_class() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(
            dir.path().join("complex.rs"),
            "fn foo_bar() {}\nfn baz123() {}\nfn _private() {}\n",
        )
        .unwrap();

        // Character class + quantifier
        let result = search_code(&sandbox, r"fn [a-z_]+\d*\(\)", None, None, 100, 0).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("baz123"));
    }

    #[test]
    fn test_search_code_alternation_regex() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(
            dir.path().join("alt.rs"),
            "let alpha = 1;\nlet beta = 2;\nlet gamma = 3;\n",
        )
        .unwrap();

        let result =
            search_code(&sandbox, r"alpha|gamma", None, None, 100, 0).unwrap();
        assert_eq!(result.metadata.get("match_count").unwrap(), "2");
    }

    #[test]
    fn test_search_code_binary_file_skipped() {
        let (sandbox, dir) = create_test_sandbox();

        // Write a file with a binary extension
        fs::write(dir.path().join("image.png"), b"fake png data with let x = 1").unwrap();
        fs::write(dir.path().join("code.rs"), "let x = 1;").unwrap();

        let result = search_code(&sandbox, "let x", None, None, 100, 0).unwrap();
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("code.rs"));
        assert!(!output.contains("image.png"));
    }

    #[test]
    fn test_search_code_nonexistent_path_diagnostic() {
        let (sandbox, _dir) = create_test_sandbox();

        let result = search_code(&sandbox, "anything", Some("no_such_dir"), None, 100, 0);
        assert!(result.is_err());
    }

    #[test]
    fn test_search_code_zero_match_regex_hint() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("hint.rs"), "normal code").unwrap();

        let result =
            search_code(&sandbox, r"foo\(bar\[baz\]", None, None, 100, 0).unwrap();
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("No matches found"));
        assert!(output.contains("regex characters"));
    }

    #[test]
    fn test_search_code_regex_size_limit() {
        let (sandbox, _dir) = create_test_sandbox();

        let huge_pattern = "a".repeat(SEARCH_REGEX_SIZE_LIMIT + 1);
        let result = search_code(&sandbox, &huge_pattern, None, None, 100, 0);
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[test]
    fn test_search_code_context_lines_clamped_to_10() {
        let (sandbox, dir) = create_test_sandbox();

        let content = (0..30)
            .map(|i| format!("line_{i}"))
            .collect::<Vec<_>>()
            .join("\n");
        fs::write(dir.path().join("ctx.txt"), &content).unwrap();

        // Passing 100 context lines should be clamped to 10
        let result = search_code(&sandbox, "line_15", None, None, 100, 100).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        // Should include context but not the entire file
        assert!(output.contains("line_15"));
    }

    #[test]
    fn test_is_text_file_known_extensions() {
        use std::path::Path;
        assert!(is_text_file(Path::new("main.rs")));
        assert!(is_text_file(Path::new("script.py")));
        assert!(is_text_file(Path::new("config.json")));
        assert!(is_text_file(Path::new("readme.md")));
        assert!(!is_text_file(Path::new("photo.jpg")));
        assert!(!is_text_file(Path::new("binary.exe")));
    }
}
