use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::ToolDefinition;
use aura_core::ToolResult;
use std::fs;
use tracing::{debug, instrument};

/// Read file contents, optionally restricted to a line range.
///
/// When `start_line` / `end_line` are provided (1-indexed, inclusive), only
/// the requested slice of lines is returned, prefixed with line numbers.
/// This avoids dumping entire large files into the context window.
#[instrument(skip(sandbox), fields(path = %path, max_bytes))]
pub fn fs_read(
    sandbox: &Sandbox,
    path: &str,
    max_bytes: usize,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<ToolResult, ToolError> {
    let resolved = sandbox.resolve_existing(path)?;
    debug!(?resolved, "Reading file");

    if !resolved.is_file() {
        return Err(ToolError::InvalidArguments(format!("{path} is not a file")));
    }

    let metadata = fs::metadata(&resolved)?;
    let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);

    if size > max_bytes {
        return Err(ToolError::SizeLimitExceeded {
            actual: size,
            limit: max_bytes,
        });
    }

    let contents = fs::read(&resolved)?;

    if start_line.is_some() || end_line.is_some() {
        let text = String::from_utf8_lossy(&contents);
        let lines: Vec<&str> = text.lines().collect();
        let total = lines.len();
        let start = start_line.unwrap_or(1).max(1);
        let end = end_line.unwrap_or(total).min(total);

        if start > total {
            return Ok(ToolResult::success(
                "read_file",
                format!("(file has {total} lines, requested start_line={start})"),
            )
            .with_metadata("total_lines", total.to_string()));
        }

        let sliced: Vec<String> = lines[(start - 1)..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{:>6}|{}", start + i, line))
            .collect();
        let output = sliced.join("\n");
        Ok(ToolResult::success("read_file", output)
            .with_metadata("size", size.to_string())
            .with_metadata("total_lines", total.to_string())
            .with_metadata("start_line", start.to_string())
            .with_metadata("end_line", end.to_string()))
    } else {
        Ok(ToolResult::success("read_file", contents).with_metadata("size", size.to_string()))
    }
}

/// `fs_read` tool: read file contents.
pub struct FsReadTool;

#[async_trait]
impl Tool for FsReadTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "read_file".into(),
            description: "Read the contents of a file. Supports optional line range to avoid reading entire large files. When start_line/end_line are provided, output is prefixed with line numbers.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read (relative to workspace root)"
                    },
                    "max_bytes": {
                        "type": "integer",
                        "description": "Maximum bytes to read (default: 1MB). Useful for large files."
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "First line to return (1-indexed, inclusive). Omit to start from the beginning."
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "Last line to return (1-indexed, inclusive). Omit to read to the end."
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
        let max_bytes = args["max_bytes"]
            .as_u64()
            .map_or(ctx.config.max_read_bytes, |n| {
                usize::try_from(n).unwrap_or(usize::MAX)
            });
        let max_bytes = max_bytes.min(ctx.config.max_read_bytes);
        let start_line = args["start_line"]
            .as_u64()
            .map(|n| usize::try_from(n).unwrap_or(1));
        let end_line = args["end_line"]
            .as_u64()
            .map(|n| usize::try_from(n).unwrap_or(usize::MAX));
        let sandbox = ctx.sandbox.clone();
        super::spawn_blocking_tool(move || {
            fs_read(&sandbox, &path, max_bytes, start_line, end_line)
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
    fn test_fs_read() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "Hello, Aura!";
        fs::write(dir.path().join("test.txt"), content).unwrap();

        let result = fs_read(&sandbox, "test.txt", 1024, None, None).unwrap();
        assert!(result.ok);
        assert_eq!(&result.stdout[..], content.as_bytes());
    }

    #[test]
    fn test_fs_read_size_limit() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "Hello, Aura!";
        fs::write(dir.path().join("test.txt"), content).unwrap();

        let result = fs_read(&sandbox, "test.txt", 5, None, None);
        assert!(matches!(result, Err(ToolError::SizeLimitExceeded { .. })));
    }

    #[test]
    fn test_fs_read_binary_content() {
        let (sandbox, dir) = create_test_sandbox();

        let content = vec![0u8, 1, 2, 255, 254, 253];
        fs::write(dir.path().join("binary.bin"), &content).unwrap();

        let result = fs_read(&sandbox, "binary.bin", 1024, None, None).unwrap();
        assert!(result.ok);
        assert_eq!(&result.stdout[..], content.as_slice());
    }

    #[test]
    fn test_fs_read_empty_file() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("empty.txt"), "").unwrap();

        let result = fs_read(&sandbox, "empty.txt", 1024, None, None).unwrap();
        assert!(result.ok);
        assert!(result.stdout.is_empty());
    }

    #[test]
    fn test_fs_read_not_a_file() {
        let (sandbox, dir) = create_test_sandbox();

        fs::create_dir(dir.path().join("dir")).unwrap();

        let result = fs_read(&sandbox, "dir", 1024, None, None);
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[test]
    fn test_fs_read_line_range() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "line1\nline2\nline3\nline4\nline5";
        fs::write(dir.path().join("lines.txt"), content).unwrap();

        let result = fs_read(&sandbox, "lines.txt", 1024, Some(2), Some(4)).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("line2"));
        assert!(output.contains("line3"));
        assert!(output.contains("line4"));
        assert!(!output.contains("line1\n"));
        assert!(!output.contains("line5"));
        assert_eq!(result.metadata.get("start_line").unwrap(), "2");
        assert_eq!(result.metadata.get("end_line").unwrap(), "4");
    }

    #[test]
    fn test_fs_read_start_line_only() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "line1\nline2\nline3";
        fs::write(dir.path().join("lines.txt"), content).unwrap();

        let result = fs_read(&sandbox, "lines.txt", 1024, Some(2), None).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("line2"));
        assert!(output.contains("line3"));
    }

    #[test]
    fn test_fs_read_start_line_past_eof() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "line1\nline2";
        fs::write(dir.path().join("lines.txt"), content).unwrap();

        let result = fs_read(&sandbox, "lines.txt", 1024, Some(100), None).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("2 lines"));
    }
}
