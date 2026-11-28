//! Which files the work in flight changes, and where two pieces of it
//! change the same ones.
//!
//! When a push is queued, the files its entry changes are recorded
//! ([`changed_files`], [`QueueState::record_files`]). The work in flight
//! is every entry waiting or running, and every branch whose latest entry
//! failed, since its agent is fixing it ([`open_work`]); the hub adds the
//! local worktrees its members work in ([`worktree_work`]). Two pieces of
//! work overlap when they change a file in common ([`find`]). Overlap
//! only warns: nothing waits for it.
//!
//! ## Invariants
//!
//! - Generated files ([`crate::resolve::Generated`]) never count: the
//!   queue resolves their conflicts itself.
//! - A branch never overlaps itself, nor work of the same agent.
//! - Files are recorded for the head they were read from only; a newer
//!   push of the entry waits for its own.
//! - Reading the files of a remote entry fetches into private
//!   `refs/aura-queue/scan/<id>/*` refs, deleted afterwards, and never
//!   writes `FETCH_HEAD`, so it can run beside the processor.

use crate::entry::{EntryId, MergeEntry, MergeStatus};
use crate::error::QueueError;
use crate::git;
use crate::integrate::RepoPaths;
use crate::resolve::Generated;
use crate::state::QueueState;
use chrono::{DateTime, Duration, Utc};
use std::collections::{BTreeSet, HashSet};
use std::path::Path;

/// Most files recorded for one entry, to bound the queue file.
pub const MAX_FILES: usize = 5_000;
/// How long a failed entry's branch counts as being fixed, in hours.
pub const FIXING_HOURS: i64 = 24;

/// A piece of work in flight: a queue entry, or a branch checked out in
/// a member's worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Work {
    /// The queue entry, when it is one
    pub entry: Option<EntryId>,
    /// Its branch
    pub branch: String,
    /// Hex id of the agent that owns it, when known
    pub agent: Option<String>,
    /// That agent's name or code, for people
    pub agent_name: Option<String>,
    /// The files it changes, relative to the repo's top
    pub files: Vec<String>,
}

impl Work {
    /// `entry` as work in flight.
    #[must_use]
    pub fn of_entry(entry: &MergeEntry) -> Self {
        Self {
            entry: Some(entry.id.clone()),
            branch: entry.branch.clone(),
            agent: entry.origin.agent.clone(),
            agent_name: entry.origin.agent_name.clone(),
            files: entry.files.clone(),
        }
    }

    /// Who owns it, for people: its agent's name, else its branch.
    #[must_use]
    pub fn owner(&self) -> &str {
        self.agent_name.as_deref().unwrap_or(&self.branch)
    }

    /// Where it is, for people: `q12 aura/x` or `worktree aura/x`.
    #[must_use]
    pub fn place(&self) -> String {
        match &self.entry {
            Some(id) => format!("{id} {}", self.branch),
            None => format!("worktree {}", self.branch),
        }
    }

    fn same_owner(&self, other: &Self) -> bool {
        self.branch == other.branch
            || matches!((&self.agent, &other.agent), (Some(a), Some(b)) if a == b)
    }
}

/// Two pieces of work that change the same files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlap {
    /// The other piece of work
    pub with: Work,
    /// The files both change, sorted
    pub files: Vec<String>,
}

/// Whether a change to `path` can conflict in a way someone must resolve.
#[must_use]
pub fn counts(path: &str) -> bool {
    Generated::of(path).is_none()
}

/// The work in flight among `entries` (oldest first) at `now`: the
/// latest entry of each branch that is queued or running, or failed in
/// the last [`FIXING_HOURS`] hours and not merged since. Entries with no recorded
/// files are left out.
#[must_use]
pub fn open_work(entries: &[MergeEntry], now: DateTime<Utc>) -> Vec<Work> {
    let mut seen = HashSet::new();
    let mut open = Vec::new();
    for entry in entries.iter().rev() {
        if !seen.insert(entry.branch.as_str()) {
            continue;
        }
        let in_flight = match entry.status {
            MergeStatus::Queued | MergeStatus::Running => true,
            MergeStatus::Failed => entry
                .finished_at
                .is_some_and(|at| now - at <= Duration::hours(FIXING_HOURS)),
            MergeStatus::Merged => false,
        };
        if in_flight && !entry.files.is_empty() {
            open.push(Work::of_entry(entry));
        }
    }
    open.reverse();
    open
}

/// Each piece of `others` that changes a file `subject` changes too,
/// with those files; work of the same branch or agent is skipped.
#[must_use]
pub fn find(subject: &Work, others: &[Work]) -> Vec<Overlap> {
    let mine: BTreeSet<&str> = subject
        .files
        .iter()
        .map(String::as_str)
        .filter(|path| counts(path))
        .collect();
    others
        .iter()
        .filter(|other| !subject.same_owner(other))
        .filter_map(|other| {
            let shared: BTreeSet<&str> = other
                .files
                .iter()
                .map(String::as_str)
                .filter(|path| mine.contains(path))
                .collect();
            (!shared.is_empty()).then(|| Overlap {
                with: other.clone(),
                files: shared.into_iter().map(str::to_string).collect(),
            })
        })
        .collect()
}

impl QueueState {
    /// Record that entry `id` at `head` changes `files`. Returns whether
    /// it did: not when the entry is gone or was pushed again since.
    pub fn record_files(&mut self, id: &EntryId, head: &str, mut files: Vec<String>) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| &entry.id == id && entry.head == head)
        else {
            return false;
        };
        files.truncate(MAX_FILES);
        entry.files = files;
        true
    }
}

/// The files `entry` changes against `target`, from where they last had
/// a common commit (`git diff --name-only <target>...<head>`).
///
/// # Errors
/// The branch or the target can't be fetched or compared.
pub async fn changed_files(
    repo: &RepoPaths,
    target: &str,
    entry: &MergeEntry,
    timeout: std::time::Duration,
) -> Result<Vec<String>, QueueError> {
    let top = &repo.top;
    let Some(remote) = entry.remote.as_deref() else {
        let range = format!("refs/heads/{target}...{}", entry.head);
        return diff_names(top, &[&range], timeout).await;
    };
    let scan = format!("refs/aura-queue/scan/{}", entry.id);
    let (target_ref, head_ref) = (format!("{scan}/target"), format!("{scan}/head"));
    let (target_spec, head_spec) = (
        format!("+refs/heads/{target}:{target_ref}"),
        format!("+refs/heads/{}:{head_ref}", entry.branch),
    );
    let args = [
        &git::FETCH_ARGS[..],
        &["--no-write-fetch-head", remote, &target_spec, &head_spec],
    ]
    .concat();
    let fetched = git::ok(top, &args, timeout).await;
    let files = match fetched {
        Ok(_) => {
            let range = format!("{target_ref}...{}", entry.head);
            diff_names(top, &[&range], timeout).await
        }
        Err(error) => Err(error),
    };
    for reference in [&target_ref, &head_ref] {
        // A ref left behind is overwritten by the next scan of the entry.
        let _ = git::run(top, &["update-ref", "-d", reference], timeout).await;
    }
    files
}

/// The branch checked out in the worktree at `dir`, and the files it
/// changes, committed or not, since it left `target` (origin's when the
/// worktree knows it, else the local branch). `None` when the worktree
/// is on `target`, on no branch, or shares no history with it.
///
/// # Errors
/// git can't run in `dir`.
pub async fn worktree_work(
    dir: &Path,
    target: &str,
    timeout: std::time::Duration,
) -> Result<Option<(String, Vec<String>)>, QueueError> {
    let branch = git::run(dir, &["rev-parse", "--abbrev-ref", "HEAD"], timeout).await?;
    let branch = branch.stdout;
    if branch.is_empty() || branch == "HEAD" || branch == target {
        return Ok(None);
    }
    let remote = format!("refs/remotes/origin/{target}");
    let known = git::run(dir, &["rev-parse", "--verify", "--quiet", &remote], timeout).await?;
    let base_ref = if known.ok {
        remote
    } else {
        format!("refs/heads/{target}")
    };
    let base = git::run(dir, &["merge-base", "HEAD", &base_ref], timeout).await?;
    if !base.ok || base.stdout.is_empty() {
        return Ok(None);
    }
    let files = diff_names(dir, &[&base.stdout], timeout).await?;
    Ok(Some((branch, files)))
}

async fn diff_names(
    dir: &Path,
    range: &[&str],
    timeout: std::time::Duration,
) -> Result<Vec<String>, QueueError> {
    let mut args = vec!["diff", "--name-only", "--no-renames"];
    args.extend_from_slice(range);
    let listed = git::ok(dir, &args, timeout).await?;
    Ok(listed
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(MAX_FILES)
        .map(str::to_string)
        .collect())
}

#[cfg(test)]
mod tests;
