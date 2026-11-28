//! Landing one entry: merge the latest target into it in the queue's own
//! worktree, run the checks there, then move the target branch to the
//! result, fast-forward only, or merge the branch's PR pinned to the
//! tested head ([`pull_request`]).
//!
//! The queue merges rather than rebases: an agent's `git_ship` already
//! merged the target into its branch and resolved what conflicted, and a
//! rebase would drop that merge and replay the conflict. A branch based
//! on the latest target lands as it is; any other gets one merge commit
//! (the target first, then the branch).
//!
//! ## Invariants
//!
//! - The target branch only ever fast-forwards: the remote push never
//!   forces, and a local target moves with a compare-and-swap
//!   `update-ref` or a `merge --ff-only` of a clean checkout.
//! - Nothing lands unless every check passed on exactly the commit that
//!   lands. The checks are those of the target's own config file
//!   ([`QueueConfig::with_checks`]), read per entry.
//! - All work happens in the queue's worktree (in its work folder outside
//!   `.git`, see [`RepoPaths`]) and in private `refs/aura-queue/*` refs; agents' workspaces
//!   and the user's checkout are never touched, except that a local
//!   target checked out clean is fast-forwarded like `git pull` would.
//!
//! ## Failure modes
//!
//! A conflict only in generated files (the changelog, lockfiles) is
//! resolved mechanically and the merge goes on to the checks
//! ([`crate::resolve`]). Any other conflict, a failed check, a missing
//! branch or commit, or a git error fails the entry with its reason. A target that moved while the entry
//! was checked is retried from the fetch, up to [`LAND_ATTEMPTS`] times.

use crate::checks::run_check;
use crate::config::{Landing, QueueConfig};
use crate::entry::{short_sha, MergeEntry};
use crate::error::QueueError;
use crate::forge::Forge;
use crate::git;
use crate::report::{Failure, FailureKind, Hold};
use crate::resolve::{self, conflict_reason, Settled};
use crate::state::RunOutcome;
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub use crate::repo::RepoPaths;

mod land;
mod pull_request;
mod worktree;

/// How many times an entry is tried again when the target moved under it.
pub const LAND_ATTEMPTS: u32 = 3;
const TARGET_REF: &str = "refs/aura-queue/target";
const ENTRY_REF: &str = "refs/aura-queue/entry";

/// Lands entries. The queue's processor runs one at a time.
#[async_trait]
pub trait Integrator: Send + Sync {
    /// Try to land `entry`; says how it went.
    async fn integrate(&self, entry: &MergeEntry) -> RunOutcome;

    /// The target's tip when it holds `entry`'s commit, which means the
    /// entry landed; `None` when it doesn't or that can't be told.
    async fn landed(&self, _entry: &MergeEntry) -> Option<String> {
        None
    }

    /// The target's tip now, read from `remote` (else the local branch);
    /// `None` when it can't be read. A held queue polls it to tell when
    /// the target moved.
    async fn target_tip(&self, _remote: Option<&str>) -> Option<String> {
        None
    }
}

/// Why landing stopped.
enum Stop {
    Fail(Box<Failure>),
    /// The target or the branch moved since it was fetched: try again.
    Moved,
    /// Not the entry's fault: hold the queue.
    Hold(Box<Hold>),
}

impl Stop {
    fn fail(kind: FailureKind, reason: impl Into<String>) -> Self {
        Self::Fail(Box::new(Failure::new(kind, reason)))
    }
}

impl From<QueueError> for Stop {
    fn from(error: QueueError) -> Self {
        Self::fail(FailureKind::Other, error.to_string())
    }
}

type Step<T> = Result<T, Stop>;

/// Lands entries with git, as [`QueueConfig`] says.
pub struct GitIntegrator {
    repo: RepoPaths,
    config: QueueConfig,
    /// Where the repo's PRs live; entries land by push without one
    forge: Option<Arc<dyn Forge>>,
}

impl GitIntegrator {
    /// An integrator for `repo` with `config`.
    #[must_use]
    pub const fn new(repo: RepoPaths, config: QueueConfig) -> Self {
        Self {
            repo,
            config,
            forge: None,
        }
    }

    /// This integrator, landing pushed entries through their PRs on
    /// `forge` ([`Landing`]).
    #[must_use]
    pub fn with_forge(mut self, forge: Arc<dyn Forge>) -> Self {
        self.forge = Some(forge);
        self
    }

    async fn attempt(&self, entry: &MergeEntry) -> Step<RunOutcome> {
        let target = self.fetch(entry).await?;
        self.prepare_worktree(&target).await?;
        if let Some(forge) = self.forge_for(entry, &target).await? {
            return self.attempt_pr(forge.as_ref(), entry, &target).await;
        }
        let Some((landing, how)) = self.merge(entry, &target).await? else {
            return Ok(RunOutcome::Merged {
                merged: target,
                note: "already on the target branch; nothing to merge".to_string(),
                pr: None,
            });
        };
        let checked = self.check(&target).await?;
        self.land(entry, &landing, &target).await?;
        Ok(RunOutcome::Merged {
            merged: landing,
            note: format!("{how}; {checked}"),
            pr: None,
        })
    }

    /// The forge `entry` lands through, as the target's config says
    /// ([`Landing`]); `None` to push it to the target.
    async fn forge_for(&self, entry: &MergeEntry, target: &str) -> Step<Option<Arc<dyn Forge>>> {
        let land = self.checks_at(target, &self.repo.worktree()).await?.land;
        let forge = entry.remote.as_ref().and(self.forge.clone());
        match (land, forge) {
            (Landing::Push, _) | (Landing::Auto, None) => Ok(None),
            (_, Some(forge)) => Ok(Some(forge)),
            (Landing::PullRequest, None) => Err(Stop::fail(
                FailureKind::Other,
                "the repo lands through pull requests, but the queue has no GitHub repo for it",
            )),
        }
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
                &[&git::FETCH_ARGS[..], &[remote, &target_spec, &entry_spec]].concat(),
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
            return Err(Stop::fail(
                FailureKind::Other,
                format!("the target branch {target} does not exist"),
            ));
        }
        let commit = format!("{}^{{commit}}", entry.head);
        let head = git::run(top, &["rev-parse", "--verify", "--quiet", &commit], timeout).await?;
        if !head.ok {
            return Err(Stop::fail(
                FailureKind::Other,
                format!(
                    "commit {} of {} is gone (was the branch force-pushed?)",
                    entry.short_head(),
                    entry.branch
                ),
            ));
        }
        Ok(tip.stdout)
    }

    /// Merge the entry with `target` in the worktree. Returns the commit
    /// to land and how it got there; `None` when it adds nothing.
    async fn merge(&self, entry: &MergeEntry, target: &str) -> Step<Option<(String, String)>> {
        let worktree = self.repo.worktree();
        let timeout = self.config.git_timeout;
        let landed = git::run(
            &worktree,
            &["merge-base", "--is-ancestor", &entry.head, target],
            timeout,
        )
        .await?;
        if landed.ok {
            return Ok(None);
        }
        let based = git::run(
            &worktree,
            &["merge-base", "--is-ancestor", target, &entry.head],
            timeout,
        )
        .await?;
        if based.ok {
            git::ok(
                &worktree,
                &["checkout", "--detach", "--force", &entry.head],
                timeout,
            )
            .await?;
            return Ok(Some((entry.head.clone(), "fast-forward".to_string())));
        }
        self.merge_commit(entry, target).await.map(Some)
    }

    /// Merge the entry into the worktree, which is at `target`, as one
    /// merge commit. Returns it and how it got there.
    async fn merge_commit(&self, entry: &MergeEntry, target: &str) -> Step<(String, String)> {
        let worktree = self.repo.worktree();
        let timeout = self.config.git_timeout;
        let message = format!(
            "Merge {} ({}) through the merge queue",
            entry.branch, entry.id
        );
        let merged = git::run(
            &worktree,
            &["merge", "--no-ff", "--no-edit", "-m", &message, &entry.head],
            timeout,
        )
        .await?;
        let mut how = format!("merged with {}", short_sha(target));
        if !merged.ok {
            let resolved = self.settle(&worktree, target, &merged.stderr).await?;
            how.push_str(&format!("; resolved {} mechanically", resolved.join(", ")));
        }
        let landing = git::ok(&worktree, &["rev-parse", "HEAD"], timeout).await?;
        Ok((landing, how))
    }

    /// Settle the conflicted merge in `worktree` ([`resolve::settle`]):
    /// the generated files it resolved, or the entry fails with the
    /// conflicted files after the merge is aborted.
    async fn settle(&self, worktree: &Path, target: &str, stderr: &str) -> Step<Vec<String>> {
        let timeout = self.config.git_timeout;
        let listed = git::run(
            worktree,
            &["diff", "--name-only", "--diff-filter=U"],
            timeout,
        )
        .await?;
        let conflicted: Vec<String> = listed
            .stdout
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect();
        let settled = resolve::settle(worktree, &conflicted, &self.config).await;
        let (real, generated) = match settled {
            Ok(Settled::Resolved(files)) => return Ok(files),
            Ok(Settled::Conflict { real, generated }) => (real, generated),
            Err(error) => (vec![error.to_string()], Vec::new()),
        };
        git::run(worktree, &["merge", "--abort"], timeout).await?;
        let reason = conflict_reason(&self.config.target, target, &real, &generated, stderr);
        Err(Stop::Fail(Box::new(Failure {
            files: real,
            ..Failure::new(FailureKind::Conflict, reason)
        })))
    }

    /// Run the checks the target's config file sets (else the inferred
    /// ones) in the worktree; says which passed.
    async fn check(&self, target: &str) -> Step<String> {
        let worktree = self.repo.worktree();
        let config = self.checks_at(target, &worktree).await?;
        let log = self.repo.check_log();
        let mut passed = Vec::new();
        for (name, command) in [("build", &config.build), ("test", &config.test)] {
            let Some(command) = command.as_deref() else {
                continue;
            };
            run_check(&worktree, command, config.check_timeout, &log)
                .await
                .map_err(|failure| check_failed(name, &failure))?;
            passed.push(name);
        }
        Ok(if passed.is_empty() {
            "no build or test configured".to_string()
        } else {
            format!("{} passed", passed.join(" and "))
        })
    }

    /// The config with the checks of the target commit `target`'s config
    /// file, inferred from `tree` where it sets none.
    async fn checks_at(&self, target: &str, tree: &Path) -> Step<QueueConfig> {
        let spec = format!("{target}:{}", crate::config::CONFIG_PATH);
        let shown = git::run(&self.repo.top, &["show", &spec], self.config.git_timeout).await?;
        let text = shown.ok.then_some(shown.stdout);
        let tree = tree.to_path_buf();
        let base = self.config.clone();
        tokio::task::spawn_blocking(move || base.with_checks(text.as_deref(), &tree))
            .await
            .map_err(|e| Stop::fail(FailureKind::Other, format!("reading the checks: {e}")))?
            .map_err(Stop::from)
    }
}

#[async_trait]
impl Integrator for GitIntegrator {
    async fn landed(&self, entry: &MergeEntry) -> Option<String> {
        match entry.remote.as_deref() {
            Some(remote) => self.remote_holds(remote, &entry.head).await,
            None => {
                let branch = format!("refs/heads/{}", self.config.target);
                self.holds(&branch, &entry.head).await
            }
        }
    }

    async fn target_tip(&self, remote: Option<&str>) -> Option<String> {
        self.tip_of_target(remote).await
    }

    async fn integrate(&self, entry: &MergeEntry) -> RunOutcome {
        for _ in 0..LAND_ATTEMPTS {
            match self.attempt(entry).await {
                Ok(outcome) => return outcome,
                Err(Stop::Fail(failure)) => return RunOutcome::Failed(*failure),
                Err(Stop::Hold(hold)) => return RunOutcome::Held(*hold),
                Err(Stop::Moved) => {
                    tracing::info!(entry = %entry.id, "Target moved while checking; trying again");
                }
            }
        }
        RunOutcome::Failed(Failure::new(
            FailureKind::Other,
            format!(
                "{} or the branch kept moving; tried {LAND_ATTEMPTS} times",
                self.config.target
            ),
        ))
    }
}

/// The failure of check `name` (`build`, `test`): its reason, and the
/// output after its first line as the excerpt.
fn check_failed(name: &str, failure: &crate::checks::CheckFailure) -> Stop {
    let output = failure
        .reason
        .split_once('\n')
        .map_or("", |(_, output)| output);
    Stop::Fail(Box::new(Failure {
        excerpt: (!output.trim().is_empty()).then(|| output.to_string()),
        ..Failure::new(
            FailureKind::Check,
            format!("{name} `{}` {}", failure.command, failure.reason),
        )
    }))
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

#[cfg(test)]
mod tests;
