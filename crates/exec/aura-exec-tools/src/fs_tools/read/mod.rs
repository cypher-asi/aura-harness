use crate::error::ToolError;
use crate::sandbox::Sandbox;
use crate::tool::{Tool, ToolContext};
use async_trait::async_trait;
use aura_core_types::ToolDefinition;
use aura_core_types::ToolResult;
use std::fs;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::{BufRead, BufReader};
use tracing::{debug, instrument};

/// Compute a short, stable hex digest of the bytes returned to the
/// agent. Stamped onto the `content_hash` metadata key so consumers
/// (the agent loop's exploration-budget tracker, the dashboard's
/// repeated-read warning) can deduplicate identical re-reads without
/// re-comparing full payloads. We use `DefaultHasher` rather than
/// pulling in a crypto-grade hash because the only consumer is
/// "did we already see this exact byte sequence?" — collision risk is
/// acceptable and the implementation has zero new dependencies.
fn content_hash_hex(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// Lines one `read_file` call returns when it names no `end_line`.
pub const DEFAULT_READ_LINES: usize = 2000;

/// Longest line `read_file` shows; longer lines (minified code, data)
/// are cut with a note of how much was left off.
const MAX_LINE_CHARS: usize = 2000;

/// Bytes at the start of a file checked for a NUL to tell binary files
/// from text.
const BINARY_SNIFF_BYTES: usize = 8 * 1024;

/// Bytes of the output budget kept free for the truncation marker.
const MARKER_RESERVE: usize = 160;

/// The marker ending a cut `read_file` result. Starts with
/// `"\n... [truncated"`, which the agent loop's read cache looks for.
fn truncation_marker(start: usize, last: usize, total: usize) -> String {
    format!(
        "\n... [truncated: showing lines {start}-{last} of {total}; continue with start_line={}.]",
        last + 1
    )
}

fn io_error(e: &std::io::Error, what: &str, path: &std::path::Path) -> ToolError {
    ToolError::Io(std::io::Error::new(
        e.kind(),
        format!("{what}({}): {e}", path.display()),
    ))
}

/// `line` without its trailing `\r`, cut to [`MAX_LINE_CHARS`].
fn display_line(raw: &[u8]) -> String {
    let raw = raw.strip_suffix(b"\r").unwrap_or(raw);
    let text = String::from_utf8_lossy(raw);
    if text.len() <= MAX_LINE_CHARS {
        return text.into_owned();
    }
    let mut end = MAX_LINE_CHARS;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}… [line cut, +{} chars]", &text[..end], text.len() - end)
}

/// Read a text file as numbered lines (`N|content`), `start_line` to
/// `end_line` (1-indexed, inclusive).
///
/// Without `end_line` at most [`DEFAULT_READ_LINES`] lines come back, and
/// the output never exceeds `max_bytes`; a cut result ends with a marker
/// naming the `start_line` to continue from. The number column is as wide
/// as the last line number shown. Binary files (a NUL in their first
/// [`BINARY_SNIFF_BYTES`]) are described, not dumped.
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

    let metadata = fs::metadata(&resolved).map_err(|e| io_error(&e, "metadata", &resolved))?;
    let size = usize::try_from(metadata.len()).unwrap_or(usize::MAX);
    let file = fs::File::open(&resolved).map_err(|e| io_error(&e, "open", &resolved))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);

    if starts_binary(&mut reader, &resolved)? {
        return Ok(binary_result(size));
    }

    let start = start_line.unwrap_or(1).max(1);
    let last_wanted = end_line.unwrap_or_else(|| start.saturating_add(DEFAULT_READ_LINES - 1));
    let (window, total) = read_window(&mut reader, start..=last_wanted, &resolved)?;

    if total == 0 {
        return Ok(ToolResult::success("read_file", String::new())
            .with_metadata("size", size.to_string())
            .with_metadata("total_lines", "0")
            .with_metadata("content_hash", content_hash_hex(b"")));
    }
    if start > total {
        return Ok(ToolResult::success(
            "read_file",
            format!("(file has {total} lines, requested start_line={start})"),
        )
        .with_metadata("total_lines", total.to_string()));
    }

    let budget = max_bytes.saturating_sub(MARKER_RESERVE);
    let (mut output, last) = numbered_lines(&window, start, budget);
    let truncated = last < total && last < end_line.unwrap_or(usize::MAX);
    if truncated {
        output.push_str(&truncation_marker(start, last, total));
    }

    let content_hash = content_hash_hex(output.as_bytes());
    let mut result = ToolResult::success("read_file", output)
        .with_metadata("size", size.to_string())
        .with_metadata("total_lines", total.to_string())
        .with_metadata("start_line", start.to_string())
        .with_metadata("end_line", last.to_string())
        .with_metadata("content_hash", content_hash);
    if truncated {
        result = result.with_metadata("truncated", "true");
    }
    Ok(result)
}

/// Whether the first [`BINARY_SNIFF_BYTES`] of the file contain a NUL.
fn starts_binary(
    reader: &mut BufReader<fs::File>,
    path: &std::path::Path,
) -> Result<bool, ToolError> {
    let head = reader.fill_buf().map_err(|e| io_error(&e, "read", path))?;
    Ok(head[..head.len().min(BINARY_SNIFF_BYTES)].contains(&0))
}

/// The description returned instead of a binary file's content.
fn binary_result(size: usize) -> ToolResult {
    let output = format!("(binary file, {size} bytes; not shown)");
    let content_hash = content_hash_hex(output.as_bytes());
    ToolResult::success("read_file", output)
        .with_metadata("size", size.to_string())
        .with_metadata("binary", "true")
        .with_metadata("content_hash", content_hash)
}

/// The display lines numbered within `wanted`, and the file's total line
/// count.
fn read_window(
    reader: &mut BufReader<fs::File>,
    wanted: std::ops::RangeInclusive<usize>,
    path: &std::path::Path,
) -> Result<(Vec<String>, usize), ToolError> {
    let mut window: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| io_error(&e, "read", path))?;
        if n == 0 {
            break;
        }
        total += 1;
        if wanted.contains(&total) {
            let line = buf.strip_suffix(b"\n").unwrap_or(&buf);
            window.push(display_line(line));
        }
    }
    Ok((window, total))
}

/// `window` as `N|line` rows starting at line `start`, as many as fit in
/// `budget` bytes (always at least one), and the number of the last row.
fn numbered_lines(window: &[String], start: usize, budget: usize) -> (String, usize) {
    let width = (start + window.len() - 1).to_string().len();
    let mut output = String::new();
    let mut last = start - 1;
    for (i, line) in window.iter().enumerate() {
        let number = start + i;
        let row = format!("{number:>width$}|{line}");
        if i > 0 && output.len() + 1 + row.len() > budget {
            break;
        }
        if i > 0 {
            output.push('\n');
        }
        output.push_str(&row);
        last = number;
    }
    (output, last)
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
            description: "Read a text file as numbered lines, `N|content`; the number and `|` are not part of the file. Returns up to 2000 lines from start_line (default 1), at most 32 KB; when more remains the result ends with the start_line to continue from. For big files, search first and read only the range you need. Binary files are described, not shown.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to read (relative to workspace root)"
                    },
                    "start_line": {
                        "type": "integer",
                        "description": "First line to return (1-indexed, inclusive). Omit to start from the beginning."
                    },
                    "end_line": {
                        "type": "integer",
                        "description": "Last line to return (1-indexed, inclusive). Omit to read up to 2000 lines."
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
mod tests;
