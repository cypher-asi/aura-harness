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
