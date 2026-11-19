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

    let head = reader
        .fill_buf()
        .map_err(|e| io_error(&e, "read", &resolved))?;
    if head[..head.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
        let output = format!("(binary file, {size} bytes; not shown)");
        let content_hash = content_hash_hex(output.as_bytes());
        return Ok(ToolResult::success("read_file", output)
            .with_metadata("size", size.to_string())
            .with_metadata("binary", "true")
            .with_metadata("content_hash", content_hash));
    }

    let start = start_line.unwrap_or(1).max(1);
    let last_wanted = end_line.unwrap_or_else(|| start.saturating_add(DEFAULT_READ_LINES - 1));
    let mut window: Vec<String> = Vec::new();
    let mut total = 0usize;
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| io_error(&e, "read", &resolved))?;
        if n == 0 {
            break;
        }
        total += 1;
        if (start..=last_wanted).contains(&total) {
            let line = buf.strip_suffix(b"\n").unwrap_or(&buf);
            window.push(display_line(line));
        }
    }

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
        assert_eq!(&result.stdout[..], b"1|Hello, Aura!");
        assert!(
            result.metadata.contains_key("content_hash"),
            "content_hash must be stamped on every read so dedup tracking works"
        );
    }

    #[test]
    fn fs_read_content_hash_is_deterministic_across_reads() {
        let (sandbox, dir) = create_test_sandbox();
        let content = "stable bytes";
        fs::write(dir.path().join("stable.txt"), content).unwrap();
        let a = fs_read(&sandbox, "stable.txt", 1024, None, None).unwrap();
        let b = fs_read(&sandbox, "stable.txt", 1024, None, None).unwrap();
        assert_eq!(
            a.metadata.get("content_hash"),
            b.metadata.get("content_hash"),
            "identical bytes must produce identical content_hash"
        );
    }

    #[test]
    fn fs_read_content_hash_changes_when_bytes_change() {
        let (sandbox, dir) = create_test_sandbox();
        fs::write(dir.path().join("mut.txt"), "before").unwrap();
        let a = fs_read(&sandbox, "mut.txt", 1024, None, None).unwrap();
        fs::write(dir.path().join("mut.txt"), "after").unwrap();
        let b = fs_read(&sandbox, "mut.txt", 1024, None, None).unwrap();
        assert_ne!(
            a.metadata.get("content_hash"),
            b.metadata.get("content_hash"),
            "different bytes must yield different content_hash"
        );
    }

    #[test]
    fn fs_read_line_range_also_emits_content_hash() {
        let (sandbox, dir) = create_test_sandbox();
        fs::write(dir.path().join("multi.txt"), "a\nb\nc\nd\n").unwrap();
        let result = fs_read(&sandbox, "multi.txt", 1024, Some(2), Some(3)).unwrap();
        assert!(result.metadata.contains_key("content_hash"));
    }

    fn numbered(n: usize) -> String {
        (1..=n).map(|i| format!("line {i}\n")).collect()
    }

    #[test]
    fn test_fs_read_byte_cap_stops_at_a_whole_line() {
        let (sandbox, dir) = create_test_sandbox();
        fs::write(dir.path().join("lines.txt"), numbered(500)).unwrap();

        let max_bytes = 1024usize;
        let result = fs_read(&sandbox, "lines.txt", max_bytes, None, None).unwrap();
        assert!(result.ok, "truncation must surface as a successful result");
        let body = String::from_utf8(result.stdout.to_vec()).unwrap();
        assert!(body.len() <= max_bytes, "{} > {max_bytes}", body.len());
        let (kept, marker) = body.split_once("\n... [truncated").unwrap();
        let last = kept.lines().last().unwrap();
        let last_number: usize = last.split_once('|').unwrap().0.trim().parse().unwrap();
        assert_eq!(last, format!("{last_number:>3}|line {last_number}"));
        assert!(
            marker.contains(&format!("continue with start_line={}", last_number + 1)),
            "{marker}"
        );
        assert_eq!(
            result.metadata.get("truncated").map(String::as_str),
            Some("true")
        );
        assert_eq!(
            result.metadata.get("total_lines").map(String::as_str),
            Some("500")
        );
        assert_eq!(
            result.metadata.get("end_line").map(String::as_str),
            Some(last_number.to_string().as_str())
        );
    }

    #[test]
    fn test_fs_read_returns_at_most_default_lines_without_end_line() {
        let (sandbox, dir) = create_test_sandbox();
        fs::write(
            dir.path().join("long.txt"),
            numbered(DEFAULT_READ_LINES + 50),
        )
        .unwrap();

        let result = fs_read(&sandbox, "long.txt", usize::MAX, None, None).unwrap();
        let body = String::from_utf8(result.stdout.to_vec()).unwrap();
        assert!(body.contains(&format!("{DEFAULT_READ_LINES}|line {DEFAULT_READ_LINES}")));
        assert!(!body.contains(&format!("|line {}", DEFAULT_READ_LINES + 1)));
        assert!(body.ends_with(&format!(
            "showing lines 1-{DEFAULT_READ_LINES} of {}; continue with start_line={}.]",
            DEFAULT_READ_LINES + 50,
            DEFAULT_READ_LINES + 1
        )));
    }

    #[test]
    fn test_fs_read_explicit_range_is_not_marked_truncated() {
        let (sandbox, dir) = create_test_sandbox();
        fs::write(dir.path().join("lines.txt"), numbered(100)).unwrap();

        let result = fs_read(&sandbox, "lines.txt", 32 * 1024, Some(10), Some(12)).unwrap();
        let body = String::from_utf8(result.stdout.to_vec()).unwrap();
        assert_eq!(body, "10|line 10\n11|line 11\n12|line 12");
        assert_eq!(result.metadata.get("truncated"), None);
    }

    #[test]
    fn test_fs_read_cuts_very_long_lines() {
        let (sandbox, dir) = create_test_sandbox();
        let minified = "x".repeat(MAX_LINE_CHARS + 500);
        fs::write(dir.path().join("min.js"), format!("{minified}\nnext\n")).unwrap();

        let result = fs_read(&sandbox, "min.js", 32 * 1024, None, None).unwrap();
        let body = String::from_utf8(result.stdout.to_vec()).unwrap();
        assert!(body.contains("[line cut, +500 chars]"), "{body}");
        assert!(body.ends_with("2|next"), "{body}");
    }

    #[test]
    fn test_fs_read_strips_carriage_returns() {
        let (sandbox, dir) = create_test_sandbox();
        fs::write(dir.path().join("crlf.txt"), "a\r\nb\r\n").unwrap();

        let result = fs_read(&sandbox, "crlf.txt", 1024, None, None).unwrap();
        assert_eq!(&result.stdout[..], b"1|a\n2|b");
    }

    #[test]
    fn test_fs_read_binary_content() {
        let (sandbox, dir) = create_test_sandbox();

        let content = vec![0u8, 1, 2, 255, 254, 253];
        fs::write(dir.path().join("binary.bin"), &content).unwrap();

        let result = fs_read(&sandbox, "binary.bin", 1024, None, None).unwrap();
        assert!(result.ok);
        assert_eq!(&result.stdout[..], b"(binary file, 6 bytes; not shown)");
        assert_eq!(
            result.metadata.get("binary").map(String::as_str),
            Some("true")
        );
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
