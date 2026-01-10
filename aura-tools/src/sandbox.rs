//! Sandbox for path validation.
//!
//! Ensures all paths resolve within the workspace root.

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
}
