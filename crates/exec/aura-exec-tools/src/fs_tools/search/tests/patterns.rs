use super::*;

#[test]
fn test_search_code_simple_pattern() {
    let (sandbox, dir) = create_test_sandbox();

    fs::write(
        dir.path().join("code.rs"),
        "fn main() { println!(\"hello\"); }",
    )
    .unwrap();

    let result = search_code(&sandbox, "fn main", None, None, 100, 0, OutputMode::Content).unwrap();
    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("fn main"));
    assert!(output.contains("code.rs"));
}

#[test]
fn test_search_code_regex_pattern() {
    let (sandbox, dir) = create_test_sandbox();

    fs::write(dir.path().join("code.rs"), "let x = 42;\nlet y = 123;").unwrap();

    let result = search_code(
        &sandbox,
        r"let \w+ = \d+",
        None,
        None,
        100,
        0,
        OutputMode::Content,
    )
    .unwrap();
    assert!(result.ok);
    assert_eq!(result.metadata.get("match_count").unwrap(), "2");
}

#[test]
fn test_search_code_no_matches() {
    let (sandbox, dir) = create_test_sandbox();

    fs::write(dir.path().join("code.rs"), "fn main() {}").unwrap();

    let result = search_code(
        &sandbox,
        "nonexistent_pattern_xyz",
        None,
        None,
        100,
        0,
        OutputMode::Content,
    )
    .unwrap();
    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("No matches found"));
}

#[test]
fn test_search_code_file_pattern() {
    let (sandbox, dir) = create_test_sandbox();

    fs::write(dir.path().join("code.rs"), "let rust_var = 1;").unwrap();
    fs::write(dir.path().join("code.ts"), "let ts_var = 2;").unwrap();

    let result = search_code(
        &sandbox,
        "let",
        None,
        Some("*.rs"),
        100,
        0,
        OutputMode::Content,
    )
    .unwrap();
    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("rust_var"));
    assert!(!output.contains("ts_var"));
}

#[test]
fn test_search_code_file_pattern_braces() {
    let (sandbox, dir) = create_test_sandbox();

    fs::write(dir.path().join("code.rs"), "let rust_var = 1;").unwrap();
    fs::write(dir.path().join("Cargo.toml"), "let toml_var = 2;").unwrap();
    fs::write(dir.path().join("code.ts"), "let ts_var = 3;").unwrap();

    let result = search_code(
        &sandbox,
        "let",
        None,
        Some("*.{rs,toml}"),
        100,
        0,
        OutputMode::Content,
    )
    .unwrap();
    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("rust_var"));
    assert!(output.contains("toml_var"));
    assert!(!output.contains("ts_var"));
}

#[test]
fn test_search_code_file_pattern_escapes_regex_metacharacters() {
    let (sandbox, dir) = create_test_sandbox();

    fs::write(dir.path().join("c++.rs"), "let plus_var = 1;").unwrap();
    fs::write(dir.path().join("cc.rs"), "let cc_var = 2;").unwrap();

    let result = search_code(
        &sandbox,
        "let",
        None,
        Some("c++.rs"),
        100,
        0,
        OutputMode::Content,
    )
    .unwrap();
    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("plus_var"));
    assert!(!output.contains("cc_var"));
}
