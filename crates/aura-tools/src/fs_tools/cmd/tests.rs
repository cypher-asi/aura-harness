use super::*;
use std::fs;
use tempfile::TempDir;

fn create_test_sandbox() -> (Sandbox, TempDir) {
    let dir = TempDir::new().unwrap();
    let sandbox = Sandbox::new(dir.path()).unwrap();
    (sandbox, dir)
}

// ========================================================================
// cmd_run Tests
// ========================================================================

#[test]
fn test_cmd_run_echo() {
    let (sandbox, _dir) = create_test_sandbox();

    let result = cmd_run(&sandbox, "echo", &["hello".to_string()], None, 5000).unwrap();
    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("hello"));
}

#[test]
fn test_cmd_run_in_cwd() {
    let (sandbox, dir) = create_test_sandbox();

    fs::create_dir(dir.path().join("subdir")).unwrap();
    fs::write(dir.path().join("subdir/marker.txt"), "found").unwrap();

    #[cfg(windows)]
    let result = cmd_run(&sandbox, "dir", &[], Some("subdir"), 5000).unwrap();
    #[cfg(not(windows))]
    let result = cmd_run(&sandbox, "ls", &[], Some("subdir"), 5000).unwrap();

    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("marker"));
}

#[test]
fn test_cmd_run_nonexistent_command() {
    let (sandbox, _dir) = create_test_sandbox();

    let result = cmd_run(
        &sandbox,
        "nonexistent_command_that_does_not_exist_xyz",
        &[],
        None,
        5000,
    );
    match result {
        Err(ToolError::CommandFailed(_)) => {}
        Ok(r) => assert!(!r.ok),
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn test_cmd_run_exit_code() {
    let (sandbox, _dir) = create_test_sandbox();

    #[cfg(windows)]
    let result = cmd_run(
        &sandbox,
        "cmd",
        &["/c".to_string(), "exit".to_string(), "1".to_string()],
        None,
        5000,
    )
    .unwrap();
    #[cfg(not(windows))]
    let result = cmd_run(&sandbox, "false", &[], None, 5000).unwrap();

    assert!(!result.ok);
    assert_eq!(result.exit_code, Some(1));
}

#[test]
fn test_cmd_run_failure_returns_structured_result() {
    let (sandbox, _dir) = create_test_sandbox();

    #[cfg(windows)]
    let result = cmd_run(
        &sandbox,
        "cmd",
        &["/c".to_string(), "exit".to_string(), "42".to_string()],
        None,
        5000,
    )
    .unwrap();
    #[cfg(not(windows))]
    let result = cmd_run(&sandbox, "sh", &["-c".into(), "exit 42".into()], None, 5000).unwrap();

    assert!(!result.ok);
    let stderr_text = String::from_utf8_lossy(&result.stderr);
    assert!(stderr_text.contains("exit_code:"));
}

#[test]
fn test_cmd_run_stdout_truncation() {
    let (sandbox, _dir) = create_test_sandbox();

    #[cfg(windows)]
    let result = cmd_run(
        &sandbox,
        "powershell",
        &["-Command".to_string(), "'x' * 10000".to_string()],
        None,
        10000,
    )
    .unwrap();
    #[cfg(not(windows))]
    let result = cmd_run(
        &sandbox,
        "python3",
        &["-c".into(), "print('x' * 10000)".into()],
        None,
        10000,
    )
    .unwrap();

    assert!(result.ok);
    let stdout_text = String::from_utf8_lossy(&result.stdout);
    assert!(stdout_text.len() <= STDOUT_TRUNCATE_LIMIT + 100);
}

#[test]
fn test_cmd_run_with_args() {
    let (sandbox, dir) = create_test_sandbox();

    fs::write(dir.path().join("test.txt"), "content").unwrap();

    #[cfg(windows)]
    let result = cmd_run(&sandbox, "type", &["test.txt".to_string()], None, 5000).unwrap();
    #[cfg(not(windows))]
    let result = cmd_run(&sandbox, "cat", &["test.txt".to_string()], None, 5000).unwrap();

    assert!(result.ok);
    let output = String::from_utf8_lossy(&result.stdout);
    assert!(output.contains("content"));
}

#[test]
fn test_cmd_run_preserves_quoted_arguments() {
    let (sandbox, _dir) = create_test_sandbox();

    #[cfg(windows)]
    {
        let result = cmd_run(&sandbox, r#"cmd /c echo "hello world""#, &[], None, 5000).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(
            output.contains("hello world"),
            "quoted argument should be preserved, got: {output}"
        );
    }

    #[cfg(not(windows))]
    {
        let result = cmd_run(&sandbox, r#"echo "hello world""#, &[], None, 5000).unwrap();
        assert!(result.ok);
        let output = String::from_utf8_lossy(&result.stdout);
        assert!(
            output.contains("hello world"),
            "quoted argument should be preserved, got: {output}"
        );
    }
}

// ========================================================================
// wait_with_threshold Tests
// ========================================================================

#[test]
fn test_fast_command_returns_output() {
    let (sandbox, _dir) = create_test_sandbox();

    let (result, _command) =
        cmd_run_with_threshold(&sandbox, "echo", &["fast_output".to_string()], None, 5000).unwrap();

    match result {
        ThresholdResult::Completed(output) => {
            assert!(output.status.success());
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("fast_output"));
        }
        ThresholdResult::Pending(_) => {
            panic!("Expected Completed, got Pending for fast command");
        }
    }
}

#[test]
fn test_slow_command_returns_child() {
    let (sandbox, _dir) = create_test_sandbox();

    #[cfg(windows)]
    let (result, command) = cmd_run_with_threshold(
        &sandbox,
        "ping",
        &["-n".to_string(), "10".to_string(), "127.0.0.1".to_string()],
        None,
        100,
    )
    .unwrap();

    #[cfg(not(windows))]
    let (result, command) =
        cmd_run_with_threshold(&sandbox, "sleep", &["10".to_string()], None, 100).unwrap();

    match result {
        ThresholdResult::Pending(mut child) => {
            assert!(
                child.try_wait().unwrap().is_none(),
                "Child should still be running"
            );
            let _ = child.kill();
            let _ = child.wait();
            assert!(!command.is_empty());
        }
        ThresholdResult::Completed(_) => {
            panic!("Expected Pending, got Completed for slow command");
        }
    }
}

#[test]
fn test_threshold_boundary_fast_completes() {
    let (sandbox, _dir) = create_test_sandbox();

    let (result, _command) =
        cmd_run_with_threshold(&sandbox, "echo", &["boundary".to_string()], None, 1000).unwrap();

    match result {
        ThresholdResult::Completed(output) => {
            assert!(output.status.success());
        }
        ThresholdResult::Pending(_) => {
            panic!("Expected Completed for fast echo command");
        }
    }
}

#[test]
fn test_cmd_spawn_returns_command_string() {
    let (sandbox, _dir) = create_test_sandbox();

    let (mut child, command) =
        cmd_spawn(&sandbox, "echo", &["test_arg".to_string()], None).unwrap();

    assert!(command.contains("echo"));
    assert!(command.contains("test_arg"));

    let _ = child.wait();
}

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
// check_command_allowlist tests
// ========================================================================

#[test]
fn test_command_allowlist_empty_allows_all() {
    assert!(check_command_allowlist("anything", &[]).is_ok());
}

#[test]
fn test_command_allowlist_blocks_unlisted() {
    let allowlist = vec!["echo".to_string(), "ls".to_string()];
    let result = check_command_allowlist("rm -rf /", &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

#[test]
fn test_command_allowlist_allows_listed() {
    let allowlist = vec!["echo".to_string(), "ls".to_string()];
    assert!(check_command_allowlist("echo hello", &allowlist).is_ok());
    assert!(check_command_allowlist("ls -la", &allowlist).is_ok());
}

#[test]
fn test_command_allowlist_extracts_first_token() {
    let allowlist = vec!["cargo".to_string()];
    assert!(check_command_allowlist("cargo build --release", &allowlist).is_ok());
}

#[test]
fn test_command_allowlist_blocks_semicolon_chaining() {
    let allowlist = vec!["cargo".to_string()];
    let result = check_command_allowlist("cargo; rm -rf /", &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

#[test]
fn test_command_allowlist_blocks_and_chaining() {
    let allowlist = vec!["cargo".to_string()];
    let result = check_command_allowlist("cargo && cat /etc/passwd", &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

#[test]
fn test_command_allowlist_blocks_pipe() {
    let allowlist = vec!["cargo".to_string()];
    let result = check_command_allowlist("cargo | grep secret", &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

#[test]
fn test_command_allowlist_blocks_subshell() {
    let allowlist = vec!["echo".to_string()];
    let result = check_command_allowlist("echo $(cat /etc/passwd)", &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

#[test]
fn test_command_allowlist_no_metachar_check_without_allowlist() {
    assert!(check_command_allowlist("echo; rm -rf /", &[]).is_ok());
}

#[test]
fn test_command_allowlist_prefix_entry_allows_matching_command() {
    let allowlist = vec!["start obsidian://".to_string()];
    assert!(check_command_allowlist("start obsidian://new?vault=test", &allowlist).is_ok());
    assert!(check_command_allowlist("start obsidian://open?vault=test", &allowlist).is_ok());
}

#[test]
fn test_command_allowlist_prefix_entry_blocks_different_args() {
    let allowlist = vec!["start obsidian://".to_string()];
    let result = check_command_allowlist("start notepad.exe", &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

#[test]
fn test_command_allowlist_prefix_entry_blocks_different_program() {
    let allowlist = vec!["start obsidian://".to_string()];
    let result = check_command_allowlist("cmd /c del *", &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

// ========================================================================
// check_binary_allowlist tests (Wave 5 / T3.2)
// ========================================================================

#[test]
fn test_binary_allowlist_empty_passes() {
    // Empty allow-list disables the check — backwards compatible.
    assert!(check_binary_allowlist("rm", &[]).is_ok());
}

#[test]
fn test_binary_allowlist_denies_unlisted() {
    // `rm` is resolvable on every CI host; we just want the check to
    // deny it when the allow-list omits it.
    let allowlist = vec!["git".to_string(), "cargo".to_string()];
    let result = check_binary_allowlist("rm", &allowlist);
    match result {
        Err(ToolError::Forbidden(_)) => {}
        other => panic!("expected Forbidden, got {other:?}"),
    }
}

#[test]
fn test_binary_allowlist_allows_listed_program_resolvable() {
    // `cargo` must be present for the test suite to run at all, so we
    // can rely on `which` finding it.
    let allowlist = vec!["cargo".to_string()];
    assert!(check_binary_allowlist("cargo", &allowlist).is_ok());
}

#[test]
fn test_binary_allowlist_strips_windows_exe_suffix() {
    // Pass an absolute path ending in `.exe` to exercise the suffix-stripping
    // branch without touching the real PATH.
    let allowlist = vec!["mytool".to_string()];
    let fake = if cfg!(windows) {
        Path::new("C:/fake/dir/mytool.exe")
    } else {
        Path::new("/fake/dir/mytool")
    };
    assert!(check_binary_allowlist(fake.to_str().unwrap(), &allowlist).is_ok());
}

// ========================================================================
// CmdRunTool::execute gating (Wave 5 / T3.1 + T3.2)
// ========================================================================

/// Build a `ToolContext` pointing at a fresh tempdir. Helper used by the
/// hardening gate tests.
fn tool_ctx(config: crate::ToolConfig) -> (ToolContext, TempDir) {
    let dir = TempDir::new().unwrap();
    let sandbox = Sandbox::new(dir.path()).unwrap();
    (ToolContext::new(sandbox, config), dir)
}

#[tokio::test]
async fn test_cmd_run_tool_rejects_command_field_without_allow_shell() {
    let (ctx, _dir) = tool_ctx(crate::ToolConfig::default());
    let tool = CmdRunTool;

    let err = tool
        .execute(&ctx, serde_json::json!({ "command": "echo hi" }))
        .await
        .expect_err("shell form must be gated behind allow_shell");

    match err {
        ToolError::InvalidArguments(msg) => {
            assert!(msg.contains("allow_shell"), "unexpected message: {msg}");
        }
        other => panic!("expected InvalidArguments, got {other:?}"),
    }
}

#[tokio::test]
async fn test_cmd_run_tool_rejects_empty_args_without_allow_shell() {
    let (ctx, _dir) = tool_ctx(crate::ToolConfig::default());
    let tool = CmdRunTool;

    // No `args` at all — would have been treated as a raw shell script
    // under the old code path. After T3.1 this must error.
    let err = tool
        .execute(&ctx, serde_json::json!({ "program": "ls" }))
        .await
        .expect_err("empty args form must be gated behind allow_shell");

    match err {
        ToolError::InvalidArguments(msg) => {
            assert!(msg.contains("empty 'args'"), "unexpected message: {msg}");
        }
        other => panic!("expected InvalidArguments, got {other:?}"),
    }
}

#[tokio::test]
async fn test_cmd_run_tool_denies_binary_outside_allowlist() {
    // Restrict to `git` only; request `rm`.
    let config = crate::ToolConfig {
        binary_allowlist: vec!["git".to_string()],
        ..crate::ToolConfig::default()
    };
    let (ctx, _dir) = tool_ctx(config);
    let tool = CmdRunTool;

    let err = tool
        .execute(
            &ctx,
            serde_json::json!({ "program": "rm", "args": ["-rf", "/tmp/x"] }),
        )
        .await
        .expect_err("rm must be denied by binary_allowlist");

    assert!(
        matches!(err, ToolError::Forbidden(_)),
        "expected Forbidden, got {err:?}"
    );
}

#[tokio::test]
async fn test_cmd_run_tool_allows_listed_binary_with_args() {
    // `cargo` is resolvable on any host that runs this test suite. We
    // pass `--version` so the invocation terminates instantly and
    // succeeds.
    let config = crate::ToolConfig {
        binary_allowlist: vec!["cargo".to_string()],
        ..crate::ToolConfig::default()
    };
    let (ctx, _dir) = tool_ctx(config);
    let tool = CmdRunTool;

    let result = tool
        .execute(
            &ctx,
            serde_json::json!({ "program": "cargo", "args": ["--version"] }),
        )
        .await;

    assert!(
        result.is_ok(),
        "cargo --version with binary_allowlist=[cargo] must pass pre-flight; got {:?}",
        result.err()
    );
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
