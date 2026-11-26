use super::*;

#[test]
fn test_search_code_max_results() {
    let (sandbox, dir) = create_test_sandbox();

    let content = (0..20)
        .map(|i| format!("line{i}"))
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(dir.path().join("many.txt"), content).unwrap();

    let result = search_code(&sandbox, "line", None, None, 5, 0, OutputMode::Content).unwrap();
    assert!(result.ok);
    assert_eq!(result.metadata.get("match_count").unwrap(), "5");
}

#[test]
fn test_search_code_in_subdirectory() {
    let (sandbox, dir) = create_test_sandbox();

    fs::create_dir_all(dir.path().join("src/nested")).unwrap();
    fs::write(dir.path().join("src/nested/code.rs"), "fn nested_fn() {}").unwrap();

    let result = search_code(
        &sandbox,
        "nested_fn",
        Some("src"),
        None,
        100,
        0,
        OutputMode::Content,
    )
    .unwrap();
    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("nested_fn"));
}

#[test]
fn test_search_code_invalid_regex() {
    let (sandbox, _dir) = create_test_sandbox();

    let result = search_code(
        &sandbox,
        "[invalid(regex",
        None,
        None,
        100,
        0,
        OutputMode::Content,
    );
    assert!(matches!(result, Err(ToolError::InvalidArguments(_))));
}

#[test]
fn test_search_code_context_lines() {
    let (sandbox, dir) = create_test_sandbox();

    let content = "alpha\nbeta\ngamma\ndelta\nepsilon\n";
    fs::write(dir.path().join("ctx.txt"), content).unwrap();

    let result = search_code(&sandbox, "gamma", None, None, 100, 1, OutputMode::Content).unwrap();
    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("beta"));
    assert!(output.contains("gamma"));
    assert!(output.contains("delta"));
    assert!(output.contains(">"));
}

#[test]
fn test_search_code_skip_dirs() {
    let (sandbox, dir) = create_test_sandbox();

    fs::create_dir_all(dir.path().join("node_modules")).unwrap();
    fs::write(dir.path().join("node_modules/dep.js"), "let hidden = true;").unwrap();
    fs::create_dir_all(dir.path().join("target")).unwrap();
    fs::write(dir.path().join("target/out.rs"), "let hidden = true;").unwrap();
    fs::write(dir.path().join("visible.rs"), "let visible = true;").unwrap();

    let result = search_code(&sandbox, "let", None, None, 100, 0, OutputMode::Content).unwrap();
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("visible"));
    assert!(!output.contains("hidden"));
}

#[test]
fn test_search_code_complex_regex_lookahead_character_class() {
    let (sandbox, dir) = create_test_sandbox();

    fs::write(
        dir.path().join("complex.rs"),
        "fn foo_bar() {}\nfn baz123() {}\nfn _private() {}\n",
    )
    .unwrap();

    let result = search_code(
        &sandbox,
        r"fn [a-z_]+\d*\(\)",
        None,
        None,
        100,
        0,
        OutputMode::Content,
    )
    .unwrap();
    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("baz123"));
}

#[test]
fn test_search_code_alternation_regex() {
    let (sandbox, dir) = create_test_sandbox();

    fs::write(
        dir.path().join("alt.rs"),
        "let alpha = 1;\nlet beta = 2;\nlet gamma = 3;\n",
    )
    .unwrap();

    let result = search_code(
        &sandbox,
        r"alpha|gamma",
        None,
        None,
        100,
        0,
        OutputMode::Content,
    )
    .unwrap();
    assert_eq!(result.metadata.get("match_count").unwrap(), "2");
}
