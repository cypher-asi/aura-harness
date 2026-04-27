use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::ToolDefinition;
use aura_core::ToolResult;
use std::fs;
use tracing::{debug, instrument};

/// Delete a file within the sandbox.
#[instrument(skip(sandbox), fields(path = %path))]
pub fn fs_delete(sandbox: &Sandbox, path: &str) -> Result<ToolResult, ToolError> {
    let resolved = sandbox.resolve_existing(path)?;
    debug!(?resolved, "Deleting file");

    if !resolved.is_file() {
        return Err(ToolError::InvalidArguments(format!("{path} is not a file")));
    }

    // Read pre-content for line counting before the file is gone.
    // Best-effort: a non-UTF-8 binary file silently maps to 0 lines
    // removed (the dashboard treats it as "unknown"), which is the
    // honest signal — we'd rather under-report than fabricate a
    // byte-count proxy that doesn't represent lines.
    let lines_removed = fs::read_to_string(&resolved)
        .ok()
        .map_or(0, |content| super::diff::count_lines(&content));

    fs::remove_file(&resolved).map_err(|e| {
        ToolError::Io(std::io::Error::new(
            e.kind(),
            format!("remove_file({}): {e}", resolved.display()),
        ))
    })?;
    Ok(
        ToolResult::success("delete_file", format!("Deleted {path}"))
            .with_line_diff(0, lines_removed),
    )
}

/// `fs_delete` tool: delete a file.
pub struct FsDeleteTool;

#[async_trait]
impl Tool for FsDeleteTool {
    fn name(&self) -> &str {
        "delete_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "delete_file".into(),
            description:
                "Delete a file within the workspace. Only files can be deleted, not directories."
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to delete (relative to workspace root)"
                    }
                },
                "required": ["path"]
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
        let path = args["path"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'path' argument".into()))?
            .to_string();
        let sandbox = ctx.sandbox.clone();
        super::spawn_blocking_tool(move || fs_delete(&sandbox, &path)).await
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
    fn test_fs_delete_file() {
        let (sandbox, dir) = create_test_sandbox();
        fs::write(dir.path().join("doomed.txt"), "bye").unwrap();

        let result = fs_delete(&sandbox, "doomed.txt").unwrap();
        assert!(result.ok);
        assert!(!dir.path().join("doomed.txt").exists());
    }

    #[test]
    fn test_fs_delete_nonexistent() {
        let (sandbox, _dir) = create_test_sandbox();
        let result = fs_delete(&sandbox, "ghost.txt");
        assert!(matches!(result, Err(ToolError::PathNotFound(_))));
    }

    #[test]
    fn test_fs_delete_directory_rejected() {
        let (sandbox, dir) = create_test_sandbox();
        fs::create_dir(dir.path().join("subdir")).unwrap();

        let result = fs_delete(&sandbox, "subdir");
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    // ====================================================================
    // line_diff coverage — verifies fs_delete reports the deleted file's
    // line count as lines_removed (lines_added always 0).
    // ====================================================================

    #[test]
    fn fs_delete_reports_removed_line_count() {
        let (sandbox, dir) = create_test_sandbox();
        fs::write(dir.path().join("five.txt"), "1\n2\n3\n4\n5\n").unwrap();
        let result = fs_delete(&sandbox, "five.txt").unwrap();
        let line_diff = result
            .line_diff
            .expect("delete should report a line diff with lines_removed");
        assert_eq!(line_diff.lines_added, 0);
        assert_eq!(line_diff.lines_removed, 5);
    }

    #[test]
    fn fs_delete_empty_file_reports_zero_lines_removed() {
        let (sandbox, dir) = create_test_sandbox();
        fs::write(dir.path().join("empty.txt"), "").unwrap();
        let result = fs_delete(&sandbox, "empty.txt").unwrap();
        let line_diff = result.line_diff.expect("delete always reports a diff");
        assert_eq!(line_diff.lines_added, 0);
        assert_eq!(line_diff.lines_removed, 0);
    }

    #[test]
    fn fs_delete_non_utf8_file_reports_zero_rather_than_panic() {
        let (sandbox, dir) = create_test_sandbox();
        // Random binary bytes that are guaranteed invalid UTF-8.
        fs::write(dir.path().join("bin"), [0xff_u8, 0xfe, 0xfd, 0x00, 0xff]).unwrap();
        let result = fs_delete(&sandbox, "bin").unwrap();
        let line_diff = result
            .line_diff
            .expect("delete still reports a diff for binary files");
        assert_eq!(line_diff.lines_removed, 0);
    }
}
