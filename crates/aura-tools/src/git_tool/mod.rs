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
//!   `push`, plus the read-only helpers `diff --cached` and
//!   `rev-parse HEAD` used to implement commit).
//! - Scrubs credentials from structured tracing output.
//!
//! Tool availability is resolved by the kernel's tri-state policy
//! (`UserToolDefaults` plus optional `AgentToolPermissions`). Even when
//! these tools resolve to `on`, this module still enforces workspace
//! confinement, subcommand allowlists, timeouts, and credential scrubbing.
//!
//! ## Layout (Phase 2b)
//!
//! - [`mod@executor`] — every `Command::new("git")` call site lives
//!   here. Owns the subcommand allowlist + timeout-wrapped `spawn_git`.
//! - [`mod@redact`] — `build_auth_url` (JWT injection into push URLs)
//!   and `redact_url` (mask user-info in tracing output).
//! - [`mod@commit`] — `git_commit_impl` (`add -A`, `diff --cached`,
//!   `commit`, `rev-parse HEAD`).
//! - [`mod@push`] — `git_push_impl` plus the retry / transient
//!   classifier / remote-storage-exhausted short-circuit.
//! - [`mod@commit_push`] — `git_commit_push_impl` (the transactional
//!   commit-and-push pair the dev-loop relies on).
//! - [`mod@sandbox`] — `ToolContext` bridging helpers
//!   (`workspace_timeout`, `push_policy_for`, `str_arg`, `opt_bool`).
//! - `tests` — unit tests for the helpers above; declared
//!   `#[cfg(test)] mod tests;` so the module path matches `super::*`.
//!
//! `mod.rs` keeps the public-facing surface: [`GitToolError`],
//! [`CommitInfo`], [`PushPolicy`], [`CommitPushOutcome`], the three
//! `Tool` impls, and the `GIT_*_TOOL_NAMES` constants used by the
//! catalog. `pub use` re-exports the impl functions so the legacy
//! `aura_tools::git_tool::git_push_impl` paths keep resolving.

use std::time::Duration;

use async_trait::async_trait;
use aura_core::{ToolDefinition, ToolResult};
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
use redact::redact_url;
use sandbox::{opt_bool, push_policy_for, str_arg, workspace_timeout};

// -----------------------------------------------------------------------
// Error type
// -----------------------------------------------------------------------

/// Errors surfaced from the `git_*` tools.
///
/// Kept as a dedicated `thiserror` enum so `aura-tools` stays free of
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

    #[error("invalid remote URL: {0}")]
    InvalidUrl(String),

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
            GitToolError::InvalidUrl(msg) => Self::InvalidArguments(msg),
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
/// than the rest of the git tools — Orbit round-trips routinely
/// exceed the 120s cap that covers `git add` / `git commit`. Keeping
/// push on its own knob also makes it safe to retry: a single slow
/// remote shouldn't force us to raise every other tool's ceiling.
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

/// Outcome of [`git_commit_push_impl`].
///
/// The commit half and the push half are reported independently so a
/// push-only failure does not mask a successful commit. The orchestrator
/// (dev-loop automaton) uses this to emit `GitCommitted` alongside
/// `GitPushFailed` when the commit landed locally but the push timed
/// out — preserving the commit SHA in the task history instead of
/// pretending the work never happened.
#[derive(Debug)]
pub struct CommitPushOutcome {
    /// Commit SHA from the local `git commit`, or `None` when there
    /// were no staged changes (i.e. the tool was a no-op).
    pub commit_sha: Option<String>,
    /// Result of the subsequent `git push`. `Ok` on success, `Err`
    /// on any push-specific failure (timeout, non-zero exit, spawn
    /// error). Independent of `commit_sha` — the commit SHA stays
    /// populated even when the push fails.
    pub push_result: Result<Vec<CommitInfo>, GitToolError>,
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

/// `git_push` — push `HEAD` to `<remote_url>:<branch>` with inline JWT auth.
pub struct GitPushTool;

#[async_trait]
impl Tool for GitPushTool {
    fn name(&self) -> &str {
        "git_push"
    }

    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "git_push".into(),
            description: "Push HEAD to the given remote URL + branch using a JWT-authenticated \
                URL (no on-disk remote registration). Runs inside the kernel's sandboxed \
                workspace root only."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["remote_url", "branch", "jwt"],
                "properties": {
                    "remote_url": { "type": "string" },
                    "branch": { "type": "string" },
                    "jwt": { "type": "string" },
                    "force": { "type": "boolean" }
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
        let remote_url = str_arg(&args, "remote_url").map_err(ToolError::from)?;
        let branch = str_arg(&args, "branch").map_err(ToolError::from)?;
        let jwt = str_arg(&args, "jwt").map_err(ToolError::from)?;
        let force = opt_bool(&args, "force");
        let workspace = ctx.sandbox.root().to_path_buf();
        let policy = push_policy_for(ctx);

        let agent_id = ctx
            .caller_agent_id
            .map_or_else(|| "unknown".to_string(), |id| id.to_string());
        info!(
            op = "git_push",
            %agent_id,
            target_branch = branch,
            remote = %redact_url(remote_url),
            per_attempt_timeout_ms = duration_millis_u64(policy.per_attempt_timeout),
            attempts = policy.attempts,
            "git tool dispatched"
        );

        match git_push_impl(&workspace, remote_url, branch, jwt, force, policy).await {
            Ok(commits) => Ok(ToolResult::success(
                "git_push",
                serde_json::to_string(&serde_json::json!({
                    "pushed": true,
                    "commits": commits
                        .iter()
                        .map(|c| serde_json::json!({"sha": c.sha, "message": c.message}))
                        .collect::<Vec<_>>(),
                }))
                .unwrap_or_else(|_| format!("pushed {} commits", commits.len())),
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
        ToolDefinition {
            name: "git_commit_push".into(),
            description: "Stage all changes, commit with the given message, and push the \
                resulting HEAD to <remote_url>:<branch>. Runs as a single kernel-mediated \
                tool so the dev-loop can treat the pair as an atomic unit."
                .into(),
            input_schema: serde_json::json!({
                "type": "object",
                "required": ["message", "remote_url", "branch", "jwt"],
                "properties": {
                    "message": { "type": "string" },
                    "remote_url": { "type": "string" },
                    "branch": { "type": "string" },
                    "jwt": { "type": "string" },
                    "force": { "type": "boolean" }
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
        let remote_url = str_arg(&args, "remote_url").map_err(ToolError::from)?;
        let branch = str_arg(&args, "branch").map_err(ToolError::from)?;
        let jwt = str_arg(&args, "jwt").map_err(ToolError::from)?;
        let force = opt_bool(&args, "force");
        let workspace = ctx.sandbox.root().to_path_buf();
        let commit_timeout = workspace_timeout(ctx);
        let push_policy = push_policy_for(ctx);

        let agent_id = ctx
            .caller_agent_id
            .map_or_else(|| "unknown".to_string(), |id| id.to_string());
        info!(
            op = "git_commit_push",
            %agent_id,
            target_branch = branch,
            remote = %redact_url(remote_url),
            commit_timeout_ms = duration_millis_u64(commit_timeout),
            push_per_attempt_timeout_ms = duration_millis_u64(push_policy.per_attempt_timeout),
            push_attempts = push_policy.attempts,
            "git tool dispatched"
        );

        match git_commit_push_impl(
            &workspace,
            message,
            remote_url,
            branch,
            jwt,
            force,
            commit_timeout,
            push_policy,
        )
        .await
        {
            Ok(outcome) => {
                let CommitPushOutcome {
                    commit_sha,
                    push_result,
                } = outcome;
                match push_result {
                    Ok(commits) => Ok(ToolResult::success(
                        "git_commit_push",
                        serde_json::to_string(&serde_json::json!({
                            "sha": commit_sha,
                            "committed": commit_sha.is_some(),
                            "pushed": true,
                            "commits": commits
                                .iter()
                                .map(|c| serde_json::json!({"sha": c.sha, "message": c.message}))
                                .collect::<Vec<_>>(),
                        }))
                        .unwrap_or_else(|_| {
                            format!(
                                "committed+pushed {}",
                                commit_sha.as_deref().unwrap_or("(none)")
                            )
                        }),
                    )),
                    Err(push_err) => {
                        // Commit landed locally; push did not. Surface
                        // this as a success-with-warning so callers
                        // (the dev-loop automaton) can preserve the
                        // commit SHA in their event stream instead of
                        // dropping the work. The tool-level result
                        // reports `pushed: false` + `push_error` so
                        // the agent can see what happened and the
                        // automaton dispatcher can emit both
                        // `GitCommitted` and `GitPushFailed`.
                        warn!(
                            error = %push_err,
                            commit_sha = ?commit_sha,
                            "git_commit_push: commit succeeded but push failed"
                        );
                        Ok(ToolResult::success(
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
                        ))
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "git_commit_push failed before commit");
                Err(e.into())
            }
        }
    }
}

/// Git tools that operate purely on the local workspace and never
/// reach a remote. Safe to set `on` for any automaton
/// that already has write access to the workspace — the mutation is
/// contained by the `GitExecutor` workspace-escape checks.
pub const GIT_LOCAL_TOOL_NAMES: &[&str] = &["git_commit"];

/// Git tools that talk to a remote (require both a repo URL and an
/// auth token to be meaningful). Without remote configuration, these
/// should usually remain unavailable or ask-gated by the caller's
/// tri-state tool policy so a misconfigured workspace does not silently
/// push to the wrong upstream.
pub const GIT_REMOTE_TOOL_NAMES: &[&str] = &["git_push", "git_commit_push"];

/// The full set of git tool names registered by this module.
pub const GIT_TOOL_NAMES: &[&str] = &["git_commit", "git_push", "git_commit_push"];
