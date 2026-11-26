
use super::*;
use tempfile::TempDir;

fn create_test_sandbox() -> (Sandbox, TempDir) {
    let dir = TempDir::new().unwrap();
    let sandbox = Sandbox::new(dir.path()).unwrap();
    (sandbox, dir)
}

#[test]
fn test_fs_write_new_file() {
    let (sandbox, dir) = create_test_sandbox();

    let result = fs_write(&sandbox, "new.txt", "Hello, world!", false).unwrap();
    assert!(result.ok);

    let content = fs::read_to_string(dir.path().join("new.txt")).unwrap();
    assert_eq!(content, "Hello, world!");
}

#[test]
fn test_fs_write_rejects_elided_history_placeholder() {
    let (sandbox, dir) = create_test_sandbox();

    let result = fs_write(
        &sandbox,
        "placeholder.txt",
        "<<<AURA_ELIDED_CONTENT::42_bytes>>>",
        false,
    );

    assert!(matches!(result, Err(ToolError::CompactionStructural(_))));
    if let Err(ToolError::CompactionStructural(msg)) = result {
        assert!(msg.contains("elided history placeholder"));
        assert!(msg.contains("supply the real file content"));
    }
    assert!(!dir.path().join("placeholder.txt").exists());
}

#[test]
fn test_write_detector_rejects_structured_redaction_marker() {
    let args = serde_json::json!({
        "path": "placeholder.txt",
        "_redacted": {
            "kind": "aura_compaction_redaction",
            "version": 1,
            "field": "content",
            "bytes": 42
        }
    });

    assert!(has_redacted_field_marker(&args, "content"));
}

#[tokio::test]
async fn test_fs_write_redaction_marker_error_is_structural() {
    let (sandbox, _dir) = create_test_sandbox();
    let ctx = ToolContext::new(sandbox, crate::ToolConfig::default());
    let args = serde_json::json!({
        "path": "placeholder.txt",
        "_redacted": {
            "kind": "aura_compaction_redaction",
            "version": 1,
            "field": "content",
            "bytes": 42
        }
    });

    let result = FsWriteTool.execute(&ctx, args).await;

    assert!(matches!(result, Err(ToolError::CompactionStructural(_))));
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
fn test_fs_write_creates_parent_dirs_by_default() {
    let (sandbox, dir) = create_test_sandbox();

    // Even with create_dirs=false, parent dirs are now always created
    let result = fs_write(&sandbox, "auto/created/file.txt", "content", false).unwrap();
    assert!(result.ok);
    assert!(dir.path().join("auto/created/file.txt").exists());
}

#[test]
fn test_fs_write_truncation_heuristic_rejects_small() {
    let (sandbox, dir) = create_test_sandbox();

    // Write a large file first
    let large = "x".repeat(1000);
    fs::write(dir.path().join("big.txt"), &large).unwrap();

    // Attempt to overwrite with tiny content (< 10%)
    let result = fs_write(&sandbox, "big.txt", "tiny", false);
    assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
    if let Err(ToolError::InvalidArguments(msg)) = result {
        assert!(msg.contains("10%"));
    }
}

#[test]
fn test_fs_write_post_write_verification() {
    let (sandbox, _dir) = create_test_sandbox();

    let content = "verified content";
    let result = fs_write(&sandbox, "verify.txt", content, false).unwrap();
    assert!(result.ok);
    assert_eq!(
        result.metadata.get("bytes_written").unwrap(),
        &content.len().to_string()
    );
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

#[test]
fn test_fs_write_large_file_over_1mb() {
    let (sandbox, dir) = create_test_sandbox();

    let content = "x".repeat(1_100_000);
    let result = fs_write(&sandbox, "large.bin", &content, false).unwrap();
    assert!(result.ok);

    let on_disk = fs::read_to_string(dir.path().join("large.bin")).unwrap();
    assert_eq!(on_disk.len(), 1_100_000);
}

#[test]
fn test_fs_write_special_chars_in_filename() {
    let (sandbox, dir) = create_test_sandbox();

    let result = fs_write(&sandbox, "file with spaces.txt", "data", false).unwrap();
    assert!(result.ok);
    assert!(dir.path().join("file with spaces.txt").exists());
}

#[test]
fn test_fs_write_empty_content() {
    let (sandbox, dir) = create_test_sandbox();

    let result = fs_write(&sandbox, "empty.txt", "", false).unwrap();
    assert!(result.ok);

    let content = fs::read_to_string(dir.path().join("empty.txt")).unwrap();
    assert!(content.is_empty());
}

#[test]
fn test_fs_write_overwrite_with_exactly_10_percent() {
    let (sandbox, dir) = create_test_sandbox();

    let large = "x".repeat(1000);
    fs::write(dir.path().join("boundary.txt"), &large).unwrap();

    // 100 bytes = exactly 10% of 1000 — should be accepted
    let small = "y".repeat(100);
    let result = fs_write(&sandbox, "boundary.txt", &small, false).unwrap();
    assert!(result.ok);
}

#[test]
fn test_fs_write_overwrite_with_just_under_10_percent() {
    let (sandbox, dir) = create_test_sandbox();

    let large = "x".repeat(1000);
    fs::write(dir.path().join("boundary2.txt"), &large).unwrap();

    // 99 bytes = just under 10% of 1000 — should be rejected
    let small = "y".repeat(99);
    let result = fs_write(&sandbox, "boundary2.txt", &small, false);
    assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
}

#[test]
fn test_fs_write_truncation_warning_unbalanced_braces_in_rust() {
    let (sandbox, _dir) = create_test_sandbox();

    // `.rs` is code-like — unbalanced braces should trip the
    // warning.
    let content = "fn main() { if true {";
    let result = fs_write(&sandbox, "warn.rs", content, false).unwrap();
    assert!(result.ok);
    assert!(result.metadata.contains_key("warning"));
}

#[test]
fn test_fs_write_no_truncation_warning_balanced() {
    let (sandbox, _dir) = create_test_sandbox();

    let content = "fn main() { println!(); }";
    let result = fs_write(&sandbox, "ok.rs", content, false).unwrap();
    assert!(result.ok);
    assert!(!result.metadata.contains_key("warning"));
}

#[test]
fn test_fs_write_no_truncation_warning_on_markdown() {
    // Markdown routinely has unbalanced braces inside string
    // literals, code fences, or prose (e.g. `Vec<T> { ... }` in a
    // doc). The heuristic must not fire for `.md` files.
    let (sandbox, _dir) = create_test_sandbox();

    let content = "# Note\n\nThe struct is shaped like `Foo { a, b`.\n";
    let result = fs_write(&sandbox, "notes.md", content, false).unwrap();
    assert!(result.ok);
    assert!(
        !result.metadata.contains_key("warning"),
        "markdown must not trip the truncation heuristic"
    );
}

#[test]
fn test_fs_write_no_truncation_warning_on_txt() {
    // Plain-text files are not code — suppress warnings even if
    // braces happen to be unbalanced.
    let (sandbox, _dir) = create_test_sandbox();

    let content = "todo: check the { state";
    let result = fs_write(&sandbox, "notes.txt", content, false).unwrap();
    assert!(result.ok);
    assert!(!result.metadata.contains_key("warning"));
}

#[test]
fn test_fs_write_file_existed_metadata() {
    let (sandbox, dir) = create_test_sandbox();

    let result = fs_write(&sandbox, "new_file.txt", "initial", false).unwrap();
    assert_eq!(result.metadata.get("file_existed").unwrap(), "false");

    fs::write(dir.path().join("old_file.txt"), "original").unwrap();
    let result = fs_write(&sandbox, "old_file.txt", "replaced!", false).unwrap();
    assert_eq!(result.metadata.get("file_existed").unwrap(), "true");
}

#[test]
fn test_fs_write_deeply_nested_path() {
    let (sandbox, dir) = create_test_sandbox();

    let result = fs_write(&sandbox, "a/b/c/d/e/f/deep.txt", "deep", true).unwrap();
    assert!(result.ok);
    assert!(dir.path().join("a/b/c/d/e/f/deep.txt").exists());
}

#[test]
fn fs_write_metadata_chunk_suggestion_when_over_threshold() {
    let (sandbox, _dir) = create_test_sandbox();

    let big = "x".repeat(13_000);
    let result = fs_write(&sandbox, "big_chunk.txt", &big, false).unwrap();
    assert!(result.ok);
    assert_eq!(
        result.metadata.get("chunk_suggestion").map(String::as_str),
        Some("true"),
        "chunk_suggestion metadata should be set when content exceeds WRITE_FILE_CHUNK_BYTES"
    );

    let small = "y".repeat(2_000);
    let result = fs_write(&sandbox, "small_chunk.txt", &small, false).unwrap();
    assert!(result.ok);
    assert!(
        !result.metadata.contains_key("chunk_suggestion"),
        "chunk_suggestion metadata should be absent for content under WRITE_FILE_CHUNK_BYTES"
    );
}

// ====================================================================
// line_diff coverage — verifies fs_write attaches accurate line counts
// for both the create-from-scratch and overwrite flows so the agent
// loop can populate FileChange.lines_added/_removed without re-reading
// the filesystem.
// ====================================================================

#[test]
fn fs_write_create_reports_added_lines_only() {
    let (sandbox, _dir) = create_test_sandbox();
    let result = fs_write(&sandbox, "new.rs", "fn a() {}\nfn b() {}\n", false).unwrap();
    let line_diff = result.line_diff.expect("create should report a line diff");
    assert_eq!(line_diff.lines_added, 2);
    assert_eq!(line_diff.lines_removed, 0);
}

#[test]
fn fs_write_overwrite_reports_replacement_diff() {
    let (sandbox, dir) = create_test_sandbox();
    // Pre-content: 3 lines.
    fs::write(dir.path().join("lib.rs"), "old1\nold2\nold3\n").unwrap();
    // Overwrite with 2 different lines plus 1 unchanged tail —
    // 2 inserts, 2 deletes (same-position lines that differ become
    // a paired insert+delete in similar's line model).
    let result = fs_write(&sandbox, "lib.rs", "new1\nnew2\nold3\n", false).unwrap();
    let line_diff = result
        .line_diff
        .expect("overwrite should report a line diff");
    assert_eq!(line_diff.lines_added, 2);
    assert_eq!(line_diff.lines_removed, 2);
}

#[test]
fn fs_write_rejects_unparseable_cargo_toml() {
    let (sandbox, dir) = create_test_sandbox();
    fs::create_dir(dir.path().join("crate")).unwrap();
    let broken = "[dependencies\nserde = \"1\"\n";
    let result = fs_write(&sandbox, "crate/Cargo.toml", broken, false);
    let Err(ToolError::InvalidArguments(msg)) = result else {
        panic!("expected InvalidArguments");
    };
    assert!(msg.contains("does not parse as TOML"));
    assert!(
        !dir.path().join("crate/Cargo.toml").exists(),
        "broken manifest must not land on disk"
    );
}

#[test]
fn fs_write_rejects_cargo_toml_with_duplicate_dependency() {
    // toml's strict parser is the first line of defense; the manual
    // scanner's diagnostic is the second. We use a manifest the
    // parser rejects to keep the test stable across `toml` minor
    // versions and assert that the user-facing message points at the
    // duplicate.
    let (sandbox, _dir) = create_test_sandbox();
    let manifest = "[package]\nname = \"x\"\nversion = \"0.1.0\"\n\
                        [dependencies]\nserde = \"1\"\nserde = \"2\"\n";
    let result = fs_write(&sandbox, "Cargo.toml", manifest, false);
    let Err(ToolError::InvalidArguments(msg)) = result else {
        panic!("expected InvalidArguments");
    };
    assert!(
        msg.contains("does not parse as TOML") || msg.contains("declares the same dependency"),
        "expected duplicate-key diagnostic, got: {msg}"
    );
}

#[test]
fn fs_write_accepts_clean_cargo_toml() {
    let (sandbox, _dir) = create_test_sandbox();
    let manifest = "[package]\nname = \"x\"\nversion = \"0.1.0\"\n\
                        [dependencies]\nserde = \"1\"\n";
    let result = fs_write(&sandbox, "Cargo.toml", manifest, false).unwrap();
    assert!(result.ok);
}

#[test]
fn fs_write_flags_unlinked_new_module() {
    let (sandbox, dir) = create_test_sandbox();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub fn ok() {}\n", // no `mod hpke_hybrid;`
    )
    .unwrap();
    let result = fs_write(&sandbox, "src/hpke_hybrid.rs", "pub fn run() {}\n", false).unwrap();
    assert_eq!(
        result.metadata.get("unlinked_module").map(String::as_str),
        Some("true"),
        "unlinked_module flag must be set"
    );
    let hint = result
        .metadata
        .get("unlinked_module_hint")
        .expect("hint present");
    assert!(hint.contains("mod hpke_hybrid"), "hint = {hint}");
    assert!(hint.contains("src/lib.rs"), "hint = {hint}");
}

#[test]
fn fs_write_does_not_flag_when_sibling_declares_module() {
    let (sandbox, dir) = create_test_sandbox();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(
        dir.path().join("src/lib.rs"),
        "pub mod hpke_hybrid;\npub fn ok() {}\n",
    )
    .unwrap();
    let result = fs_write(&sandbox, "src/hpke_hybrid.rs", "pub fn run() {}\n", false).unwrap();
    assert!(
        !result.metadata.contains_key("unlinked_module"),
        "metadata = {:?}",
        result.metadata
    );
}

#[test]
fn fs_write_does_not_flag_overwrites() {
    let (sandbox, dir) = create_test_sandbox();
    fs::create_dir_all(dir.path().join("src")).unwrap();
    fs::write(dir.path().join("src/lib.rs"), "pub fn ok() {}\n").unwrap();
    // Pre-existing file: write the initial version, THEN overwrite.
    // The overwrite path should NOT re-flag the module-link gap; it
    // only fires for fresh creations to avoid spamming on every edit.
    let _ = fs_write(&sandbox, "src/hpke_hybrid.rs", "pub fn a() {}\n", false).unwrap();
    let result = fs_write(&sandbox, "src/hpke_hybrid.rs", "pub fn b() {}\nx\n", false).unwrap();
    assert!(
        !result.metadata.contains_key("unlinked_module"),
        "overwrite must not re-emit the warning"
    );
}

#[test]
fn fs_write_overwrite_with_identical_content_reports_zero_diff() {
    let (sandbox, dir) = create_test_sandbox();
    fs::write(dir.path().join("noop.rs"), "same\nsame\n").unwrap();
    let result = fs_write(&sandbox, "noop.rs", "same\nsame\n", false).unwrap();
    let line_diff = result
        .line_diff
        .expect("identical overwrite still reports a diff (just zero)");
    assert_eq!(line_diff.lines_added, 0);
    assert_eq!(line_diff.lines_removed, 0);
}
