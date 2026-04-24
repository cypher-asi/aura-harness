//! `git_commit_impl` — the underlying implementation behind
//! [`crate::git_tool::GitCommitTool`].
//!
//! Stages every tracked change (`git add -A`), checks for staged
//! content (`git diff --cached --quiet`), and creates a commit with the
//! provided message. Returns the new SHA, or `None` when there is
//! nothing to commit.

use std::time::Duration;

use tracing::{info, instrument};

use super::executor::{run_git_expect_ok, spawn_git};
use super::GitToolError;

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
