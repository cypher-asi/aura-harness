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
    /// The primary root directory (workspace).
    root: PathBuf,
    /// Additional allowed roots granted by skill permissions.
    extra_roots: Vec<PathBuf>,
}

impl Sandbox {
    /// Create a new sandbox with the given root.
    ///
    /// # Errors
    /// Returns error if root cannot be canonicalized.
    pub fn new(root: impl AsRef<Path>) -> Result<Self, ToolError> {
        let root = root.as_ref();

        if !root.exists() {
            std::fs::create_dir_all(root).map_err(|e| {
                ToolError::Io(std::io::Error::new(
                    e.kind(),
                    format!("create_dir_all({}): {e}", root.display()),
                ))
            })?;
        }

        let root = strip_unc_prefix(&root.canonicalize().map_err(|e| {
            ToolError::Io(std::io::Error::new(
                e.kind(),
                format!("canonicalize({}): {e}", root.display()),
            ))
        })?);
        debug!(?root, "Sandbox initialized");

        Ok(Self {
            root,
            extra_roots: Vec::new(),
        })
    }

    /// Create a sandbox with extra allowed roots (from skill permissions).
    ///
    /// Each extra root is canonicalized. Roots that don't exist or can't
    /// be canonicalized are silently skipped (logged as warnings).
    ///
    /// # Errors
    /// Returns error if the primary root cannot be canonicalized.
    pub fn with_extra_roots(root: impl AsRef<Path>, extra: &[PathBuf]) -> Result<Self, ToolError> {
        let mut sandbox = Self::new(root)?;
        for path in extra {
            if !path.exists() {
                debug!(?path, "Extra sandbox root does not exist, skipping");
                continue;
            }
            match path.canonicalize() {
                Ok(canonical) => {
                    let canonical = strip_unc_prefix(&canonical);
                    debug!(?canonical, "Added extra sandbox root");
                    sandbox.extra_roots.push(canonical);
                }
                Err(e) => {
                    debug!(?path, error = %e, "Failed to canonicalize extra root, skipping");
                }
            }
        }
        Ok(sandbox)
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

        if is_workspace_root_alias(path) {
            debug!(original = ?path, resolved = ?self.root, "Path resolved to workspace root alias");
            return Ok(self.root.clone());
        }

        // Join with root if relative
        let joined = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };

        // Normalize the path (handle .., ., etc.)
        let normalized = normalize_path(&joined);

        if !self.is_within_allowed(&normalized) {
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
        let resolved = match self.resolve(path.as_ref()) {
            Ok(resolved) => resolved,
            Err(err @ ToolError::SandboxViolation { .. }) => {
                let candidate = if path.as_ref().is_absolute() {
                    path.as_ref().to_path_buf()
                } else {
                    self.root.join(path.as_ref())
                };

                if !candidate.exists() {
                    return Err(err);
                }

                let canonical = strip_unc_prefix(&candidate.canonicalize().map_err(|e| {
                    ToolError::Io(std::io::Error::new(
                        e.kind(),
                        format!("canonicalize({}): {e}", candidate.display()),
                    ))
                })?);

                if self.is_within_allowed(&canonical) {
                    return Ok(canonical);
                }

                return Err(err);
            }
            Err(err) => return Err(err),
        };

        if !resolved.exists() {
            return Err(ToolError::PathNotFound(path.as_ref().display().to_string()));
        }

        let canonical = strip_unc_prefix(&resolved.canonicalize().map_err(|e| {
            ToolError::Io(std::io::Error::new(
                e.kind(),
                format!("canonicalize({}): {e}", resolved.display()),
            ))
        })?);

        if !self.is_within_allowed(&canonical) {
            return Err(ToolError::SandboxViolation {
                path: path.as_ref().display().to_string(),
            });
        }

        Ok(canonical)
    }

    /// Check whether a path falls under the primary root or any extra root.
    fn is_within_allowed(&self, path: &Path) -> bool {
        let normalized = strip_unc_prefix(path);
        if normalized.starts_with(&self.root) {
            return true;
        }
        self.extra_roots.iter().any(|r| normalized.starts_with(r))
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

/// Strip the `\\?\` verbatim prefix that Windows `canonicalize()` adds.
/// On non-Windows this is a no-op.
pub(crate) fn strip_unc_prefix(path: &Path) -> PathBuf {
    let s = path.to_string_lossy();
    s.strip_prefix(r"\\?\")
        .map_or_else(|| path.to_path_buf(), PathBuf::from)
}

fn is_workspace_root_alias(path: &Path) -> bool {
    use std::path::Component;

    let mut components = path.components();
    matches!(
        (components.next(), components.next(), components.next()),
        (Some(Component::RootDir), None, None)
            | (Some(Component::Prefix(_)), Some(Component::RootDir), None)
    )
}

#[cfg(test)]
mod tests;
