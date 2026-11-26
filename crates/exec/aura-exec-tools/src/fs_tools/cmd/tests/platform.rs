use super::*;

// ========================================================================
// windows_resolve_program — PATHEXT-aware bare-name resolution
// ========================================================================
//
// Bare names may be expanded to PATHEXT shims for direct spawn, but
// caller-supplied paths and explicit extensions remain unchanged.

/// Operator intent wins for non-bare names.
#[cfg(windows)]
#[test]
fn windows_resolve_program_passes_through_paths_and_extensions() {
    use std::path::Path;

    let cwd = Path::new(".");
    assert!(windows_resolve_program(r"tools\foo.exe", None, cwd).is_none());
    assert!(windows_resolve_program("./build.sh", None, cwd).is_none());
    assert!(windows_resolve_program(r"C:\Windows\System32\cmd.exe", None, cwd).is_none());
    assert!(windows_resolve_program("npm.cmd", None, cwd).is_none());
    assert!(windows_resolve_program("BUILD.BAT", None, cwd).is_none());
    assert!(windows_resolve_program("python.exe", None, cwd).is_none());
}

/// Smoke test against a real Windows binary so the PATHEXT integration is
/// exercised without depending on Node or another optional toolchain.
#[cfg(windows)]
#[test]
fn windows_resolve_program_finds_cmd_via_pathext() {
    use std::path::Path;

    let cwd = Path::new(".");
    let Some(resolved) = windows_resolve_program("cmd", None, cwd) else {
        return;
    };
    assert!(
        resolved.is_absolute(),
        "expected absolute resolved path, got {}",
        resolved.display()
    );
    let lower = resolved
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    assert!(
        lower.starts_with("cmd."),
        "expected resolved file to start with 'cmd.', got {lower}"
    );
}

// ========================================================================
// cargo stderr classification — verifies we lift compiler diagnostics
// into ToolResult metadata so the agent loop sees `cargo check` failures
// even when stdout is empty.
// ========================================================================

#[test]
fn classify_cargo_invocation_matches_known_subcommands() {
    assert_eq!(
        classify_cargo_invocation(Some("cargo"), &["check".to_string()]),
        Some("check")
    );
    assert_eq!(
        classify_cargo_invocation(Some("cargo.exe"), &["build".to_string()]),
        Some("build")
    );
    assert_eq!(
        classify_cargo_invocation(
            Some("/usr/local/bin/cargo"),
            &["+nightly".to_string(), "test".to_string()]
        ),
        Some("test")
    );
    assert_eq!(
        classify_cargo_invocation(Some("cargo"), &["clippy".to_string()]),
        Some("clippy")
    );
    assert_eq!(classify_cargo_invocation(Some("cargo"), &[]), None);
    assert_eq!(
        classify_cargo_invocation(Some("rustc"), &["check".to_string()]),
        None
    );
    assert_eq!(
        classify_cargo_invocation(Some("cargo"), &["doc".to_string()]),
        None
    );
}

#[test]
fn extract_cargo_errors_lifts_first_error_with_location() {
    let stderr = "\
warning: unused import: `Foo`
 --> src/lib.rs:3:5
error[E0432]: unresolved import `crate::missing`
 --> src/lib.rs:7:5
  |
7 | use crate::missing;
  |     ^^^^^^^^^^^^^^ no `missing` in the crate root

error[E0277]: the trait bound `Foo: Bar` is not satisfied
 --> crates/foo/src/lib.rs:42:1

error: aborting due to 2 previous errors
";
    let errors = extract_cargo_errors(stderr);
    assert_eq!(errors.len(), 2);
    assert_eq!(errors[0].code, "E0432");
    assert!(errors[0].message.contains("unresolved import"));
    assert_eq!(errors[0].location.as_deref(), Some("src/lib.rs:7:5"));
    assert_eq!(errors[1].code, "E0277");
    assert_eq!(
        errors[1].location.as_deref(),
        Some("crates/foo/src/lib.rs:42:1")
    );
}

#[test]
fn extract_cargo_errors_caps_count_at_five() {
    let mut stderr = String::new();
    for i in 0..20 {
        stderr.push_str(&format!(
            "error[E{i:04}]: bad thing\n --> src/lib.rs:{i}:1\n\n"
        ));
    }
    let errors = extract_cargo_errors(&stderr);
    assert_eq!(errors.len(), 5);
}

#[test]
fn extract_cargo_errors_returns_empty_on_clean_stderr() {
    let errors = extract_cargo_errors("warning: unused variable: `x`\n");
    assert!(errors.is_empty());
}

/// `cmd_spawn` keeps bare-name resolution on the direct-spawn path.
#[cfg(windows)]
#[tokio::test]
async fn cmd_spawn_resolves_bare_cmd_via_pathext() {
    let (sandbox, _dir) = create_test_sandbox();
    let result = cmd_spawn(
        &sandbox,
        "cmd",
        &["/C".into(), "exit".into(), "0".into()],
        None,
    );
    let (mut child, _display) = match result {
        Ok(t) => t,
        Err(e) => panic!("expected cmd_spawn to find cmd via PATHEXT, got: {e:?}"),
    };
    let status = child.wait().expect("child wait failed");
    assert!(
        status.success(),
        "expected `cmd /C exit 0` to succeed, got {status:?}"
    );
}
