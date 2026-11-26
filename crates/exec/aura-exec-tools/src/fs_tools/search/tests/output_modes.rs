use super::*;

// Repro for the silent-truncation bug: when results hit `max_results`, the
// caller is given no signal that matches were dropped, so a capped result is
// treated as exhaustive (false-negatives). A capped search MUST flag itself.
#[test]
fn test_search_code_signals_truncation_when_results_capped() {
    let (sandbox, dir) = create_test_sandbox();

    // 20 matching lines, cap at 5 -> 15 matches silently dropped.
    let content = (0..20)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.path().join("many.txt"), content).unwrap();

    let result = search_code(&sandbox, "line", None, None, 5, 0, OutputMode::Content).unwrap();
    assert!(result.ok);
    // The cap is enforced (existing behavior)...
    assert_eq!(result.metadata.get("match_count").unwrap(), "5");
    // ...but the caller MUST be told results were dropped.
    assert_eq!(
        result.metadata.get("truncated").map(String::as_str),
        Some("true"),
        "capped search must set truncated=\"true\" metadata"
    );
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(
        output.to_lowercase().contains("truncat"),
        "capped search output must include a truncation marker; got:\n{output}"
    );
}

// Guard against a spurious marker: a search that does NOT hit the cap must
// not claim truncation.
#[test]
fn test_search_code_no_truncation_signal_when_under_cap() {
    let (sandbox, dir) = create_test_sandbox();
    fs::write(dir.path().join("few.txt"), "line0\nline1\nline2").unwrap();

    let result = search_code(&sandbox, "line", None, None, 100, 0, OutputMode::Content).unwrap();
    assert!(result.ok);
    assert_eq!(result.metadata.get("match_count").unwrap(), "3");
    assert_eq!(result.metadata.get("truncated"), None);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(!output.to_lowercase().contains("truncat"));
}

// search_code must not silently skip source files whose extension isn't on a
// hardcoded text allowlist — the old behavior dropped .tsx/.jsx/.vue/.scss etc.,
// which are most of a typical web codebase (and caused real false-negatives).
#[test]
fn test_search_code_finds_unlisted_text_extensions() {
    let (sandbox, dir) = create_test_sandbox();
    fs::write(dir.path().join("Component.tsx"), "const a = needle;").unwrap();
    fs::write(dir.path().join("widget.jsx"), "const b = needle;").unwrap();
    fs::write(dir.path().join("styles.scss"), "/* needle */").unwrap();
    fs::write(dir.path().join("App.vue"), "<!-- needle -->").unwrap();

    let result = search_code(&sandbox, "needle", None, None, 100, 0, OutputMode::Content).unwrap();
    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("Component.tsx"), "missed .tsx:\n{output}");
    assert!(output.contains("widget.jsx"), "missed .jsx:\n{output}");
    assert!(output.contains("styles.scss"), "missed .scss:\n{output}");
    assert!(output.contains("App.vue"), "missed .vue:\n{output}");
}

// Binary files must still be skipped (denylist + the UTF-8 read backstop).
#[test]
fn test_search_code_still_skips_binary_files() {
    let (sandbox, dir) = create_test_sandbox();
    fs::write(dir.path().join("img.png"), "needle in png").unwrap();
    fs::write(dir.path().join("font.woff2"), "needle in font").unwrap();
    fs::write(dir.path().join("real.ts"), "const c = needle;").unwrap();

    let result = search_code(&sandbox, "needle", None, None, 100, 0, OutputMode::Content).unwrap();
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("real.ts"));
    assert!(!output.contains("img.png"), "should skip .png");
    assert!(!output.contains("font.woff2"), "should skip .woff2");
}

// Classification is denylist-based and case-insensitive.
#[test]
fn test_is_text_file_denylist_and_case_insensitive() {
    use std::path::Path;
    assert!(is_text_file(Path::new("Component.tsx")));
    assert!(is_text_file(Path::new("widget.jsx")));
    assert!(is_text_file(Path::new("Main.RS"))); // uppercase source still text
    assert!(!is_text_file(Path::new("photo.PNG"))); // uppercase binary still skipped
    assert!(!is_text_file(Path::new("lib.dylib")));
}

#[test]
fn test_search_code_respects_gitignore() {
    let (sandbox, dir) = create_test_sandbox();
    fs::write(dir.path().join(".gitignore"), "generated/\n").unwrap();
    fs::create_dir_all(dir.path().join("generated")).unwrap();
    fs::write(dir.path().join("generated/out.rs"), "let needle = 1;").unwrap();
    fs::write(dir.path().join("src.rs"), "let needle = 2;").unwrap();

    let result = search_code(&sandbox, "needle", None, None, 100, 0, OutputMode::Content).unwrap();
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("src.rs"), "{output}");
    assert!(!output.contains("generated"), "{output}");
}

#[test]
fn test_search_code_files_with_matches_lists_each_file_once() {
    let (sandbox, dir) = create_test_sandbox();
    fs::write(dir.path().join("a.rs"), "needle\nneedle\nneedle").unwrap();
    fs::write(dir.path().join("b.rs"), "needle").unwrap();
    fs::write(dir.path().join("c.rs"), "nothing").unwrap();

    let result = search_code(
        &sandbox,
        "needle",
        None,
        None,
        100,
        0,
        OutputMode::FilesWithMatches,
    )
    .unwrap();
    let output = String::from_utf8_lossy(&result.stdout);
    assert_eq!(output, "a.rs\nb.rs");
    assert_eq!(result.metadata.get("file_count").unwrap(), "2");
}

#[test]
fn test_search_code_count_mode_reports_matches_per_file() {
    let (sandbox, dir) = create_test_sandbox();
    fs::write(dir.path().join("a.rs"), "needle\nneedle\nneedle").unwrap();
    fs::write(dir.path().join("b.rs"), "needle").unwrap();

    let result = search_code(&sandbox, "needle", None, None, 100, 0, OutputMode::Count).unwrap();
    let output = String::from_utf8_lossy(&result.stdout);
    assert_eq!(output, "a.rs:3\nb.rs:1");
    assert_eq!(result.metadata.get("match_count").unwrap(), "4");
}

#[test]
fn test_search_code_cuts_long_lines() {
    let (sandbox, dir) = create_test_sandbox();
    let minified = format!("needle{}", "x".repeat(10_000));
    fs::write(dir.path().join("bundle.js"), &minified).unwrap();

    let result = search_code(&sandbox, "needle", None, None, 100, 0, OutputMode::Content).unwrap();
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(
        output.len() < 500,
        "line must be cut, got {} bytes",
        output.len()
    );
    assert!(output.contains("[+"), "{output}");
}

#[test]
fn test_search_code_max_results_has_a_ceiling() {
    let (sandbox, dir) = create_test_sandbox();
    let content = (0..1_000)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.path().join("many.txt"), content).unwrap();

    let result = search_code(
        &sandbox,
        "line",
        None,
        None,
        100_000,
        0,
        OutputMode::Content,
    )
    .unwrap();
    assert_eq!(
        result.metadata.get("match_count").unwrap(),
        &MAX_RESULTS_CEILING.to_string()
    );
    assert_eq!(
        result.metadata.get("truncated").map(String::as_str),
        Some("true")
    );
}

#[test]
fn test_output_mode_parse_rejects_unknown_modes() {
    assert_eq!(OutputMode::parse(None).unwrap(), OutputMode::Content);
    assert_eq!(
        OutputMode::parse(Some("files_with_matches")).unwrap(),
        OutputMode::FilesWithMatches
    );
    assert!(matches!(
        OutputMode::parse(Some("lines")),
        Err(ToolError::InvalidArguments(_))
    ));
}
