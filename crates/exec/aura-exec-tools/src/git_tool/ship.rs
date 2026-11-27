//! `git_ship_impl` — how an agent finishes a task: commit every change,
//! pull the merge queue's target branch into its own branch, push that
//! branch, and hand the push to the merge queue (the result's marker).
//!
//! Invariants:
//! - The shipped branch is never one of [`PROTECTED_BRANCHES`]; that is
//!   checked before anything is committed, so shipping from `main`
//!   changes nothing.
//! - A pull that conflicts leaves no merge in progress (`merge --abort`)
//!   and pushes nothing; the agent's commit stays on its branch.
//! - Without `origin` nothing is fetched or pushed, and the local branch
//!   is still queued (a marker with `remote: None`).
//!
//! Every git step runs with a timeout: the local ones with
//! [`ShipRequest::timeout`], the fetch with the push policy's
//! per-attempt timeout, the push with the push policy.
//!
//! [`PROTECTED_BRANCHES`]: super::PROTECTED_BRANCHES

use std::path::Path;
use std::time::Duration;

use aura_exec_merge_queue::{QueuedPush, MERGE_QUEUE_KEY};
use tracing::{info, instrument};

use super::commit::git_commit_impl;
use super::executor::{head_commit, spawn_git};
use super::push::{
    check_branch_name, current_branch, git_push_impl, remote_url, resolve_branch, REMOTE,
};
use super::{GitToolError, PushPolicy, PushTarget, Pushed};

/// Branch `git_ship` pulls when the call names none.
pub(super) const DEFAULT_TARGET: &str = "main";

/// What a `git_ship` call asks for.
#[derive(Debug, Clone, Copy)]
pub struct ShipRequest<'a> {
    /// Commit message for the uncommitted changes.
    pub message: &'a str,
    /// Branch of `origin` merged in before the push (the queue's target).
    pub target: &'a str,
    /// What the push updates: the current branch, within the runtime's pin.
    pub push: PushTarget<'a>,
    /// Timeout of each local git step (`add`, `commit`, `merge`, ...).
    pub timeout: Duration,
    /// Budget of the network steps; `fetch` uses its per-attempt timeout.
    pub push_policy: PushPolicy,
}

/// A finished `git_ship`.
#[derive(Debug, Clone)]
pub struct Shipped {
    /// The commit of the uncommitted changes; `None` when there were none.
    pub commit: Option<String>,
    /// What the pull did: `merged origin/<target>`, `up to date` or
    /// `skipped: <why>`.
    pub pulled: String,
    /// The shipped branch.
    pub branch: String,
    /// The commit the shipped branch points at.
    pub head: String,
    /// The push; `None` when the workspace has no `origin`.
    pub pushed: Option<Pushed>,
}

impl Shipped {
    /// The merge queue's marker for this ship.
    #[must_use]
    pub fn marker(&self) -> QueuedPush {
        self.pushed.as_ref().map_or_else(
            || QueuedPush {
                branch: self.branch.clone(),
                head: self.head.clone(),
                remote: None,
            },
            Pushed::marker,
        )
    }

    /// The tool result.
    pub(super) fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "shipped": true,
            "commit": self.commit,
            "pulled": self.pulled,
            "pushed": self.pushed.is_some(),
            "branch": self.branch,
            "remote": self.pushed.as_ref().map(|pushed| pushed.remote.as_str()),
            "head": self.head,
            MERGE_QUEUE_KEY: self.marker().to_json(),
            "queued": true,
        })
    }
}

/// Commit every change, merge `origin/<target>` into the current branch,
/// push the branch and report the push for the merge queue.
///
/// Fails before committing when the branch is protected, pinned away or
/// detached; fails with [`GitToolError::MergeConflict`] (after aborting
/// the merge) when the target conflicts with the branch.
#[instrument(skip_all, fields(op = "ship", target = %request.target))]
pub async fn git_ship_impl(
    workspace: &Path,
    request: ShipRequest<'_>,
) -> Result<Shipped, GitToolError> {
    check_target_name(request.target)?;
    let current = current_branch(workspace, request.timeout).await?;
    let branch = resolve_branch(request.push, current.as_deref())?;
    let commit = git_commit_impl(workspace, request.message, request.timeout).await?;

    let has_origin = match remote_url(workspace, request.timeout).await {
        Ok(_) => true,
        Err(GitToolError::NoOrigin) => false,
        Err(err) => return Err(err),
    };
    if !has_origin {
        let head = head_commit(workspace, request.timeout).await?;
        info!(%branch, %head, "no origin: shipped the local branch");
        return Ok(Shipped {
            commit,
            pulled: "skipped: this workspace has no `origin` remote".to_string(),
            branch,
            head,
            pushed: None,
        });
    }

    let pulled = pull_target(workspace, &request).await?;
    let pushed = git_push_impl(workspace, request.push, request.push_policy).await?;
    info!(branch = %pushed.branch, head = %pushed.head, %pulled, "shipped");
    Ok(Shipped {
        commit,
        pulled,
        branch: pushed.branch.clone(),
        head: pushed.head.clone(),
        pushed: Some(pushed),
    })
}

/// The target goes into a refspec, so it must be a plain branch name.
fn check_target_name(target: &str) -> Result<(), GitToolError> {
    check_branch_name(target).map_err(|_| GitToolError::InvalidArg {
        name: "target",
        reason: format!("{target:?} is not a valid branch name"),
    })
}

/// Fetch `origin/<target>` and merge it into the current branch. An
/// `origin` without the target skips the pull.
async fn pull_target(workspace: &Path, request: &ShipRequest<'_>) -> Result<String, GitToolError> {
    let target = request.target;
    let refspec = format!("refs/heads/{target}");
    let fetched = spawn_git(
        workspace,
        "fetch",
        &["--no-tags", REMOTE, &refspec],
        request.push_policy.per_attempt_timeout,
        "fetch",
    )
    .await?;
    if !fetched.status.success() {
        let stderr = String::from_utf8_lossy(&fetched.stderr).trim().to_string();
        if stderr
            .to_ascii_lowercase()
            .contains("couldn't find remote ref")
        {
            info!(%target, "origin has no target branch; not pulling");
            return Ok(format!("skipped: origin has no branch `{target}`"));
        }
        return Err(GitToolError::NonZeroExit {
            op: "fetch",
            exit_code: fetched.status.code().unwrap_or(-1),
            stderr,
        });
    }
    merge_fetched(workspace, target, request.timeout).await
}

/// Merge `FETCH_HEAD` into the current branch. On failure, abort the
/// merge and name the conflicted files.
async fn merge_fetched(
    workspace: &Path,
    target: &str,
    timeout: Duration,
) -> Result<String, GitToolError> {
    let before = head_commit(workspace, timeout).await?;
    let merged = spawn_git(
        workspace,
        "merge",
        &["--no-edit", "FETCH_HEAD"],
        timeout,
        "merge",
    )
    .await?;
    if merged.status.success() {
        let after = head_commit(workspace, timeout).await?;
        return Ok(if after == before {
            "up to date".to_string()
        } else {
            format!("merged origin/{target}")
        });
    }

    let files = abort_merge(workspace, timeout).await?;
    if files.is_empty() {
        return Err(GitToolError::NonZeroExit {
            op: "merge",
            exit_code: merged.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&merged.stderr).trim().to_string(),
        });
    }
    Err(GitToolError::MergeConflict {
        target: target.to_string(),
        files,
    })
}

/// Abort the merge a failed `git merge` left; returns the files it
/// conflicted in.
async fn abort_merge(workspace: &Path, timeout: Duration) -> Result<Vec<String>, GitToolError> {
    let conflicts = spawn_git(
        workspace,
        "diff",
        &["--name-only", "--diff-filter=U"],
        timeout,
        "diff --diff-filter=U",
    )
    .await?;
    let files = String::from_utf8_lossy(&conflicts.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    // A merge that stopped before starting (nothing to abort) makes
    // `--abort` fail; that is fine, the tree is as it was either way.
    let _aborted = spawn_git(workspace, "merge", &["--abort"], timeout, "merge --abort").await?;
    Ok(files)
}
