//! The queue's state and every change to it, as pure functions of the
//! state and the time, so ordering, state changes and recovery are
//! tested without git or a clock.
//!
//! ## Invariants
//!
//! - Entries keep the order they were queued in; [`QueueState::claim_next`]
//!   takes the oldest queued one (FIFO).
//! - At most one entry is [`MergeStatus::Running`]: `claim_next` returns
//!   `None` while one is.
//! - A finished entry never changes again.
//! - A branch has at most one queued entry: a new push of a queued branch
//!   updates its head and keeps its place.

use crate::entry::{EntryId, MergeEntry, MergeRequest, MergeStatus};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// Version of the persisted queue file.
pub const STATE_VERSION: u32 = 1;

/// A repo's merge queue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueState {
    /// File format version
    pub version: u32,
    /// Sequence number of the next entry's id
    pub next_seq: u64,
    /// Every entry, oldest first
    pub entries: Vec<MergeEntry>,
}

impl Default for QueueState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            next_seq: 1,
            entries: Vec::new(),
        }
    }
}

/// What [`QueueState::enqueue`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enqueued {
    /// A new entry joined the end of the queue.
    Added(EntryId),
    /// The branch's queued entry now merges the new head, in its place.
    Updated(EntryId),
    /// That exact push is running or merged already.
    Duplicate(EntryId),
}

impl Enqueued {
    /// The entry the push is in.
    #[must_use]
    pub const fn id(&self) -> &EntryId {
        match self {
            Self::Added(id) | Self::Updated(id) | Self::Duplicate(id) => id,
        }
    }
}

/// How a run of an entry ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// Landed; the target branch is at `merged`.
    Merged {
        /// Commit the target branch points at now
        merged: String,
        /// How it merged, for the Queue tab
        note: String,
    },
    /// Not merged, and why.
    Failed(String),
}

impl QueueState {
    /// Queue `request` at `now` (see [`Enqueued`]).
    pub fn enqueue(&mut self, request: MergeRequest, now: DateTime<Utc>) -> Enqueued {
        let push = &request.push;
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.branch == push.branch && entry.status == MergeStatus::Queued)
        {
            entry.head.clone_from(&push.head);
            entry.remote.clone_from(&push.remote);
            entry.origin = request.origin;
            return Enqueued::Updated(entry.id.clone());
        }
        if let Some(entry) = self.entries.iter().rev().find(|entry| {
            entry.branch == push.branch
                && entry.head == push.head
                && matches!(entry.status, MergeStatus::Running | MergeStatus::Merged)
        }) {
            return Enqueued::Duplicate(entry.id.clone());
        }
        let id = EntryId::from_seq(self.next_seq);
        self.next_seq += 1;
        self.entries.push(MergeEntry {
            id: id.clone(),
            branch: request.push.branch,
            head: request.push.head,
            remote: request.push.remote,
            origin: request.origin,
            status: MergeStatus::Queued,
            enqueued_at: now,
            started_at: None,
            finished_at: None,
            attempts: 0,
            detail: None,
            merged: None,
        });
        Enqueued::Added(id)
    }

    /// The entry that is running, if any.
    #[must_use]
    pub fn running(&self) -> Option<&MergeEntry> {
        self.entries
            .iter()
            .find(|entry| entry.status == MergeStatus::Running)
    }

    /// Start the oldest queued entry at `now` and return it; `None` while
    /// one runs or none waits.
    pub fn claim_next(&mut self, now: DateTime<Utc>) -> Option<MergeEntry> {
        if self.running().is_some() {
            return None;
        }
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.status == MergeStatus::Queued)?;
        entry.status = MergeStatus::Running;
        entry.started_at = Some(now);
        entry.attempts += 1;
        entry.detail = None;
        Some(entry.clone())
    }

    /// End the running entry `id` as `outcome` says, at `now`. Returns the
    /// finished entry; `None` when `id` is not running.
    pub fn finish(
        &mut self,
        id: &EntryId,
        outcome: RunOutcome,
        now: DateTime<Utc>,
    ) -> Option<MergeEntry> {
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| &entry.id == id && entry.status == MergeStatus::Running)?;
        entry.finished_at = Some(now);
        match outcome {
            RunOutcome::Merged { merged, note } => {
                entry.status = MergeStatus::Merged;
                entry.merged = Some(merged);
                entry.detail = Some(note);
            }
            RunOutcome::Failed(reason) => {
                entry.status = MergeStatus::Failed;
                entry.detail = Some(reason);
            }
        }
        Some(entry.clone())
    }

    /// After a crash: put an entry that was running back at its place to
    /// run again, or fail it once it was interrupted `max_attempts` times.
    /// Returns the entries that failed.
    pub fn recover(&mut self, now: DateTime<Utc>, max_attempts: u32) -> Vec<MergeEntry> {
        let mut failed = Vec::new();
        for entry in &mut self.entries {
            if entry.status != MergeStatus::Running {
                continue;
            }
            if entry.attempts >= max_attempts {
                entry.status = MergeStatus::Failed;
                entry.finished_at = Some(now);
                entry.detail = Some(format!(
                    "interrupted {} times while merging; push again to retry",
                    entry.attempts
                ));
                failed.push(entry.clone());
            } else {
                entry.status = MergeStatus::Queued;
                entry.detail = Some("interrupted; queued again".to_string());
            }
        }
        failed
    }

    /// Fail every entry queued longer than `ttl` before `now`. Returns
    /// them.
    pub fn expire(&mut self, now: DateTime<Utc>, ttl: Duration) -> Vec<MergeEntry> {
        let mut expired = Vec::new();
        for entry in &mut self.entries {
            if entry.status == MergeStatus::Queued && now - entry.enqueued_at > ttl {
                entry.status = MergeStatus::Failed;
                entry.finished_at = Some(now);
                entry.detail = Some(format!(
                    "waited in the queue longer than {} hours",
                    ttl.num_hours()
                ));
                expired.push(entry.clone());
            }
        }
        expired
    }

    /// Keep at most `keep` finished entries, dropping the oldest. Waiting
    /// and running entries always stay.
    pub fn prune(&mut self, keep: usize) {
        let finished = self
            .entries
            .iter()
            .filter(|entry| entry.status.is_finished())
            .count();
        let mut drop = finished.saturating_sub(keep);
        self.entries.retain(|entry| {
            if drop > 0 && entry.status.is_finished() {
                drop -= 1;
                return false;
            }
            true
        });
    }

    /// Whether an entry waits to run.
    #[must_use]
    pub fn has_queued(&self) -> bool {
        self.entries
            .iter()
            .any(|entry| entry.status == MergeStatus::Queued)
    }
}

#[cfg(test)]
mod tests;
