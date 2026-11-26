use super::*;

// ========================================================================
// check_command_allowlist tests
// ========================================================================

#[test]
fn test_command_allowlist_empty_allows_all() {
    assert!(check_command_allowlist("anything", false, &[]).is_ok());
}

#[test]
fn test_command_allowlist_blocks_unlisted() {
    let allowlist = vec!["echo".to_string(), "ls".to_string()];
    let result = check_command_allowlist("rm -rf /", false, &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

#[test]
fn test_command_allowlist_allows_listed() {
    let allowlist = vec!["echo".to_string(), "ls".to_string()];
    assert!(check_command_allowlist("echo hello", false, &allowlist).is_ok());
    assert!(check_command_allowlist("ls -la", false, &allowlist).is_ok());
}

#[test]
fn test_command_allowlist_extracts_first_token() {
    let allowlist = vec!["cargo".to_string()];
    assert!(check_command_allowlist("cargo build --release", false, &allowlist).is_ok());
}

#[test]
fn test_command_allowlist_blocks_semicolon_chaining() {
    let allowlist = vec!["cargo".to_string()];
    let result = check_command_allowlist("cargo; rm -rf /", false, &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

#[test]
fn test_command_allowlist_blocks_and_chaining() {
    let allowlist = vec!["cargo".to_string()];
    let result = check_command_allowlist("cargo && cat /etc/passwd", false, &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

#[test]
fn test_command_allowlist_blocks_pipe() {
    let allowlist = vec!["cargo".to_string()];
    let result = check_command_allowlist("cargo | grep secret", false, &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

#[test]
fn test_command_allowlist_blocks_subshell() {
    let allowlist = vec!["echo".to_string()];
    let result = check_command_allowlist("echo $(cat /etc/passwd)", false, &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

#[test]
fn test_command_allowlist_no_metachar_check_without_allowlist() {
    assert!(check_command_allowlist("echo; rm -rf /", false, &[]).is_ok());
}

#[test]
fn test_command_allowlist_prefix_entry_allows_matching_command() {
    let allowlist = vec!["start obsidian://".to_string()];
    assert!(check_command_allowlist("start obsidian://new?vault=test", false, &allowlist).is_ok());
    assert!(check_command_allowlist("start obsidian://open?vault=test", false, &allowlist).is_ok());
}

#[test]
fn test_command_allowlist_prefix_entry_blocks_different_args() {
    let allowlist = vec!["start obsidian://".to_string()];
    let result = check_command_allowlist("start notepad.exe", false, &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

#[test]
fn test_command_allowlist_prefix_entry_blocks_different_program() {
    let allowlist = vec!["start obsidian://".to_string()];
    let result = check_command_allowlist("cmd /c del *", false, &allowlist);
    assert!(matches!(result, Err(ToolError::CommandNotAllowed(_))));
}

// ========================================================================
// check_binary_allowlist tests
// ========================================================================

#[test]
fn test_binary_allowlist_skipped_when_commands_disabled() {
    // command.enabled=false: the command tool will refuse the tool call
    // elsewhere, so the binary check short-circuits without enforcement.
    assert!(check_binary_allowlist("rm", false, false, &[]).is_ok());
    assert!(check_binary_allowlist("rm", false, false, &["git".to_string()]).is_ok());
}

#[test]
fn test_binary_allowlist_empty_fails_closed_when_commands_enabled() {
    // Phase 2 fail-closed fix: previously empty list meant "allow
    // everything"; now it's treated as a configuration error.
    let result = check_binary_allowlist("rm", true, false, &[]);
    match result {
        Err(ToolError::Forbidden(msg)) => {
            assert!(
                msg.contains("binary_allowlist"),
                "unexpected message: {msg}"
            );
        }
        other => panic!("expected Forbidden, got {other:?}"),
    }
}

#[test]
fn test_binary_allowlist_denies_unlisted() {
    let allowlist = vec!["git".to_string(), "cargo".to_string()];
    let result = check_binary_allowlist("rm", true, false, &allowlist);
    match result {
        Err(ToolError::Forbidden(_)) => {}
        other => panic!("expected Forbidden, got {other:?}"),
    }
}

#[test]
fn test_binary_allowlist_allows_listed_program_resolvable() {
    let allowlist = vec!["cargo".to_string()];
    assert!(check_binary_allowlist("cargo", true, false, &allowlist).is_ok());
}

#[test]
fn test_binary_allowlist_strips_windows_exe_suffix() {
    let allowlist = vec!["mytool".to_string()];
    let fake = if cfg!(windows) {
        Path::new("C:/fake/dir/mytool.exe")
    } else {
        Path::new("/fake/dir/mytool")
    };
    assert!(check_binary_allowlist(fake.to_str().unwrap(), true, false, &allowlist).is_ok());
}

#[test]
fn test_strip_windows_executable_suffix_handles_common_shims() {
    assert_eq!(strip_windows_executable_suffix("npm"), "npm");
    assert_eq!(strip_windows_executable_suffix("npm.exe"), "npm");
    assert_eq!(strip_windows_executable_suffix("npm.EXE"), "npm");
    assert_eq!(strip_windows_executable_suffix("npm.cmd"), "npm");
    assert_eq!(strip_windows_executable_suffix("NPM.CMD"), "NPM");
    assert_eq!(strip_windows_executable_suffix("npx.cmd"), "npx");
    assert_eq!(strip_windows_executable_suffix("pnpm.CMD"), "pnpm");
    assert_eq!(strip_windows_executable_suffix("yarn.cmd"), "yarn");
    assert_eq!(strip_windows_executable_suffix("setup.bat"), "setup");
    assert_eq!(strip_windows_executable_suffix("setup.BAT"), "setup");
    assert_eq!(strip_windows_executable_suffix(".cmd"), ".cmd");
    assert_eq!(strip_windows_executable_suffix("foo.txt"), "foo.txt");
}

#[test]
fn test_binary_allowlist_strips_windows_cmd_suffix() {
    // Regression: on Windows, `which("npm")` resolves to `npm.cmd` (not
    // `npm.exe`). Allow-list entries are platform-agnostic ("npm"), so
    // the file-name comparison must strip `.cmd` the same way it strips
    // `.exe`. Without this, real Node-tooling invocations from the
    // dev-loop are rejected with `Forbidden(npm.cmd)`.
    let allowlist = vec!["npm".to_string()];
    let fake = if cfg!(windows) {
        Path::new("C:/fake/dir/npm.cmd")
    } else {
        Path::new("/fake/dir/npm")
    };
    assert!(check_binary_allowlist(fake.to_str().unwrap(), true, false, &allowlist).is_ok());
}

#[test]
fn test_binary_allowlist_strips_windows_bat_suffix() {
    let allowlist = vec!["legacy-tool".to_string()];
    let fake = if cfg!(windows) {
        Path::new("C:/fake/dir/legacy-tool.bat")
    } else {
        Path::new("/fake/dir/legacy-tool")
    };
    assert!(check_binary_allowlist(fake.to_str().unwrap(), true, false, &allowlist).is_ok());
}
