//! `git_commit_push_impl` — the transactional `add -A`+`commit`+`push`
//! group the dev-loop relies on.
//!
//! Reports commit and push outcomes independently so a push-only
//! failure does not mask a successful commit. The orchestrator uses
//! this to emit `GitCommitted` alongside `GitPushFailed` when the
//! commit landed locally but the push timed out.

use std::time::Duration;

use tracing::instrument;

use super::commit::git_commit_impl;
use super::push::git_push_impl;
use super::{CommitPushOutcome, GitToolError, PushPolicy};

/// Combined `add -A` + `commit` + `push` — the transactional group the
/// dev-loop relies on.
///
/// Commit failures propagate via the outer `Result` (nothing landed).
/// Push failures are reported inline via [`CommitPushOutcome::push_result`]
/// so the caller still sees the commit SHA — the task's work is locally
/// persisted even if Orbit wasn't reachable.
#[instrument(skip_all, fields(op = "commit_and_push", branch = %branch))]
#[allow(clippy::too_many_arguments)]
pub async fn git_commit_push_impl(
    workspace: &std::path::Path,
    message: &str,
    remote_url: &str,
    branch: &str,
    jwt: &str,
    force: bool,
    commit_timeout: Duration,
    push_policy: PushPolicy,
) -> Result<CommitPushOutcome, GitToolError> {
    let commit_sha = git_commit_impl(workspace, message, commit_timeout).await?;
    let push_result = git_push_impl(workspace, remote_url, branch, jwt, force, push_policy).await;
    Ok(CommitPushOutcome {
        commit_sha,
        push_result,
    })
}
