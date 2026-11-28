//! Landing an entry through its branch's pull request.
//!
//! 1. The branch's tip is the head that gets tested: a push since the
//!    entry was queued is tested too, a force-push that dropped the
//!    queued commit fails the entry.
//! 2. The queue finds or opens the PR and sets its status to pending.
//! 3. It merges the latest target in its own worktree (resolving
//!    generated files as for a push) and runs the checks there.
//! 4. A merge commit it made goes onto the PR's branch as a fast-forward,
//!    so the PR's head is exactly the tested commit.
//! 5. With the target unchanged, it sets the status to success and has
//!    the forge merge the PR with a merge commit, only at that head.
//! 6. After a confirmed merge it deletes the branch on the remote, unless
//!    something was pushed to it since.
//!
//! ## Invariants
//!
//! - Nothing merges that the checks didn't pass on: the forge merges the
//!   tested head or nothing ([`crate::forge::Forge::merge`]), and the
//!   target must not have moved since the test.
//! - A problem never merges: the PR stays open with a comment saying why
//!   and a failure status, and the entry fails with the files or output.
//! - A moved branch or target is retried from the fetch
//!   ([`super::LAND_ATTEMPTS`]); a forge that can't be reached holds the
//!   queue ([`HoldKind::ForgeUnavailable`]).
//! - A branch is deleted only with a lease on the commit that merged (or
//!   was abandoned), so newer work on it is never lost.

use super::{GitIntegrator, Step, Stop, ENTRY_REF};
use crate::entry::{short_sha, MergeEntry};
use crate::forge::{CheckState, Forge, ForgeError, PrRequest, PrState, PullRequest};
use crate::git;
use crate::report::{Failure, FailureKind, Hold, HoldKind};
use crate::state::RunOutcome;
use chrono::Utc;

/// How long a queue waits before trying an unreachable forge again.
const FORGE_RETRY: chrono::Duration = chrono::Duration::minutes(5);
/// Most characters of output a failure comment quotes.
const COMMENT_EXCERPT_CHARS: usize = 6_000;
/// Most commit subjects a PR the queue opens lists.
const BODY_COMMITS: usize = 20;

impl GitIntegrator {
    /// Land `entry` through its PR on `forge`, the worktree being at
    /// `target`.
    pub(super) async fn attempt_pr(
        &self,
        forge: &dyn Forge,
        entry: &MergeEntry,
        target: &str,
    ) -> Step<RunOutcome> {
        let head = self.branch_tip(entry).await?;
        let (title, body) = self.pr_text(entry, target, &head).await?;
        let request = PrRequest {
            branch: &entry.branch,
            target: &self.config.target,
            title: &title,
            body: &body,
        };
        let pr = forge.open_or_find(&request).await.map_err(forge_stop)?;
        match self.test_and_merge(forge, entry, &pr, &head, target).await {
            Err(Stop::Fail(failure)) => {
                let failure = Box::new(Failure {
                    pr: Some(pr.pr.clone()),
                    ..*failure
                });
                self.report(forge, entry, &pr, &failure).await;
                Err(Stop::Fail(failure))
            }
            other => other,
        }
    }

    async fn test_and_merge(
        &self,
        forge: &dyn Forge,
        entry: &MergeEntry,
        pr: &PullRequest,
        head: &str,
        target: &str,
    ) -> Step<RunOutcome> {
        let name = &self.config.target;
        status(
            forge,
            head,
            CheckState::Pending,
            &format!("testing the merge with {name}"),
        )
        .await;
        let tested = MergeEntry {
            head: head.to_string(),
            ..entry.clone()
        };
        if self.on_target(head, target).await? {
            return self.already_landed(forge, pr, target).await;
        }
        // Always a merge commit, the target first, even for a branch based
        // on the target: it is what the checks run on, what the PR's
        // branch moves to, and what the target moves to, unchanged.
        let (landing, how) = self.merge_commit(&tested, target).await?;
        let checked = self.check(target).await?;
        self.push_to_branch(entry, &landing).await?;
        self.target_unchanged(entry, target).await?;
        status(forge, &landing, CheckState::Success, &checked).await;
        if checked.contains("flaky") {
            let said = format!("The merge queue merged this with flaky tests: {checked}.");
            if let Err(error) = forge.comment(pr.pr.number, &said).await {
                tracing::warn!(%error, "Could not name the flaky tests on the PR");
            }
        }
        let note = format!("{how}; {checked}");
        self.merge_pr(forge, entry, pr, &landing, target, &note)
            .await
    }

    /// Whether `head` is on the target's tip `target` already.
    async fn on_target(&self, head: &str, target: &str) -> Step<bool> {
        let landed = crate::git::run(
            &self.repo.worktree(),
            &["merge-base", "--is-ancestor", head, target],
            self.config.git_timeout,
        )
        .await?;
        Ok(landed.ok)
    }

    /// Land `pr` at the tested `landing` (a merge commit onto the
    /// target's tip `target`, now also the PR's head): a non-forced push
    /// of exactly that commit to the target, so the target is only ever
    /// a commit the checks ran on, and GitHub marks the PR merged as its
    /// head is now on the target. `note` says how it was made and checked.
    ///
    /// The PR is read first: a closed one fails the entry and keeps its
    /// branch; a head that moved is tested again. A push someone makes
    /// after that read can't reach the target: what lands is `landing`.
    async fn merge_pr(
        &self,
        forge: &dyn Forge,
        entry: &MergeEntry,
        pr: &PullRequest,
        landing: &str,
        target: &str,
        note: &str,
    ) -> Step<RunOutcome> {
        let number = pr.pr.number;
        let now = forge.pr(number).await.map_err(forge_stop)?;
        match now.state {
            PrState::Closed => {
                return Err(Stop::fail(
                    FailureKind::Refused,
                    format!("PR #{number} was closed without merging; its branch is kept"),
                ))
            }
            PrState::Merged => {
                return Err(Stop::fail(
                    FailureKind::Refused,
                    format!("PR #{number} was merged outside the queue"),
                ))
            }
            PrState::Open if now.head != landing => {
                tracing::info!(entry = %entry.id, now = %short_sha(&now.head), "The PR's head moved; testing it again");
                return Err(Stop::Moved);
            }
            PrState::Open => {}
        }
        self.land(entry, landing, target).await?;
        self.delete_branch(entry, landing).await;
        Ok(RunOutcome::Merged {
            merged: landing.to_string(),
            note: format!(
                "PR #{number} merged as the tested {}; {note}",
                short_sha(landing)
            ),
            pr: Some(pr.pr.clone()),
        })
    }

    /// The entry's head is already on the target: done, through the PR
    /// when GitHub saw it merge.
    async fn already_landed(
        &self,
        forge: &dyn Forge,
        pr: &PullRequest,
        target: &str,
    ) -> Step<RunOutcome> {
        let merged = forge.pr(pr.pr.number).await.ok();
        let merged = merged.filter(|now| now.state == PrState::Merged);
        Ok(RunOutcome::Merged {
            merged: merged
                .and_then(|now| now.merge_commit)
                .unwrap_or_else(|| target.to_string()),
            note: "already on the target branch; nothing to merge".to_string(),
            pr: Some(pr.pr.clone()),
        })
    }

    /// The branch's tip as fetched: the head to test. It must still hold
    /// the queued commit.
    async fn branch_tip(&self, entry: &MergeEntry) -> Step<String> {
        let top = &self.repo.top;
        let timeout = self.config.git_timeout;
        let tip = git::run(
            top,
            &["rev-parse", "--verify", "--quiet", ENTRY_REF],
            timeout,
        )
        .await?;
        if !tip.ok {
            return Err(Stop::fail(
                FailureKind::Other,
                format!("branch {} is gone from the remote", entry.branch),
            ));
        }
        let holds = git::run(
            top,
            &["merge-base", "--is-ancestor", &entry.head, &tip.stdout],
            timeout,
        )
        .await?;
        if !holds.ok {
            return Err(Stop::fail(
                FailureKind::Other,
                format!(
                    "{} no longer holds the queued commit {} (was it force-pushed?); ship again",
                    entry.branch,
                    entry.short_head()
                ),
            ));
        }
        Ok(tip.stdout)
    }

    /// Put the queue's merge commit `landing` on the entry's branch, as a
    /// fast-forward: rejected (the branch moved) means try again.
    async fn push_to_branch(&self, entry: &MergeEntry, landing: &str) -> Step<()> {
        let Some(remote) = entry.remote.as_deref() else {
            return Ok(());
        };
        let spec = format!("{landing}:refs/heads/{}", entry.branch);
        let pushed = git::run(
            &self.repo.top,
            &["push", "--porcelain", remote, &spec],
            self.config.git_timeout,
        )
        .await?;
        if pushed.ok {
            return Ok(());
        }
        let said = format!("{}\n{}", pushed.stdout, pushed.stderr);
        if said.contains("[rejected]")
            || said.contains("non-fast-forward")
            || said.contains("fetch first")
        {
            return Err(Stop::Moved);
        }
        Err(Stop::fail(
            FailureKind::Other,
            format!(
                "pushing the tested merge to {}: {}",
                entry.branch, pushed.stderr
            ),
        ))
    }

    /// The remote's target is still `target`; else try again.
    async fn target_unchanged(&self, entry: &MergeEntry, target: &str) -> Step<()> {
        match self.tip_of_target(entry.remote.as_deref()).await {
            Some(tip) if tip == target => Ok(()),
            Some(_) => Err(Stop::Moved),
            None => Err(Stop::fail(
                FailureKind::Other,
                format!("could not read {} on the remote", self.config.target),
            )),
        }
    }

    /// Delete the entry's branch on its remote, only while it is at
    /// `expected`. A failure is logged: the merge stands either way.
    async fn delete_branch(&self, entry: &MergeEntry, expected: &str) {
        let Some(remote) = entry.remote.as_deref() else {
            return;
        };
        let branch = format!("refs/heads/{}", entry.branch);
        let lease = format!("--force-with-lease={branch}:{expected}");
        let spec = format!(":{branch}");
        let deleted = git::run(
            &self.repo.top,
            &["push", "--porcelain", &lease, remote, &spec],
            self.config.git_timeout,
        )
        .await;
        match deleted {
            Ok(out) if out.ok => {
                tracing::info!(branch = %entry.branch, "Deleted the merged branch");
            }
            Ok(out) => {
                tracing::warn!(branch = %entry.branch, stderr = %out.stderr, "Kept the branch: it changed since, or could not be deleted");
            }
            Err(error) => {
                tracing::warn!(branch = %entry.branch, %error, "Could not delete the branch");
            }
        }
    }

    /// The title and description of a PR the queue opens for `entry`:
    /// its first own commit's subject, and every subject.
    async fn pr_text(
        &self,
        entry: &MergeEntry,
        target: &str,
        head: &str,
    ) -> Step<(String, String)> {
        let range = format!("{target}..{head}");
        let subjects = git::run(
            &self.repo.top,
            &["log", "--no-merges", "--reverse", "--format=%s", &range],
            self.config.git_timeout,
        )
        .await?;
        let subjects: Vec<&str> = subjects
            .stdout
            .lines()
            .filter(|line| !line.is_empty())
            .collect();
        let title = subjects.first().map_or_else(
            || entry.branch.clone(),
            |subject| subject.chars().take(120).collect(),
        );
        let who = entry
            .origin
            .agent_name
            .as_deref()
            .map_or(String::new(), |name| format!(" for {name}"));
        let mut body = format!(
            "Opened by the aura merge queue{who} from `{}`. The queue tests this branch merged with \
             `{}` and merges it at the tested head. Don't merge it by hand.\n\n",
            entry.branch, self.config.target
        );
        for subject in subjects.iter().take(BODY_COMMITS) {
            body.push_str(&format!("- {subject}\n"));
        }
        Ok((title, body))
    }

    /// Tell the PR why it didn't merge: a comment and a failure status on
    /// its head. Logged when the forge can't be told.
    async fn report(
        &self,
        forge: &dyn Forge,
        entry: &MergeEntry,
        pr: &PullRequest,
        failure: &Failure,
    ) {
        let body = failure_comment(&entry.branch, failure);
        if let Err(error) = forge.comment(pr.pr.number, &body).await {
            tracing::warn!(%error, pr = pr.pr.number, "Could not comment on the PR");
        }
        let head = forge
            .pr(pr.pr.number)
            .await
            .map_or_else(|_| pr.head.clone(), |now| now.head);
        let first = failure.reason.lines().next().unwrap_or("failed");
        status(forge, &head, CheckState::Failure, first).await;
    }
}

/// Set the queue's status; logged when it can't be.
async fn status(forge: &dyn Forge, sha: &str, state: CheckState, text: &str) {
    if let Err(error) = forge.set_status(sha, state, text).await {
        tracing::warn!(%error, sha = %short_sha(sha), "Could not set the merge queue's status");
    }
}

/// How a forge error stops an entry: an unreachable forge holds the
/// queue, any other error fails it.
fn forge_stop(error: ForgeError) -> Stop {
    match error {
        ForgeError::Unavailable(why) => {
            let now = Utc::now();
            Stop::Hold(Box::new(Hold {
                reason: format!("GitHub can't be reached or refused the queue's sign-in: {why}"),
                kind: HoldKind::ForgeUnavailable,
                since: now,
                retry_at: now + FORGE_RETRY,
                target_tip: None,
                tests: Vec::new(),
                excerpt: None,
                suspects: Vec::new(),
            }))
        }
        ForgeError::Failed(why) => Stop::fail(FailureKind::Refused, why),
    }
}

/// The comment a failed entry leaves on its PR.
pub(crate) fn failure_comment(branch: &str, failure: &Failure) -> String {
    let what = match failure.kind {
        FailureKind::Conflict => "it conflicts with the target",
        FailureKind::Check => "a check failed on its merge with the target",
        FailureKind::Refused => "GitHub refused the merge",
        FailureKind::Other => "the queue could not land it",
    };
    let mut body = format!(
        "**The merge queue did not merge this PR:** {what}.\n\n{}\n",
        failure.reason.lines().next().unwrap_or_default()
    );
    let list = |title: &str, items: &[String]| {
        let mut text = format!("\n{title}\n");
        for item in items {
            text.push_str(&format!("- `{item}`\n"));
        }
        text
    };
    if !failure.files.is_empty() {
        body.push_str(&list("Conflicting files:", &failure.files));
    }
    if !failure.tests.is_empty() {
        body.push_str(&list("Failing tests:", &failure.tests));
    }
    if let Some(excerpt) = &failure.excerpt {
        let start = excerpt
            .chars()
            .count()
            .saturating_sub(COMMENT_EXCERPT_CHARS);
        let excerpt: String = excerpt.chars().skip(start).collect();
        body.push_str(&format!(
            "\n<details><summary>Output</summary>\n\n```text\n{excerpt}\n```\n</details>\n"
        ));
    }
    body.push_str(&format!(
        "\nFix it on this branch (`{branch}`) and ship again; the queue tests it again. \
         The PR stays open."
    ));
    body
}
