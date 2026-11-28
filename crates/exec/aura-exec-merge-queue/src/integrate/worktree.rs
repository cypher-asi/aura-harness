//! The queue's own worktree, in its work folder outside `.git`.
//!
//! ## Invariants
//!
//! - Only the queue's own worktrees are ever removed: the one in its work
//!   folder when its folder is there but git no longer knows it, and the
//!   one from before the move, `<common>/aura/merge-worktree`.
//! - This runs only in the processor, which holds the repo's processor
//!   lock, so no other processor is using either worktree.

use super::{GitIntegrator, Step, Stop};
use crate::git;
use crate::report::FailureKind;
use std::path::PathBuf;

impl GitIntegrator {
    /// Put the queue's worktree at `target`, clean, creating it if needed.
    pub(super) async fn prepare_worktree(&self, target: &str) -> Step<()> {
        let worktree = self.repo.worktree();
        let timeout = self.config.git_timeout;
        if !worktree.join(".git").exists() {
            self.retire_legacy_worktree().await;
            let top = &self.repo.top;
            git::run(top, &["worktree", "prune"], timeout).await?;
            // A folder git no longer knows (its `.git` was cleaned away
            // with the cache) is ours to start again.
            remove_folder(worktree.clone()).await?;
            create_folder(self.repo.work().to_path_buf()).await?;
            let path = worktree.display().to_string();
            git::ok(
                top,
                &["worktree", "add", "--detach", &path, target],
                timeout,
            )
            .await?;
            return Ok(());
        }
        // A merge a crash left open; there is usually none.
        git::run(&worktree, &["merge", "--abort"], timeout).await?;
        git::ok(
            &worktree,
            &["checkout", "--detach", "--force", target],
            timeout,
        )
        .await?;
        git::ok(&worktree, &["reset", "--hard", target], timeout).await?;
        // Ignored files (build caches) stay, so the next build is quick.
        git::ok(&worktree, &["clean", "-fd"], timeout).await?;
        Ok(())
    }

    /// Remove the worktree and check log the queue kept in `.git` before
    /// they moved out: `git worktree remove`, else delete the folder and
    /// prune. A failure is logged and left for the next start; the queue
    /// works without it.
    pub(super) async fn retire_legacy_worktree(&self) {
        let legacy = self.repo.legacy_worktree();
        let log = self.repo.legacy_check_log();
        if log.exists() {
            if let Err(error) = tokio::fs::remove_file(&log).await {
                tracing::warn!(path = %log.display(), %error, "Could not remove the old check log");
            }
        }
        if !legacy.exists() {
            return;
        }
        let timeout = self.config.git_timeout;
        let top = &self.repo.top;
        let path = legacy.display().to_string();
        let removed = git::run(top, &["worktree", "remove", "--force", &path], timeout).await;
        if !removed.as_ref().is_ok_and(|out| out.ok) || legacy.exists() {
            if let Err(Stop::Fail(why)) = remove_folder(legacy.clone()).await {
                tracing::warn!(path = %path, why = %why.reason, "Could not remove the old merge worktree");
                return;
            }
            if let Err(error) = git::run(top, &["worktree", "prune"], timeout).await {
                tracing::warn!(%error, "Could not prune the old merge worktree");
            }
        }
        tracing::info!(path = %path, "Removed the merge queue's old worktree from .git");
    }
}

/// Delete `folder` and everything in it, when it is there.
async fn remove_folder(folder: PathBuf) -> Step<()> {
    blocking(move || match std::fs::remove_dir_all(&folder) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("removing {}: {e}", folder.display())),
    })
    .await
}

/// Create `folder` and its parents.
async fn create_folder(folder: PathBuf) -> Step<()> {
    blocking(move || {
        std::fs::create_dir_all(&folder).map_err(|e| format!("creating {}: {e}", folder.display()))
    })
    .await
}

async fn blocking(work: impl FnOnce() -> Result<(), String> + Send + 'static) -> Step<()> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| Stop::fail(FailureKind::Other, format!("file work stopped: {e}")))?
        .map_err(|why| Stop::fail(FailureKind::Other, why))
}
