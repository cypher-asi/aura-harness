//! Where a repo's queue keeps its things.
//!
//! - The queue file and its locks live in `<git-common-dir>/aura`
//!   ([`RepoPaths::queue_dir`]): small, and shared by every worktree and
//!   clone path of the repo, which is what keys the queue.
//! - The queue's own worktree and its check log live outside `.git`, in
//!   a per-repo work folder ([`RepoPaths::work`]) under the user's cache
//!   folder: `<cache>/aura/merge-queue/<repo>-<hash of the common dir>`.
//!   Build output there (a `target/`, `node_modules/`) can be large and is
//!   only a cache, so it does not belong inside the repository.
//!
//! Before the move the worktree was `<common>/aura/merge-worktree`; see
//! [`RepoPaths::legacy_worktree`] and `integrate::worktree`, which retires
//! it.

use crate::error::QueueError;
use crate::git;
use std::path::{Path, PathBuf};

/// Folder of the work folders, under the cache folder.
const WORK_ROOT: &str = "aura/merge-queue";

/// The repo a queue lands entries in, and where the queue keeps its
/// things.
#[derive(Debug, Clone)]
pub struct RepoPaths {
    /// Top of the repo's main worktree
    pub top: PathBuf,
    /// The repo's common git dir
    pub common: PathBuf,
    /// The queue's work folder: its worktree and check log
    work: PathBuf,
}

impl RepoPaths {
    /// The repo `dir` is in, with its work folder under the user's cache
    /// folder (the temp folder when there is none).
    ///
    /// # Errors
    /// `dir` is not in a git repository.
    pub async fn of(dir: &Path) -> Result<Self, QueueError> {
        let timeout = crate::config::GIT_TIMEOUT;
        let top = git::main_top(dir, timeout).await?;
        let common = git::common_dir(dir, timeout).await?;
        Ok(Self::new(top, common, &default_work_root()))
    }

    /// The repo at `top` with common git dir `common`, its work folder
    /// under `work_root`.
    #[must_use]
    pub fn new(top: PathBuf, common: PathBuf, work_root: &Path) -> Self {
        let work = work_root.join(work_name(&top, &common));
        Self { top, common, work }
    }

    /// This repo with its work folder under `work_root` instead.
    #[must_use]
    pub fn with_work_root(self, work_root: &Path) -> Self {
        Self::new(self.top, self.common, work_root)
    }

    /// Where the queue keeps its file and locks.
    #[must_use]
    pub fn queue_dir(&self) -> PathBuf {
        self.common.join("aura")
    }

    /// The queue's work folder.
    #[must_use]
    pub fn work(&self) -> &Path {
        &self.work
    }

    /// The queue's own worktree.
    #[must_use]
    pub fn worktree(&self) -> PathBuf {
        self.work.join("worktree")
    }

    /// Where a check's output goes.
    #[must_use]
    pub fn check_log(&self) -> PathBuf {
        self.work.join("check.log")
    }

    /// Where the worktree was before it moved out of `.git`.
    #[must_use]
    pub fn legacy_worktree(&self) -> PathBuf {
        self.queue_dir().join("merge-worktree")
    }

    /// Where the check log was before it moved out of `.git`.
    #[must_use]
    pub fn legacy_check_log(&self) -> PathBuf {
        self.queue_dir().join("check.log")
    }
}

/// The folder work folders go in.
fn default_work_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(WORK_ROOT)
}

/// The work folder's name: the repo folder's name, made safe, and a hash
/// of the common git dir, so two repos of one name never share one.
fn work_name(top: &Path, common: &Path) -> String {
    let name: String = top
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .take(40)
        .collect();
    let name = if name.is_empty() {
        "repo".to_string()
    } else {
        name
    };
    format!(
        "{name}-{:016x}",
        fnv1a(common.as_os_str().as_encoded_bytes())
    )
}

/// 64-bit FNV-1a: stable across runs and builds, unlike the std hasher.
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_work_folder_is_outside_git_and_unique_per_repo() {
        let root = Path::new("/cache");
        let a = RepoPaths::new("/src/zode".into(), "/src/zode/.git".into(), root);
        let b = RepoPaths::new("/other/zode".into(), "/other/zode/.git".into(), root);
        assert!(a.work().starts_with("/cache"));
        assert!(!a.worktree().starts_with(&a.common));
        assert!(!a.check_log().starts_with(&a.common));
        assert_ne!(a.work(), b.work(), "same name, other repo");
        let name = a.work().file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.starts_with("zode-") && name.len() == "zode-".len() + 16,
            "{name}"
        );
        assert_eq!(a.queue_dir(), PathBuf::from("/src/zode/.git/aura"));
        assert_eq!(
            a.legacy_worktree(),
            PathBuf::from("/src/zode/.git/aura/merge-worktree")
        );
    }

    #[test]
    fn the_work_name_is_stable_and_safe() {
        let common = Path::new("/r/.git");
        let named = work_name(Path::new("/r/my repo!"), common);
        assert!(named.starts_with("my_repo_-"), "{named}");
        assert_eq!(named, work_name(Path::new("/r/my repo!"), common));
        assert!(work_name(Path::new("/"), common).starts_with("repo-"));
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
    }
}
