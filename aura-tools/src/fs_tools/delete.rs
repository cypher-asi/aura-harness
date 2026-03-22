use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::ToolResult;
use aura_reasoner::ToolDefinition;
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

    fs::remove_file(&resolved)?;
    Ok(ToolResult::success("fs_delete", format!("Deleted {path}")))
}

/// `fs_delete` tool: delete a file.
pub struct FsDeleteTool;

#[async_trait]
impl Tool for FsDeleteTool {
    fn name(&self) -> &str {
        "fs_delete"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "fs_delete".into(),
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
}
