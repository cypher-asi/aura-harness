//! Where the confinement tests put a workspace: somewhere whose parent a
//! confined command may not write, so `../outside` must fail.
//!
//! The crate's own folder is that place in a plain checkout. In the merge
//! queue's worktree, under `~/.cache`, it is not: confined commands may
//! write the home caches. So the first candidate outside every shared
//! writable root is used.

use super::{shared_roots, WRITABLE_DEVICES};
use std::path::{Path, PathBuf};

/// A temporary folder to make a workspace in, whose own folder confined
/// commands may not write; `None` when no candidate is one (the test then
/// has nothing to check here).
pub(crate) fn unconfined_parent() -> Option<tempfile::TempDir> {
    let candidates = [std::env::current_dir().ok(), super::home_dir()];
    let dir = candidates
        .into_iter()
        .flatten()
        .find(|dir| !under_a_writable_root(dir))?;
    tempfile::tempdir_in(dir).ok()
}

/// Whether a confined command working below `dir` may write `dir`.
fn under_a_writable_root(dir: &Path) -> bool {
    let probe = dir.join("probe-workspace");
    let dir = canonical(dir);
    shared_roots(&probe)
        .into_iter()
        .chain(WRITABLE_DEVICES.iter().map(PathBuf::from))
        .map(|root| canonical(&root))
        .any(|root| dir.starts_with(root))
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}
