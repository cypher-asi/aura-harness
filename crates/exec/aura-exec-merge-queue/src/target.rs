//! Which branch a repo's queue merges into, read the same way by
//! everything that needs it: the queue, `git_ship`, the push guard,
//! cloud machines and the QUEUE tab.
//!
//! The target is what `.aura/merge-queue.toml` says as **committed** on
//! the repo's `main` (the local branch, else `origin/main`), so an
//! uncommitted edit in some checkout can't move it, and every worktree
//! and clone of a repo agrees. Without that file the target is
//! [`DEFAULT_TARGET`].
//!
//! A repo with no `main` at all falls back to the file in the working
//! tree at `dir`, as the queue read it before.
//!
//! Every git command here only reads.

use crate::config::{target_in, CONFIG_PATH, DEFAULT_TARGET, GIT_TIMEOUT};
use crate::git;
use std::path::Path;

/// Where the committed config is looked for, first that exists.
const CONFIG_REFS: [&str; 2] = ["refs/heads/main", "refs/remotes/origin/main"];

/// The target branch of the repo `dir` is in (see the module docs). A
/// directory that is not a repo, or a git that fails, gives the working
/// tree's file or [`DEFAULT_TARGET`]: never an error, so callers always
/// have a branch to name.
pub async fn resolve_target(dir: &Path) -> String {
    let working = working_target(dir).await;
    let Some(committed) = committed_target(dir).await else {
        return working.unwrap_or_else(|| DEFAULT_TARGET.to_string());
    };
    if let Some(working) = working.filter(|working| *working != committed) {
        tracing::warn!(
            dir = %dir.display(),
            %committed,
            uncommitted = %working,
            "The working tree's merge queue config names another target than the committed one; \
             the committed one is used"
        );
    }
    committed
}

/// The target the committed config names, or [`DEFAULT_TARGET`] when
/// `main` has no config; `None` when there is no `main` to read.
async fn committed_target(dir: &Path) -> Option<String> {
    for reference in CONFIG_REFS {
        let commit = format!("{reference}^{{commit}}");
        let found = git::run(
            dir,
            &["rev-parse", "--verify", "--quiet", &commit],
            GIT_TIMEOUT,
        )
        .await;
        if !found.is_ok_and(|out| out.ok && !out.stdout.is_empty()) {
            continue;
        }
        let spec = format!("{reference}:{CONFIG_PATH}");
        let text = git::run(dir, &["show", &spec], GIT_TIMEOUT).await.ok();
        let named = text
            .filter(|out| out.ok)
            .and_then(|out| target_in(&out.stdout));
        return Some(named.unwrap_or_else(|| DEFAULT_TARGET.to_string()));
    }
    None
}

/// The target the working tree's config at `dir` names, if any.
async fn working_target(dir: &Path) -> Option<String> {
    let top = git::main_top(dir, GIT_TIMEOUT)
        .await
        .unwrap_or_else(|_| dir.to_path_buf());
    let mut candidates = vec![dir.join(CONFIG_PATH)];
    if top != dir {
        candidates.push(top.join(CONFIG_PATH));
    }
    for path in candidates {
        if let Ok(text) = tokio::fs::read_to_string(&path).await {
            return target_in(&text);
        }
    }
    None
}

#[cfg(test)]
mod tests;
