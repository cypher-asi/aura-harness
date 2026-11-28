//! A repo's merge queue at work: takes pushes, keeps them on disk, and
//! lands them one at a time.
//!
//! ## Invariants
//!
//! - The queue file is the queue. Every change is made to the file under
//!   its lock ([`QueueStore::update`]) and only then announced, so
//!   processes sharing a queue never overwrite each other and the
//!   announcements never run ahead of the file.
//! - Only a [`Processor`], which holds the repo's [`ProcessorLock`] for
//!   as long as it lives, recovers the queue and lands entries. One
//!   entry runs at a time ([`QueueState::claim_next`]).
//! - Recovery happens once the lock is held: an entry left running then
//!   was cut off by its processor's end, so it goes back in its place, or
//!   fails once it was interrupted [`Limits::max_attempts`] times.
//! - An entry that runs past [`Limits::entry_timeout`] fails, unless it
//!   landed before its time ran out ([`Integrator::landed`]); one whose
//!   branch waits past [`Limits::queued_ttl`] since its last push
//!   expires.

use crate::entry::{MergeEntry, MergeRequest};
use crate::error::QueueError;
use crate::integrate::{Integrator, RepoPaths};
use crate::state::{Enqueued, QueueState, RunOutcome};
use crate::store::{ProcessorLock, QueueStore};
use chrono::Utc;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, Mutex, Notify};
use tokio_util::sync::CancellationToken;

/// How often a processor looks for pushes queued by another process, and
/// how often a waiting one retries the lock.
const POLL: Duration = Duration::from_secs(15);
/// How long a processor waits after failing to save before trying again.
const SAVE_RETRY: Duration = Duration::from_secs(30);
/// Longest the check whether a timed-out entry landed may take.
const LANDED_CHECK: Duration = Duration::from_secs(120);

/// The queue's limits.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Times an entry may be interrupted by a crash before it fails
    pub max_attempts: u32,
    /// Longest one entry may run
    pub entry_timeout: Duration,
    /// Longest an entry may wait after its branch's last push
    pub queued_ttl: chrono::Duration,
    /// Finished entries kept for the Queue tab
    pub keep_finished: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            entry_timeout: Duration::from_secs(3 * 60 * 60),
            queued_ttl: chrono::Duration::hours(24),
            keep_finished: 1_000,
        }
    }
}

/// Something changed in a queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueEvent {
    /// An entry was queued, updated or started.
    Changed,
    /// An entry was merged or failed.
    Finished(Box<MergeEntry>),
}

/// A repo's merge queue. Clones share it.
#[derive(Clone)]
pub struct MergeQueue {
    inner: Arc<Inner>,
}

struct Inner {
    repo: RepoPaths,
    store: QueueStore,
    /// The queue as this process last read or wrote it
    seen: Mutex<QueueState>,
    wake: Notify,
    events: broadcast::Sender<QueueEvent>,
    integrator: Arc<dyn Integrator>,
    limits: Limits,
}

impl MergeQueue {
    /// Open `repo`'s queue. Nothing is recovered or changed until a
    /// [`Processor`] takes it.
    ///
    /// # Errors
    /// The queue file can't be read.
    pub async fn open(
        repo: RepoPaths,
        integrator: Arc<dyn Integrator>,
        limits: Limits,
    ) -> Result<Self, QueueError> {
        let store = QueueStore::new(repo.queue_dir());
        let loading = store.clone();
        let (state, _) = blocking(move || loading.load()).await?;
        let (events, _) = broadcast::channel(256);
        let inner = Inner {
            repo,
            store,
            seen: Mutex::new(state),
            wake: Notify::new(),
            events,
            integrator,
            limits,
        };
        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// The repo this queue lands entries in.
    #[must_use]
    pub fn repo(&self) -> &RepoPaths {
        &self.inner.repo
    }

    /// Hear what changes in this process from now on.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<QueueEvent> {
        self.inner.events.subscribe()
    }

    /// Every entry, oldest first, as the file has them now; what this
    /// process saw last when the file can't be read (logged).
    pub async fn entries(&self) -> Vec<MergeEntry> {
        let store = self.inner.store.clone();
        match blocking(move || store.load()).await {
            Ok((state, _)) => {
                let entries = state.entries.clone();
                *self.inner.seen.lock().await = state;
                entries
            }
            Err(error) => {
                tracing::warn!(%error, "Could not read the merge queue; showing the last one read");
                self.inner.seen.lock().await.entries.clone()
            }
        }
    }

    /// Queue `request` and wake the processor.
    ///
    /// # Errors
    /// The queue can't be saved; the request is not queued then.
    pub async fn enqueue(&self, request: MergeRequest) -> Result<Enqueued, QueueError> {
        let now = Utc::now();
        let enqueued = self
            .change(move |state| state.enqueue(request, now))
            .await?;
        self.inner.wake.notify_one();
        self.announce(QueueEvent::Changed);
        Ok(enqueued)
    }

    /// Become the queue's processor, unless a live one holds it.
    ///
    /// # Errors
    /// The lock can't be taken for another reason.
    pub async fn processor(&self) -> Result<Option<Processor>, QueueError> {
        let dir = self.inner.repo.queue_dir();
        let lock = blocking(move || ProcessorLock::acquire(&dir)).await?;
        Ok(lock.map(|lock| Processor {
            queue: self.clone(),
            _lock: lock,
        }))
    }

    /// Process entries until `cancel` fires: wait to become the processor,
    /// recover, then land one entry at a time, waiting for pushes while
    /// none is queued. An entry it was running when cancelled is
    /// recovered by the next processor.
    pub async fn run(&self, cancel: CancellationToken) {
        let Some(processor) = self.wait_for_processor(&cancel).await else {
            return;
        };
        if let Err(error) = processor.recover().await {
            tracing::error!(%error, "Could not recover the merge queue");
        }
        loop {
            processor.expire().await;
            let step = tokio::select! {
                () = cancel.cancelled() => return,
                step = processor.process_next() => step,
            };
            let pause = match step {
                Ok(Some(_)) => continue,
                Ok(None) => POLL,
                Err(error) => {
                    tracing::error!(%error, "The merge queue could not save its state");
                    SAVE_RETRY
                }
            };
            tokio::select! {
                () = cancel.cancelled() => return,
                () = self.inner.wake.notified() => {}
                () = tokio::time::sleep(pause) => {}
            }
        }
    }

    async fn wait_for_processor(&self, cancel: &CancellationToken) -> Option<Processor> {
        loop {
            match self.processor().await {
                Ok(Some(processor)) => return Some(processor),
                Ok(None) => tracing::info!("Another processor runs this merge queue; waiting"),
                Err(error) => tracing::warn!(%error, "Could not take the merge queue lock"),
            }
            tokio::select! {
                () = cancel.cancelled() => return None,
                () = tokio::time::sleep(POLL) => {}
            }
        }
    }

    /// Apply `edit` to the queue file under its lock; remember the result.
    async fn change<T: Send + 'static>(
        &self,
        edit: impl FnOnce(&mut QueueState) -> T + Send + 'static,
    ) -> Result<T, QueueError> {
        let store = self.inner.store.clone();
        let (state, result) = blocking(move || store.update(edit)).await?;
        *self.inner.seen.lock().await = state;
        Ok(result)
    }

    fn announce(&self, event: QueueEvent) {
        // No one listening is fine: the file has the change.
        let _ = self.inner.events.send(event);
    }

    fn announce_finished(&self, entries: Vec<MergeEntry>) {
        for entry in entries {
            self.announce(QueueEvent::Finished(Box::new(entry)));
        }
    }
}

/// The one processor of a repo's queue, for as long as it lives.
pub struct Processor {
    queue: MergeQueue,
    _lock: ProcessorLock,
}

impl Processor {
    /// Put back what an earlier processor left running, or fail it once
    /// it was interrupted too often. Returns the entries that failed.
    ///
    /// # Errors
    /// The queue can't be saved.
    pub async fn recover(&self) -> Result<Vec<MergeEntry>, QueueError> {
        let max = self.queue.inner.limits.max_attempts;
        let failed = self
            .queue
            .change(move |state| state.recover(Utc::now(), max))
            .await?;
        self.queue.announce_finished(failed.clone());
        Ok(failed)
    }

    /// Run the oldest queued entry to its end. Returns it finished; `None`
    /// when none waits.
    ///
    /// # Errors
    /// The queue can't be saved.
    pub async fn process_next(&self) -> Result<Option<MergeEntry>, QueueError> {
        let queue = &self.queue;
        let Some(entry) = queue.change(|state| state.claim_next(Utc::now())).await? else {
            return Ok(None);
        };
        queue.announce(QueueEvent::Changed);
        tracing::info!(entry = %entry.id, branch = %entry.branch, head = %entry.short_head(), "Merging");
        let limit = queue.inner.limits.entry_timeout;
        let outcome =
            match tokio::time::timeout(limit, queue.inner.integrator.integrate(&entry)).await {
                Ok(outcome) => outcome,
                Err(_) => self.timed_out(&entry, limit).await,
            };
        let keep = queue.inner.limits.keep_finished;
        let id = entry.id.clone();
        let finished = queue
            .change(move |state| {
                let finished = state.finish(&id, outcome, Utc::now());
                state.prune(keep);
                finished
            })
            .await?;
        if let Some(finished) = &finished {
            tracing::info!(entry = %finished.id, status = finished.status.label(), "Merge ended");
            queue.announce_finished(vec![finished.clone()]);
        }
        Ok(finished)
    }

    /// How an entry that ran past `limit` ended: merged when it landed
    /// before it was cut off, else failed.
    async fn timed_out(&self, entry: &MergeEntry, limit: Duration) -> RunOutcome {
        let minutes = limit.as_secs() / 60;
        let integrator = &self.queue.inner.integrator;
        match tokio::time::timeout(LANDED_CHECK, integrator.landed(entry)).await {
            Ok(Some(merged)) => RunOutcome::Merged {
                merged,
                note: format!("landed; the run then passed its {minutes} minute limit"),
            },
            _ => RunOutcome::Failed(format!("took longer than {minutes} minutes")),
        }
    }

    /// Fail the entries that waited too long since their last push.
    pub async fn expire(&self) {
        let ttl = self.queue.inner.limits.queued_ttl;
        match self
            .queue
            .change(move |state| state.expire(Utc::now(), ttl))
            .await
        {
            Ok(expired) => self.queue.announce_finished(expired),
            Err(error) => tracing::warn!(%error, "Could not expire old merge queue entries"),
        }
    }
}

/// Run blocking file IO off the async threads.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, QueueError> + Send + 'static,
) -> Result<T, QueueError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| QueueError::Corrupt(format!("queue file task failed: {e}")))?
}

#[cfg(test)]
mod tests;
