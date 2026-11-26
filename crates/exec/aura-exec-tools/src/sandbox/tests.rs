
use super::*;
use tempfile::TempDir;

fn create_sandbox() -> (Sandbox, TempDir) {
    let dir = TempDir::new().unwrap();
    let sandbox = Sandbox::new(dir.path()).unwrap();
    (sandbox, dir)
}

#[test]
fn test_resolve_relative() {
    let (sandbox, _dir) = create_sandbox();

    let resolved = sandbox.resolve("foo/bar.txt").unwrap();
    assert!(resolved.starts_with(sandbox.root()));
    assert!(resolved.ends_with("foo/bar.txt"));
}

#[test]
fn test_resolve_absolute_inside() {
    let (sandbox, _dir) = create_sandbox();

    let path = sandbox.root().join("foo/bar.txt");
    let resolved = sandbox.resolve(&path).unwrap();
    assert_eq!(resolved, path);
}

#[test]
fn test_resolve_dotdot_escape() {
    let (sandbox, _dir) = create_sandbox();

    let result = sandbox.resolve("../escape.txt");
    assert!(matches!(result, Err(ToolError::SandboxViolation { .. })));
}

#[test]
fn test_resolve_complex_dotdot() {
    let (sandbox, _dir) = create_sandbox();

    // foo/../bar should be fine (stays in root)
    let resolved = sandbox.resolve("foo/../bar.txt").unwrap();
    assert!(resolved.starts_with(sandbox.root()));

    // foo/../../escape should fail
    let result = sandbox.resolve("foo/../../escape.txt");
    assert!(matches!(result, Err(ToolError::SandboxViolation { .. })));
}

#[test]
fn test_resolve_absolute_outside() {
    let (sandbox, _dir) = create_sandbox();

    let result = sandbox.resolve("/etc/passwd");
    assert!(matches!(result, Err(ToolError::SandboxViolation { .. })));
}

#[test]
fn test_resolve_workspace_root_alias() {
    let (sandbox, _dir) = create_sandbox();

    let resolved = sandbox.resolve("/").unwrap();
    assert_eq!(resolved, sandbox.root());
}

#[test]
fn test_resolve_existing() {
    let (sandbox, dir) = create_sandbox();

    // Create a file
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello").unwrap();

    let resolved = sandbox.resolve_existing("test.txt").unwrap();
    let expected = strip_unc_prefix(&file_path.canonicalize().unwrap());
    assert_eq!(resolved, expected);
}

#[test]
fn test_resolve_existing_not_found() {
    let (sandbox, _dir) = create_sandbox();

    let result = sandbox.resolve_existing("nonexistent.txt");
    assert!(matches!(result, Err(ToolError::PathNotFound(_))));
}

#[cfg(unix)]
#[test]
fn test_symlink_pointing_outside_sandbox_blocked() {
    use std::os::unix::fs::symlink;

    let (sandbox, dir) = create_sandbox();

    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "top secret").unwrap();

    symlink(
        outside.path().join("secret.txt"),
        dir.path().join("escape_link"),
    )
    .unwrap();

    let result = sandbox.resolve_existing("escape_link");
    assert!(
        matches!(result, Err(ToolError::SandboxViolation { .. })),
        "Symlink to outside should be blocked, got: {result:?}"
    );
}

#[cfg(windows)]
#[test]
fn test_symlink_directory_junction_outside_blocked() {
    // On Windows, directory junctions don't require elevated privileges
    let (sandbox, dir) = create_sandbox();

    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("secret.txt"), "top secret").unwrap();

    // Create a junction point (requires std::process::Command)
    let junction_path = dir.path().join("escape_junction");
    let status = std::process::Command::new("cmd")
        .args([
            "/C",
            "mklink",
            "/J",
            &junction_path.to_string_lossy(),
            &outside.path().to_string_lossy(),
        ])
        .output();

    if let Ok(output) = status {
        if output.status.success() {
            let result = sandbox.resolve_existing("escape_junction/secret.txt");
            assert!(
                matches!(result, Err(ToolError::SandboxViolation { .. })),
                "Junction to outside should be blocked, got: {result:?}"
            );
        }
        // If mklink fails (e.g. permissions), skip the test gracefully
    }
}

#[test]
fn test_resolve_new_allows_nonexistent_path() {
    let (sandbox, _dir) = create_sandbox();

    let result = sandbox.resolve_new("brand/new/file.txt");
    assert!(result.is_ok());
    let resolved = result.unwrap();
    assert!(resolved.starts_with(sandbox.root()));
}

#[test]
fn test_resolve_new_blocks_escape() {
    let (sandbox, _dir) = create_sandbox();

    let result = sandbox.resolve_new("../../etc/passwd");
    assert!(matches!(result, Err(ToolError::SandboxViolation { .. })));
}

#[test]
fn test_sandbox_root_is_canonical() {
    let dir = TempDir::new().unwrap();
    let sandbox = Sandbox::new(dir.path()).unwrap();
    let root = sandbox.root();
    // Canonical path should not contain "." or ".."
    for component in root.components() {
        assert_ne!(component, std::path::Component::CurDir);
        assert_ne!(component, std::path::Component::ParentDir);
    }
}

#[test]
fn test_sandbox_clone() {
    let (sandbox, _dir) = create_sandbox();
    let cloned = sandbox.clone();
    assert_eq!(sandbox.root(), cloned.root());
}

#[test]
fn test_extra_roots_allow_access() {
    let main_dir = TempDir::new().unwrap();
    let extra_dir = TempDir::new().unwrap();
    std::fs::write(extra_dir.path().join("note.md"), "hello").unwrap();

    let sandbox =
        Sandbox::with_extra_roots(main_dir.path(), &[extra_dir.path().to_path_buf()]).unwrap();

    let resolved = sandbox.resolve_existing(extra_dir.path().join("note.md"));
    assert!(
        resolved.is_ok(),
        "should be able to access file in extra root"
    );
}

#[cfg(unix)]
#[test]
fn test_extra_roots_allow_access_through_canonical_alias() {
    use std::os::unix::fs::symlink;

    let main_dir = TempDir::new().unwrap();
    let extra_dir = TempDir::new().unwrap();
    let alias_dir = TempDir::new().unwrap();
    std::fs::write(extra_dir.path().join("note.md"), "hello").unwrap();
    let alias = alias_dir.path().join("extra");
    symlink(extra_dir.path(), &alias).unwrap();

    let sandbox =
        Sandbox::with_extra_roots(main_dir.path(), &[extra_dir.path().to_path_buf()]).unwrap();

    let resolved = sandbox.resolve_existing(alias.join("note.md"));
    assert!(
            resolved.is_ok(),
            "should accept an existing extra-root path whose spelling canonicalizes inside the extra root"
        );
}

#[test]
fn test_extra_roots_still_block_outside() {
    let main_dir = TempDir::new().unwrap();
    let extra_dir = TempDir::new().unwrap();
    let outside_dir = TempDir::new().unwrap();
    std::fs::write(outside_dir.path().join("secret.txt"), "secret").unwrap();

    let sandbox =
        Sandbox::with_extra_roots(main_dir.path(), &[extra_dir.path().to_path_buf()]).unwrap();

    let result = sandbox.resolve(outside_dir.path().join("secret.txt"));
    assert!(
        matches!(result, Err(ToolError::SandboxViolation { .. })),
        "paths outside both roots should still be blocked"
    );
}

#[test]
fn test_extra_roots_nonexistent_skipped() {
    let main_dir = TempDir::new().unwrap();
    let sandbox = Sandbox::with_extra_roots(
        main_dir.path(),
        &[PathBuf::from("/nonexistent/path/that/does/not/exist")],
    )
    .unwrap();

    assert_eq!(sandbox.extra_roots.len(), 0);
}
