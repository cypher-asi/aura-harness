use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::ToolDefinition;
use aura_core::ToolResult;
use std::fs;
use tracing::{debug, instrument};

/// Check whether `content` has unbalanced `{}`/`()` pairs, which may
/// indicate truncated output from an LLM.
fn looks_truncated(content: &str) -> bool {
    let mut brace_depth: i64 = 0;
    let mut paren_depth: i64 = 0;
    for ch in content.chars() {
        match ch {
            '{' => brace_depth += 1,
            '}' => brace_depth -= 1,
            '(' => paren_depth += 1,
            ')' => paren_depth -= 1,
            _ => {}
        }
    }
    brace_depth != 0 || paren_depth != 0
}

/// Write content to a file.
///
/// Parent directories are always created automatically (matching aura-app
/// behaviour). The `create_dirs` parameter is kept for backward compatibility
/// but effectively defaults to `true`.
///
/// Safety heuristics:
/// - Rejects writes that would replace an existing file with content < 10%
///   of the original size.
/// - Warns (via metadata) when the content has unbalanced braces/parens.
/// - Performs post-write verification of byte count.
#[instrument(skip(sandbox, content), fields(path = %path))]
pub fn fs_write(
    sandbox: &Sandbox,
    path: &str,
    content: &str,
    create_dirs: bool,
) -> Result<ToolResult, ToolError> {
    let _ = create_dirs; // kept for API compat; always creates dirs
    let resolved = sandbox.resolve_new(path)?;
    debug!(?resolved, "Writing file");

    let file_existed = resolved.exists();
    let existing_size = if file_existed {
        usize::try_from(fs::metadata(&resolved).map(|m| m.len()).unwrap_or(0)).unwrap_or(usize::MAX)
    } else {
        0
    };

    // Truncation heuristic: reject if new content < 10% of existing file
    if file_existed && existing_size > 0 && content.len() < existing_size / 10 {
        return Err(ToolError::InvalidArguments(
            "New content is less than 10% of existing file size. \
             This likely indicates truncated output."
                .to_string(),
        ));
    }

    // Always create parent directories
    if let Some(parent) = resolved.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| {
                ToolError::Io(std::io::Error::new(
                    e.kind(),
                    format!("create_dir_all({}): {e}", parent.display()),
                ))
            })?;
        }
    }

    fs::write(&resolved, content).map_err(|e| {
        ToolError::Io(std::io::Error::new(
            e.kind(),
            format!("write({}): {e}", resolved.display()),
        ))
    })?;

    // Post-write verification
    let on_disk_size = usize::try_from(fs::metadata(&resolved).map(|m| m.len()).unwrap_or(0))
        .unwrap_or(usize::MAX);
    if on_disk_size != content.len() {
        return Err(ToolError::InvalidArguments(format!(
            "Post-write verification failed: wrote {} bytes but file is {} bytes on disk",
            content.len(),
            on_disk_size
        )));
    }

    let bytes_written = content.len();
    let truncated_warning = looks_truncated(content);

    let mut result = ToolResult::success(
        "write_file",
        format!("Wrote {bytes_written} bytes to {path}"),
    )
    .with_metadata("bytes_written", bytes_written.to_string())
    .with_metadata("file_existed", file_existed.to_string());

    if truncated_warning {
        result = result.with_metadata(
            "warning",
            "Content has unbalanced braces/parentheses – may be truncated".to_string(),
        );
    }

    Ok(result)
}

/// `fs_write` tool: write content to a file.
pub struct FsWriteTool;

#[async_trait]
impl Tool for FsWriteTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "write_file".into(),
            description:
                "Write content to a file. Creates the file if it doesn't exist, overwrites if it does."
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to write (relative to workspace root)"
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file"
                    },
                    "create_dirs": {
                        "type": "boolean",
                        "description": "Create parent directories if they don't exist (default: true)"
                    }
                },
                "required": ["path", "content"]
            }),
            cache_control: None,
            // Stream the `content` string live as the model writes it so the
            // UI's file preview fills in character-by-character instead of
            // waiting for the full tool-use block to close.
            eager_input_streaming: Some(true),
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'path' argument".into()))?
            .to_string();
        let content = args["content"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'content' argument".into()))?
            .to_string();
        let create_dirs = args["create_dirs"].as_bool().unwrap_or(true);
        let sandbox = ctx.sandbox.clone();
        super::spawn_blocking_tool(move || fs_write(&sandbox, &path, &content, create_dirs)).await
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
    fn test_fs_write_new_file() {
        let (sandbox, dir) = create_test_sandbox();

        let result = fs_write(&sandbox, "new.txt", "Hello, world!", false).unwrap();
        assert!(result.ok);

        let content = fs::read_to_string(dir.path().join("new.txt")).unwrap();
        assert_eq!(content, "Hello, world!");
    }

    #[test]
    fn test_fs_write_overwrite_file() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("existing.txt"), "old content").unwrap();

        let result = fs_write(&sandbox, "existing.txt", "new content", false).unwrap();
        assert!(result.ok);

        let content = fs::read_to_string(dir.path().join("existing.txt")).unwrap();
        assert_eq!(content, "new content");
    }

    #[test]
    fn test_fs_write_create_dirs() {
        let (sandbox, dir) = create_test_sandbox();

        let result = fs_write(&sandbox, "nested/deep/file.txt", "content", true).unwrap();
        assert!(result.ok);

        assert!(dir.path().join("nested/deep/file.txt").exists());
        let content = fs::read_to_string(dir.path().join("nested/deep/file.txt")).unwrap();
        assert_eq!(content, "content");
    }

    #[test]
    fn test_fs_write_creates_parent_dirs_by_default() {
        let (sandbox, dir) = create_test_sandbox();

        // Even with create_dirs=false, parent dirs are now always created
        let result = fs_write(&sandbox, "auto/created/file.txt", "content", false).unwrap();
        assert!(result.ok);
        assert!(dir.path().join("auto/created/file.txt").exists());
    }

    #[test]
    fn test_fs_write_truncation_heuristic_rejects_small() {
        let (sandbox, dir) = create_test_sandbox();

        // Write a large file first
        let large = "x".repeat(1000);
        fs::write(dir.path().join("big.txt"), &large).unwrap();

        // Attempt to overwrite with tiny content (< 10%)
        let result = fs_write(&sandbox, "big.txt", "tiny", false);
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
        if let Err(ToolError::InvalidArguments(msg)) = result {
            assert!(msg.contains("10%"));
        }
    }

    #[test]
    fn test_fs_write_post_write_verification() {
        let (sandbox, _dir) = create_test_sandbox();

        let content = "verified content";
        let result = fs_write(&sandbox, "verify.txt", content, false).unwrap();
        assert!(result.ok);
        assert_eq!(
            result.metadata.get("bytes_written").unwrap(),
            &content.len().to_string()
        );
    }

    #[test]
    fn test_fs_write_bytes_written_metadata() {
        let (sandbox, _dir) = create_test_sandbox();

        let content = "12345";
        let result = fs_write(&sandbox, "counted.txt", content, false).unwrap();

        assert_eq!(result.metadata.get("bytes_written").unwrap(), "5");
    }

    #[test]
    fn test_fs_write_unicode_content() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "Hello 世界! 🌍 Привет";
        let result = fs_write(&sandbox, "unicode.txt", content, false).unwrap();
        assert!(result.ok);

        let read_back = fs::read_to_string(dir.path().join("unicode.txt")).unwrap();
        assert_eq!(read_back, content);
    }

    #[test]
    fn test_fs_write_large_file_over_1mb() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "x".repeat(1_100_000);
        let result = fs_write(&sandbox, "large.bin", &content, false).unwrap();
        assert!(result.ok);

        let on_disk = fs::read_to_string(dir.path().join("large.bin")).unwrap();
        assert_eq!(on_disk.len(), 1_100_000);
    }

    #[test]
    fn test_fs_write_special_chars_in_filename() {
        let (sandbox, dir) = create_test_sandbox();

        let result = fs_write(&sandbox, "file with spaces.txt", "data", false).unwrap();
        assert!(result.ok);
        assert!(dir.path().join("file with spaces.txt").exists());
    }

    #[test]
    fn test_fs_write_empty_content() {
        let (sandbox, dir) = create_test_sandbox();

        let result = fs_write(&sandbox, "empty.txt", "", false).unwrap();
        assert!(result.ok);

        let content = fs::read_to_string(dir.path().join("empty.txt")).unwrap();
        assert!(content.is_empty());
    }

    #[test]
    fn test_fs_write_overwrite_with_exactly_10_percent() {
        let (sandbox, dir) = create_test_sandbox();

        let large = "x".repeat(1000);
        fs::write(dir.path().join("boundary.txt"), &large).unwrap();

        // 100 bytes = exactly 10% of 1000 — should be accepted
        let small = "y".repeat(100);
        let result = fs_write(&sandbox, "boundary.txt", &small, false).unwrap();
        assert!(result.ok);
    }

    #[test]
    fn test_fs_write_overwrite_with_just_under_10_percent() {
        let (sandbox, dir) = create_test_sandbox();

        let large = "x".repeat(1000);
        fs::write(dir.path().join("boundary2.txt"), &large).unwrap();

        // 99 bytes = just under 10% of 1000 — should be rejected
        let small = "y".repeat(99);
        let result = fs_write(&sandbox, "boundary2.txt", &small, false);
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[test]
    fn test_fs_write_truncation_warning_unbalanced_braces() {
        let (sandbox, _dir) = create_test_sandbox();

        let content = "fn main() { if true {";
        let result = fs_write(&sandbox, "warn.txt", content, false).unwrap();
        assert!(result.ok);
        assert!(result.metadata.contains_key("warning"));
    }

    #[test]
    fn test_fs_write_no_truncation_warning_balanced() {
        let (sandbox, _dir) = create_test_sandbox();

        let content = "fn main() { println!(); }";
        let result = fs_write(&sandbox, "ok.txt", content, false).unwrap();
        assert!(result.ok);
        assert!(!result.metadata.contains_key("warning"));
    }

    #[test]
    fn test_fs_write_file_existed_metadata() {
        let (sandbox, dir) = create_test_sandbox();

        let result = fs_write(&sandbox, "new_file.txt", "initial", false).unwrap();
        assert_eq!(result.metadata.get("file_existed").unwrap(), "false");

        fs::write(dir.path().join("old_file.txt"), "original").unwrap();
        let result = fs_write(&sandbox, "old_file.txt", "replaced!", false).unwrap();
        assert_eq!(result.metadata.get("file_existed").unwrap(), "true");
    }

    #[test]
    fn test_fs_write_deeply_nested_path() {
        let (sandbox, dir) = create_test_sandbox();

        let result = fs_write(&sandbox, "a/b/c/d/e/f/deep.txt", "deep", true).unwrap();
        assert!(result.ok);
        assert!(dir.path().join("a/b/c/d/e/f/deep.txt").exists());
    }
}
