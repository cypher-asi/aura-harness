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
//! - An entry's wait counts from its last change ([`MergeEntry::updated_at`]):
//!   its last push, or recovery putting it back after a crash
//!   ([`QueueState::expire`]).
//! - While the queue is held ([`QueueState::hold`]) no entry starts
//!   before the hold's retry time; the entry that ran into it went back
//!   to its place without being charged an attempt.
//! - Finished entries past the newest few leave the state for the
//!   history file ([`QueueState::take_archivable`]); [`QueueState::history`]
//!   says how much of that file is committed.

use crate::entry::{EntryId, MergeEntry, MergeRequest, MergeStatus};
use crate::report::{Failure, Hold, HoldKind, PrRef};
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
    /// How much of the history file holds archived entries; anything past
    /// it was written by an archive that never got saved, and is not
    /// history
    #[serde(default)]
    pub history: HistoryMark,
    /// Why no entry starts for now, when the queue is held
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold: Option<Hold>,
    /// The target fails its own checks, as an entry found; its parked
    /// entries wait for the target to move
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broken: Option<Hold>,
}

/// How far the history file is committed: entries and bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryMark {
    /// Entries archived
    pub count: u64,
    /// Bytes of the file they take
    pub bytes: u64,
}

impl Default for QueueState {
    fn default() -> Self {
        Self {
            version: STATE_VERSION,
            next_seq: 1,
            entries: Vec::new(),
            history: HistoryMark::default(),
            hold: None,
            broken: None,
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
    /// That exact push is queued, running or merged already.
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
        /// The PR it merged through, if any
        pr: Option<PrRef>,
    },
    /// Not merged, and why.
    Failed(Failure),
    /// Not the entry's fault: it goes back to its place, uncharged, and
    /// the queue waits as the hold says.
    Held(Hold),
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
            if entry.head == push.head {
                return Enqueued::Duplicate(entry.id.clone());
            }
            entry.pushed_at = Some(now);
            entry.updated_at = Some(now);
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
            pushed_at: None,
            started_at: None,
            finished_at: None,
            updated_at: Some(now),
            attempts: 0,
            detail: None,
            merged: None,
            files: Vec::new(),
            pr: None,
            failure: None,
            parked_at: None,
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
        if let Some(hold) = &self.hold {
            if now < hold.retry_at || !self.has_queued() {
                return None;
            }
            self.hold = None;
        }
        let entry = self
            .entries
            .iter_mut()
            .find(|entry| entry.status == MergeStatus::Queued && entry.parked_at.is_none())?;
        entry.status = MergeStatus::Running;
        entry.started_at = Some(now);
        entry.updated_at = Some(now);
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
        entry.updated_at = Some(now);
        match outcome {
            RunOutcome::Merged { merged, note, pr } => {
                entry.status = MergeStatus::Merged;
                entry.merged = Some(merged);
                entry.detail = Some(note);
                entry.pr = pr.or(entry.pr.take());
            }
            RunOutcome::Failed(failure) => {
                entry.status = MergeStatus::Failed;
                entry.detail = Some(failure.reason.clone());
                entry.pr = failure.pr.clone().or(entry.pr.take());
                entry.failure = Some(failure);
            }
            RunOutcome::Held(hold) => {
                entry.status = MergeStatus::Queued;
                entry.finished_at = None;
                entry.attempts = entry.attempts.saturating_sub(1);
                entry.detail = Some(format!("waiting: {}", hold.reason));
                let parked = hold.kind == HoldKind::TargetBroken;
                if parked {
                    entry.parked_at.clone_from(&hold.target_tip);
                }
                let entry = entry.clone();
                if parked {
                    self.broken = Some(hold);
                } else {
                    self.hold = Some(hold);
                }
                return Some(entry);
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
            entry.updated_at = Some(now);
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

    /// Fail every queued entry that has waited longer than `ttl` before
    /// `now` since it last changed ([`MergeEntry::waiting_since`]).
    /// Returns them.
    pub fn expire(&mut self, now: DateTime<Utc>, ttl: Duration) -> Vec<MergeEntry> {
        let mut expired = Vec::new();
        for entry in &mut self.entries {
            if entry.status == MergeStatus::Queued && now - entry.waiting_since() > ttl {
                entry.status = MergeStatus::Failed;
                entry.finished_at = Some(now);
                entry.updated_at = Some(now);
                entry.detail = Some(format!(
                    "waited in the queue longer than {} hours",
                    ttl.num_hours()
                ));
                expired.push(entry.clone());
            }
        }
        expired
    }

    /// Take out the oldest finished entries past the newest `keep`, for
    /// the history file, oldest first. Waiting and running entries always
    /// stay.
    pub fn take_archivable(&mut self, keep: usize) -> Vec<MergeEntry> {
        let finished = self
            .entries
            .iter()
            .filter(|entry| entry.status.is_finished())
            .count();
        let mut left = finished.saturating_sub(keep);
        let mut taken = Vec::with_capacity(left);
        let mut kept = Vec::with_capacity(self.entries.len() - left);
        for entry in self.entries.drain(..) {
            if left > 0 && entry.status.is_finished() {
                left -= 1;
                taken.push(entry);
            } else {
                kept.push(entry);
            }
        }
        self.entries = kept;
        taken
    }

    /// How many entries are finished.
    #[must_use]
    pub fn finished_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.status.is_finished())
            .count()
    }

    /// The target's tip is `tip` at `now`: end the hold and the broken
    /// target's report when the target moved, and let the parked entries
    /// run again once it moved or the broken report's retry time came.
    /// Returns whether anything changed.
    pub fn release_hold(&mut self, tip: &str, now: DateTime<Utc>) -> bool {
        let moved = |hold: &Option<Hold>| {
            hold.as_ref()
                .and_then(|hold| hold.target_tip.as_deref())
                .is_some_and(|held_at| held_at != tip)
        };
        let mut changed = false;
        if moved(&self.hold) {
            self.hold = None;
            changed = true;
        }
        let retry = self
            .broken
            .as_ref()
            .is_some_and(|hold| now >= hold.retry_at);
        if moved(&self.broken) || retry {
            self.broken = None;
            changed = true;
        }
        for entry in &mut self.entries {
            let parked = entry.parked_at.as_deref();
            if parked.is_some_and(|at| at != tip || retry) {
                entry.parked_at = None;
                changed = true;
            }
        }
        changed
    }

    /// Whether the queue waits on the target's tip: a hold or a broken
    /// target to end, or parked entries to let go.
    #[must_use]
    pub fn waits_on_target(&self) -> bool {
        self.hold
            .as_ref()
            .is_some_and(|hold| hold.target_tip.is_some())
            || self.broken.is_some()
            || self.entries.iter().any(|entry| entry.parked_at.is_some())
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
