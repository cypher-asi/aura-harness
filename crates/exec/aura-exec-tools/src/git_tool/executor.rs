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

use std::path::Path;
use std::time::Duration;

use tokio::process::Command;
use tracing::{debug, instrument};

use super::{CommitInfo, GitToolError};
use crate::sandbox::strip_unc_prefix;

/// Subcommands the executor is permitted to invoke. Any other value is
/// rejected with [`GitToolError::DisallowedSubcommand`].
///
/// `fetch` and `merge` exist for `git_ship`, which pulls the merge
/// queue's target branch into the agent's branch before pushing it:
/// `fetch origin refs/heads/<target>` then `merge --no-edit FETCH_HEAD`
/// (or `merge --abort` after a conflict). Both are called with fixed
/// arguments only; the target is checked as a branch name first.
///
/// `checkout` only ever runs as `checkout -q -b <new branch> <start>`
/// on a clean tree ([`super::branches`]), `status`, `rev-list` and
/// `ls-remote` only read.
///
/// `reset` only ever runs as `reset -q HEAD -- <paths>`, to leave out of
/// a commit the hub's uncommitted work a cloud machine was seeded with
/// ([`super::seed`]); it never moves a branch or touches the worktree.
pub(crate) const ALLOWED_SUBCOMMANDS: &[&str] = &[
    "add",
    "commit",
    "push",
    "diff",
    "rev-parse",
    "remote",
    "fetch",
    "merge",
    "reset",
    "checkout",
    "status",
    "rev-list",
    "ls-remote",
    "merge-base",
];

fn ensure_allowed(subcmd: &str) -> Result<(), GitToolError> {
    if ALLOWED_SUBCOMMANDS.contains(&subcmd) {
        Ok(())
    } else {
        Err(GitToolError::DisallowedSubcommand(subcmd.to_string()))
    }
}

/// A `git` command run in `workspace` that never looks above it for a
/// repository. A workspace that isn't the top of its own repository
/// fails as "not a git repository" instead of committing, or pushing
/// from, a checkout that happens to contain it.
fn git_in(workspace: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(workspace)
        // Never prompt for credentials: a push authenticates through
        // git's configured helper or fails.
        .env("GIT_TERMINAL_PROMPT", "0");
    // Git compares the ceiling with the real path of its cwd, so a
    // workspace reached through a symlink is resolved first.
    let root = std::fs::canonicalize(workspace).map(|root| strip_unc_prefix(&root));
    if let Some(parent) = root.as_deref().ok().and_then(Path::parent) {
        cmd.env("GIT_CEILING_DIRECTORIES", parent);
    }
    cmd
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

    let mut cmd = git_in(workspace);
    cmd.arg(subcmd).args(args);

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

/// The commit `HEAD` points at (`git rev-parse HEAD`).
pub(super) async fn head_commit(
    workspace: &std::path::Path,
    timeout: Duration,
) -> Result<String, GitToolError> {
    let out =
        run_git_expect_ok(workspace, "rev-parse", &["HEAD"], timeout, "rev-parse HEAD").await?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Read `git log HEAD --not --remotes=<remote> --pretty=format:'%H %s'`
/// up to 50 entries: the commits `remote` doesn't have as far as this
/// repository knows. Only used for the push result and audit logging.
/// Missing refs yield an empty vec rather than an error.
pub(super) async fn list_unpushed_commits(
    workspace: &std::path::Path,
    remote: &str,
    timeout: Duration,
) -> Result<Vec<CommitInfo>, GitToolError> {
    // `log` is a read-only subcommand. The allow-list keeps it close
    // to `rev-parse` — both are inspection helpers for the mutating
    // flow and never take caller-controlled args.
    let mut cmd = git_in(workspace);
    cmd.arg("log")
        .arg("HEAD")
        .arg("--not")
        .arg(format!("--remotes={remote}"))
        .arg("--pretty=format:%H %s")
        .arg("-50");

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
