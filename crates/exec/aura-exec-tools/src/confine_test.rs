//! For tests of confined commands: a parent folder that confined commands
//! may not write to, so a test can check that writing next to its
//! workspace fails.
//!
//! `/tmp` and the home caches are writable roots ([`super::confine`]), so
//! the parent can't be a plain temp dir. The test's own folder usually
//! isn't one either, but the merge queue checks entries in a worktree
//! under the user's cache folder (`~/.cache/aura-merge-queue/…`), which
//! is: a test that assumed its folder was off-limits failed there.

use crate::confine::writable_roots;
use std::path::{Path, PathBuf};

/// A new temp folder no writable root contains: under the test's folder,
/// else the crate's, else the home folder. `None` (the test should skip)
/// when every one of them is inside a writable root.
pub(crate) fn unwritable_parent() -> Option<tempfile::TempDir> {
    let candidates = [
        std::env::current_dir().ok(),
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR"))),
        std::env::var_os("HOME").map(PathBuf::from),
    ];
    candidates
        .into_iter()
        .flatten()
        .find(|dir| !inside_a_writable_root(dir))
        .and_then(|dir| tempfile::tempdir_in(dir).ok())
}

/// Whether `dir` is inside a root a confined command in a workspace
/// below it may write.
fn inside_a_writable_root(dir: &Path) -> bool {
    let real = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let dir = real(dir);
    let workspace = dir.join("ws");
    writable_roots(&workspace, None)
        .iter()
        .filter(|root| **root != workspace)
        .any(|root| dir.starts_with(real(root)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_parent_is_outside_every_shared_root() {
        assert!(inside_a_writable_root(&std::env::temp_dir()));
        if let Some(parent) = unwritable_parent() {
            assert!(!inside_a_writable_root(parent.path()), "{parent:?}");
        }
    }
}
