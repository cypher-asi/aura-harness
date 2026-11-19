use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::ToolDefinition;
use aura_core_types::ToolResult;
use std::fmt::Write;
use std::time::SystemTime;
use tracing::{debug, instrument, warn};

/// Most paths one `find_files` call returns.
const MAX_RESULTS: usize = 200;

/// Matches collected before sorting stops, so a pattern like `*` over a
/// huge tree stays bounded.
const MAX_SCANNED_MATCHES: usize = 20_000;

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

    let max_results = max_results.min(MAX_RESULTS);
    let mut matches: Vec<(SystemTime, String)> = Vec::new();

    for entry in super::walk::walk(&search_root, should_skip_dir) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                warn!(error = %e, "skipping unreadable entry during find");
                continue;
            }
        };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        let relative = path.strip_prefix(&search_root).unwrap_or(path);
        let relative = relative.to_string_lossy();
        let name = entry.file_name().to_string_lossy();
        if !(glob_pattern.matches(&relative) || glob_pattern.matches(&name)) {
            continue;
        }
        let modified = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        matches.push((modified, relative.into_owned()));
        if matches.len() >= MAX_SCANNED_MATCHES {
            break;
        }
    }

    matches.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let total = matches.len();
    let truncated = total > max_results;
    let shown: Vec<String> = matches
        .into_iter()
        .take(max_results)
        .map(|(_, path)| path)
        .collect();

    let mut output = if shown.is_empty() {
        "No files found".to_string()
    } else {
        shown.join("\n")
    };
    if truncated {
        let more = if total >= MAX_SCANNED_MATCHES {
            "more than ".to_string()
        } else {
            String::new()
        };
        let _ = write!(
            output,
            "\n... [truncated: showing the {max_results} most recently modified of {more}{total} matches; narrow the pattern or path.]"
        );
    }

    let mut result = ToolResult::success("find_files", output)
        .with_metadata("match_count", shown.len().to_string());
    if truncated {
        result = result.with_metadata("truncated", "true");
    }
    Ok(result)
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
            description: "Find files matching a glob pattern, most recently modified first. Skips files the project's .gitignore excludes, plus node_modules, target, __pycache__ and dot-prefixed directories. Returns at most 200 paths and says when more matched.".into(),
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
            eager_input_streaming: None,
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
        super::spawn_blocking_tool(move || {
            fs_find(&sandbox, &pattern, path.as_deref(), MAX_RESULTS)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
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

    #[test]
    fn test_fs_find_skips_gitignored_files() {
        let (sandbox, dir) = create_test_sandbox();
        fs::write(dir.path().join(".gitignore"), "gen/\n").unwrap();
        fs::create_dir_all(dir.path().join("gen")).unwrap();
        fs::write(dir.path().join("gen/out.rs"), "").unwrap();
        fs::write(dir.path().join("lib.rs"), "").unwrap();

        let result = fs_find(&sandbox, "*.rs", None, 200).unwrap();
        let output = String::from_utf8_lossy(&result.stdout);
        assert_eq!(output, "lib.rs");
    }

    #[test]
    fn test_fs_find_says_when_results_are_capped() {
        let (sandbox, dir) = create_test_sandbox();
        for i in 0..5 {
            fs::write(dir.path().join(format!("f{i}.rs")), "").unwrap();
        }

        let result = fs_find(&sandbox, "*.rs", None, 3).unwrap();
        let output = String::from_utf8_lossy(&result.stdout);
        assert_eq!(output.lines().filter(|l| l.ends_with(".rs")).count(), 3);
        assert!(output.contains("[truncated"), "{output}");
        assert!(output.contains("of 5 matches"), "{output}");
        assert_eq!(
            result.metadata.get("truncated").map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn test_fs_find_lists_newest_first() {
        let (sandbox, dir) = create_test_sandbox();
        let old = dir.path().join("old.rs");
        let new = dir.path().join("new.rs");
        fs::write(&old, "").unwrap();
        fs::write(&new, "").unwrap();
        let past = SystemTime::now() - std::time::Duration::from_secs(3600);
        fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(past)
            .unwrap();

        let result = fs_find(&sandbox, "*.rs", None, 200).unwrap();
        let output = String::from_utf8_lossy(&result.stdout);
        assert_eq!(output, "new.rs\nold.rs");
    }
}
