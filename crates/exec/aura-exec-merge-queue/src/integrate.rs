//! Landing one entry: rebase it onto the latest target in the queue's own
//! worktree, run the checks there, then move the target branch to it,
//! fast-forward only.
//!
//! ## Invariants
//!
//! - The target branch only ever fast-forwards: the remote push never
//!   forces, and a local target moves with a compare-and-swap
//!   `update-ref` or a `merge --ff-only` of a clean checkout.
//! - Nothing lands unless every configured check passed on exactly the
//!   commit that lands.
//! - All work happens in the queue's worktree (under the repo's common
//!   git dir) and in private `refs/aura-queue/*` refs; agents' workspaces
//!   and the user's checkout are never touched, except that a local
//!   target checked out clean is fast-forwarded like `git pull` would.
//!
//! ## Failure modes
//!
//! A conflict, a failed check, a missing branch or commit, or a git error
//! fails the entry with its reason. A target that moved while the entry
//! was checked is retried from the fetch, up to [`LAND_ATTEMPTS`] times.

use crate::checks::run_check;
use crate::config::QueueConfig;
use crate::entry::{short_sha, MergeEntry};
use crate::error::QueueError;
use crate::git;
use crate::state::RunOutcome;
use async_trait::async_trait;
use std::path::{Path, PathBuf};

/// How many times an entry is tried again when the target moved under it.
pub const LAND_ATTEMPTS: u32 = 3;
const TARGET_REF: &str = "refs/aura-queue/target";
const ENTRY_REF: &str = "refs/aura-queue/entry";

/// Lands entries. The queue's processor runs one at a time.
#[async_trait]
pub trait Integrator: Send + Sync {
    /// Try to land `entry`; says how it went.
    async fn integrate(&self, entry: &MergeEntry) -> RunOutcome;
}

/// Why landing stopped.
enum Stop {
    Fail(String),
    /// The target moved since it was fetched: try again.
    Moved,
}

impl From<QueueError> for Stop {
    fn from(error: QueueError) -> Self {
        Self::Fail(error.to_string())
    }
}

type Step<T> = Result<T, Stop>;

/// The repo a queue lands entries in.
#[derive(Debug, Clone)]
pub struct RepoPaths {
    /// Top of the repo's main worktree
    pub top: PathBuf,
    /// The repo's common git dir
    pub common: PathBuf,
}

impl RepoPaths {
    /// Where the queue keeps its file, lock and worktree.
    #[must_use]
    pub fn queue_dir(&self) -> PathBuf {
        self.common.join("aura")
    }

    /// The queue's own worktree.
    #[must_use]
    pub fn worktree(&self) -> PathBuf {
        self.queue_dir().join("merge-worktree")
    }

    /// The repo `dir` is in.
    ///
    /// # Errors
    /// `dir` is not in a git repository.
    pub async fn of(dir: &Path) -> Result<Self, QueueError> {
        let timeout = crate::config::GIT_TIMEOUT;
        Ok(Self {
            top: git::main_top(dir, timeout).await?,
            common: git::common_dir(dir, timeout).await?,
        })
    }
}

/// Lands entries with git, as [`QueueConfig`] says.
pub struct GitIntegrator {
    repo: RepoPaths,
    config: QueueConfig,
}

impl GitIntegrator {
    /// An integrator for `repo` with `config`.
    #[must_use]
    pub const fn new(repo: RepoPaths, config: QueueConfig) -> Self {
        Self { repo, config }
    }

    async fn attempt(&self, entry: &MergeEntry) -> Step<RunOutcome> {
        let target = self.fetch(entry).await?;
        self.prepare_worktree(&target).await?;
        let Some((landing, how)) = self.rebase(entry, &target).await? else {
            return Ok(RunOutcome::Merged {
                merged: target,
                note: "already on the target branch; nothing to merge".to_string(),
            });
        };
        let checked = self.check().await?;
        self.land(entry, &landing, &target).await?;
        Ok(RunOutcome::Merged {
            merged: landing,
            note: format!("{how}; {checked}"),
        })
    }

    /// Fetch the target and the entry's branch; returns the target's tip.
    async fn fetch(&self, entry: &MergeEntry) -> Step<String> {
        let top = &self.repo.top;
        let timeout = self.config.git_timeout;
        let target = &self.config.target;
        let target_ref = if let Some(remote) = entry.remote.as_deref() {
            let target_spec = format!("+refs/heads/{target}:{TARGET_REF}");
            let entry_spec = format!("+refs/heads/{}:{ENTRY_REF}", entry.branch);
            git::ok(
                top,
                &["fetch", "--no-tags", remote, &target_spec, &entry_spec],
                timeout,
            )
            .await?;
            TARGET_REF.to_string()
        } else {
            format!("refs/heads/{target}")
        };
        let tip = git::run(
            top,
            &["rev-parse", "--verify", "--quiet", &target_ref],
            timeout,
        )
        .await?;
        if !tip.ok {
            return Err(Stop::Fail(format!(
                "the target branch {target} does not exist"
            )));
        }
        let commit = format!("{}^{{commit}}", entry.head);
        let head = git::run(top, &["rev-parse", "--verify", "--quiet", &commit], timeout).await?;
        if !head.ok {
            return Err(Stop::Fail(format!(
                "commit {} of {} is gone (was the branch force-pushed?)",
                entry.short_head(),
                entry.branch
            )));
        }
        Ok(tip.stdout)
    }

    /// Put the queue's worktree at `target`, clean, creating it if needed.
    async fn prepare_worktree(&self, target: &str) -> Step<()> {
        let worktree = self.repo.worktree();
        let timeout = self.config.git_timeout;
        if !worktree.join(".git").exists() {
            let top = &self.repo.top;
            git::run(top, &["worktree", "prune"], timeout).await?;
            let path = worktree.display().to_string();
            git::ok(
                top,
                &["worktree", "add", "--detach", &path, target],
                timeout,
            )
            .await?;
            return Ok(());
        }
        // A rebase a crash left open; there is usually none.
        git::run(&worktree, &["rebase", "--abort"], timeout).await?;
        git::ok(
            &worktree,
            &["checkout", "--detach", "--force", target],
            timeout,
        )
        .await?;
        git::ok(&worktree, &["reset", "--hard", target], timeout).await?;
        // Ignored files (build caches) stay, so the next build is quick.
        git::ok(&worktree, &["clean", "-fd"], timeout).await?;
        Ok(())
    }

    /// Rebase the entry onto `target` in the worktree. Returns the commit
    /// to land and how it got there; `None` when it adds nothing.
    async fn rebase(&self, entry: &MergeEntry, target: &str) -> Step<Option<(String, String)>> {
        let worktree = self.repo.worktree();
        let timeout = self.config.git_timeout;
        git::ok(
            &worktree,
            &["checkout", "--detach", "--force", &entry.head],
            timeout,
        )
        .await?;
        let based = git::run(
            &worktree,
            &["merge-base", "--is-ancestor", target, "HEAD"],
            timeout,
        )
        .await?;
        let how = if based.ok {
            "fast-forward".to_string()
        } else {
            let rebased = git::run(&worktree, &["rebase", target], timeout).await?;
            if !rebased.ok {
                let conflicts = git::run(
                    &worktree,
                    &["diff", "--name-only", "--diff-filter=U"],
                    timeout,
                )
                .await?;
                git::run(&worktree, &["rebase", "--abort"], timeout).await?;
                return Err(Stop::Fail(conflict_reason(
                    target,
                    &conflicts.stdout,
                    &rebased.stderr,
                )));
            }
            format!("rebased onto {}", short_sha(target))
        };
        let landing = git::ok(&worktree, &["rev-parse", "HEAD"], timeout).await?;
        Ok((landing != target).then_some((landing, how)))
    }

    /// Run the configured checks in the worktree; says which passed.
    async fn check(&self) -> Step<String> {
        let worktree = self.repo.worktree();
        let mut passed = Vec::new();
        for (name, command) in [("build", &self.config.build), ("test", &self.config.test)] {
            let Some(command) = command.as_deref() else {
                continue;
            };
            run_check(&worktree, command, self.config.check_timeout)
                .await
                .map_err(|failure| {
                    Stop::Fail(format!("{name} `{}` {}", failure.command, failure.reason))
                })?;
            passed.push(name);
        }
        Ok(if passed.is_empty() {
            "no build or test configured".to_string()
        } else {
            format!("{} passed", passed.join(" and "))
        })
    }

    /// Move the target branch from `target` to `landing`.
    async fn land(&self, entry: &MergeEntry, landing: &str, target: &str) -> Step<()> {
        let timeout = self.config.git_timeout;
        let branch = format!("refs/heads/{}", self.config.target);
        let Some(remote) = entry.remote.as_deref() else {
            return self.land_local(&branch, landing, target).await;
        };
        let spec = format!("{landing}:{branch}");
        let pushed = git::run(
            &self.repo.top,
            &["push", "--porcelain", remote, &spec],
            timeout,
        )
        .await?;
        if pushed.ok {
            return Ok(());
        }
        let said = format!("{}\n{}", pushed.stdout, pushed.stderr);
        if said.contains("non-fast-forward")
            || said.contains("fetch first")
            || said.contains("[rejected]")
        {
            return Err(Stop::Moved);
        }
        Err(Stop::Fail(format!(
            "pushing to {remote}: {}",
            pushed.stderr
        )))
    }

    /// Move a local target: fast-forward its checkout when it has one,
    /// else swap the ref.
    async fn land_local(&self, branch: &str, landing: &str, target: &str) -> Step<()> {
        let timeout = self.config.git_timeout;
        let top = &self.repo.top;
        let listed = git::ok(top, &["worktree", "list", "--porcelain"], timeout).await?;
        let Some(checkout) = checkout_of(&listed, branch) else {
            let swapped = git::run(top, &["update-ref", branch, landing, target], timeout).await?;
            return if swapped.ok { Ok(()) } else { Err(Stop::Moved) };
        };
        let status = git::ok(
            &checkout,
            &["status", "--porcelain", "--untracked-files=no"],
            timeout,
        )
        .await?;
        if !status.is_empty() {
            return Err(Stop::Fail(format!(
                "{} is checked out with uncommitted changes at {}; commit or stash them",
                self.config.target,
                checkout.display()
            )));
        }
        let merged = git::run(&checkout, &["merge", "--ff-only", landing], timeout).await?;
        if merged.ok {
            Ok(())
        } else {
            Err(Stop::Fail(format!(
                "fast-forwarding {}: {}",
                checkout.display(),
                merged.stderr
            )))
        }
    }
}

#[async_trait]
impl Integrator for GitIntegrator {
    async fn integrate(&self, entry: &MergeEntry) -> RunOutcome {
        for _ in 0..LAND_ATTEMPTS {
            match self.attempt(entry).await {
                Ok(outcome) => return outcome,
                Err(Stop::Fail(reason)) => return RunOutcome::Failed(reason),
                Err(Stop::Moved) => {
                    tracing::info!(entry = %entry.id, "Target moved while checking; trying again");
                }
            }
        }
        RunOutcome::Failed(format!(
            "{} kept moving; tried {LAND_ATTEMPTS} times",
            self.config.target
        ))
    }
}

/// The worktree `worktree list --porcelain` shows with `branch` checked
/// out, if any.
fn checkout_of(listed: &str, branch: &str) -> Option<PathBuf> {
    let mut path = None;
    for line in listed.lines() {
        if let Some(found) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(found));
        } else if line.strip_prefix("branch ") == Some(branch) {
            return path;
        }
    }
    None
}

/// Why a rebase onto `target` stopped, naming the conflicted files.
fn conflict_reason(target: &str, conflicts: &str, stderr: &str) -> String {
    let files: Vec<&str> = conflicts.lines().filter(|line| !line.is_empty()).collect();
    if files.is_empty() {
        let said = stderr.lines().last().unwrap_or("rebase failed");
        return format!("rebasing onto {} failed: {said}", short_sha(target));
    }
    format!(
        "conflicts with {} in {}; pull main, resolve them and push again",
        short_sha(target),
        files.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_checkout_of_a_branch() {
        let listed = "worktree /repo\nHEAD 1\nbranch refs/heads/main\n\n\
                      worktree /wt\nHEAD 2\ndetached\n";
        assert_eq!(
            checkout_of(listed, "refs/heads/main"),
            Some(PathBuf::from("/repo"))
        );
        assert_eq!(checkout_of(listed, "refs/heads/dev"), None);
    }

    #[test]
    fn a_conflict_names_its_files() {
        let reason = conflict_reason("0123456789", "a.rs\nb.rs\n", "");
        assert!(reason.contains("a.rs, b.rs"), "{reason}");
        assert!(reason.contains("01234567"), "{reason}");
    }
}
