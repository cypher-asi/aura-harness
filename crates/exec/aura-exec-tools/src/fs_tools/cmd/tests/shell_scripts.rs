use super::*;

#[tokio::test]
async fn test_cmd_run_tool_shell_script_requires_allow_shell() {
    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allowed_shell_scripts: vec!["echo hi".to_string()],
        binary_allowlist: vec!["echo".to_string()],
        ..Default::default()
    });
    let (ctx, _dir) = tool_ctx(config);
    let tool = CmdRunTool;

    let err = tool
        .execute(&ctx, serde_json::json!({ "shell_script": "echo hi" }))
        .await
        .expect_err("shell_script without allow_shell must be refused");

    match err {
        ToolError::InvalidArguments(msg) => {
            assert!(msg.contains("allow_shell"), "unexpected message: {msg}");
        }
        other => panic!("expected InvalidArguments, got {other:?}"),
    }
}

#[tokio::test]
async fn test_cmd_run_tool_shell_script_not_in_allowlist() {
    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        allowed_shell_scripts: vec!["echo approved".to_string()],
        binary_allowlist: vec!["echo".to_string()],
        ..Default::default()
    });
    let (ctx, _dir) = tool_ctx(config);
    let tool = CmdRunTool;

    let err = tool
        .execute(&ctx, serde_json::json!({ "shell_script": "rm -rf /" }))
        .await
        .expect_err("unlisted shell_script must be refused");

    match err {
        ToolError::Forbidden(msg) => {
            assert!(
                msg.contains("allowed_shell_scripts"),
                "unexpected message: {msg}"
            );
        }
        other => panic!("expected Forbidden, got {other:?}"),
    }
}

#[tokio::test]
async fn test_cmd_run_tool_shell_script_mutually_exclusive_with_program() {
    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        allowed_shell_scripts: vec!["echo hi".to_string()],
        binary_allowlist: vec!["echo".to_string()],
        ..Default::default()
    });
    let (ctx, _dir) = tool_ctx(config);
    let tool = CmdRunTool;

    let err = tool
        .execute(
            &ctx,
            serde_json::json!({
                "shell_script": "echo hi",
                "program": "echo",
                "args": ["hi"]
            }),
        )
        .await
        .expect_err("shell_script + program must be refused");

    match err {
        ToolError::InvalidArguments(msg) => {
            assert!(
                msg.contains("mutually exclusive"),
                "unexpected message: {msg}"
            );
        }
        other => panic!("expected InvalidArguments, got {other:?}"),
    }
}

#[test]
fn test_script_program_skips_assignments_subshells_and_builtins() {
    assert_eq!(script_program("cargo test"), Some("cargo"));
    assert_eq!(
        script_program("CARGO_TARGET_DIR=/tmp/t RUST_LOG=1 cargo test"),
        Some("cargo")
    );
    assert_eq!(script_program("(git status)"), Some("git"));
    assert_eq!(script_program("cd crates && find . -name '*.rs'"), None);
    assert_eq!(script_program("FOO=bar export FOO"), None);
    assert_eq!(script_program("   "), None);
}

#[test]
fn test_is_shell_program() {
    assert!(is_shell_program("sh"));
    assert!(is_shell_program("/usr/bin/bash"));
    assert!(!is_shell_program("cargo"));
}

#[test]
fn test_stray_zero_width_characters_are_dropped() {
    for (typed, meant) in [
        ("python\u{200b} -m pytest -q", "python -m pytest -q"),
        (
            "sh .swe/run 'python\u{200b}\u{200b} -c 1'",
            "sh .swe/run 'python -c 1'",
        ),
        ("\u{feff}echo hi\u{2060}", "echo hi"),
    ] {
        assert_eq!(without_stray_zero_width(typed), meant);
    }
    for kept in ["echo '👨\u{200d}👩\u{200d}👧'", "printf 'क्\u{200c}ष'"] {
        assert_eq!(without_stray_zero_width(kept), kept);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn test_cmd_run_tool_runs_a_command_with_a_stray_zero_width_space() {
    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        binary_allowlist: vec!["sh".to_string(), "echo".to_string()],
        ..Default::default()
    });
    let (ctx, _dir) = tool_ctx(config);
    for args in [
        serde_json::json!({ "program": "sh", "args": ["-c", "echo\u{200b} zero_width_ok"] }),
        serde_json::json!({ "shell_script": "echo\u{200b} zero_width_ok" }),
    ] {
        let result = CmdRunTool
            .execute(&ctx, args.clone())
            .await
            .unwrap_or_else(|e| panic!("{args}: {e:?}"));
        assert!(result.ok, "{args}: {result:?}");
        assert!(String::from_utf8_lossy(&result.stdout).contains("zero_width_ok"));
    }
}

#[cfg(not(windows))]
#[tokio::test]
async fn test_cmd_run_tool_shell_script_ignores_bare_shell_program() {
    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        binary_allowlist: vec!["echo".to_string()],
        ..Default::default()
    });
    let (ctx, _dir) = tool_ctx(config);

    let result = CmdRunTool
        .execute(
            &ctx,
            serde_json::json!({ "program": "sh", "shell_script": "echo bare_shell_ok" }),
        )
        .await
        .expect("a bare shell program next to shell_script must be ignored");

    assert!(result.ok, "expected success, got {result:?}");
    assert!(String::from_utf8_lossy(&result.stdout).contains("bare_shell_ok"));
}

#[cfg(not(windows))]
#[tokio::test]
async fn test_cmd_run_tool_shell_script_starting_with_builtin_or_assignment_runs() {
    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        binary_allowlist: vec!["sh".to_string()],
        ..Default::default()
    });
    let (ctx, _dir) = tool_ctx(config);

    for (script, expected) in [
        ("cd . && echo builtin_ok", "builtin_ok"),
        ("GREETING=assign_ok sh -c 'echo $GREETING'", "assign_ok"),
    ] {
        let result = CmdRunTool
            .execute(&ctx, serde_json::json!({ "shell_script": script }))
            .await
            .unwrap_or_else(|e| panic!("{script} must run: {e:?}"));
        assert!(result.ok, "{script}: {result:?}");
        assert!(String::from_utf8_lossy(&result.stdout).contains(expected));
    }
}

#[tokio::test]
async fn test_cmd_run_tool_shell_script_runs_when_allowlisted() {
    // Use a first-token that both PATH lookups can resolve:
    // `ping` on Windows lives in System32, `echo` is in /bin on Unix.
    // The shell interpreter itself (cmd.exe / sh) does the real work.
    #[cfg(windows)]
    let (script, allow_binaries) = (
        "ping -n 1 127.0.0.1",
        // Windows file names are upper-case (`PING.EXE`); the allow
        // list stores the resolved file name verbatim minus `.exe`.
        vec!["PING".to_string(), "ping".to_string()],
    );
    #[cfg(not(windows))]
    let (script, allow_binaries) = ("echo shell_path_ok", vec!["echo".to_string()]);

    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        allowed_shell_scripts: vec![script.to_string()],
        binary_allowlist: allow_binaries,
        ..Default::default()
    });
    let (ctx, _dir) = tool_ctx(config);
    let tool = CmdRunTool;

    let result = tool
        .execute(&ctx, serde_json::json!({ "shell_script": script }))
        .await
        .expect("allow-listed script must run");

    assert!(result.ok, "expected success, got {result:?}");
    #[cfg(not(windows))]
    {
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(stdout.contains("shell_path_ok"), "stdout was: {stdout}");
    }
}

/// Empty `allowed_shell_scripts` means "all shell scripts allowed"
/// once `allow_shell == true`, matching the existing empty-allowlist
/// convention on `command_allowlist` / `binary_allowlist`. Pins the
/// behavior Claude-style automatons rely on when emitting
/// `run_command({ command: "cargo check ..." })`: the harness can't
/// enumerate every script up front, so an empty list must not
/// degrade to deny-all.
#[tokio::test]
async fn test_cmd_run_tool_shell_script_empty_allowlist_is_permissive() {
    #[cfg(windows)]
    let (script, allow_binaries) = (
        "ping -n 1 127.0.0.1",
        vec!["PING".to_string(), "ping".to_string()],
    );
    #[cfg(not(windows))]
    let (script, allow_binaries) = ("echo empty_allowlist_ok", vec!["echo".to_string()]);

    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        allowed_shell_scripts: vec![],
        binary_allowlist: allow_binaries,
        ..Default::default()
    });
    let (ctx, _dir) = tool_ctx(config);
    let tool = CmdRunTool;

    let result = tool
        .execute(&ctx, serde_json::json!({ "shell_script": script }))
        .await
        .expect("empty allowlist with allow_shell=true must execute any script");

    assert!(result.ok, "expected success, got {result:?}");
    #[cfg(not(windows))]
    {
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(
            stdout.contains("empty_allowlist_ok"),
            "stdout was: {stdout}"
        );
    }
}

/// The legacy `command` alias must follow the same empty-allowlist =
/// permissive rule as `shell_script`. This is the shape Claude-style
/// tool proposals arrive in, so regressing it would re-break the
/// autonomous dev loop.
#[tokio::test]
async fn test_cmd_run_tool_command_alias_empty_allowlist_is_permissive() {
    #[cfg(windows)]
    let (script, allow_binaries) = (
        "ping -n 1 127.0.0.1",
        vec!["PING".to_string(), "ping".to_string()],
    );
    #[cfg(not(windows))]
    let (script, allow_binaries) = ("echo command_alias_ok", vec!["echo".to_string()]);

    let config = command_config(crate::CommandPolicy {
        enabled: true,
        allow_shell: true,
        allowed_shell_scripts: vec![],
        binary_allowlist: allow_binaries,
        ..Default::default()
    });
    let (ctx, _dir) = tool_ctx(config);
    let tool = CmdRunTool;

    let result = tool
        .execute(&ctx, serde_json::json!({ "command": script }))
        .await
        .expect("empty allowlist with allow_shell=true must accept the `command` alias");

    assert!(result.ok, "expected success, got {result:?}");
}
