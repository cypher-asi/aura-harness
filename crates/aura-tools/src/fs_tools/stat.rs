use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::ToolDefinition;
use aura_core::ToolResult;
use std::collections::HashMap;
use std::fs;
#[cfg(windows)]
use std::os::windows::fs::MetadataExt;
use tracing::{debug, instrument};

/// Get file metadata.
#[instrument(skip(sandbox), fields(path = %path))]
pub fn fs_stat(sandbox: &Sandbox, path: &str) -> Result<ToolResult, ToolError> {
    let resolved = sandbox.resolve_existing(path)?;
    debug!(?resolved, "Getting file stats");

    let metadata = fs::metadata(&resolved)?;

    let mut result_metadata = HashMap::new();
    result_metadata.insert("size".to_string(), metadata.len().to_string());
    result_metadata.insert("is_file".to_string(), metadata.is_file().to_string());
    result_metadata.insert("is_dir".to_string(), metadata.is_dir().to_string());
    result_metadata.insert(
        "readonly".to_string(),
        metadata.permissions().readonly().to_string(),
    );

    #[cfg(windows)]
    result_metadata.insert(
        "file_attributes".to_string(),
        metadata.file_attributes().to_string(),
    );

    // Format output as key=value pairs
    let output: Vec<String> = result_metadata
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();

    let mut tool_result = ToolResult::success("stat_file", output.join("\n"));
    tool_result.metadata = result_metadata;
    Ok(tool_result)
}

/// `fs_stat` tool: get file/directory metadata.
pub struct FsStatTool;

#[async_trait]
impl Tool for FsStatTool {
    fn name(&self) -> &str {
        "stat_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "stat_file".into(),
            description: "Get file or directory metadata including size, type, and permissions."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file or directory (relative to workspace root)"
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
        super::spawn_blocking_tool(move || fs_stat(&sandbox, &path)).await
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
    fn test_fs_stat() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "Hello!";
        fs::write(dir.path().join("test.txt"), content).unwrap();

        let result = fs_stat(&sandbox, "test.txt").unwrap();
        assert!(result.ok);
        assert_eq!(result.metadata.get("size").unwrap(), "6");
        assert_eq!(result.metadata.get("is_file").unwrap(), "true");
        assert_eq!(result.metadata.get("is_dir").unwrap(), "false");
    }

    #[test]
    fn test_fs_stat_directory() {
        let (sandbox, dir) = create_test_sandbox();

        fs::create_dir(dir.path().join("subdir")).unwrap();

        let result = fs_stat(&sandbox, "subdir").unwrap();
        assert!(result.ok);
        assert_eq!(result.metadata.get("is_file").unwrap(), "false");
        assert_eq!(result.metadata.get("is_dir").unwrap(), "true");
    }

    #[test]
    fn test_fs_stat_nonexistent() {
        let (sandbox, _dir) = create_test_sandbox();

        let result = fs_stat(&sandbox, "nonexistent.txt");
        assert!(matches!(result, Err(ToolError::PathNotFound(_))));
    }
}
