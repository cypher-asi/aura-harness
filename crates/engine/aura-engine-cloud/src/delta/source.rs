//! Which commit a project's snapshot is built from, and machines of
//! channel members start at: `origin`'s merge target, as the hub fetches
//! it, not the hub checkout's HEAD.
//!
//! # Invariant
//!
//! A snapshot never builds from commits only this computer has. The hub's
//! checkout may be behind `origin`, or carry unpushed commits that don't
//! build; what is shipped is what the merge queue landed on the target.
//! Without `origin` or its target branch, the snapshot falls back to HEAD
//! and says so.

use std::path::Path;
use std::process::Command;

use super::{bundle, git, head_commit, repo_error, HistoryBundle};
use crate::error::CloudResult;

/// Seconds a fetch of the target from `origin` may take before the last
/// fetched copy is used instead.
const FETCH_TIMEOUT_SECS: u32 = 60;

/// The commit a project snapshot is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildSource {
    /// The commit.
    pub commit: String,
    /// `refs/remotes/origin/<target>`, when it is `origin`'s target.
    pub upstream: Option<String>,
    /// What the hub's checkout has that the snapshot leaves out, or why it
    /// builds from HEAD: shown as a warning, never silently.
    pub note: Option<String>,
}

/// Where a snapshot of `project` builds from: `origin`'s `target`, fetched
/// first (the last fetched copy when the fetch fails), else HEAD. Blocking.
///
/// # Errors
///
/// When `project` is not a repository with a commit.
pub fn build_source(project: &Path, target: &str) -> CloudResult<BuildSource> {
    let head = head_commit(project)?;
    let Some(remote) =
        crate::workspace::is_branch_name(target).then(|| format!("refs/remotes/origin/{target}"))
    else {
        return Ok(from_head(head, &format!("`{target}` is not a branch name")));
    };
    let fetched = fetch_target(project, target, &remote);
    let Some(commit) = resolve(project, &remote) else {
        let why = match fetched {
            Err(why) => format!("origin/{target} could not be fetched ({why})"),
            Ok(()) => format!("there is no origin/{target}"),
        };
        return Ok(from_head(head, &why));
    };
    let mut notes = Vec::new();
    if let Err(why) = fetched {
        notes.push(format!(
            "origin/{target} could not be fetched ({why}); using the copy fetched last"
        ));
    }
    let ahead = git(
        project,
        &["rev-list", "--count", &format!("{remote}..HEAD")],
    )
    .ok()
    .and_then(|count| count.trim().parse::<u64>().ok())
    .unwrap_or(0);
    if ahead > 0 {
        notes.push(format!(
            "the hub's checkout has {ahead} commit(s) not on origin/{target}; the snapshot \
             leaves them out (push them to share them)"
        ));
    }
    Ok(BuildSource {
        commit,
        upstream: Some(remote),
        note: (!notes.is_empty()).then(|| notes.join("; ")),
    })
}

/// `source`'s history as a bundle for the snapshot builder. Blocking.
///
/// # Errors
///
/// When `project` is a shallow clone, git fails, or the bundle is too
/// large.
pub fn bundle_source(project: &Path, source: &BuildSource) -> CloudResult<HistoryBundle> {
    if git(project, &["rev-parse", "--is-shallow-repository"])?.trim() == "true" {
        return Err(repo_error(
            project,
            "it is a shallow clone, so its history can't go to a cloud machine; \
             run `git fetch --unshallow` first",
        ));
    }
    let Some(upstream) = source.upstream.as_deref() else {
        return super::bundle_head(project);
    };
    Ok(HistoryBundle {
        commit: source.commit.clone(),
        upstream: Some(upstream.to_string()),
        bytes: bundle(project, &[upstream])?,
    })
}

/// The bytes of `path` at `commit` in `project`, `None` when it has none.
/// Blocking.
#[must_use]
pub fn file_at(project: &Path, commit: &str, path: &str) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(project)
        .args(["show", &format!("{commit}:{path}")])
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// Building from HEAD at `head`, because of `why`.
fn from_head(head: String, why: &str) -> BuildSource {
    let short: String = head.chars().take(12).collect();
    BuildSource {
        commit: head,
        upstream: None,
        note: Some(format!("{why}; building from the hub's HEAD {short}")),
    }
}

/// The commit `name` points at in `project`, if any.
#[must_use]
pub fn resolve_commit(project: &Path, name: &str) -> Option<String> {
    resolve(project, name)
}

/// The commit `name` points at in `project`, if any.
fn resolve(project: &Path, name: &str) -> Option<String> {
    git(
        project,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{name}^{{commit}}"),
        ],
    )
    .ok()
    .map(|commit| commit.trim().to_string())
    .filter(|commit| !commit.is_empty())
}

/// Fetch `target` from `origin` into `remote`, never asking for
/// credentials and given up after [`FETCH_TIMEOUT_SECS`].
fn fetch_target(project: &Path, target: &str, remote: &str) -> Result<(), String> {
    let refspec = format!("+refs/heads/{target}:{remote}");
    let mut command = if Command::new("timeout").arg("--version").output().is_ok() {
        let mut timed = Command::new("timeout");
        timed.arg(FETCH_TIMEOUT_SECS.to_string()).arg("git");
        timed
    } else {
        Command::new("git")
    };
    let output = command
        .arg("-C")
        .arg(project)
        .args(["fetch", "-q", "--no-tags", "origin", &refspec])
        .env("GIT_TERMINAL_PROMPT", "0")
        .env(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=15",
        )
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let said = String::from_utf8_lossy(&output.stderr);
    let said = said.trim();
    Err(if said.is_empty() {
        format!("git fetch exited with {}", output.status)
    } else {
        said.chars().take(200).collect()
    })
}
