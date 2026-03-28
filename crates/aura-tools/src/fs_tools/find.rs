use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::ToolResult;
use aura_reasoner::ToolDefinition;
use std::fs;
use tracing::{debug, instrument, warn};

/// Skip directories that shouldn't be included in find results.
fn should_skip_dir(name: &str) -> bool {
    matches!(
        name,
        "node_modules" | "target" | ".git" | "__pycache__" | ".venv"
    ) || name.starts_with('.')
}

/// Find files matching a glob pattern within the sandbox.
#[instrument(skip(sandbox), fields(pattern = %pattern))]
pub fn fs_find(
    sandbox: &Sandbox,
    pattern: &str,
    path: Option<&str>,
    max_results: usize,
) -> Result<ToolResult, ToolError> {
    use glob::Pattern;

    let search_root = match path {
        Some(p) => sandbox.resolve_existing(p)?,
        None => sandbox.root().to_path_buf(),
    };

    debug!(?search_root, "Finding files");

    let glob_pattern = Pattern::new(pattern)
        .map_err(|e| ToolError::InvalidArguments(format!("Invalid glob pattern: {e}")))?;

    let max_results = max_results.min(200);
    let mut results = Vec::new();

    fn walk(
        dir: &std::path::Path,
        root: &std::path::Path,
        pattern: &Pattern,
        results: &mut Vec<String>,
        max: usize,
    ) {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(e) => {
                warn!(path = %dir.display(), error = %e, "Failed to read directory during find");
                return;
            }
        };
        for entry in entries.flatten() {
            if results.len() >= max {
                return;
            }
            let path = entry.path();
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();

            if path.is_dir() {
                if should_skip_dir(&name) {
                    continue;
                }
                walk(&path, root, pattern, results, max);
            } else {
                let relative = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
                if pattern.matches(&relative) || pattern.matches(&name) {
                    results.push(relative.to_string());
                }
            }
        }
    }

    walk(
        &search_root,
        &search_root,
        &glob_pattern,
        &mut results,
        max_results,
    );

    let output = if results.is_empty() {
        "No files found".to_string()
    } else {
        results.join("\n")
    };

    Ok(ToolResult::success("find_files", output)
        .with_metadata("match_count", results.len().to_string()))
}

/// `fs_find` tool: find files by glob pattern.
pub struct FsFindTool;

#[async_trait]
impl Tool for FsFindTool {
    fn name(&self) -> &str {
        "find_files"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "find_files".into(),
            description: "Find files matching a glob pattern. Skips node_modules, target, .git, __pycache__, and dot-prefixed directories. Results capped at 200.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match (e.g., '*.rs', '**/*.ts', 'src/*.json')"
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (default: workspace root)"
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
        let sandbox = ctx.sandbox.clone();
        super::spawn_blocking_tool(move || fs_find(&sandbox, &pattern, path.as_deref(), 200)).await
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
    fn test_fs_find_simple() {
        let (sandbox, dir) = create_test_sandbox();
        fs::write(dir.path().join("hello.rs"), "fn main() {}").unwrap();
        fs::write(dir.path().join("hello.txt"), "hello").unwrap();

        let result = fs_find(&sandbox, "*.rs", None, 200).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("hello.rs"));
        assert!(!output.contains("hello.txt"));
    }

    #[test]
    fn test_fs_find_no_matches() {
        let (sandbox, _dir) = create_test_sandbox();
        let result = fs_find(&sandbox, "*.xyz", None, 200).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("No files found"));
    }

    #[test]
    fn test_fs_find_nested() {
        let (sandbox, dir) = create_test_sandbox();
        fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        fs::write(dir.path().join("src/nested/deep.rs"), "").unwrap();

        let result = fs_find(&sandbox, "*.rs", Some("src"), 200).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("deep.rs"));
    }

    #[test]
    fn test_fs_find_skips_hidden_dirs() {
        let (sandbox, dir) = create_test_sandbox();
        fs::create_dir_all(dir.path().join(".hidden")).unwrap();
        fs::write(dir.path().join(".hidden/secret.rs"), "").unwrap();
        fs::write(dir.path().join("visible.rs"), "").unwrap();

        let result = fs_find(&sandbox, "*.rs", None, 200).unwrap();
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("visible.rs"));
        assert!(!output.contains("secret.rs"));
    }

    #[test]
    fn test_fs_find_invalid_pattern() {
        let (sandbox, _dir) = create_test_sandbox();
        let result = fs_find(&sandbox, "[invalid", None, 200);
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[test]
    fn test_fs_find_workspace_root_alias() {
        let (sandbox, dir) = create_test_sandbox();
        fs::write(dir.path().join("hello.rs"), "fn main() {}").unwrap();

        let result = fs_find(&sandbox, "*.rs", Some("/"), 200).unwrap();
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("hello.rs"));
    }
}
