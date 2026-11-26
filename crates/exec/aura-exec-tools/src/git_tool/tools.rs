//! The three `Tool` impls (`git_commit`, `git_push`, `git_commit_push`):
//! thin shims that parse arguments, log the dispatch and shape the
//! JSON result around the impl functions.

use std::time::Duration;

use async_trait::async_trait;
use aura_core_types::{ToolDefinition, ToolResult};
use tracing::{info, warn};

use super::push::duration_millis_u64;
use super::sandbox::{
    opt_bool, opt_str, push_branch_for, push_policy_for, str_arg, workspace_timeout,
};
use super::{
    git_commit_impl, git_commit_push_impl, git_push_impl, CommitPushOutcome, PushPolicy, PushTarget,
};
use crate::error::ToolError;
use crate::tool::{Tool, ToolContext};

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
