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
//! The default `PolicyConfig` treats all three tools as
//! `PermissionLevel::RequireApproval` (see
//! `aura_kernel::default_tool_permission`). Trusted orchestrators such
//! as the dev-loop automaton opt in via
//! `PolicyConfig::add_allowed_tool` after the operator explicitly
//! provides a git repo URL + auth token.

use std::time::Duration;

use async_trait::async_trait;
use aura_core::{ToolDefinition, ToolResult};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::process::Command;
use tracing::{debug, info, instrument, warn};

use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};

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
        }
    }
}

// -----------------------------------------------------------------------
// Allow-list + internal runner
// -----------------------------------------------------------------------

/// Subcommands the executor is permitted to invoke. Any other value is
/// rejected with [`GitToolError::DisallowedSubcommand`].
pub(crate) const ALLOWED_SUBCOMMANDS: &[&str] =
    &["add", "commit", "push", "diff", "rev-parse", "remote"];

fn ensure_allowed(subcmd: &str) -> Result<(), GitToolError> {
    if ALLOWED_SUBCOMMANDS.contains(&subcmd) {
        Ok(())
    } else {
        Err(GitToolError::DisallowedSubcommand(subcmd.to_string()))
    }
}

/// Spawn `git <subcmd> <args>` under `workspace`, with a hard timeout.
///
/// Returns the raw [`std::process::Output`] on completion. Neither the
/// program (`git`) nor the subcommand set is caller-controlled once
/// this helper is reached — both are fixed by the calling tool.
#[instrument(skip_all, fields(op = %op_label, subcmd = %subcmd))]
pub(crate) async fn spawn_git(
    workspace: &std::path::Path,
    subcmd: &str,
    args: &[&str],
    timeout: Duration,
    op_label: &'static str,
) -> Result<std::process::Output, GitToolError> {
    ensure_allowed(subcmd)?;

    let mut cmd = Command::new("git");
    cmd.arg(subcmd)
        .args(args)
        .current_dir(workspace)
        // Never prompt for credentials — we always pre-embed auth into
        // the URL and rely on the non-interactive push path.
        .env("GIT_TERMINAL_PROMPT", "0");

    debug!(?workspace, "spawning git");

    let output = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| GitToolError::Timeout(op_label, timeout))?
        .map_err(|e| GitToolError::Spawn(op_label, e))?;

    Ok(output)
}

/// Shortcut that converts a non-zero exit into [`GitToolError::NonZeroExit`].
async fn run_git_expect_ok(
    workspace: &std::path::Path,
    subcmd: &str,
    args: &[&str],
    timeout: Duration,
    op_label: &'static str,
) -> Result<std::process::Output, GitToolError> {
    let output = spawn_git(workspace, subcmd, args, timeout, op_label).await?;
    if !output.status.success() {
        return Err(GitToolError::NonZeroExit {
            op: op_label,
            exit_code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    Ok(output)
}

// -----------------------------------------------------------------------
// Operation implementations (shared between tools + orbit)
// -----------------------------------------------------------------------

/// Metadata recorded for each commit surfaced by `git_push`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommitInfo {
    pub sha: String,
    pub message: String,
}

/// Stage every tracked change and create a commit with `message`.
/// Returns `Ok(None)` when there is nothing to commit (`git diff
/// --cached --quiet` exits 0 after staging).
#[instrument(skip_all, fields(op = "commit"))]
pub async fn git_commit_impl(
    workspace: &std::path::Path,
    message: &str,
    timeout: Duration,
) -> Result<Option<String>, GitToolError> {
    if message.trim().is_empty() {
        return Err(GitToolError::InvalidArg {
            name: "message",
            reason: "commit message must not be empty".into(),
        });
    }

    run_git_expect_ok(workspace, "add", &["-A"], timeout, "add").await?;

    let diff = spawn_git(
        workspace,
        "diff",
        &["--cached", "--quiet"],
        timeout,
        "diff --cached",
    )
    .await?;
    if diff.status.success() {
        info!("no staged changes; skipping commit");
        return Ok(None);
    }

    run_git_expect_ok(workspace, "commit", &["-m", message], timeout, "commit").await?;

    let sha_output =
        run_git_expect_ok(workspace, "rev-parse", &["HEAD"], timeout, "rev-parse HEAD").await?;
    let sha = String::from_utf8_lossy(&sha_output.stdout)
        .trim()
        .to_string();
    info!(%sha, "git commit created");
    Ok(Some(sha))
}

/// Push the current `HEAD` to `remote_url:branch` using a JWT-auth URL.
///
/// The push is executed with the fully-authenticated URL passed inline
/// (rather than registering a named remote) so no credentials survive
/// in the on-disk `.git/config`. Returns the list of SHAs that were
/// newly pushed (derived from `git log orbit/branch..HEAD` before the
/// push — best-effort, empty on failure).
#[instrument(skip_all, fields(op = "push", branch = %branch))]
pub async fn git_push_impl(
    workspace: &std::path::Path,
    remote_url: &str,
    branch: &str,
    jwt: &str,
    force: bool,
    timeout: Duration,
) -> Result<Vec<CommitInfo>, GitToolError> {
    if remote_url.is_empty() {
        return Err(GitToolError::MissingArg("remote_url"));
    }
    if branch.is_empty() {
        return Err(GitToolError::MissingArg("branch"));
    }
    if jwt.is_empty() {
        return Err(GitToolError::MissingArg("jwt"));
    }

    let auth_url = build_auth_url(remote_url, jwt)?;
    let safe_remote = redact_url(remote_url);

    // Best-effort unpushed commit listing. Missing remote tracking
    // branches are normal for first-push flows; swallow the error.
    let refspec = format!("HEAD:refs/heads/{branch}");
    let commits = list_unpushed_commits(workspace, "HEAD", timeout)
        .await
        .unwrap_or_default();

    let mut args: Vec<&str> = vec![auth_url.as_str()];
    if force {
        args.push("--force");
    }
    args.push(refspec.as_str());

    let _ = run_git_expect_ok(workspace, "push", &args, timeout, "push").await?;

    info!(
        remote = %safe_remote,
        %branch,
        commit_count = commits.len(),
        "git push succeeded"
    );
    Ok(commits)
}

/// Combined `add -A` + `commit` + `push` — the transactional group the
/// dev-loop relies on. Returns the new commit SHA (or `None` if there
/// was nothing to commit) and the list of commits pushed.
#[instrument(skip_all, fields(op = "commit_and_push", branch = %branch))]
pub async fn git_commit_push_impl(
    workspace: &std::path::Path,
    message: &str,
    remote_url: &str,
    branch: &str,
    jwt: &str,
    force: bool,
    timeout: Duration,
) -> Result<(Option<String>, Vec<CommitInfo>), GitToolError> {
    let sha = git_commit_impl(workspace, message, timeout).await?;
    let commits = git_push_impl(workspace, remote_url, branch, jwt, force, timeout).await?;
    Ok((sha, commits))
}

/// Read `git log HEAD --pretty=format:'%H %s'` up to 50 entries. Only
/// used for audit logging on successful pushes. Missing refs yield an
/// empty vec rather than an error.
async fn list_unpushed_commits(
    workspace: &std::path::Path,
    git_ref: &str,
    timeout: Duration,
) -> Result<Vec<CommitInfo>, GitToolError> {
    // `log` is a read-only subcommand. The allow-list keeps it close
    // to `rev-parse` — both are inspection helpers for the mutating
    // flow and never take caller-controlled args beyond a bounded ref.
    let mut cmd = Command::new("git");
    cmd.arg("log")
        .arg(git_ref)
        .arg("--pretty=format:%H %s")
        .arg("-50")
        .current_dir(workspace)
        .env("GIT_TERMINAL_PROMPT", "0");

    let output = match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => return Err(GitToolError::Spawn("log", e)),
        Err(_) => return Err(GitToolError::Timeout("log", timeout)),
    };
    if !output.status.success() {
        debug!(
            stderr = %String::from_utf8_lossy(&output.stderr),
            "git log returned non-zero exit; treating as empty"
        );
        return Ok(Vec::new());
    }

    let commits = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let (sha, msg) = line.split_once(' ')?;
            Some(CommitInfo {
                sha: sha.to_string(),
                message: msg.to_string(),
            })
        })
        .collect();
    Ok(commits)
}

/// Inject `x-token:<jwt>` into `remote_url`'s user-info component.
///
/// Accepts `https://host[:port]/path` URLs. Any other shape — including
/// ssh://, file://, scp-style `git@host:path` — is rejected with
/// [`GitToolError::InvalidUrl`]. The caller is responsible for stripping
/// existing credentials; we do not attempt to merge them.
fn build_auth_url(remote_url: &str, jwt: &str) -> Result<String, GitToolError> {
    // Find scheme.
    let Some((scheme, rest)) = remote_url.split_once("://") else {
        return Err(GitToolError::InvalidUrl(
            "remote URL must contain '://'".into(),
        ));
    };
    if scheme != "https" && scheme != "http" {
        return Err(GitToolError::InvalidUrl(format!(
            "unsupported scheme '{scheme}' (expected https or http)"
        )));
    }
    if rest.is_empty() {
        return Err(GitToolError::InvalidUrl("remote URL is empty".into()));
    }
    // Strip any pre-existing user-info segment so we don't leak
    // credentials or end up with `user:token@newuser:newtoken@host`.
    let without_auth = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
    if without_auth.is_empty() {
        return Err(GitToolError::InvalidUrl("remote URL has no host".into()));
    }
    // Reject obvious control chars in the JWT. The JWT itself is
    // user-provided and ends up on the command line, so make sure
    // no newline / whitespace / shell meta sneaks in — these are not
    // meaningful in a JWT anyway.
    for c in jwt.chars() {
        if c.is_ascii_whitespace() || c == '@' || c == '#' || c.is_control() {
            return Err(GitToolError::InvalidUrl(format!(
                "auth token contains disallowed character: {c:?}"
            )));
        }
    }
    Ok(format!("{scheme}://x-token:{jwt}@{without_auth}"))
}

fn redact_url(url: &str) -> String {
    // Collapse any user-info portion to `***` so logs never leak a JWT.
    if let Some((scheme, rest)) = url.split_once("://") {
        if let Some((_, host)) = rest.rsplit_once('@') {
            return format!("{scheme}://***@{host}");
        }
        return format!("{scheme}://{rest}");
    }
    url.to_string()
}

// -----------------------------------------------------------------------
// Tool implementations
// -----------------------------------------------------------------------

fn workspace_timeout(ctx: &ToolContext) -> Duration {
    // Keep this comfortably above the kernel's default command_timeout
    // (10s) since `git push` over slow networks routinely takes longer.
    // Tools still respect the hard upper bound via `max_async_timeout_ms`.
    Duration::from_millis(ctx.config.max_async_timeout_ms.min(120_000))
}

fn str_arg<'a>(args: &'a serde_json::Value, name: &'static str) -> Result<&'a str, GitToolError> {
    args.get(name)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or(GitToolError::MissingArg(name))
}

fn opt_bool(args: &serde_json::Value, name: &str) -> bool {
    args.get(name)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

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
        let timeout = workspace_timeout(ctx);

        let agent_id = ctx
            .caller_agent_id
            .map_or_else(|| "unknown".to_string(), |id| id.to_string());
        info!(
            op = "git_push",
            %agent_id,
            target_branch = branch,
            remote = %redact_url(remote_url),
            "git tool dispatched"
        );

        match git_push_impl(&workspace, remote_url, branch, jwt, force, timeout).await {
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
        let timeout = workspace_timeout(ctx);

        let agent_id = ctx
            .caller_agent_id
            .map_or_else(|| "unknown".to_string(), |id| id.to_string());
        info!(
            op = "git_commit_push",
            %agent_id,
            target_branch = branch,
            remote = %redact_url(remote_url),
            "git tool dispatched"
        );

        match git_commit_push_impl(&workspace, message, remote_url, branch, jwt, force, timeout)
            .await
        {
            Ok((sha, commits)) => Ok(ToolResult::success(
                "git_commit_push",
                serde_json::to_string(&serde_json::json!({
                    "sha": sha,
                    "committed": sha.is_some(),
                    "commits": commits
                        .iter()
                        .map(|c| serde_json::json!({"sha": c.sha, "message": c.message}))
                        .collect::<Vec<_>>(),
                }))
                .unwrap_or_else(|_| {
                    format!("committed+pushed {}", sha.as_deref().unwrap_or("(none)"))
                }),
            )),
            Err(e) => {
                warn!(error = %e, "git_commit_push failed");
                Err(e.into())
            }
        }
    }
}

/// The set of tool names registered by this module. Kept in sync with
/// the [`aura_kernel::default_tool_permission`] branch that maps them
/// to [`aura_kernel::PermissionLevel::RequireApproval`].
pub const GIT_TOOL_NAMES: &[&str] = &["git_commit", "git_push", "git_commit_push"];

#[cfg(test)]
mod tests;
