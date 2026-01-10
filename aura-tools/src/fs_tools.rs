//! Filesystem tool implementations.

#![allow(dead_code)] // Functions for future tool executor integration

use crate::error::ToolError;
use crate::sandbox::Sandbox;
use aura_core::ToolResult;
use std::collections::HashMap;
use std::fs;
use std::os::windows::fs::MetadataExt;
use tracing::{debug, instrument};

/// List directory contents.
#[instrument(skip(sandbox), fields(path = %path))]
pub fn fs_ls(sandbox: &Sandbox, path: &str) -> Result<ToolResult, ToolError> {
    let resolved = sandbox.resolve_existing(path)?;
    debug!(?resolved, "Listing directory");

    if !resolved.is_dir() {
        return Err(ToolError::InvalidArguments(format!(
            "{path} is not a directory"
        )));
    }

    let mut entries = Vec::new();
    for entry in fs::read_dir(&resolved)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        let metadata = entry.metadata()?;

        let entry_type = if metadata.is_dir() {
            "dir"
        } else if metadata.is_file() {
            "file"
        } else {
            "other"
        };

        entries.push(format!("{}\t{}\t{}", entry_type, metadata.len(), name));
    }

    let output = entries.join("\n");
    Ok(ToolResult::success("fs.ls", output))
}

/// Read file contents.
#[instrument(skip(sandbox), fields(path = %path, max_bytes))]
pub fn fs_read(sandbox: &Sandbox, path: &str, max_bytes: usize) -> Result<ToolResult, ToolError> {
    let resolved = sandbox.resolve_existing(path)?;
    debug!(?resolved, "Reading file");

    if !resolved.is_file() {
        return Err(ToolError::InvalidArguments(format!("{path} is not a file")));
    }

    // Check file size before reading
    let metadata = fs::metadata(&resolved)?;
    let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);

    if size > max_bytes {
        return Err(ToolError::SizeLimitExceeded {
            actual: size,
            limit: max_bytes,
        });
    }

    let contents = fs::read(&resolved)?;
    Ok(ToolResult::success("fs.read", contents).with_metadata("size", size.to_string()))
}

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

    // Windows-specific attributes
    result_metadata.insert(
        "file_attributes".to_string(),
        metadata.file_attributes().to_string(),
    );

    // Format output as key=value pairs
    let output: Vec<String> = result_metadata
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect();

    let mut tool_result = ToolResult::success("fs.stat", output.join("\n"));
    tool_result.metadata = result_metadata;
    Ok(tool_result)
}

/// Write content to a file.
#[instrument(skip(sandbox, content), fields(path = %path))]
pub fn fs_write(
    sandbox: &Sandbox,
    path: &str,
    content: &str,
    create_dirs: bool,
) -> Result<ToolResult, ToolError> {
    let resolved = sandbox.resolve_new(path)?;
    debug!(?resolved, "Writing file");

    // Create parent directories if requested
    if create_dirs {
        if let Some(parent) = resolved.parent() {
            fs::create_dir_all(parent)?;
        }
    } else if let Some(parent) = resolved.parent() {
        if !parent.exists() {
            return Err(ToolError::PathNotFound(
                parent.to_string_lossy().to_string(),
            ));
        }
    }

    fs::write(&resolved, content)?;

    let bytes_written = content.len();
    Ok(ToolResult::success(
        "fs_write",
        format!("Wrote {bytes_written} bytes to {path}"),
    )
    .with_metadata("bytes_written", bytes_written.to_string()))
}

/// Edit a file by replacing text.
#[instrument(skip(sandbox, old_text, new_text), fields(path = %path))]
pub fn fs_edit(
    sandbox: &Sandbox,
    path: &str,
    old_text: &str,
    new_text: &str,
) -> Result<ToolResult, ToolError> {
    let resolved = sandbox.resolve_existing(path)?;
    debug!(?resolved, "Editing file");

    if !resolved.is_file() {
        return Err(ToolError::InvalidArguments(format!("{path} is not a file")));
    }

    let content = fs::read_to_string(&resolved)?;

    // Check if the old_text exists in the file
    let count = content.matches(old_text).count();
    if count == 0 {
        return Err(ToolError::InvalidArguments(
            "The specified text was not found in the file".to_string(),
        ));
    }

    // Replace and write back
    let new_content = content.replace(old_text, new_text);
    fs::write(&resolved, &new_content)?;

    Ok(ToolResult::success(
        "fs_edit",
        format!("Replaced {count} occurrence(s) in {path}"),
    )
    .with_metadata("replacements", count.to_string()))
}

/// Search for patterns in code.
#[instrument(skip(sandbox), fields(pattern = %pattern))]
pub fn search_code(
    sandbox: &Sandbox,
    pattern: &str,
    path: Option<&str>,
    file_pattern: Option<&str>,
    max_results: usize,
) -> Result<ToolResult, ToolError> {
    use regex::Regex;
    use walkdir::WalkDir;

    let search_root = match path {
        Some(p) => sandbox.resolve_existing(p)?,
        None => sandbox.root().to_path_buf(),
    };

    debug!(?search_root, "Searching code");

    let regex = Regex::new(pattern)
        .map_err(|e| ToolError::InvalidArguments(format!("Invalid regex: {e}")))?;

    let file_pattern_regex = file_pattern
        .map(|p| {
            // Convert glob-like pattern to regex
            let regex_pattern = p
                .replace('.', r"\.")
                .replace('*', ".*")
                .replace('?', ".");
            Regex::new(&format!("^{regex_pattern}$"))
        })
        .transpose()
        .map_err(|e| ToolError::InvalidArguments(format!("Invalid file pattern: {e}")))?;

    let mut results = Vec::new();

    for entry in WalkDir::new(&search_root)
        .follow_links(true)
        .into_iter()
        .filter_map(Result::ok)
    {
        if results.len() >= max_results {
            break;
        }

        let entry_path = entry.path();
        if !entry_path.is_file() {
            continue;
        }

        // Check file pattern filter
        if let Some(ref fp_regex) = file_pattern_regex {
            let file_name = entry_path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if !fp_regex.is_match(file_name) {
                continue;
            }
        }

        // Skip binary files (simple heuristic)
        let extension = entry_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let text_extensions = [
            "rs", "ts", "js", "py", "go", "java", "c", "cpp", "h", "hpp", "md", "txt", "json",
            "yaml", "yml", "toml", "xml", "html", "css", "sql", "sh", "bat", "ps1",
        ];
        if !text_extensions.contains(&extension) && !extension.is_empty() {
            continue;
        }

        // Read and search file
        if let Ok(file_content) = fs::read_to_string(entry_path) {
            for (line_num, line) in file_content.lines().enumerate() {
                if results.len() >= max_results {
                    break;
                }

                if regex.is_match(line) {
                    let relative_path = entry_path
                        .strip_prefix(&search_root)
                        .unwrap_or(entry_path)
                        .to_string_lossy();
                    results.push(format!("{}:{}:{}", relative_path, line_num + 1, line));
                }
            }
        }
    }

    let output = if results.is_empty() {
        "No matches found".to_string()
    } else {
        results.join("\n")
    };

    Ok(ToolResult::success("search_code", output)
        .with_metadata("match_count", results.len().to_string()))
}

// ============================================================================
// Command Execution
// ============================================================================

/// Result of a threshold-based wait operation.
///
/// When a command is run with a sync threshold:
/// - `Completed`: The command finished within the threshold
/// - `Pending`: The command is still running, handle returned for async tracking
pub enum ThresholdResult {
    /// Command completed within the threshold.
    Completed(std::process::Output),
    /// Command is still running after the threshold.
    Pending(std::process::Child),
}

/// Spawn a shell command and return the child process handle.
///
/// This is the low-level spawn operation that doesn't wait for completion.
/// Use this when you need to manage the process lifecycle yourself.
///
/// On Windows, commands are run through `cmd.exe /c`.
/// On Unix, commands are run through `sh -c`.
#[instrument(skip(sandbox), fields(program = %program))]
pub fn cmd_spawn(
    sandbox: &Sandbox,
    program: &str,
    args: &[String],
    cwd: Option<&str>,
) -> Result<(std::process::Child, String), ToolError> {
    use std::process::{Command, Stdio};

    let working_dir = match cwd {
        Some(dir) => sandbox.resolve_existing(dir)?,
        None => sandbox.root().to_path_buf(),
    };

    debug!(?working_dir, ?args, "Spawning command");

    // Build the full command string
    let full_command = if args.is_empty() {
        program.to_string()
    } else {
        format!("{} {}", program, args.join(" "))
    };

    // Use shell to run the command for better compatibility
    #[cfg(windows)]
    let mut cmd = {
        let mut c = Command::new("cmd.exe");
        c.args(["/C", &full_command]);
        c
    };

    #[cfg(not(windows))]
    let mut cmd = {
        let mut c = Command::new("sh");
        c.args(["-c", &full_command]);
        c
    };

    cmd.current_dir(&working_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let child = cmd.spawn().map_err(|e| {
        ToolError::CommandFailed(format!("Failed to spawn command '{program}': {e}"))
    })?;

    Ok((child, full_command))
}

/// Run a shell command with threshold-based execution.
///
/// This waits for the command to complete up to `sync_threshold_ms`.
/// - If the command completes within the threshold, returns `ThresholdResult::Completed`
/// - If the command is still running after the threshold, returns `ThresholdResult::Pending`
///   with the child handle for async tracking
///
/// On Windows, commands are run through `cmd.exe /c`.
/// On Unix, commands are run through `sh -c`.
#[instrument(skip(sandbox), fields(program = %program))]
pub fn cmd_run_with_threshold(
    sandbox: &Sandbox,
    program: &str,
    args: &[String],
    cwd: Option<&str>,
    sync_threshold_ms: u64,
) -> Result<(ThresholdResult, String), ToolError> {
    use std::time::Duration;

    let (child, full_command) = cmd_spawn(sandbox, program, args, cwd)?;

    // Wait with threshold
    let result = wait_with_threshold(child, Duration::from_millis(sync_threshold_ms));
    Ok((result, full_command))
}

/// Run a shell command synchronously with a timeout.
///
/// This is the original synchronous API that waits for completion or kills on timeout.
/// Use `cmd_run_with_threshold` for async-capable execution.
///
/// On Windows, commands are run through `cmd.exe /c`.
/// On Unix, commands are run through `sh -c`.
#[instrument(skip(sandbox), fields(program = %program))]
pub fn cmd_run(
    sandbox: &Sandbox,
    program: &str,
    args: &[String],
    cwd: Option<&str>,
    timeout_ms: u64,
) -> Result<ToolResult, ToolError> {
    use std::time::Duration;

    let (child, _full_command) = cmd_spawn(sandbox, program, args, cwd)?;

    // Wait with hard timeout (kills on timeout)
    let output = match wait_with_hard_timeout(child, Duration::from_millis(timeout_ms)) {
        Ok(out) => out,
        Err(e) => {
            return Err(ToolError::CommandFailed(format!("Command timed out: {e}")));
        }
    };

    output_to_tool_result(output)
}

/// Convert process output to a tool result.
pub fn output_to_tool_result(output: std::process::Output) -> Result<ToolResult, ToolError> {
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        let mut result = ToolResult::success("cmd_run", stdout);
        if !stderr.is_empty() {
            result.stderr = stderr.into_bytes().into();
        }
        result = result.with_metadata("exit_code", "0".to_string());
        Ok(result)
    } else {
        let exit_code = output.status.code().unwrap_or(-1);
        let combined = if stderr.is_empty() {
            stdout
        } else {
            format!("{stdout}\n{stderr}")
        };
        Err(ToolError::CommandFailed(format!(
            "Command exited with code {exit_code}: {combined}"
        )))
    }
}

/// Wait for a child process with a threshold.
///
/// If the process completes within the threshold, returns `ThresholdResult::Completed`.
/// If the process is still running after the threshold, returns `ThresholdResult::Pending`
/// with the child handle intact (NOT killed).
fn wait_with_threshold(
    mut child: std::process::Child,
    threshold: std::time::Duration,
) -> ThresholdResult {
    use std::io::Read;
    use std::thread;
    use std::time::Instant;

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // Process finished, collect output
                let stdout = child.stdout.take().map_or_else(Vec::new, |mut s| {
                    let mut buf = Vec::new();
                    let _ = s.read_to_end(&mut buf);
                    buf
                });
                let stderr = child.stderr.take().map_or_else(Vec::new, |mut s| {
                    let mut buf = Vec::new();
                    let _ = s.read_to_end(&mut buf);
                    buf
                });
                return ThresholdResult::Completed(std::process::Output {
                    status,
                    stdout,
                    stderr,
                });
            }
            Ok(None) => {
                // Process still running
                if start.elapsed() > threshold {
                    // Threshold exceeded - return the child for async tracking
                    // Do NOT kill the process
                    return ThresholdResult::Pending(child);
                }
                thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(_) => {
                // Error checking status - return child for caller to handle
                if start.elapsed() > threshold {
                    return ThresholdResult::Pending(child);
                }
                thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

/// Wait for a child process with a hard timeout (kills on timeout).
///
/// This is the original timeout behavior - if the process doesn't complete
/// within the timeout, it is killed and an error is returned.
fn wait_with_hard_timeout(
    mut child: std::process::Child,
    timeout: std::time::Duration,
) -> std::io::Result<std::process::Output> {
    use std::io::Read;
    use std::thread;
    use std::time::Instant;

    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            // Process finished, collect output
            let stdout = child.stdout.take().map_or_else(Vec::new, |mut s| {
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf);
                buf
            });
            let stderr = child.stderr.take().map_or_else(Vec::new, |mut s| {
                let mut buf = Vec::new();
                let _ = s.read_to_end(&mut buf);
                buf
            });
            return Ok(std::process::Output {
                status,
                stdout,
                stderr,
            });
        }
        
        if start.elapsed() > timeout {
            // Kill the process
            let _ = child.kill();
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "Process timed out",
            ));
        }
        thread::sleep(std::time::Duration::from_millis(10));
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

    // ========================================================================
    // fs_ls Tests
    // ========================================================================

    #[test]
    fn test_fs_ls() {
        let (sandbox, dir) = create_test_sandbox();

        // Create some files and dirs
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
        // Empty directory should produce empty or whitespace-only output
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

    // ========================================================================
    // fs_read Tests
    // ========================================================================

    #[test]
    fn test_fs_read() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "Hello, Aura!";
        fs::write(dir.path().join("test.txt"), content).unwrap();

        let result = fs_read(&sandbox, "test.txt", 1024).unwrap();
        assert!(result.ok);
        assert_eq!(&result.stdout[..], content.as_bytes());
    }

    #[test]
    fn test_fs_read_size_limit() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "Hello, Aura!";
        fs::write(dir.path().join("test.txt"), content).unwrap();

        let result = fs_read(&sandbox, "test.txt", 5);
        assert!(matches!(result, Err(ToolError::SizeLimitExceeded { .. })));
    }

    #[test]
    fn test_fs_read_binary_content() {
        let (sandbox, dir) = create_test_sandbox();

        let content = vec![0u8, 1, 2, 255, 254, 253];
        fs::write(dir.path().join("binary.bin"), &content).unwrap();

        let result = fs_read(&sandbox, "binary.bin", 1024).unwrap();
        assert!(result.ok);
        assert_eq!(&result.stdout[..], content.as_slice());
    }

    #[test]
    fn test_fs_read_empty_file() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("empty.txt"), "").unwrap();

        let result = fs_read(&sandbox, "empty.txt", 1024).unwrap();
        assert!(result.ok);
        assert!(result.stdout.is_empty());
    }

    #[test]
    fn test_fs_read_not_a_file() {
        let (sandbox, dir) = create_test_sandbox();

        fs::create_dir(dir.path().join("dir")).unwrap();

        let result = fs_read(&sandbox, "dir", 1024);
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    // ========================================================================
    // fs_stat Tests
    // ========================================================================

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

    // ========================================================================
    // fs_write Tests
    // ========================================================================

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
    fn test_fs_write_no_create_dirs() {
        let (sandbox, _dir) = create_test_sandbox();

        let result = fs_write(&sandbox, "nonexistent/file.txt", "content", false);
        assert!(matches!(result, Err(ToolError::PathNotFound(_))));
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

    // ========================================================================
    // fs_edit Tests
    // ========================================================================

    #[test]
    fn test_fs_edit_single_replacement() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("edit.txt"), "Hello, World!").unwrap();

        let result = fs_edit(&sandbox, "edit.txt", "World", "Aura").unwrap();
        assert!(result.ok);
        assert_eq!(result.metadata.get("replacements").unwrap(), "1");

        let content = fs::read_to_string(dir.path().join("edit.txt")).unwrap();
        assert_eq!(content, "Hello, Aura!");
    }

    #[test]
    fn test_fs_edit_multiple_replacements() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("edit.txt"), "foo bar foo baz foo").unwrap();

        let result = fs_edit(&sandbox, "edit.txt", "foo", "qux").unwrap();
        assert!(result.ok);
        assert_eq!(result.metadata.get("replacements").unwrap(), "3");

        let content = fs::read_to_string(dir.path().join("edit.txt")).unwrap();
        assert_eq!(content, "qux bar qux baz qux");
    }

    #[test]
    fn test_fs_edit_text_not_found() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("edit.txt"), "Hello, World!").unwrap();

        let result = fs_edit(&sandbox, "edit.txt", "NotFound", "Replacement");
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[test]
    fn test_fs_edit_not_a_file() {
        let (sandbox, dir) = create_test_sandbox();

        fs::create_dir(dir.path().join("dir")).unwrap();

        let result = fs_edit(&sandbox, "dir", "old", "new");
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[test]
    fn test_fs_edit_multiline() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "line1\nold_content\nline3";
        fs::write(dir.path().join("multi.txt"), content).unwrap();

        let result = fs_edit(&sandbox, "multi.txt", "old_content", "new_content").unwrap();
        assert!(result.ok);

        let updated = fs::read_to_string(dir.path().join("multi.txt")).unwrap();
        assert_eq!(updated, "line1\nnew_content\nline3");
    }

    // ========================================================================
    // search_code Tests
    // ========================================================================

    #[test]
    fn test_search_code_simple_pattern() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("code.rs"), "fn main() { println!(\"hello\"); }").unwrap();

        let result = search_code(&sandbox, "fn main", None, None, 100).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("fn main"));
        assert!(output.contains("code.rs"));
    }

    #[test]
    fn test_search_code_regex_pattern() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("code.rs"), "let x = 42;\nlet y = 123;").unwrap();

        let result = search_code(&sandbox, r"let \w+ = \d+", None, None, 100).unwrap();
        assert!(result.ok);
        assert_eq!(result.metadata.get("match_count").unwrap(), "2");
    }

    #[test]
    fn test_search_code_no_matches() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("code.rs"), "fn main() {}").unwrap();

        let result = search_code(&sandbox, "nonexistent_pattern_xyz", None, None, 100).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("No matches found"));
    }

    #[test]
    fn test_search_code_file_pattern() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("code.rs"), "let rust_var = 1;").unwrap();
        fs::write(dir.path().join("code.ts"), "let ts_var = 2;").unwrap();

        let result = search_code(&sandbox, "let", None, Some("*.rs"), 100).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("rust_var"));
        assert!(!output.contains("ts_var"));
    }

    #[test]
    fn test_search_code_max_results() {
        let (sandbox, dir) = create_test_sandbox();

        let content = (0..20).map(|i| format!("line{i}")).collect::<Vec<_>>().join("\n");
        fs::write(dir.path().join("many.txt"), content).unwrap();

        let result = search_code(&sandbox, "line", None, None, 5).unwrap();
        assert!(result.ok);
        assert_eq!(result.metadata.get("match_count").unwrap(), "5");
    }

    #[test]
    fn test_search_code_in_subdirectory() {
        let (sandbox, dir) = create_test_sandbox();

        fs::create_dir_all(dir.path().join("src/nested")).unwrap();
        fs::write(dir.path().join("src/nested/code.rs"), "fn nested_fn() {}").unwrap();

        let result = search_code(&sandbox, "nested_fn", Some("src"), None, 100).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("nested_fn"));
    }

    #[test]
    fn test_search_code_invalid_regex() {
        let (sandbox, _dir) = create_test_sandbox();

        let result = search_code(&sandbox, "[invalid(regex", None, None, 100);
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    // ========================================================================
    // cmd_run Tests
    // ========================================================================

    #[test]
    fn test_cmd_run_echo() {
        let (sandbox, _dir) = create_test_sandbox();

        let result = cmd_run(&sandbox, "echo", &["hello".to_string()], None, 5000).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("hello"));
    }

    #[test]
    fn test_cmd_run_in_cwd() {
        let (sandbox, dir) = create_test_sandbox();

        fs::create_dir(dir.path().join("subdir")).unwrap();
        fs::write(dir.path().join("subdir/marker.txt"), "found").unwrap();

        // On Windows, use 'dir' or 'type'; on Unix, use 'ls' or 'cat'
        #[cfg(windows)]
        let result = cmd_run(&sandbox, "dir", &[], Some("subdir"), 5000).unwrap();
        #[cfg(not(windows))]
        let result = cmd_run(&sandbox, "ls", &[], Some("subdir"), 5000).unwrap();

        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("marker"));
    }

    #[test]
    fn test_cmd_run_nonexistent_command() {
        let (sandbox, _dir) = create_test_sandbox();

        let result = cmd_run(
            &sandbox,
            "nonexistent_command_that_does_not_exist_xyz",
            &[],
            None,
            5000,
        );
        assert!(matches!(result, Err(ToolError::CommandFailed(_))));
    }

    #[test]
    fn test_cmd_run_exit_code() {
        let (sandbox, _dir) = create_test_sandbox();

        // Run a command that exits with non-zero status
        #[cfg(windows)]
        let result = cmd_run(&sandbox, "cmd", &["/c".to_string(), "exit".to_string(), "1".to_string()], None, 5000);
        #[cfg(not(windows))]
        let result = cmd_run(&sandbox, "false", &[], None, 5000);

        assert!(matches!(result, Err(ToolError::CommandFailed(_))));
    }

    #[test]
    fn test_cmd_run_with_args() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("test.txt"), "content").unwrap();

        #[cfg(windows)]
        let result = cmd_run(&sandbox, "type", &["test.txt".to_string()], None, 5000).unwrap();
        #[cfg(not(windows))]
        let result = cmd_run(&sandbox, "cat", &["test.txt".to_string()], None, 5000).unwrap();

        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(output.contains("content"));
    }

    // ========================================================================
    // wait_with_threshold Tests
    // ========================================================================

    #[test]
    fn test_fast_command_returns_output() {
        let (sandbox, _dir) = create_test_sandbox();

        // Run a fast command with generous threshold
        let (result, _command) = cmd_run_with_threshold(
            &sandbox,
            "echo",
            &["fast_output".to_string()],
            None,
            5000, // 5 second threshold - plenty of time
        )
        .unwrap();

        // Should complete within threshold
        match result {
            ThresholdResult::Completed(output) => {
                assert!(output.status.success());
                let stdout = String::from_utf8_lossy(&output.stdout);
                assert!(stdout.contains("fast_output"));
            }
            ThresholdResult::Pending(_) => {
                panic!("Expected Completed, got Pending for fast command");
            }
        }
    }

    #[test]
    fn test_slow_command_returns_child() {
        let (sandbox, _dir) = create_test_sandbox();

        // Run a slow command with very short threshold
        #[cfg(windows)]
        let (result, command) = cmd_run_with_threshold(
            &sandbox,
            "ping",
            &["-n".to_string(), "10".to_string(), "127.0.0.1".to_string()],
            None,
            100, // 100ms threshold - too short for ping
        )
        .unwrap();

        #[cfg(not(windows))]
        let (result, command) = cmd_run_with_threshold(
            &sandbox,
            "sleep",
            &["10".to_string()],
            None,
            100, // 100ms threshold - too short for sleep
        )
        .unwrap();

        // Should return Pending with child handle
        match result {
            ThresholdResult::Pending(mut child) => {
                // Verify we have a live child process
                assert!(child.try_wait().unwrap().is_none(), "Child should still be running");
                // Clean up - kill the process
                let _ = child.kill();
                let _ = child.wait();
                assert!(!command.is_empty());
            }
            ThresholdResult::Completed(_) => {
                panic!("Expected Pending, got Completed for slow command");
            }
        }
    }

    #[test]
    fn test_threshold_boundary_fast_completes() {
        let (sandbox, _dir) = create_test_sandbox();

        // Command that should complete just under a reasonable threshold
        let (result, _command) = cmd_run_with_threshold(
            &sandbox,
            "echo",
            &["boundary".to_string()],
            None,
            1000, // 1 second should be enough for echo
        )
        .unwrap();

        // echo is fast, should complete
        match result {
            ThresholdResult::Completed(output) => {
                assert!(output.status.success());
            }
            ThresholdResult::Pending(_) => {
                panic!("Expected Completed for fast echo command");
            }
        }
    }

    #[test]
    fn test_cmd_spawn_returns_command_string() {
        let (sandbox, _dir) = create_test_sandbox();

        let (mut child, command) = cmd_spawn(
            &sandbox,
            "echo",
            &["test_arg".to_string()],
            None,
        )
        .unwrap();

        // Command string should include the program and args
        assert!(command.contains("echo"));
        assert!(command.contains("test_arg"));

        // Clean up
        let _ = child.wait();
    }

    #[test]
    fn test_output_to_tool_result_success() {
        // Create a successful output
        #[cfg(windows)]
        let status = {
            // On Windows, we need to run an actual command to get a valid ExitStatus
            let output = std::process::Command::new("cmd.exe")
                .args(["/C", "exit 0"])
                .output()
                .unwrap();
            output.status
        };

        #[cfg(not(windows))]
        let status = {
            let output = std::process::Command::new("true")
                .output()
                .unwrap();
            output.status
        };

        let output = std::process::Output {
            status,
            stdout: b"success output".to_vec(),
            stderr: Vec::new(),
        };

        let result = output_to_tool_result(output).unwrap();
        assert!(result.ok);
        assert_eq!(String::from_utf8_lossy(&result.stdout), "success output");
    }

    #[test]
    fn test_output_to_tool_result_failure() {
        // Create a failed output
        #[cfg(windows)]
        let status = {
            let output = std::process::Command::new("cmd.exe")
                .args(["/C", "exit 1"])
                .output()
                .unwrap();
            output.status
        };

        #[cfg(not(windows))]
        let status = {
            let output = std::process::Command::new("false")
                .output()
                .unwrap();
            output.status
        };

        let output = std::process::Output {
            status,
            stdout: Vec::new(),
            stderr: b"error message".to_vec(),
        };

        let result = output_to_tool_result(output);
        assert!(matches!(result, Err(ToolError::CommandFailed(_))));
    }
}
