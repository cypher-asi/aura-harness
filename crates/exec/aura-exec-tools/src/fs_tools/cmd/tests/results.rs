use super::*;

// ========================================================================
// truncate_output boundary tests
// ========================================================================

#[test]
fn test_truncate_output_under_limit() {
    let s = "short";
    let result = truncate_output(s, 100);
    assert_eq!(result, "short");
}

#[test]
fn test_truncate_output_exact_limit() {
    let s = "x".repeat(STDOUT_TRUNCATE_LIMIT);
    let result = truncate_output(&s, STDOUT_TRUNCATE_LIMIT);
    assert_eq!(result.len(), STDOUT_TRUNCATE_LIMIT);
    assert!(!result.contains("truncated"));
}

#[test]
fn test_truncate_output_over_limit() {
    let s = "x".repeat(STDOUT_TRUNCATE_LIMIT + 500);
    let result = truncate_output(&s, STDOUT_TRUNCATE_LIMIT);
    assert!(result.contains("truncated"));
    assert!(result.len() <= STDOUT_TRUNCATE_LIMIT + 100);
}

#[test]
fn test_truncate_output_keeps_start_and_end() {
    let s = format!("BEGIN{}error: the end", "x".repeat(20_000));
    let result = truncate_output(&s, STDOUT_TRUNCATE_LIMIT);
    assert!(result.starts_with("BEGIN"));
    assert!(result.ends_with("error: the end"));
    assert!(result.contains("bytes cut from the middle"));
}

#[test]
fn test_truncate_output_multibyte_boundary() {
    let s = "\u{20ac}".repeat(4000);
    let result = truncate_output(&s, 10);
    assert!(result.is_char_boundary(result.find('\n').unwrap_or(result.len())));
}

#[test]
fn test_truncate_output_empty() {
    let result = truncate_output("", 100);
    assert_eq!(result, "");
}

// ========================================================================
// output_to_tool_result
// ========================================================================

#[test]
fn test_output_to_tool_result_success() {
    #[cfg(windows)]
    let status = {
        let output = std::process::Command::new("cmd.exe")
            .args(["/C", "exit 0"])
            .output()
            .unwrap();
        output.status
    };

    #[cfg(not(windows))]
    let status = {
        let output = std::process::Command::new("true").output().unwrap();
        output.status
    };

    let output = std::process::Output {
        status,
        stdout: b"success output".to_vec(),
        stderr: Vec::new(),
    };

    let result = output_to_tool_result(output).unwrap();
    assert!(result.ok);
    assert_eq!(String::from_utf8_lossy(&result.stdout), "success output");
}

#[test]
fn test_output_to_tool_result_failure() {
    #[cfg(windows)]
    let status = {
        let output = std::process::Command::new("cmd.exe")
            .args(["/C", "exit 1"])
            .output()
            .unwrap();
        output.status
    };

    #[cfg(not(windows))]
    let status = {
        let output = std::process::Command::new("false").output().unwrap();
        output.status
    };

    let output = std::process::Output {
        status,
        stdout: Vec::new(),
        stderr: b"error message".to_vec(),
    };

    let result = output_to_tool_result(output).unwrap();
    assert!(!result.ok);
    assert_eq!(result.exit_code, Some(1));
    let stderr_text = String::from_utf8_lossy(&result.stderr);
    assert!(stderr_text.contains("error message"));
}

#[test]
fn test_output_to_tool_result_exit_code_metadata() {
    #[cfg(windows)]
    let status = {
        let output = std::process::Command::new("cmd.exe")
            .args(["/C", "exit 0"])
            .output()
            .unwrap();
        output.status
    };

    #[cfg(not(windows))]
    let status = {
        let output = std::process::Command::new("true").output().unwrap();
        output.status
    };

    let output = std::process::Output {
        status,
        stdout: b"ok".to_vec(),
        stderr: Vec::new(),
    };

    let result = output_to_tool_result(output).unwrap();
    assert_eq!(result.metadata.get("exit_code").unwrap(), "0");
}
