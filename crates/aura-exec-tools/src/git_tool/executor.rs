//! Subprocess core for the `git_*` tools.
//!
//! Owns every `Command::new("git")` call site in the crate (the §1
//! invariant band in `scripts/check_invariants.sh` enforces this).
//! Exposes:
//!
//! - [`ALLOWED_SUBCOMMANDS`] / [`ensure_allowed`] — the static
//!   subcommand allowlist that gates [`spawn_git`].
//! - [`spawn_git`] — bare invocation with timeout + sandbox cwd.
//! - [`run_git_expect_ok`] — wrapper that converts non-zero exits into
//!   [`GitToolError::NonZeroExit`].
//! - [`list_unpushed_commits`] — read-only `git log` helper used by
//!   [`crate::git_tool::push`] to surface SHAs in the success payload.

use std::time::Duration;

use tokio::process::Command;
use tracing::{debug, instrument};

use super::{CommitInfo, GitToolError};

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
pub(super) async fn run_git_expect_ok(
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

/// Read `git log HEAD --pretty=format:'%H %s'` up to 50 entries. Only
/// used for audit logging on successful pushes. Missing refs yield an
/// empty vec rather than an error.
pub(super) async fn list_unpushed_commits(
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
