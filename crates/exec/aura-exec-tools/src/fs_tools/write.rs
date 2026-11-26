use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::ToolDefinition;
use aura_core_types::ToolResult;
use std::fs;
use std::path::Path;
use tracing::{debug, instrument};

/// Soft cap on `write_file` content size mirrored from
/// `aura_config::WRITE_FILE_CHUNK_BYTES`. Kept local to avoid a
/// circular dep between `aura-exec-tools` and `aura-agent`; tests guard against
/// drift by asserting on the numeric threshold directly.
const WRITE_FILE_CHUNK_BYTES: usize = 12_000;

/// File extensions for which the unbalanced-brace/paren heuristic is
/// worth running. Anything outside this set (Markdown, plain text,
/// YAML/TOML, HTML, …) routinely contains unbalanced braces in
/// legitimate content (string literals, prose, closing tags) and
/// produced noisy warnings before this gate was added.
///
/// Intentionally narrow: the heuristic is a stopgap, not a parser, and
/// the cost of a false negative on a rare extension is much lower than
/// the cost of spamming a false positive on every README write.
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "js", "ts", "tsx", "jsx", "c", "cpp", "h", "java", "go", "cs", "swift", "kt",
];

fn is_elided_write_placeholder(content: &str) -> bool {
    content.starts_with("<<<AURA_ELIDED_CONTENT::") && content.ends_with(">>>")
}

fn has_redacted_field_marker(args: &serde_json::Value, field: &str) -> bool {
    let Some(marker) = args.get("_redacted").and_then(serde_json::Value::as_object) else {
        return false;
    };
    if marker
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|kind| kind == "aura_compaction_redaction")
        && marker
            .get("field")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|marked| marked == field)
    {
        return true;
    }
    marker
        .get("fields")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|fields| {
            fields.iter().any(|entry| {
                entry
                    .get("field")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|marked| marked == field)
            })
        })
}

fn is_code_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            CODE_EXTENSIONS
                .iter()
                .any(|code_ext| ext.eq_ignore_ascii_case(code_ext))
        })
}

/// Check whether `content` has unbalanced `{}`/`()` pairs, which may
/// indicate truncated output from an LLM.
///
/// Only runs the balance check for files whose extension appears in
/// [`CODE_EXTENSIONS`] — Markdown, prose, and config files routinely
/// carry unbalanced braces inside string literals / code fences and
/// shouldn't trip the warning.
fn looks_truncated(path: &Path, content: &str) -> bool {
    if !is_code_file(path) {
        return false;
    }
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
    if is_elided_write_placeholder(content) {
        return Err(ToolError::CompactionStructural(
            "content is an elided history placeholder; supply the real file content. \
             Prior turns redact write_file/edit_file inputs to save context; never copy \
             the placeholder verbatim. Re-emit the full intended content here."
                .to_string(),
        ));
    }

    let _ = create_dirs; // kept for API compat; always creates dirs
    let resolved = sandbox.resolve_new(path)?;
    debug!(?resolved, "Writing file");

    let file_existed = resolved.exists();
    // Snapshot pre-content before the write so we can compute a real
    // line-level diff rather than guessing from sizes alone. Only
    // captured when the file already exists; for a fresh create the
    // pre-content is the empty string (no allocation needed). We read
    // it lazily into a String so a non-UTF-8 file silently falls back
    // to "no pre-content" (lines_added counts the new content as a
    // pure insert) rather than failing the write — line counting is a
    // best-effort observability signal, not a correctness gate.
    let pre_content = if file_existed {
        fs::read_to_string(&resolved).unwrap_or_default()
    } else {
        String::new()
    };
    let existing_size = if file_existed {
        usize::try_from(fs::metadata(&resolved).map_or(0, |m| m.len())).unwrap_or(usize::MAX)
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

    // Structural guard: refuse to land a busted `Cargo.toml` on disk.
    // Running BEFORE `fs::write` keeps the previous on-disk content
    // intact when the new content is malformed; the agent then sees
    // the structural error and can retry with a corrected manifest.
    if super::cargo_toml_guard::is_cargo_manifest(&resolved) {
        super::cargo_toml_guard::validate_cargo_toml(content)?;
    }

    fs::write(&resolved, content).map_err(|e| {
        ToolError::Io(std::io::Error::new(
            e.kind(),
            format!("write({}): {e}", resolved.display()),
        ))
    })?;

    // Post-write verification
    let on_disk_size =
        usize::try_from(fs::metadata(&resolved).map_or(0, |m| m.len())).unwrap_or(usize::MAX);
    if on_disk_size != content.len() {
        return Err(ToolError::InvalidArguments(format!(
            "Post-write verification failed: wrote {} bytes but file is {} bytes on disk",
            content.len(),
            on_disk_size
        )));
    }

    let bytes_written = content.len();
    let truncated_warning = looks_truncated(&resolved, content);
    let module_link_warning = if file_existed {
        None
    } else {
        super::module_link::check_module_link(sandbox, path, &resolved)
    };

    // Compute the actual line diff between pre-content and new content.
    // For a fresh create, pre_content is empty so this collapses to
    // "lines_added = new line count, lines_removed = 0".
    let (lines_added, lines_removed) = super::diff::count_line_diff(&pre_content, content);

    let mut result = ToolResult::success(
        "write_file",
        format!("Wrote {bytes_written} bytes to {path}"),
    )
    .with_metadata("bytes_written", bytes_written.to_string())
    .with_metadata("file_existed", file_existed.to_string())
    .with_line_diff(lines_added, lines_removed);

    if truncated_warning {
        result = result.with_metadata(
            "warning",
            "Content has unbalanced braces/parentheses – may be truncated".to_string(),
        );
    }

    if let Some(check) = module_link_warning {
        if !check.linked {
            result = result.with_metadata("unlinked_module", "true");
            if let Some(message) = check.message {
                result = result.with_metadata("unlinked_module_hint", message);
            }
        }
    }

    if bytes_written > WRITE_FILE_CHUNK_BYTES {
        result = result.with_metadata("chunk_suggestion", "true");
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

    fn changed_path<'a>(&self, args: &'a serde_json::Value) -> Option<&'a str> {
        crate::tool::path_arg(args)
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        if has_redacted_field_marker(&args, "content") {
            return Err(ToolError::CompactionStructural(
                "content is an elided history placeholder; supply the real file content. \
                 Prior turns redact write_file/edit_file inputs to save context; never copy \
                 the placeholder verbatim. Re-emit the full intended content here."
                    .to_string(),
            ));
        }
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
mod tests;
