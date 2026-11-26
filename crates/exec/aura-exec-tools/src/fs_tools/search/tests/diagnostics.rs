use super::*;

#[test]
fn test_search_code_binary_file_skipped() {
    let (sandbox, dir) = create_test_sandbox();

    fs::write(
        dir.path().join("image.png"),
        b"fake png data with let x = 1",
    )
    .unwrap();
    fs::write(dir.path().join("code.rs"), "let x = 1;").unwrap();

    let result = search_code(&sandbox, "let x", None, None, 100, 0, OutputMode::Content).unwrap();
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("code.rs"));
    assert!(!output.contains("image.png"));
}

#[test]
fn test_search_code_nonexistent_path_diagnostic() {
    let (sandbox, _dir) = create_test_sandbox();

    let result = search_code(
        &sandbox,
        "anything",
        Some("no_such_dir"),
        None,
        100,
        0,
        OutputMode::Content,
    );
    assert!(result.is_err());
}

#[test]
fn test_search_code_zero_match_regex_hint() {
    let (sandbox, dir) = create_test_sandbox();

    fs::write(dir.path().join("hint.rs"), "normal code").unwrap();

    let result = search_code(
        &sandbox,
        r"foo\(bar\[baz\]",
        None,
        None,
        100,
        0,
        OutputMode::Content,
    )
    .unwrap();
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("No matches found"));
    assert!(output.contains("regex characters"));
}

#[test]
fn test_search_code_regex_size_limit() {
    let (sandbox, _dir) = create_test_sandbox();

    let huge_pattern = "a".repeat(SEARCH_REGEX_SIZE_LIMIT + 1);
    let result = search_code(
        &sandbox,
        &huge_pattern,
        None,
        None,
        100,
        0,
        OutputMode::Content,
    );
    assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
}

#[test]
fn test_search_code_context_lines_clamped_to_10() {
    let (sandbox, dir) = create_test_sandbox();

    let content = (0..30)
        .map(|i| format!("line_{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.path().join("ctx.txt"), &content).unwrap();

    let result = search_code(
        &sandbox,
        "line_15",
        None,
        None,
        100,
        100,
        OutputMode::Content,
    )
    .unwrap();
    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("line_15"));
}

#[test]
fn test_is_text_file_known_extensions() {
    use std::path::Path;
    assert!(is_text_file(Path::new("main.rs")));
    assert!(is_text_file(Path::new("script.py")));
    assert!(is_text_file(Path::new("config.json")));
    assert!(is_text_file(Path::new("readme.md")));
    assert!(!is_text_file(Path::new("photo.jpg")));
    assert!(!is_text_file(Path::new("binary.exe")));
}
