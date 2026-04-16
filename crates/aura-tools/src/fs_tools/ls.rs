use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::ToolDefinition;
use aura_core::ToolResult;
use std::fs;
use tracing::{debug, instrument};

/// Directories that are filtered from `fs_ls` output to reduce noise.
const LS_NOISE_DIRS: &[&str] = &["node_modules", "target", ".git", "__pycache__"];

/// List directory contents.
///
/// Results are sorted with directories first, then alphabetical within each
/// group. Noise directories (`node_modules`, `target`, `.git`, `__pycache__`)
/// are omitted from output.
#[instrument(skip(sandbox), fields(path = %path))]
pub fn fs_ls(sandbox: &Sandbox, path: &str) -> Result<ToolResult, ToolError> {
    let resolved = sandbox.resolve_existing(path)?;
    debug!(?resolved, "Listing directory");

    if !resolved.is_dir() {
        return Err(ToolError::InvalidArguments(format!(
            "{path} is not a directory"
        )));
    }

    let mut dirs: Vec<(String, u64)> = Vec::new();
    let mut files: Vec<(String, u64, &str)> = Vec::new();

    let read_dir = fs::read_dir(&resolved).map_err(|e| {
        ToolError::Io(std::io::Error::new(
            e.kind(),
            format!("read_dir({}): {e}", resolved.display()),
        ))
    })?;
    for entry in read_dir {
        let entry = entry.map_err(|e| {
            ToolError::Io(std::io::Error::new(
                e.kind(),
                format!("read_dir entry in {}: {e}", resolved.display()),
            ))
        })?;
        let name = entry.file_name().to_string_lossy().to_string();
        let metadata = entry.metadata().map_err(|e| {
            ToolError::Io(std::io::Error::new(
                e.kind(),
                format!("metadata({}): {e}", entry.path().display()),
            ))
        })?;

        if metadata.is_dir() {
            if LS_NOISE_DIRS.contains(&name.as_str()) {
                continue;
            }
            dirs.push((name, metadata.len()));
        } else if metadata.is_file() {
            files.push((name, metadata.len(), "file"));
        } else {
            files.push((name, metadata.len(), "other"));
        }
    }

    dirs.sort_by(|a, b| a.0.cmp(&b.0));
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut entries: Vec<String> = Vec::with_capacity(dirs.len() + files.len());
    for (name, size) in &dirs {
        entries.push(format!("dir\t{size}\t{name}"));
    }
    for (name, size, kind) in &files {
        entries.push(format!("{kind}\t{size}\t{name}"));
    }

    let output = entries.join("\n");
    Ok(ToolResult::success("list_files", output))
}

/// `fs_ls` tool: list directory contents.
pub struct FsLsTool;

#[async_trait]
impl Tool for FsLsTool {
    fn name(&self) -> &str {
        "list_files"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "list_files".into(),
            description:
                "List directory contents. Returns files and directories with their types and sizes."
                    .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the directory to list (relative to workspace root)"
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
        let path = args["path"].as_str().unwrap_or(".").to_string();
        let sandbox = ctx.sandbox.clone();
        super::spawn_blocking_tool(move || fs_ls(&sandbox, &path)).await
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
    fn test_fs_ls() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("file1.txt"), "hello").unwrap();
        fs::write(dir.path().join("file2.txt"), "world").unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();

        let result = fs_ls(&sandbox, ".").unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("file1.txt"));
        assert!(output.contains("file2.txt"));
        assert!(output.contains("subdir"));
    }

    #[test]
    fn test_fs_ls_empty_directory() {
        let (sandbox, _dir) = create_test_sandbox();

        let result = fs_ls(&sandbox, ".").unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.trim().is_empty());
    }

    #[test]
    fn test_fs_ls_nested_directory() {
        let (sandbox, dir) = create_test_sandbox();

        fs::create_dir_all(dir.path().join("a/b/c")).unwrap();
        fs::write(dir.path().join("a/b/c/deep.txt"), "content").unwrap();

        let result = fs_ls(&sandbox, "a/b/c").unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("deep.txt"));
    }

    #[test]
    fn test_fs_ls_not_a_directory() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("file.txt"), "content").unwrap();

        let result = fs_ls(&sandbox, "file.txt");
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[test]
    fn test_fs_ls_noise_dirs_filtered() {
        let (sandbox, dir) = create_test_sandbox();

        fs::create_dir(dir.path().join("node_modules")).unwrap();
        fs::create_dir(dir.path().join("target")).unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();
        fs::create_dir(dir.path().join("__pycache__")).unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("main.rs"), "fn main() {}").unwrap();

        let result = fs_ls(&sandbox, ".").unwrap();
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(!output.contains("node_modules"));
        assert!(!output.contains("target"));
        assert!(!output.contains(".git"));
        assert!(!output.contains("__pycache__"));
        assert!(output.contains("src"));
        assert!(output.contains("main.rs"));
    }

    #[test]
    fn test_fs_ls_dirs_first_sorting() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("aaa_file.txt"), "").unwrap();
        fs::create_dir(dir.path().join("zzz_dir")).unwrap();
        fs::create_dir(dir.path().join("aaa_dir")).unwrap();
        fs::write(dir.path().join("zzz_file.txt"), "").unwrap();

        let result = fs_ls(&sandbox, ".").unwrap();
        let output = String::from_utf8_lossy(&result.stdout);
        let lines: Vec<&str> = output.lines().collect();

        // Dirs should come first, sorted alphabetically
        assert!(lines[0].contains("aaa_dir"));
        assert!(lines[1].contains("zzz_dir"));
        // Then files, sorted alphabetically
        assert!(lines[2].contains("aaa_file.txt"));
        assert!(lines[3].contains("zzz_file.txt"));
    }

    #[test]
    fn test_fs_ls_workspace_root_alias() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("root.txt"), "hello").unwrap();

        let result = fs_ls(&sandbox, "/").unwrap();
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("root.txt"));
    }
}
