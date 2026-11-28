//! The queue's `git`: a closed set of subcommands, each run with a
//! timeout and no terminal prompt.
//!
//! This is a declared exception to the "every mutating git subprocess goes
//! through the `GitExecutor`" rule (docs/invariants.md §1): the queue
//! integrates pushes the executor already made, in a worktree of its own,
//! and lands them on the target branch. It never touches an agent's
//! workspace.

use crate::error::QueueError;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// The subcommands the queue runs. `merge` merges an entry in the
/// queue's worktree, or fast-forwards a clean checkout of the target
/// branch; `push` never forces the target (it deletes a merged PR's
/// branch only with a lease on the merged commit); `show` reads the
/// target's config file; `ls-remote` reads the remote target's tip and
/// `log` a PR's commit subjects.
/// `checkout-index`, `merge-file`, `add` and `commit` only settle a merge
/// of the queue's own that conflicted in generated files
/// ([`crate::resolve`]).
pub const ALLOWED_SUBCOMMANDS: &[&str] = &[
    "fetch",
    "worktree",
    "checkout",
    "reset",
    "clean",
    "show",
    "merge-base",
    "rev-parse",
    "rev-list",
    "ls-remote",
    "log",
    "diff",
    "status",
    "push",
    "update-ref",
    "merge",
    "checkout-index",
    "merge-file",
    "add",
    "commit",
];

/// What every queue `fetch` passes first. `--refmap=` stops git from also
/// updating the remote-tracking ref (`refs/remotes/<remote>/<branch>`) of
/// each branch it fetches: the queue only writes its private
/// `refs/aura-queue/*` refs. Without it, two queue fetches of one branch
/// at once (the processor's and an overlap scan's) race for that ref's
/// lock, and the loser fails ("cannot lock ref").
pub const FETCH_ARGS: [&str; 3] = ["fetch", "--no-tags", "--refmap="];

/// Author and committer of the queue's merge commits.
const COMMITTER_NAME: &str = "aura merge queue";
const COMMITTER_EMAIL: &str = "merge-queue@aura.local";

/// What a finished git command printed.
#[derive(Debug, Clone)]
pub struct GitOutput {
    /// Whether it exited 0
    pub ok: bool,
    /// Its standard output, trimmed
    pub stdout: String,
    /// Its standard error, trimmed
    pub stderr: String,
}

/// Run `git <args>` in `dir` within `timeout`, whatever its exit status.
///
/// # Errors
/// The subcommand isn't allowed, git can't start, or it ran past
/// `timeout` (it is killed).
pub async fn run(dir: &Path, args: &[&str], timeout: Duration) -> Result<GitOutput, QueueError> {
    let subcommand = args.first().copied().unwrap_or_default();
    if !ALLOWED_SUBCOMMANDS.contains(&subcommand) {
        return Err(QueueError::Git {
            command: subcommand.to_string(),
            reason: "not a subcommand the merge queue runs".to_string(),
        });
    }
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_COMMITTER_NAME", COMMITTER_NAME)
        .env("GIT_COMMITTER_EMAIL", COMMITTER_EMAIL)
        .env("GIT_AUTHOR_NAME", COMMITTER_NAME)
        .env("GIT_AUTHOR_EMAIL", COMMITTER_EMAIL)
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let output = tokio::time::timeout(timeout, command.output())
        .await
        .map_err(|_| QueueError::Git {
            command: subcommand.to_string(),
            reason: format!("timed out after {}s", timeout.as_secs()),
        })?
        .map_err(|e| QueueError::Git {
            command: subcommand.to_string(),
            reason: format!("could not start git: {e}"),
        })?;
    Ok(GitOutput {
        ok: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

/// [`run`], failing on a non-zero exit; returns standard output.
///
/// # Errors
/// As [`run`], or git exited non-zero (its stderr is the reason).
pub async fn ok(dir: &Path, args: &[&str], timeout: Duration) -> Result<String, QueueError> {
    let output = run(dir, args, timeout).await?;
    if output.ok {
        return Ok(output.stdout);
    }
    Err(QueueError::Git {
        command: args.first().copied().unwrap_or_default().to_string(),
        reason: if output.stderr.is_empty() {
            output.stdout
        } else {
            output.stderr
        },
    })
}

/// Where `dir`'s repository keeps what all its worktrees share (its
/// common git dir), absolute.
///
/// # Errors
/// `dir` is not in a git repository.
pub async fn common_dir(dir: &Path, timeout: Duration) -> Result<std::path::PathBuf, QueueError> {
    let out = run(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        timeout,
    )
    .await?;
    if !out.ok || out.stdout.is_empty() {
        return Err(QueueError::NotARepo(dir.display().to_string()));
    }
    Ok(std::path::PathBuf::from(out.stdout))
}

/// The top of the main worktree of `dir`'s repository: the parent of its
/// common git dir for an ordinary repo.
///
/// # Errors
/// `dir` is not in a git repository.
pub async fn main_top(dir: &Path, timeout: Duration) -> Result<std::path::PathBuf, QueueError> {
    let common = common_dir(dir, timeout).await?;
    match common.parent() {
        Some(parent) if common.file_name().is_some_and(|name| name == ".git") => {
            Ok(parent.to_path_buf())
        }
        _ => Ok(dir.to_path_buf()),
    }
}
