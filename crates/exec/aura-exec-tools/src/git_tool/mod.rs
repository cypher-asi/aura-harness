//! Git mutation tools (`git_commit`, `git_push`, `git_commit_push`,
//! `git_ship`).
//!
//! This module centralises every mutating `git` subprocess invocation
//! behind the kernel-mediated tool pipeline. See `docs/invariants.md` §1
//! (Sole External Gateway): spawning `git` with `add`, `commit`, or
//! `push` outside this module is a bug. Read-only git inspection
//! (`diff`, `status`, `log`) remains an [exception](../../../docs/invariants.md)
//! and lives in `aura-agent/src/git.rs`.
//!
//! Each tool in this module:
//! - Uses `tokio::process::Command` with a hard timeout.
//! - Operates strictly inside `ToolContext::sandbox.root()` — the kernel
//!   rooted workspace. No caller-supplied cwd is honored, and git never
//!   looks above the root for a repository: a root that isn't the top of
//!   its own repository fails instead of committing the checkout around it.
//! - Restricts itself to the allow-listed subcommand (`add`, `commit`,
//!   `push`, `fetch` and `merge` for `git_ship`'s pull, plus the
//!   read-only helpers `diff --cached`, `rev-parse` and `remote get-url`
//!   used to implement commit and push).
//! - Never updates a [`PROTECTED_BRANCHES`] branch: the merge queue's
//!   target only changes through the queue.
//! - Takes no credentials as arguments: `git push` goes to `origin` and
//!   authenticates with git's own configuration (a credential helper or
//!   SSH key; on a cloud machine, the credential store its claim wrote,
//!   which holds only a placeholder).
//! - Scrubs credentials from structured tracing output.
//!
//! Tool availability is resolved by the kernel's tri-state policy
//! (`UserToolDefaults` plus optional `AgentToolPermissions`). Even when
//! these tools resolve to `on`, this module still enforces workspace
//! confinement, subcommand allowlists, timeouts, the runtime's pinned
//! push branch ([`crate::ToolConfig::push_branch`]) and credential
//! scrubbing.
//!
//! ## Layout (Phase 2b)
//!
//! - [`mod@executor`] — every `Command::new("git")` call site lives
//!   here. Owns the subcommand allowlist + timeout-wrapped `spawn_git`.
//! - [`mod@redact`] — `redact_url` (mask user-info in tracing output).
//! - [`mod@commit`] — `git_commit_impl` (`add -A`, `diff --cached`,
//!   `commit`, `rev-parse HEAD`).
//! - [`mod@push`] — `git_push_impl` plus the branch checks, the retry
//!   loop and the failure classifiers.
//! - [`mod@commit_push`] — `git_commit_push_impl` (the transactional
//!   commit-and-push pair the dev-loop relies on).
//! - [`mod@ship`] — `git_ship_impl` (commit, pull the target branch,
//!   push, queue: how an agent finishes a task).
//! - [`mod@sandbox`] — `ToolContext` bridging helpers
//!   (`workspace_timeout`, `push_policy_for`, `push_branch_for`,
//!   `str_arg`, `opt_str`, `opt_bool`).
//! - [`mod@tools`] — the commit and push `Tool` impls and their argument
//!   parsing and result shaping; [`mod@tools_ship`] holds `git_ship`'s.
//! - `tests` — unit tests for the helpers above; declared
//!   `#[cfg(test)] mod tests;` so the module path matches `super::*`.
//!
//! `mod.rs` keeps the public-facing surface: [`GitToolError`],
//! [`CommitInfo`], [`PushPolicy`], [`PushTarget`], [`Pushed`],
//! [`CommitPushOutcome`] and the `GIT_*_TOOL_NAMES` constants used by
//! the catalog. `pub use` re-exports the impl functions and the `Tool`
//! impls so the `aura_exec_tools::git_tool::git_push_impl` and
//! `aura_exec_tools::git_tool::GitPushTool` paths keep resolving.

use std::time::Duration;

use aura_exec_merge_queue::{QueuedPush, MERGE_QUEUE_KEY};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::ToolError;

mod commit;
mod commit_push;
mod executor;
mod push;
mod redact;
mod sandbox;
mod ship;
mod tools;
mod tools_ship;

#[cfg(test)]
mod tests;

pub use commit::git_commit_impl;
pub use commit_push::git_commit_push_impl;
pub use push::git_push_impl;
pub use ship::{git_ship_impl, ShipRequest, Shipped};
pub use tools::{GitCommitPushTool, GitCommitTool, GitPushTool};
pub use tools_ship::GitShipTool;

// -----------------------------------------------------------------------
// Error type
// -----------------------------------------------------------------------

/// Errors surfaced from the `git_*` tools.
///
/// Kept as a dedicated `thiserror` enum so `aura-exec-tools` stays free of
/// `anyhow` (per crate rules) and callers can match on specific
/// failure modes without string-matching.
#[derive(Debug, Error)]
pub enum GitToolError {
    #[error("git subcommand '{0}' is not on the allow-list (add, commit, push)")]
    DisallowedSubcommand(String),

    #[error("missing required argument: {0}")]
    MissingArg(&'static str),

    #[error("invalid argument {name}: {reason}")]
    InvalidArg { name: &'static str, reason: String },

    #[error("git {op} failed with exit code {exit_code}: {stderr}")]
    NonZeroExit {
        op: &'static str,
        exit_code: i32,
        stderr: String,
    },

    #[error("git {0} timed out after {1:?}")]
    Timeout(&'static str, Duration),

    #[error("failed to spawn git {0}: {1}")]
    Spawn(&'static str, std::io::Error),

    /// The workspace has no `origin` to push to.
    #[error(
        "this workspace has no `origin` remote, so there is nowhere to push; \
         commits stay local (a cloud agent's commits come back with the next pull)"
    )]
    NoOrigin,

    /// `HEAD` is on no branch and no `branch` was given.
    #[error("HEAD is detached: pass `branch` to name the branch to push")]
    DetachedHead,

    /// The runtime pinned this workspace to one branch
    /// ([`crate::ToolConfig::push_branch`]) and the push named another.
    #[error("this machine pushes only its own branch `{allowed}`; pushing `{branch}` is refused")]
    BranchNotAllowed { branch: String, allowed: String },

    /// The push named a [`PROTECTED_BRANCHES`] branch, which only the
    /// merge queue updates.
    #[error(
        "`{branch}` only changes through the merge queue: push your own branch \
         (git_ship does it) and the queue merges it"
    )]
    ProtectedBranch { branch: String },

    /// `git_ship`'s merge of the target branch into the agent's branch
    /// conflicted; the merge was aborted, so the branch is as it was.
    #[error(
        "merging origin/{target} into your branch conflicts in: {}. The merge was aborted \
         and nothing was pushed. Merge {target} into your branch yourself, resolve the \
         conflicts, commit, and call git_ship again",
        files.join(", ")
    )]
    MergeConflict { target: String, files: Vec<String> },

    /// The remote refused the credentials or they grant no access to
    /// the repository. Not retried.
    #[error(
        "push to {remote} was not authorized: {} (server reported: {stderr})",
        if *cloud {
            "the GitHub App isn't installed on this repository or lacks permission"
        } else {
            "check your git credentials for this remote"
        }
    )]
    Unauthorized {
        remote: String,
        /// Pushed from a cloud machine (with the GitHub App's token).
        cloud: bool,
        stderr: String,
    },

    /// The remote rejected the push because it could not persist the
    /// objects (out of disk, HTTP 507, disk quota exceeded, etc.).
    /// This is deliberately a non-retryable variant because retrying
    /// within the dev-loop's backoff window cannot heal remote
    /// storage — the operator must free space or redirect the remote
    /// before the next push attempt. Surfaced as a specific variant
    /// so the dev-loop can render an actionable recovery hint instead
    /// of a generic "push failed" message.
    #[error(
        "remote storage exhausted on git {op}; free space on the remote or switch remotes. \
         server reported: {stderr}"
    )]
    RemoteStorageExhausted { op: &'static str, stderr: String },
}

impl From<GitToolError> for ToolError {
    fn from(value: GitToolError) -> Self {
        match value {
            GitToolError::DisallowedSubcommand(s) => Self::CommandNotAllowed(s),
            GitToolError::MissingArg(name) => {
                Self::InvalidArguments(format!("missing required field '{name}'"))
            }
            GitToolError::InvalidArg { name, reason } => {
                Self::InvalidArguments(format!("{name}: {reason}"))
            }
            GitToolError::Timeout(_, d) => Self::CommandTimeout {
                timeout_ms: u64::try_from(d.as_millis()).unwrap_or(u64::MAX),
            },
            GitToolError::Spawn(_, e) => Self::Io(e),
            GitToolError::NonZeroExit {
                op,
                exit_code,
                stderr,
            } => Self::CommandFailed(format!("git {op} exited {exit_code}: {stderr}")),
            err @ (GitToolError::DetachedHead
            | GitToolError::BranchNotAllowed { .. }
            | GitToolError::ProtectedBranch { .. }) => Self::InvalidArguments(err.to_string()),
            err @ (GitToolError::NoOrigin
            | GitToolError::Unauthorized { .. }
            | GitToolError::MergeConflict { .. }) => Self::CommandFailed(err.to_string()),
            GitToolError::RemoteStorageExhausted { op, stderr } => Self::CommandFailed(format!(
                "git {op} failed: remote storage exhausted ({stderr}). \
                 Free space on the remote (or switch remotes) before retrying."
            )),
        }
    }
}

// -----------------------------------------------------------------------
// Public types
// -----------------------------------------------------------------------

/// Metadata recorded for each commit surfaced by `git_push`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
}

/// Per-attempt timeout and attempt budget for `git push`.
///
/// Push is a network operation and benefits from a different budget
/// than the rest of the git tools. Keeping push on its own knob also
/// makes it safe to retry: a single slow remote shouldn't force us to
/// raise every other tool's ceiling.
#[derive(Debug, Clone, Copy)]
pub struct PushPolicy {
    /// Per-attempt timeout passed to the underlying `git push`
    /// subprocess.
    pub per_attempt_timeout: Duration,
    /// Total number of attempts, including the initial one. Values
    /// below 1 are coerced to 1 at the call-site.
    pub attempts: u32,
}

impl PushPolicy {
    /// Single-attempt policy with `timeout` — preserves the pre-retry
    /// behavior for callers that don't care about the retry surface
    /// (tests, ad-hoc tools).
    pub const fn single(timeout: Duration) -> Self {
        Self {
            per_attempt_timeout: timeout,
            attempts: 1,
        }
    }

    /// Build a push policy from the crate's `ToolConfig` knobs
    /// (`git_push_timeout_ms` + `git_push_attempts`). Any `attempts`
    /// value below 1 is coerced to 1 so the call-site is free to
    /// trust the `attempts` field non-zero.
    pub fn from_config(config: &crate::ToolConfig) -> Self {
        Self {
            per_attempt_timeout: Duration::from_millis(config.git_push_timeout_ms),
            attempts: config.git_push_attempts.max(1),
        }
    }
}

/// What a push updates on `origin`.
#[derive(Debug, Clone, Copy, Default)]
pub struct PushTarget<'a> {
    /// Branch to update; `None` pushes to the current branch's name.
    pub branch: Option<&'a str>,
    /// Overwrite the remote branch even when it isn't an ancestor.
    pub force: bool,
    /// The only branch this workspace may update (a cloud machine's own
    /// branch); `None` allows any.
    pub only_branch: Option<&'a str>,
}

/// A push that landed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pushed {
    /// `origin`'s URL, user-info masked.
    pub remote: String,
    /// The branch it updated.
    pub branch: String,
    /// The commit the branch points at after the push.
    pub head: String,
    /// Commits `origin` didn't have as far as this repository knew
    /// (best-effort, at most 50).
    pub commits: Vec<CommitInfo>,
}

impl Pushed {
    /// The merge queue's marker for this push. Its `remote` is the
    /// remote's name (`origin`), not the masked URL: the queue fetches
    /// from and lands through it with git.
    #[must_use]
    pub fn marker(&self) -> QueuedPush {
        QueuedPush {
            branch: self.branch.clone(),
            head: self.head.clone(),
            remote: Some(push::REMOTE.to_string()),
        }
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "pushed": true,
            "remote": self.remote,
            "branch": self.branch,
            "commits": self
                .commits
                .iter()
                .map(|c| serde_json::json!({"sha": c.sha, "message": c.message}))
                .collect::<Vec<_>>(),
            MERGE_QUEUE_KEY: self.marker().to_json(),
            "queued": true,
        })
    }
}

/// Branches no push may update: whichever of them is the merge queue's
/// target only moves when the queue lands an entry on it.
pub const PROTECTED_BRANCHES: &[&str] = &["main", "master"];

/// Refuse `branch` when it is one of [`PROTECTED_BRANCHES`].
pub(crate) fn check_not_protected(branch: &str) -> Result<(), GitToolError> {
    if PROTECTED_BRANCHES.contains(&branch) {
        return Err(GitToolError::ProtectedBranch {
            branch: branch.to_string(),
        });
    }
    Ok(())
}

/// Outcome of [`git_commit_push_impl`].
///
/// The commit half and the push half are reported independently so a
/// push-only failure does not mask a successful commit. Callers
/// preserve the commit SHA in the task history instead of pretending
/// the work never happened when only the push half fails.
#[derive(Debug)]
pub struct CommitPushOutcome {
    /// Commit SHA from the local `git commit`, or `None` when there
    /// were no staged changes (i.e. the tool was a no-op).
    pub commit_sha: Option<String>,
    /// Result of the subsequent `git push`. `Ok` on success, `Err`
    /// on any push-specific failure (timeout, non-zero exit, spawn
    /// error). Independent of `commit_sha` — the commit SHA stays
    /// populated even when the push fails.
    pub push_result: Result<Pushed, GitToolError>,
}

/// Git tools that operate purely on the local workspace and never
/// reach a remote. Safe to set `on` for any automaton
/// that already has write access to the workspace — the mutation is
/// contained by the `GitExecutor` workspace-escape checks.
pub const GIT_LOCAL_TOOL_NAMES: &[&str] = &["git_commit"];

/// Git tools that talk to a remote: they push to the workspace's
/// `origin` with git's configured credentials (`git_ship` also fetches
/// from it). Without a remote they fail (`git_ship` then only commits
/// and queues the local branch); they should usually stay ask-gated by
/// the caller's tri-state tool policy so an agent does not push to an
/// upstream unasked.
pub const GIT_REMOTE_TOOL_NAMES: &[&str] = &["git_push", "git_commit_push", "git_ship"];

/// The full set of git tool names registered by this module.
pub const GIT_TOOL_NAMES: &[&str] = &["git_commit", "git_push", "git_commit_push", "git_ship"];
