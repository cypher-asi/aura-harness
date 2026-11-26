//! Git mutation tools (`git_commit`, `git_push`, `git_commit_push`).
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
//!   rooted workspace. No caller-supplied cwd is honored.
//! - Restricts itself to the allow-listed subcommand (`add`, `commit`,
//!   `push`, plus the read-only helpers `diff --cached`, `rev-parse`
//!   and `remote get-url` used to implement commit and push).
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
//! - [`mod@sandbox`] — `ToolContext` bridging helpers
//!   (`workspace_timeout`, `push_policy_for`, `push_branch_for`,
//!   `str_arg`, `opt_str`, `opt_bool`).
//! - `tests` — unit tests for the helpers above; declared
//!   `#[cfg(test)] mod tests;` so the module path matches `super::*`.
//!
//! `mod.rs` keeps the public-facing surface: [`GitToolError`],
//! [`CommitInfo`], [`PushPolicy`], [`PushTarget`], [`Pushed`],
//! [`CommitPushOutcome`], the three `Tool` impls, and the
//! `GIT_*_TOOL_NAMES` constants used by the catalog. `pub use`
//! re-exports the impl functions so the
//! `aura_exec_tools::git_tool::git_push_impl` paths keep resolving.

use std::time::Duration;

use async_trait::async_trait;
use aura_core_types::{ToolDefinition, ToolResult};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{info, warn};

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};

mod commit;
mod commit_push;
mod executor;
mod push;
mod redact;
mod sandbox;

#[cfg(test)]
mod tests;

pub use commit::git_commit_impl;
pub use commit_push::git_commit_push_impl;
pub use push::git_push_impl;

use push::duration_millis_u64;
use sandbox::{opt_bool, opt_str, push_branch_for, push_policy_for, str_arg, workspace_timeout};

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
            err @ (GitToolError::DetachedHead | GitToolError::BranchNotAllowed { .. }) => {
                Self::InvalidArguments(err.to_string())
            }
            err @ (GitToolError::NoOrigin | GitToolError::Unauthorized { .. }) => {
                Self::CommandFailed(err.to_string())
            }
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
    /// Commits `origin` didn't have as far as this repository knew
    /// (best-effort, at most 50).
    pub commits: Vec<CommitInfo>,
}

impl Pushed {
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
        })
    }
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

/// The JSON schema properties `git_push` and `git_commit_push` share.
fn push_properties() -> serde_json::Value {
    serde_json::json!({
        "branch": {
            "type": "string",
            "description": "Branch to update on origin. Defaults to the current branch."
        },
        "force": {
            "type": "boolean",
            "description": "Overwrite the remote branch even when it isn't an ancestor of HEAD."
        }
    })
}

/// Read `branch` / `force` from `args`, with the runtime's pinned
/// branch.
fn push_target<'a>(args: &'a serde_json::Value, ctx: &'a ToolContext) -> PushTarget<'a> {
    PushTarget {
        branch: opt_str(args, "branch"),
        force: opt_bool(args, "force"),
        only_branch: push_branch_for(ctx),
    }
}

// -----------------------------------------------------------------------
// Tool implementations — thin shims over the impl functions above.
// -----------------------------------------------------------------------

/// `git_commit` — stage all changes (`git add -A`) and create a commit.
pub struct GitCommitTool;

#[async_trait]
impl Tool for GitCommitTool {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_commit".into(),
            description: "Stage every change under the workspace root (`git add -A`) and \
                create a commit with the provided message. Returns the new commit SHA, or \
                reports when there were no changes to commit. Runs inside the kernel's \
                sandboxed workspace root only."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["message"],
                "properties": {
                    "message": {
                        "type": "string",
                        "description": "Commit message (non-empty)."
                    }
                }
            }),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let message = str_arg(&args, "message").map_err(ToolError::from)?;
        let workspace = ctx.sandbox.root().to_path_buf();
        let timeout = workspace_timeout(ctx);

        let agent_id = ctx
            .caller_agent_id
            .map_or_else(|| "unknown".to_string(), |id| id.to_string());
        info!(op = "git_commit", %agent_id, "git tool dispatched");

        match git_commit_impl(&workspace, message, timeout).await {
            Ok(Some(sha)) => Ok(ToolResult::success(
                "git_commit",
                serde_json::to_string(&serde_json::json!({
                    "sha": sha,
                    "committed": true,
                }))
                .unwrap_or_else(|_| format!("committed {sha}")),
            )),
            Ok(None) => Ok(ToolResult::success(
                "git_commit",
                serde_json::to_string(&serde_json::json!({
                    "sha": null,
                    "committed": false,
                    "reason": "no staged changes",
                }))
                .unwrap_or_else(|_| "no staged changes".to_string()),
            )),
            Err(e) => {
                warn!(error = %e, "git_commit failed");
                Err(e.into())
            }
        }
    }
}

/// `git_push` — push `HEAD` to a branch of `origin` with git's own
/// credentials.
pub struct GitPushTool;

#[async_trait]
impl Tool for GitPushTool {
    fn name(&self) -> &str {
        "git_push"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_push".into(),
            description: "Push HEAD to a branch of the workspace's `origin` remote, \
                authenticating with git's configured credentials. Pushing the current branch \
                also sets it to track origin. Runs inside the kernel's sandboxed workspace root \
                only."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": push_properties(),
            }),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let target = push_target(&args, ctx);
        let workspace = ctx.sandbox.root().to_path_buf();
        let policy = push_policy_for(ctx);

        let agent_id = ctx
            .caller_agent_id
            .map_or_else(|| "unknown".to_string(), |id| id.to_string());
        info!(
            op = "git_push",
            %agent_id,
            target_branch = target.branch.unwrap_or("(current)"),
            pinned_branch = target.only_branch.is_some(),
            per_attempt_timeout_ms = duration_millis_u64(policy.per_attempt_timeout),
            attempts = policy.attempts,
            "git tool dispatched"
        );

        match git_push_impl(&workspace, target, policy).await {
            Ok(pushed) => Ok(ToolResult::success(
                "git_push",
                serde_json::to_string(&pushed.to_json()).unwrap_or_else(|_| {
                    format!(
                        "pushed {} commits to {}",
                        pushed.commits.len(),
                        pushed.branch
                    )
                }),
            )),
            Err(e) => {
                warn!(error = %e, "git_push failed");
                Err(e.into())
            }
        }
    }
}

/// `git_commit_push` — transactional `git add -A`, `git commit`, `git push`.
pub struct GitCommitPushTool;

#[async_trait]
impl Tool for GitCommitPushTool {
    fn name(&self) -> &str {
        "git_commit_push"
    }

    fn definition(&self) -> ToolDefinition {
        let mut properties = push_properties();
        properties["message"] = serde_json::json!({
            "type": "string",
            "description": "Commit message (non-empty)."
        });
        ToolDefinition {
            name: "git_commit_push".into(),
            description: "Stage all changes, commit with the given message, and push the \
                resulting HEAD to a branch of the workspace's `origin` remote with git's \
                configured credentials. Runs as a single kernel-mediated tool so the dev-loop \
                can treat the pair as an atomic unit."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["message"],
                "properties": properties,
            }),
            cache_control: None,
            eager_input_streaming: None,
        }
    }

    async fn execute(
        &self,
        ctx: &ToolContext,
        args: serde_json::Value,
    ) -> Result<ToolResult, ToolError> {
        let parsed = parse_commit_push_args(&args, ctx)?;
        log_commit_push_dispatch(ctx, &parsed);

        match git_commit_push_impl(
            &parsed.workspace,
            parsed.message,
            parsed.target,
            parsed.commit_timeout,
            parsed.push_policy,
        )
        .await
        {
            Ok(outcome) => Ok(assemble_commit_push_result(outcome)),
            Err(e) => {
                warn!(error = %e, "git_commit_push failed before commit");
                Err(e.into())
            }
        }
    }
}

/// Parsed-and-validated `git_commit_push` inputs plus the workspace /
/// timeout / push-policy fields the impl needs.
struct CommitPushArgs<'a> {
    message: &'a str,
    target: PushTarget<'a>,
    workspace: std::path::PathBuf,
    commit_timeout: Duration,
    push_policy: PushPolicy,
}

/// Validate and parse the JSON arguments for `git_commit_push`, also
/// pulling the workspace/timeout/push-policy and the pinned branch from
/// the [`ToolContext`].
fn parse_commit_push_args<'a>(
    args: &'a serde_json::Value,
    ctx: &'a ToolContext,
) -> Result<CommitPushArgs<'a>, ToolError> {
    let message = str_arg(args, "message").map_err(ToolError::from)?;
    Ok(CommitPushArgs {
        message,
        target: push_target(args, ctx),
        workspace: ctx.sandbox.root().to_path_buf(),
        commit_timeout: workspace_timeout(ctx),
        push_policy: push_policy_for(ctx),
    })
}

/// Emit the `git tool dispatched` info line before
/// [`git_commit_push_impl`] runs. Kept as a separate function so the
/// orchestrator stays short.
fn log_commit_push_dispatch(ctx: &ToolContext, parsed: &CommitPushArgs<'_>) {
    let agent_id = ctx
        .caller_agent_id
        .map_or_else(|| "unknown".to_string(), |id| id.to_string());
    info!(
        op = "git_commit_push",
        %agent_id,
        target_branch = parsed.target.branch.unwrap_or("(current)"),
        pinned_branch = parsed.target.only_branch.is_some(),
        commit_timeout_ms = duration_millis_u64(parsed.commit_timeout),
        push_per_attempt_timeout_ms = duration_millis_u64(parsed.push_policy.per_attempt_timeout),
        push_attempts = parsed.push_policy.attempts,
        "git tool dispatched"
    );
}

/// Build the `ToolResult` from a successful [`git_commit_push_impl`]
/// outcome.
///
/// Two branches:
/// * `push_result == Ok` — the happy path. Report the commit SHA and
///   the post-push commits the remote acknowledged.
/// * `push_result == Err` — commit landed locally; push did not.
///   Surface this as a success-with-warning so callers (the dev-loop
///   automaton) can preserve the commit SHA in their event stream
///   instead of dropping the work. The tool-level result reports
///   `pushed: false` + `push_error` so the agent and downstream
///   dispatcher can see exactly which half failed.
fn assemble_commit_push_result(outcome: CommitPushOutcome) -> ToolResult {
    let CommitPushOutcome {
        commit_sha,
        push_result,
    } = outcome;
    match push_result {
        Ok(pushed) => ToolResult::success(
            "git_commit_push",
            serde_json::to_string(&{
                let mut json = pushed.to_json();
                json["sha"] = serde_json::json!(commit_sha);
                json["committed"] = serde_json::json!(commit_sha.is_some());
                json
            })
            .unwrap_or_else(|_| {
                format!(
                    "committed+pushed {}",
                    commit_sha.as_deref().unwrap_or("(none)")
                )
            }),
        ),
        Err(push_err) => {
            warn!(
                error = %push_err,
                commit_sha = ?commit_sha,
                "git_commit_push: commit succeeded but push failed"
            );
            ToolResult::success(
                "git_commit_push",
                serde_json::to_string(&serde_json::json!({
                    "sha": commit_sha,
                    "committed": commit_sha.is_some(),
                    "pushed": false,
                    "push_error": push_err.to_string(),
                    "commits": Vec::<serde_json::Value>::new(),
                }))
                .unwrap_or_else(|_| {
                    format!(
                        "committed {} but push failed: {push_err}",
                        commit_sha.as_deref().unwrap_or("(none)")
                    )
                }),
            )
        }
    }
}

/// Git tools that operate purely on the local workspace and never
/// reach a remote. Safe to set `on` for any automaton
/// that already has write access to the workspace — the mutation is
/// contained by the `GitExecutor` workspace-escape checks.
pub const GIT_LOCAL_TOOL_NAMES: &[&str] = &["git_commit"];

/// Git tools that talk to a remote: they push to the workspace's
/// `origin` with git's configured credentials. Without a remote they
/// fail; they should usually stay ask-gated by the caller's tri-state
/// tool policy so an agent does not push to an upstream unasked.
pub const GIT_REMOTE_TOOL_NAMES: &[&str] = &["git_push", "git_commit_push"];

/// The full set of git tool names registered by this module.
pub const GIT_TOOL_NAMES: &[&str] = &["git_commit", "git_push", "git_commit_push"];
