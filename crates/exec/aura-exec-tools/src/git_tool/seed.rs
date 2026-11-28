//! Leaving out of a commit the hub's uncommitted work a cloud machine was
//! seeded with.
//!
//! Machines placed before the hub stopped seeding started with the hub's
//! uncommitted changes unpacked in their workspace, and the hub recorded
//! that starting point as a commit at [`BASE_REF`] whose parent is the
//! branch's commit then. Those changes are the diff of [`BASE_REF`]'s
//! parent against it. A commit (and so `git_commit_push` and `git_ship`)
//! stages every change, which would push that unfinished work. So after
//! staging:
//!
//! - a seeded file the agent left as it found it is unstaged
//!   (`reset -q HEAD -- <file>`): it stays in the worktree, out of the
//!   commit;
//! - a seeded file the agent changed too fails the commit
//!   ([`GitToolError::SeededHubChanges`]): its edits can't be told apart
//!   from the hub's, and nothing is committed.
//!
//! A workspace without [`BASE_REF`] (a local agent, a machine placed
//! since) has nothing seeded, and is left alone.

use std::path::Path;
use std::time::Duration;

use tracing::info;

use super::executor::{run_git_expect_ok, spawn_git};
use super::GitToolError;

/// Where a cloud machine's starting point is recorded.
pub(crate) const BASE_REF: &str = "refs/aura/base";

/// Unstage the hub's seeded changes the agent didn't touch, or fail when
/// it touched some. Call after staging.
pub(crate) async fn unstage_hub_seed(
    workspace: &Path,
    timeout: Duration,
) -> Result<(), GitToolError> {
    let seeded = seeded_files(workspace, timeout).await?;
    if seeded.is_empty() {
        return Ok(());
    }
    let paths: Vec<&str> = seeded.iter().map(String::as_str).collect();
    let touched = staged(workspace, &[BASE_REF], &paths, timeout).await?;
    if !touched.is_empty() {
        return Err(GitToolError::SeededHubChanges { files: touched });
    }
    let args = [&["-q", "HEAD", "--"][..], &paths].concat();
    // `reset` may exit 1 when the files stay changed in the worktree,
    // which is the point; whether they left the index is what counts.
    let reset = spawn_git(workspace, "reset", &args, timeout, "reset seeded").await?;
    if !staged(workspace, &[], &paths, timeout).await?.is_empty() {
        return Err(GitToolError::NonZeroExit {
            op: "reset seeded",
            exit_code: reset.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&reset.stderr).trim().to_string(),
        });
    }
    info!(
        files = seeded.len(),
        "left the hub's seeded changes out of the commit"
    );
    Ok(())
}

/// Which of `paths` the index holds differently from `against` (a
/// commit, else `HEAD`).
async fn staged(
    workspace: &Path,
    against: &[&str],
    paths: &[&str],
    timeout: Duration,
) -> Result<Vec<String>, GitToolError> {
    let args = [&["--cached", "--name-only"][..], against, &["--"], paths].concat();
    let out = run_git_expect_ok(workspace, "diff", &args, timeout, "diff --cached").await?;
    Ok(lines(&out.stdout))
}

/// The files the hub seeded the machine with: those its starting point
/// changed against the branch's commit then. None without a starting
/// point.
async fn seeded_files(workspace: &Path, timeout: Duration) -> Result<Vec<String>, GitToolError> {
    let base = spawn_git(
        workspace,
        "rev-parse",
        &["-q", "--verify", BASE_REF],
        timeout,
        "rev-parse base",
    )
    .await?;
    if !base.status.success() {
        return Ok(Vec::new());
    }
    let parent = format!("{BASE_REF}^");
    let out = spawn_git(
        workspace,
        "diff",
        &["--name-only", "--no-renames", &parent, BASE_REF],
        timeout,
        "diff seeded",
    )
    .await?;
    // A starting point without a parent was never seeded.
    if !out.status.success() {
        return Ok(Vec::new());
    }
    Ok(lines(&out.stdout))
}

fn lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}
