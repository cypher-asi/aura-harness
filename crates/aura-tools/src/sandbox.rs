//! Sandbox for path validation.
//!
//! Ensures all file-system paths supplied by an agent resolve within the
//! workspace root, preventing access to the wider host file system.
//!
//! # How Enforcement Works
//!
//! Every path goes through two stages of validation:
//!
//! 1. **Normalisation** – the path is joined with the sandbox root (if
//!    relative) and then [`normalize_path`] resolves all `.` and `..`
//!    components *without* touching the file system.  The result is compared
//!    against the canonical root via [`Path::starts_with`].
//! 2. **Symlink re-check** – for paths that must already exist
//!    ([`Sandbox::resolve_existing`]), the path is additionally
//!    [`canonicalize`](std::fs::canonicalize)d, which follows symlinks to their
//!    real target, and the prefix check is repeated.  This catches symlinks
//!    whose target lies outside the sandbox.
//!
//! # Attacks Prevented
//!
//! * **Directory traversal** (`../../../etc/passwd`) – caught during
//!   normalisation because `..` components that would move above the root are
//!   collapsed and the prefix check fails.
//! * **Symlinks / junctions to outside** – a symlink at
//!   `<root>/escape -> /etc` is caught by the post-canonicalize prefix check
//!   in `resolve_existing`.
//! * **Absolute paths outside root** (`/tmp/evil`) – fail the prefix check
//!   immediately.
//!
//! # Assumptions
//!
//! * **`workspace_root` is trusted** – the root path itself is provided by the
//!   system, not the agent.  It is canonicalized once at construction time.
//! * **No TOCTOU for new files** – [`Sandbox::resolve_new`] validates the
//!   *intended* path but cannot follow symlinks for files that do not yet
//!   exist.  A race where a symlink is created between validation and use is
//!   outside the sandbox's scope (mitigated at the OS/container level).
//! * **OS path semantics** – the normalisation logic relies on
//!   [`std::path::Component`] for correct handling of platform-specific path
//!   separators and prefixes (e.g. `\\?\` on Windows).

use crate::error::ToolError;
use std::path::{Path, PathBuf};
use tracing::debug;

/// Sandbox for validating and normalizing paths.
#[derive(Debug, Clone)]
pub struct Sandbox {
    /// The root directory all paths must be under
    root: PathBuf,
}

impl Sandbox {
    /// Create a new sandbox with the given root.
    ///
    /// # Errors
    /// Returns error if root cannot be canonicalized.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        let root = root.as_ref();

        // Create root if it doesn't exist
        if !root.exists() {
            std::fs::create_dir_all(root)?;
        }

        let root = root.canonicalize()?;
        debug!(?root, "Sandbox initialized");

        Ok(Self { root })
    }

    /// Get the sandbox root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve and validate a path within the sandbox.
    ///
    /// The path can be:
    /// - Absolute (must be under root)
    /// - Relative (resolved relative to root)
    ///
    /// # Errors
    /// Returns `SandboxViolation` if the resolved path escapes the root.
    pub fn resolve(&self, path: impl AsRef<Path>) -> Result<PathBuf, ToolError> {
        let path = path.as_ref();

        // Join with root if relative
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };

        // Normalize the path (handle .., ., etc.)
        let normalized = normalize_path(&joined);

        // Check if the normalized path starts with our root
        if !normalized.starts_with(&self.root) {
            return Err(ToolError::SandboxViolation {
                path: path.display().to_string(),
            });
        }

        debug!(original = ?path, resolved = ?normalized, "Path resolved");
        Ok(normalized)
    }

    /// Resolve a path that must exist.
    ///
    /// # Errors
    /// Returns error if path doesn't exist or escapes sandbox.
    pub fn resolve_existing(&self, path: impl AsRef<Path>) -> Result<PathBuf, ToolError> {
        let resolved = self.resolve(path.as_ref())?;

        if !resolved.exists() {
            return Err(ToolError::PathNotFound(path.as_ref().display().to_string()));
        }

        // Re-canonicalize to resolve symlinks
        let canonical = resolved.canonicalize()?;

        // Check again after following symlinks
        if !canonical.starts_with(&self.root) {
            return Err(ToolError::SandboxViolation {
                path: path.as_ref().display().to_string(),
            });
        }

        Ok(canonical)
    }

    /// Resolve a path for a new file (doesn't need to exist).
    ///
    /// This validates that the target path would be within the sandbox
    /// but doesn't require the file to already exist.
    ///
    /// # Errors
    /// Returns error if path would escape sandbox.
    pub fn resolve_new(&self, path: impl AsRef<Path>) -> Result<PathBuf, ToolError> {
        self.resolve(path)
    }
}

/// Normalize a path by resolving `.` and `..` components.
///
/// Unlike `canonicalize`, this doesn't require the path to exist.
fn normalize_path(path: &Path) -> PathBuf {
    let mut components = Vec::new();

    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                // Go up one level if possible
                if !components.is_empty() {
                    components.pop();
                }
            }
            std::path::Component::CurDir => {
                // Skip current dir references
            }
            other => {
                components.push(other);
            }
        }
    }

    components.iter().collect()
}

#[cfg(test)]
mod tests {
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
    fn test_resolve_existing() {
        let (sandbox, dir) = create_sandbox();

        // Create a file
        let file_path = dir.path().join("test.txt");
        std::fs::write(&file_path, "hello").unwrap();

        // Should resolve
        let resolved = sandbox.resolve_existing("test.txt").unwrap();
        assert_eq!(resolved, file_path.canonicalize().unwrap());
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
}
