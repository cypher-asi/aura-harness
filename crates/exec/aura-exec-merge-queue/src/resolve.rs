//! Settling a merge that conflicted only in generated files.
//!
//! Parallel branches conflict most often in files no one writes by hand
//! line by line: the changelog every change adds an entry to, and the
//! lockfiles a dependency change rewrites. When merging the target into
//! an entry conflicts only there, the queue resolves them itself
//! ([`settle`]) and the merge carries on to the checks, which still run
//! on exactly the commit that lands. Any other conflicted file is a real
//! conflict: the merge is aborted and the entry goes back to its agent
//! with the real files listed first ([`conflict_reason`]).
//!
//! | File | Resolution |
//! |---|---|
//! | `CHANGELOG.md` | Union of both sides over their base (`merge-file --union`) |
//! | `Cargo.lock` | The target's side, then `cargo metadata` updates it |
//! | `package-lock.json` | The target's side, then `npm install --package-lock-only` |
//!
//! ## Invariants
//!
//! - Only the queue's own worktree is touched, in the middle of the
//!   queue's own merge.
//! - Nothing is resolved unless every conflicted file can be: a single
//!   real file fails the whole entry, and the caller aborts the merge.
//! - The merge commit keeps the queue's merge message (`commit --no-edit`).
//!
//! ## Failure modes
//!
//! A generated file deleted on one side, or a regenerate command that
//! fails or runs past its time, makes it a real conflict, and the reason
//! says why.

use crate::config::QueueConfig;
use crate::entry::short_sha;
use crate::error::QueueError;
use crate::git;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

/// The empty base of a file both sides added, in the worktree's top.
const EMPTY_BASE: &str = ".aura-queue-empty-base";

/// A file the queue regenerates instead of asking an agent to resolve.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generated {
    /// `CHANGELOG.md`: both sides' lines are kept
    Changelog,
    /// `Cargo.lock`: the target's, updated by cargo
    CargoLock,
    /// `package-lock.json`: the target's, updated by npm
    NpmLock,
}

impl Generated {
    /// What `path` (relative to the repo's top) is, matched by its file
    /// name at any depth; `None` for a file someone writes.
    #[must_use]
    pub fn of(path: &str) -> Option<Self> {
        match path.rsplit('/').next().unwrap_or(path) {
            "CHANGELOG.md" => Some(Self::Changelog),
            "Cargo.lock" => Some(Self::CargoLock),
            "package-lock.json" => Some(Self::NpmLock),
            _ => None,
        }
    }

    /// The command that brings the lockfile up to date with its folder's
    /// manifest; none for the changelog.
    const fn regenerate(self) -> Option<(&'static str, &'static [&'static str])> {
        match self {
            Self::Changelog => None,
            Self::CargoLock => Some(("cargo", &["metadata", "--format-version", "1"])),
            Self::NpmLock => Some((
                "npm",
                &[
                    "install",
                    "--package-lock-only",
                    "--ignore-scripts",
                    "--no-audit",
                    "--no-fund",
                ],
            )),
        }
    }
}

/// How a conflicted merge was settled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Settled {
    /// Every conflict was in a generated file; they were resolved and the
    /// merge committed. The files, in the order given.
    Resolved(Vec<String>),
    /// A real conflict: the files someone must resolve, and why any
    /// generated file couldn't be.
    Conflict {
        /// Files that need a person or agent
        real: Vec<String>,
        /// Generated files that conflicted too
        generated: Vec<String>,
    },
}

/// `conflicted` split into the files someone writes and the generated
/// ones, each in the order given.
#[must_use]
pub fn split(conflicted: &[String]) -> (Vec<String>, Vec<String>) {
    conflicted
        .iter()
        .cloned()
        .partition(|path| Generated::of(path).is_none())
}

/// Settle the merge in progress in `worktree`, which conflicted in
/// `conflicted`: resolve and commit when every file is generated.
/// Leaves the merge in progress on [`Settled::Conflict`]; the caller
/// aborts it.
///
/// # Errors
/// A git command couldn't run. The merge is still in progress then.
pub async fn settle(
    worktree: &Path,
    conflicted: &[String],
    config: &QueueConfig,
) -> Result<Settled, QueueError> {
    let (real, generated) = split(conflicted);
    if !real.is_empty() || generated.is_empty() {
        return Ok(Settled::Conflict { real, generated });
    }
    for path in &generated {
        if let Err(why) = resolve_one(worktree, path, config).await {
            tracing::info!(%path, %why, "A generated file could not be resolved mechanically");
            return Ok(Settled::Conflict {
                real: vec![format!("{path} ({why})")],
                generated: Vec::new(),
            });
        }
    }
    git::ok(
        worktree,
        &["commit", "--no-edit", "--quiet"],
        config.git_timeout,
    )
    .await?;
    Ok(Settled::Resolved(generated))
}

/// Resolve one generated file in the worktree and stage it. `Err` says
/// why it can't be done mechanically.
async fn resolve_one(worktree: &Path, path: &str, config: &QueueConfig) -> Result<(), String> {
    let kind = Generated::of(path).ok_or_else(|| "not a generated file".to_string())?;
    let timeout = config.git_timeout;
    match kind.regenerate() {
        None => union(worktree, path, timeout).await?,
        Some((program, args)) => {
            let ours = git::run(worktree, &["checkout", "--ours", "--", path], timeout)
                .await
                .map_err(|e| e.to_string())?;
            if !ours.ok {
                return Err(format!("deleted on {}'s side", config.target));
            }
            let folder = Path::new(path).parent().unwrap_or_else(|| Path::new(""));
            regenerate(&worktree.join(folder), program, args, config.check_timeout).await?;
        }
    }
    let added = git::run(worktree, &["add", "--", path], timeout)
        .await
        .map_err(|e| e.to_string())?;
    if added.ok {
        Ok(())
    } else {
        Err(format!("git add: {}", added.stderr))
    }
}

/// Write `path` as the union of both sides' changes over their base.
async fn union(worktree: &Path, path: &str, timeout: Duration) -> Result<(), String> {
    let stages = git::run(
        worktree,
        &["checkout-index", "--stage=all", "--temp", "--", path],
        timeout,
    )
    .await
    .map_err(|e| e.to_string())?;
    if !stages.ok {
        return Err(format!("reading its versions: {}", stages.stderr));
    }
    let temps = temp_names(&stages.stdout);
    let result = union_temps(worktree, path, &temps, timeout).await;
    for temp in temps.iter().flatten() {
        // A leftover temp file is cleaned with the worktree next time.
        let _ = tokio::fs::remove_file(worktree.join(temp)).await;
    }
    result
}

async fn union_temps(
    worktree: &Path,
    path: &str,
    temps: &[Option<String>; 3],
    timeout: Duration,
) -> Result<(), String> {
    let [base, Some(ours), Some(theirs)] = temps else {
        return Err("deleted on one side".to_string());
    };
    let base = match base {
        Some(base) => base.clone(),
        None => {
            // Added on both sides: their base is empty.
            tokio::fs::write(worktree.join(EMPTY_BASE), b"")
                .await
                .map_err(|e| e.to_string())?;
            EMPTY_BASE.to_string()
        }
    };
    let merged = git::run(
        worktree,
        &["merge-file", "--union", "-q", ours, &base, theirs],
        timeout,
    )
    .await
    .map_err(|e| e.to_string());
    if base == EMPTY_BASE {
        let _ = tokio::fs::remove_file(worktree.join(EMPTY_BASE)).await;
    }
    let merged = merged?;
    if !merged.ok {
        return Err(format!("merge-file: {}", merged.stderr));
    }
    tokio::fs::copy(worktree.join(ours), worktree.join(path))
        .await
        .map(|_| ())
        .map_err(|e| format!("writing it: {e}"))
}

/// The base, ours and theirs temp files `checkout-index --stage=all
/// --temp` printed (`base ours theirs\tpath`, `.` for a missing stage).
fn temp_names(listed: &str) -> [Option<String>; 3] {
    let names = listed.split('\t').next().unwrap_or_default();
    let mut found = names
        .split(' ')
        .map(|name| (name != "." && !name.is_empty()).then(|| name.to_string()));
    [
        found.next().flatten(),
        found.next().flatten(),
        found.next().flatten(),
    ]
}

/// Run `program args` in `folder` within `timeout`; `Err` says how it
/// failed.
async fn regenerate(
    folder: &Path,
    program: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<(), String> {
    let run = Command::new(program)
        .args(args)
        .current_dir(folder)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    let shown = format!("{program} {}", args.join(" "));
    let output = tokio::time::timeout(timeout, run)
        .await
        .map_err(|_| format!("`{shown}` took longer than {}s", timeout.as_secs()))?
        .map_err(|e| format!("`{shown}` could not start: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let said = stderr.trim().lines().last().unwrap_or("failed");
    Err(format!("`{shown}` failed: {said}"))
}

/// Why merging with `target`, the tip of branch `name`, stopped: the
/// files someone must resolve first, then the generated ones the queue
/// would have resolved; git's last word when no file conflicted.
#[must_use]
pub fn conflict_reason(
    name: &str,
    target: &str,
    real: &[String],
    generated: &[String],
    stderr: &str,
) -> String {
    let at = short_sha(target);
    if real.is_empty() && generated.is_empty() {
        let said = stderr.lines().last().unwrap_or("merge failed");
        return format!("merging with {name} at {at} failed: {said}");
    }
    let mut reason = format!(
        "conflicts with {name} at {at} in {}; pull {name}, resolve them and push again",
        real.join(", ")
    );
    if !generated.is_empty() {
        reason.push_str(&format!(
            " ({} conflicted too; the queue resolves those itself)",
            generated.join(", ")
        ));
    }
    reason
}

#[cfg(test)]
mod tests;
