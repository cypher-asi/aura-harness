//! A git worktree of its own for each local channel member, on a branch
//! of its own, so its ships never commit another member's work.
//!
//! A member that joins a local channel whose project folder is in a git
//! repository works in `<data dir>/worktrees/<agent hex>`, a worktree of
//! that repository on `aura/<definition>-<hex8>` started at the
//! repository's HEAD ([`provision_member_worktree`]). A project folder
//! below the repository's top maps to the same folder inside the
//! worktree. When the member is archived its worktree is removed if it
//! holds no uncommitted work; its branch, with every commit, stays
//! ([`remove_member_worktree`]). Restoring it adds the worktree back on
//! its branch.
//!
//! The git commands here are local only (`rev-parse`, `worktree add`,
//! `worktree remove`, `status`) and blocking: callers run them on a
//! blocking thread or where the registry's own synchronous writes run.
//! This file is a declared §1 exception (docs/invariants.md).

use std::path::{Path, PathBuf};
use std::process::Command;

use super::IsolationError;

/// Where a member's worktree goes, under the app's data directory.
#[must_use]
pub(crate) fn member_worktree_path(data_dir: &Path, agent_hex: &str) -> PathBuf {
    data_dir.join("worktrees").join(agent_hex)
}

/// The branch a member works on: `aura/<definition>-<hex8>`.
#[must_use]
pub(crate) fn member_branch(definition: Option<&str>, agent_hex: &str) -> String {
    let slug: String = definition
        .unwrap_or("agent")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let short = agent_hex.get(..8).unwrap_or(agent_hex);
    format!("aura/{}-{short}", slug.trim_matches('-'))
}

/// Whether `path` is under `data_dir`'s worktrees: a member worktree.
#[must_use]
pub(crate) fn is_member_worktree(data_dir: &Path, path: &Path) -> bool {
    path.starts_with(data_dir.join("worktrees"))
}

/// The folder a new local channel member `agent_hex` (an instance of
/// `definition`) works in: its own worktree of the repository `shared`
/// is in. `None` (it works in `shared`, as before) when `shared` is in
/// no repository or the worktree can't be made; that is logged.
#[must_use]
pub(crate) fn member_folder(
    data_dir: &Path,
    shared: Option<&Path>,
    definition: Option<&str>,
    agent_hex: &str,
) -> Option<PathBuf> {
    let shared = shared?;
    let worktree = member_worktree_path(data_dir, agent_hex);
    let branch = member_branch(definition, agent_hex);
    match provision_member_worktree(shared, &worktree, &branch) {
        Ok(folder) => {
            if let Some(folder) = &folder {
                tracing::info!(agent = %agent_hex, %branch, folder = %folder.display(), "Channel member works in its own worktree");
            }
            folder
        }
        Err(err) => {
            tracing::warn!(agent = %agent_hex, error = %err, "Could not give a channel member its own worktree; it shares the project folder");
            None
        }
    }
}

/// Make `worktree` a worktree of the repository `folder` is in, on
/// `branch` (made at the repository's HEAD when new), and return the
/// folder the member works in: `folder`'s place inside it. `None` when
/// `folder` is in no repository, or one without a commit yet: the member
/// works in `folder` then.
///
/// # Errors
/// [`IsolationError::Git`] when git fails to add the worktree.
pub(crate) fn provision_member_worktree(
    folder: &Path,
    worktree: &Path,
    branch: &str,
) -> Result<Option<PathBuf>, IsolationError> {
    let Some(top) = git_out(folder, &["rev-parse", "--show-toplevel"]).map(PathBuf::from) else {
        return Ok(None);
    };
    if git_out(&top, &["rev-parse", "--verify", "-q", "HEAD"]).is_none() {
        return Ok(None);
    }
    let inside = folder
        .canonicalize()
        .ok()
        .zip(top.canonicalize().ok())
        .and_then(|(folder, top)| folder.strip_prefix(&top).ok().map(Path::to_path_buf))
        .unwrap_or_default();
    if !worktree.join(".git").exists() {
        if let Some(parent) = worktree.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let target = worktree.to_string_lossy().into_owned();
        let exists = git_out(
            &top,
            &[
                "rev-parse",
                "--verify",
                "-q",
                &format!("refs/heads/{branch}"),
            ],
        )
        .is_some();
        let args: Vec<&str> = if exists {
            vec!["worktree", "add", "-q", &target, branch]
        } else {
            vec!["worktree", "add", "-q", "-b", branch, &target, "HEAD"]
        };
        git_ok(&top, &args)?;
    }
    Ok(Some(worktree.join(inside)))
}

/// What [`remove_member_worktree`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Removal {
    /// The worktree is gone; its branch stays.
    Removed,
    /// It holds uncommitted work, so it stays where it is.
    Kept,
    /// There was none.
    Missing,
}

/// Remove the member worktree `worktree` unless it holds uncommitted
/// work. Its branch and commits stay.
///
/// # Errors
/// [`IsolationError::Git`] when git can't read or remove it.
pub(crate) fn remove_member_worktree(worktree: &Path) -> Result<Removal, IsolationError> {
    if !worktree.join(".git").exists() {
        return Ok(Removal::Missing);
    }
    let status = git_ok(worktree, &["status", "--porcelain"])?;
    if !status.trim().is_empty() {
        return Ok(Removal::Kept);
    }
    let target = worktree.to_string_lossy().into_owned();
    git_ok(worktree, &["worktree", "remove", &target])?;
    Ok(Removal::Removed)
}

/// Standard output of `git <args>` in `dir`, trimmed, when it succeeds.
fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
    git_ok(dir, args).ok().filter(|out| !out.is_empty())
}

/// `git <args>` in `dir`; its standard output, trimmed.
fn git_ok(dir: &Path, args: &[&str]) -> Result<String, IsolationError> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    let subcommand = args.first().copied().unwrap_or_default();
    Err(IsolationError::Git(format!(
        "git {subcommand}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

#[cfg(test)]
mod tests;
