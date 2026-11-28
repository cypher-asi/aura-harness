//! Moving the target to a checked merge without a forge: a non-forced
//! push to the remote's target, or a compare-and-swap of a local one.

use super::{checkout_of, GitIntegrator, Step, Stop, TARGET_REF};
use crate::entry::MergeEntry;
use crate::git;
use crate::report::FailureKind;

impl GitIntegrator {
    /// Move the target branch from `target` to `landing`.
    pub(super) async fn land(&self, entry: &MergeEntry, landing: &str, target: &str) -> Step<()> {
        let timeout = self.config.git_timeout;
        let branch = format!("refs/heads/{}", self.config.target);
        let Some(remote) = entry.remote.as_deref() else {
            return self.land_local(&branch, landing, target).await;
        };
        let spec = format!("{landing}:{branch}");
        let pushed = match git::run(
            &self.repo.top,
            &["push", "--porcelain", remote, &spec],
            timeout,
        )
        .await
        {
            Ok(pushed) => pushed,
            // A push cut off by its timeout may have landed all the same.
            Err(error) => {
                return match self.remote_holds(remote, landing).await {
                    Some(_) => Ok(()),
                    None => Err(Stop::from(error)),
                };
            }
        };
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
        Err(Stop::fail(
            FailureKind::Other,
            format!("pushing to {remote}: {}", pushed.stderr),
        ))
    }

    /// Move a local target: fast-forward its checkout when it has one,
    /// else swap the ref.
    pub(super) async fn land_local(&self, branch: &str, landing: &str, target: &str) -> Step<()> {
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
            return Err(Stop::fail(
                FailureKind::Other,
                format!(
                    "{} is checked out with uncommitted changes at {}; commit or stash them",
                    self.config.target,
                    checkout.display()
                ),
            ));
        }
        let merged = git::run(&checkout, &["merge", "--ff-only", landing], timeout).await?;
        if merged.ok {
            Ok(())
        } else {
            Err(Stop::fail(
                FailureKind::Other,
                format!("fast-forwarding {}: {}", checkout.display(), merged.stderr),
            ))
        }
    }

    /// The tip of the remote's target when it holds `commit`.
    pub(super) async fn remote_holds(&self, remote: &str, commit: &str) -> Option<String> {
        let timeout = self.config.git_timeout;
        let top = &self.repo.top;
        let spec = format!("+refs/heads/{}:{TARGET_REF}", self.config.target);
        let args = [&git::FETCH_ARGS[..], &[remote, &spec]].concat();
        git::ok(top, &args, timeout).await.ok()?;
        self.holds(TARGET_REF, commit).await
    }

    /// The tip of `reference` when it holds `commit`.
    pub(super) async fn holds(&self, reference: &str, commit: &str) -> Option<String> {
        let timeout = self.config.git_timeout;
        let top = &self.repo.top;
        let held = git::run(
            top,
            &["merge-base", "--is-ancestor", commit, reference],
            timeout,
        )
        .await
        .ok()?;
        if !held.ok {
            return None;
        }
        git::ok(top, &["rev-parse", reference], timeout).await.ok()
    }

    /// The target's tip on `remote` (`ls-remote`), else the local
    /// branch's; `None` when it can't be read.
    pub(super) async fn tip_of_target(&self, remote: Option<&str>) -> Option<String> {
        let timeout = self.config.git_timeout;
        let top = &self.repo.top;
        let branch = format!("refs/heads/{}", self.config.target);
        let Some(remote) = remote else {
            return git::ok(top, &["rev-parse", "--verify", "--quiet", &branch], timeout)
                .await
                .ok();
        };
        let listed = git::ok(top, &["ls-remote", remote, &branch], timeout)
            .await
            .ok()?;
        listed.lines().find_map(|line| {
            let (sha, name) = line.split_once('\t')?;
            (name == branch).then(|| sha.to_string())
        })
    }
}
