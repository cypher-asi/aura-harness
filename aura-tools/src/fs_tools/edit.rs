use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core::ToolResult;
use aura_reasoner::ToolDefinition;
use std::fs;
use tracing::{debug, instrument};

/// Try fuzzy (trimmed, line-wise) matching when exact match fails.
///
/// Returns `Some((start_byte, end_byte))` of the *original* content slice that
/// matches the trimmed `old_text` lines. Only succeeds when exactly one
/// contiguous block matches.
fn fuzzy_line_match(content: &str, old_text: &str) -> Result<Option<(usize, usize)>, String> {
    let needle_lines: Vec<&str> = old_text.lines().map(str::trim).collect();
    if needle_lines.is_empty() {
        return Ok(None);
    }

    let content_lines: Vec<&str> = content.lines().collect();
    let mut matches: Vec<(usize, usize)> = Vec::new();

    'outer: for start in 0..content_lines.len() {
        if start + needle_lines.len() > content_lines.len() {
            break;
        }
        for (i, needle_line) in needle_lines.iter().enumerate() {
            if content_lines[start + i].trim() != *needle_line {
                continue 'outer;
            }
        }
        // Compute byte offsets in the original content
        let byte_start: usize = content_lines[..start].iter().map(|l| l.len() + 1).sum();
        let match_end_line = start + needle_lines.len() - 1;
        let byte_end: usize = content_lines[..match_end_line]
            .iter()
            .map(|l| l.len() + 1)
            .sum::<usize>()
            + content_lines[match_end_line].len();
        matches.push((byte_start, byte_end));
    }

    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches[0])),
        n => Err(format!(
            "Found {n} occurrences of the search text (fuzzy match). \
             Use replace_all=true to replace all, or make the search text more specific."
        )),
    }
}

/// Edit a file by replacing text.
///
/// When `replace_all` is `false` (default), exactly one occurrence must exist
/// (returns an error if there are 0 or 2+ matches). When `true`, all
/// occurrences are replaced.
///
/// If the exact match fails, a fuzzy line-wise trimmed match is attempted.
///
/// Safety guards:
/// - **Shrinkage guard**: rejects edits that would reduce the file to < 20%
///   of its original size.
/// - **CRLF normalization**: matching is performed on LF-normalized text; the
///   original line ending style is restored on write.
#[instrument(skip(sandbox, old_text, new_text), fields(path = %path))]
pub fn fs_edit(
    sandbox: &Sandbox,
    path: &str,
    old_text: &str,
    new_text: &str,
    replace_all: bool,
) -> Result<ToolResult, ToolError> {
    let resolved = sandbox.resolve_existing(path)?;
    debug!(?resolved, "Editing file");

    if !resolved.is_file() {
        return Err(ToolError::InvalidArguments(format!("{path} is not a file")));
    }

    let raw_content = fs::read_to_string(&resolved)?;

    // Detect CRLF and normalise to LF for matching
    let had_crlf = raw_content.contains("\r\n");
    let content = if had_crlf {
        raw_content.replace("\r\n", "\n")
    } else {
        raw_content
    };
    let old_text_norm = old_text.replace("\r\n", "\n");
    let new_text_norm = new_text.replace("\r\n", "\n");

    let exact_count = content.matches(old_text_norm.as_str()).count();

    let (new_content, replacements) = if exact_count == 0 {
        // Try fuzzy line-wise match
        match fuzzy_line_match(&content, &old_text_norm) {
            Ok(Some((start, end))) => {
                let mut buf = String::with_capacity(content.len());
                buf.push_str(&content[..start]);
                buf.push_str(&new_text_norm);
                buf.push_str(&content[end..]);
                (buf, 1usize)
            }
            Ok(None) => {
                return Err(ToolError::InvalidArguments(
                    "The specified text was not found in the file".to_string(),
                ));
            }
            Err(msg) => {
                return Err(ToolError::InvalidArguments(msg));
            }
        }
    } else if !replace_all && exact_count > 1 {
        return Err(ToolError::InvalidArguments(format!(
            "Found {exact_count} occurrences of the search text. \
             Use replace_all=true to replace all, or make the search text more specific."
        )));
    } else if replace_all {
        (
            content.replace(old_text_norm.as_str(), &new_text_norm),
            exact_count,
        )
    } else {
        (
            content.replacen(old_text_norm.as_str(), &new_text_norm, 1),
            1,
        )
    };

    // Shrinkage guard
    if !content.is_empty() && new_content.len() < content.len() / 5 {
        return Err(ToolError::InvalidArguments(
            "Edit would reduce file to less than 20% of original size. \
             This likely indicates truncated content."
                .to_string(),
        ));
    }

    // Restore CRLF if the original file used it
    let final_content = if had_crlf {
        new_content.replace('\n', "\r\n")
    } else {
        new_content
    };

    fs::write(&resolved, &final_content)?;

    Ok(ToolResult::success(
        "fs_edit",
        format!("Replaced {replacements} occurrence(s) in {path}"),
    )
    .with_metadata("replacements", replacements.to_string()))
}

/// `fs_edit` tool: edit a file by replacing text.
pub struct FsEditTool;

#[async_trait]
impl Tool for FsEditTool {
    fn name(&self) -> &str {
        "fs_edit"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "fs_edit".into(),
            description: "Edit an existing file by replacing a specific portion of text. By default replaces only the first occurrence.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to edit (relative to workspace root)"
                    },
                    "old_text": {
                        "type": "string",
                        "description": "The exact text to find and replace"
                    },
                    "new_text": {
                        "type": "string",
                        "description": "The text to replace it with"
                    },
                    "replace_all": {
                        "type": "boolean",
                        "description": "Replace all occurrences (default: false, replaces only first)"
                    }
                },
                "required": ["path", "old_text", "new_text"]
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
        let old_text = args["old_text"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'old_text' argument".into()))?
            .to_string();
        let new_text = args["new_text"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidArguments("missing 'new_text' argument".into()))?
            .to_string();
        let replace_all = args["replace_all"].as_bool().unwrap_or(false);
        let sandbox = ctx.sandbox.clone();
        super::spawn_blocking_tool(move || fs_edit(&sandbox, &path, &old_text, &new_text, replace_all))
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
    fn test_fs_edit_single_replacement() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("edit.txt"), "Hello, World!").unwrap();

        let result = fs_edit(&sandbox, "edit.txt", "World", "Aura", false).unwrap();
        assert!(result.ok);
        assert_eq!(result.metadata.get("replacements").unwrap(), "1");

        let content = fs::read_to_string(dir.path().join("edit.txt")).unwrap();
        assert_eq!(content, "Hello, Aura!");
    }

    #[test]
    fn test_fs_edit_replace_all() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("edit.txt"), "foo bar foo baz foo").unwrap();

        let result = fs_edit(&sandbox, "edit.txt", "foo", "qux", true).unwrap();
        assert!(result.ok);
        assert_eq!(result.metadata.get("replacements").unwrap(), "3");

        let content = fs::read_to_string(dir.path().join("edit.txt")).unwrap();
        assert_eq!(content, "qux bar qux baz qux");
    }

    #[test]
    fn test_fs_edit_multi_match_without_replace_all_errors() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("edit.txt"), "foo bar foo baz foo").unwrap();

        let result = fs_edit(&sandbox, "edit.txt", "foo", "qux", false);
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
        if let Err(ToolError::InvalidArguments(msg)) = result {
            assert!(msg.contains("3 occurrences"));
            assert!(msg.contains("replace_all=true"));
        }
    }

    #[test]
    fn test_fs_edit_text_not_found() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("edit.txt"), "Hello, World!").unwrap();

        let result = fs_edit(&sandbox, "edit.txt", "NotFound", "Replacement", false);
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[test]
    fn test_fs_edit_not_a_file() {
        let (sandbox, dir) = create_test_sandbox();

        fs::create_dir(dir.path().join("dir")).unwrap();

        let result = fs_edit(&sandbox, "dir", "old", "new", false);
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    }

    #[test]
    fn test_fs_edit_multiline() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "line1\nold_content\nline3";
        fs::write(dir.path().join("multi.txt"), content).unwrap();

        let result = fs_edit(&sandbox, "multi.txt", "old_content", "new_content", false).unwrap();
        assert!(result.ok);

        let updated = fs::read_to_string(dir.path().join("multi.txt")).unwrap();
        assert_eq!(updated, "line1\nnew_content\nline3");
    }

    #[test]
    fn test_fs_edit_fuzzy_match_whitespace_difference() {
        let (sandbox, dir) = create_test_sandbox();

        // File has extra leading whitespace
        let content = "fn main() {\n    let x = 1;\n    let y = 2;\n}\n";
        fs::write(dir.path().join("fuzzy.rs"), content).unwrap();

        // old_text has different indentation (trimmed)
        let old_text = "let x = 1;\nlet y = 2;";
        let new_text = "let x = 10;\nlet y = 20;";

        let result = fs_edit(&sandbox, "fuzzy.rs", old_text, new_text, false).unwrap();
        assert!(result.ok);

        let updated = fs::read_to_string(dir.path().join("fuzzy.rs")).unwrap();
        assert!(updated.contains("let x = 10;"));
        assert!(updated.contains("let y = 20;"));
    }

    #[test]
    fn test_fs_edit_fuzzy_match_no_match() {
        let (sandbox, dir) = create_test_sandbox();

        fs::write(dir.path().join("nope.txt"), "alpha\nbeta\ngamma\n").unwrap();

        let result = fs_edit(&sandbox, "nope.txt", "totally\ndifferent", "new", false);
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
        if let Err(ToolError::InvalidArguments(msg)) = result {
            assert!(msg.contains("not found"));
        }
    }

    #[test]
    fn test_fs_edit_shrinkage_guard_rejects_large_reduction() {
        let (sandbox, dir) = create_test_sandbox();

        let big_content = "a\n".repeat(500);
        fs::write(dir.path().join("shrink.txt"), &big_content).unwrap();

        // Replace the entire content with something tiny
        let result = fs_edit(&sandbox, "shrink.txt", &big_content, "x", false);
        assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
        if let Err(ToolError::InvalidArguments(msg)) = result {
            assert!(msg.contains("20%"));
        }
    }

    #[test]
    fn test_fs_edit_shrinkage_guard_allows_normal_edit() {
        let (sandbox, dir) = create_test_sandbox();

        let content = "Hello, World! This is a test file with enough content.";
        fs::write(dir.path().join("normal.txt"), content).unwrap();

        let result = fs_edit(&sandbox, "normal.txt", "World", "Aura", false).unwrap();
        assert!(result.ok);

        let updated = fs::read_to_string(dir.path().join("normal.txt")).unwrap();
        assert_eq!(
            updated,
            "Hello, Aura! This is a test file with enough content."
        );
    }

    #[test]
    fn test_fs_edit_crlf_normalization() {
        let (sandbox, dir) = create_test_sandbox();

        // Write a CRLF file
        let crlf_content = "line1\r\nline2\r\nline3\r\n";
        fs::write(dir.path().join("crlf.txt"), crlf_content).unwrap();

        let result = fs_edit(&sandbox, "crlf.txt", "line2", "replaced", false).unwrap();
        assert!(result.ok);

        let updated = fs::read_to_string(dir.path().join("crlf.txt")).unwrap();
        // Output should still be CRLF
        assert!(updated.contains("\r\n"));
        assert!(updated.contains("replaced"));
        assert!(!updated.contains("line2"));
    }
}
