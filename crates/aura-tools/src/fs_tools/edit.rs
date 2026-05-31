use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::ToolDefinition;
use aura_core_types::ToolResult;
use std::fs;
use std::path::PathBuf;
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

/// Window of file context surrounding a near-miss anchor returned by
/// [`best_partial_anchor`]. The `lines` are pre-formatted as
/// `<lineno>|<text>` so the agent can copy them verbatim into the next
/// `old_text` attempt.
struct AnchorWindow {
    matched_lines: usize,
    anchor_start_line: usize,
    anchor_end_line: usize,
    lines: Vec<String>,
}

/// Find the longest prefix of `needle` (line-wise, trimmed) that has a
/// unique match in `content` and return a ±3-line window around it so
/// the caller can show the model exactly what is in the file at the
/// site it was trying to edit.
///
/// Returns `None` when not even a single needle line matches anywhere
/// in the file — at that point a directional hint would be guesswork.
fn best_partial_anchor(content: &str, needle: &str) -> Option<AnchorWindow> {
    const CONTEXT: usize = 3;
    let needle_lines: Vec<&str> = needle.lines().map(str::trim).collect();
    if needle_lines.is_empty() {
        return None;
    }
    let content_lines: Vec<&str> = content.lines().collect();

    for k in (1..=needle_lines.len()).rev() {
        let prefix = &needle_lines[..k];
        let mut matches: Vec<usize> = Vec::new();
        'outer: for start in 0..content_lines.len() {
            if start + prefix.len() > content_lines.len() {
                break;
            }
            for (i, line) in prefix.iter().enumerate() {
                if content_lines[start + i].trim() != *line {
                    continue 'outer;
                }
            }
            matches.push(start);
            if matches.len() > 1 {
                break;
            }
        }
        if matches.len() == 1 {
            let start = matches[0];
            let end = start + prefix.len() - 1;
            let window_start = start.saturating_sub(CONTEXT);
            let window_end = (end + CONTEXT).min(content_lines.len().saturating_sub(1));
            let lines: Vec<String> = (window_start..=window_end)
                .map(|i| format!("{:>6}|{}", i + 1, content_lines[i]))
                .collect();
            return Some(AnchorWindow {
                matched_lines: k,
                anchor_start_line: start + 1,
                anchor_end_line: end + 1,
                lines,
            });
        }
    }
    None
}

/// Format a "text not found" error with a best-effort anchor and a
/// ±3-line file window so the model can re-derive `old_text` from real
/// bytes instead of guessing again. Falls back to a short message when
/// no needle line matched anywhere in the file.
fn format_not_found_error(content: &str, needle: &str) -> String {
    let needle_total = needle.lines().count();
    match best_partial_anchor(content, needle) {
        Some(window) => {
            let body = window.lines.join("\n");
            format!(
                "The specified text was not found in the file.\n\
                 Closest partial match: {matched} of {total} needle line(s) matched at lines {start}-{end}.\n\
                 ---- file context (\u{00b1}3 lines, line-numbered) ----\n\
                 {body}\n\
                 ----\n\
                 Re-derive old_text from the bytes shown above; do not retry the previous old_text.",
                matched = window.matched_lines,
                total = needle_total,
                start = window.anchor_start_line,
                end = window.anchor_end_line,
            )
        }
        None => format!(
            "The specified text was not found in the file. \
             None of the {needle_total} needle line(s) match any line in the file; \
             read_file the target path first and copy real bytes into old_text."
        ),
    }
}

struct ValidatedEdit {
    resolved: PathBuf,
    content: String,
    had_crlf: bool,
    old_text_norm: String,
    new_text_norm: String,
}

fn is_elided_edit_placeholder(value: &str) -> bool {
    value.starts_with("<<<AURA_ELIDED_OLD::") || value.starts_with("<<<AURA_ELIDED_NEW::")
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

fn reject_elided_edit_placeholder(field: &str, value: &str) -> Result<(), ToolError> {
    if is_elided_edit_placeholder(value) && value.ends_with(">>>") {
        return Err(ToolError::CompactionStructural(format!(
            "{field} is an elided history placeholder; supply the real edit text. \
             Prior turns redact write_file/edit_file inputs to save context; never copy \
             the placeholder verbatim. Re-derive the intended old_text/new_text here."
        )));
    }
    Ok(())
}

fn validate_edit_input(
    sandbox: &Sandbox,
    path: &str,
    old_text: &str,
    new_text: &str,
) -> Result<ValidatedEdit, ToolError> {
    let resolved = sandbox.resolve_existing(path)?;
    debug!(?resolved, "Editing file");

    if !resolved.is_file() {
        return Err(ToolError::InvalidArguments(format!("{path} is not a file")));
    }

    let raw_content = fs::read_to_string(&resolved).map_err(|e| {
        ToolError::Io(std::io::Error::new(
            e.kind(),
            format!("read_to_string({}): {e}", resolved.display()),
        ))
    })?;

    let had_crlf = raw_content.contains("\r\n");
    let content = if had_crlf {
        raw_content.replace("\r\n", "\n")
    } else {
        raw_content
    };

    Ok(ValidatedEdit {
        resolved,
        content,
        had_crlf,
        old_text_norm: old_text.replace("\r\n", "\n"),
        new_text_norm: new_text.replace("\r\n", "\n"),
    })
}

fn find_match_in_content(
    content: &str,
    old_text_norm: &str,
    new_text_norm: &str,
    replace_all: bool,
) -> Result<(String, usize), ToolError> {
    let exact_count = content.matches(old_text_norm).count();

    if exact_count == 0 {
        match fuzzy_line_match(content, old_text_norm) {
            Ok(Some((start, end))) => {
                let mut buf = String::with_capacity(content.len());
                buf.push_str(&content[..start]);
                buf.push_str(new_text_norm);
                buf.push_str(&content[end..]);
                Ok((buf, 1))
            }
            Ok(None) => Err(ToolError::InvalidArguments(format_not_found_error(
                content,
                old_text_norm,
            ))),
            Err(msg) => Err(ToolError::InvalidArguments(msg)),
        }
    } else if !replace_all && exact_count > 1 {
        Err(ToolError::InvalidArguments(format!(
            "Found {exact_count} occurrences of the search text. \
             Use replace_all=true to replace all, or make the search text more specific."
        )))
    } else if replace_all {
        Ok((content.replace(old_text_norm, new_text_norm), exact_count))
    } else {
        Ok((content.replacen(old_text_norm, new_text_norm, 1), 1))
    }
}

fn apply_edit(
    resolved: &PathBuf,
    path: &str,
    content: &str,
    new_content: String,
    had_crlf: bool,
    replacements: usize,
) -> Result<ToolResult, ToolError> {
    if !content.is_empty() && new_content.len() < content.len() / 5 {
        return Err(ToolError::InvalidArguments(
            "Edit would reduce file to less than 20% of original size. \
             This likely indicates truncated content."
                .to_string(),
        ));
    }

    // Structural guard for Cargo manifests. Mirrors the pre-write
    // check in `fs_write`: if the edit would leave the manifest
    // unparseable or with duplicate dependency keys, refuse it before
    // touching the filesystem so the existing on-disk content is
    // preserved for the agent to inspect / retry.
    if super::cargo_toml_guard::is_cargo_manifest(resolved) {
        super::cargo_toml_guard::validate_cargo_toml(&new_content)?;
    }

    // Compute the file-level line diff before we move new_content into
    // the CRLF-restoration branch below. Working off the LF-normalised
    // pre/post pair (rather than the raw old_text/new_text inputs) gives
    // accurate counts even when replace_all=true expands across many
    // sites: the harness sees the actual net effect on the file.
    let (lines_added, lines_removed) = super::diff::count_line_diff(content, &new_content);

    let final_content = if had_crlf {
        new_content.replace('\n', "\r\n")
    } else {
        new_content
    };

    fs::write(resolved, &final_content).map_err(|e| {
        ToolError::Io(std::io::Error::new(
            e.kind(),
            format!("write({}): {e}", resolved.display()),
        ))
    })?;

    Ok(ToolResult::success(
        "edit_file",
        format!("Replaced {replacements} occurrence(s) in {path}"),
    )
    .with_metadata("replacements", replacements.to_string())
    .with_line_diff(lines_added, lines_removed))
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
    reject_elided_edit_placeholder("old_text", old_text)?;
    reject_elided_edit_placeholder("new_text", new_text)?;

    let edit = validate_edit_input(sandbox, path, old_text, new_text)?;
    let (new_content, replacements) = find_match_in_content(
        &edit.content,
        &edit.old_text_norm,
        &edit.new_text_norm,
        replace_all,
    )?;
    apply_edit(
        &edit.resolved,
        path,
        &edit.content,
        new_content,
        edit.had_crlf,
        replacements,
    )
}

/// `fs_edit` tool: edit a file by replacing text.
pub struct FsEditTool;

#[async_trait]
impl Tool for FsEditTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "edit_file".into(),
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
            // Stream the `old_text` / `new_text` strings live as the model
            // writes them so the UI's diff preview fills in character-by-
            // character instead of waiting for the full tool-use block.
            eager_input_streaming: Some(true),
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        for field in ["old_text", "new_text"] {
            if has_redacted_field_marker(&args, field) {
                return Err(ToolError::CompactionStructural(format!(
                    "{field} is an elided history placeholder; supply the real edit text. \
                     Prior turns redact write_file/edit_file inputs to save context; never copy \
                     the placeholder verbatim. Re-derive the intended old_text/new_text here."
                )));
            }
        }
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
        super::spawn_blocking_tool(move || {
            fs_edit(&sandbox, &path, &old_text, &new_text, replace_all)
        })
        .await
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
