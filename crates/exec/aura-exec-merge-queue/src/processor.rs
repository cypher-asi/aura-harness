//! A repo's queue at work: takes pushes, keeps them on disk, and lands
//! them one at a time.
//!
//! ## Invariants
//!
//! - Every change to the queue is saved before it is announced, under
//!   one lock, so the file and the announcements never disagree and saves
//!   never reorder.
//! - One entry runs at a time ([`QueueState::claim_next`]), and one
//!   processor runs per repo ([`ProcessorLock`]).
//! - Opening a queue recovers it: an entry a crash left running goes back
//!   in its place, or fails once it was interrupted
//!   [`Limits::max_attempts`] times.
//! - An entry that runs past [`Limits::entry_timeout`] fails, and one that
//!   waits past [`Limits::queued_ttl`] expires.

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

/// How often a processor shows it is alive, and retries a held lock.
const HEARTBEAT: Duration = Duration::from_secs(60);
/// How long a processor waits after failing to save before trying again.
const SAVE_RETRY: Duration = Duration::from_secs(30);

/// The queue's limits.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Times an entry may be interrupted by a crash before it fails
    pub max_attempts: u32,
    /// Longest one entry may run
    pub entry_timeout: Duration,
    /// Longest an entry may wait
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
            keep_finished: 200,
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
    state: Mutex<QueueState>,
    wake: Notify,
    events: broadcast::Sender<QueueEvent>,
    integrator: Arc<dyn Integrator>,
    limits: Limits,
}

impl MergeQueue {
    /// Open `repo`'s queue, recovering what a crash left. Returns the
    /// queue and the entries recovery failed.
    ///
    /// # Errors
    /// The queue file can't be read or saved.
    pub async fn open(
        repo: RepoPaths,
        integrator: Arc<dyn Integrator>,
        limits: Limits,
    ) -> Result<(Self, Vec<MergeEntry>), QueueError> {
        let store = QueueStore::new(repo.queue_dir());
        let loading = store.clone();
        let (mut state, aside) = blocking(move || loading.load()).await?;
        if let Some(aside) = aside {
            tracing::warn!(moved_to = %aside.display(), "The merge queue file was unreadable; started a new one");
        }
        let now = Utc::now();
        let mut failed = state.recover(now, limits.max_attempts);
        failed.extend(state.expire(now, limits.queued_ttl));
        let saving = store.clone();
        let saved = state.clone();
        blocking(move || saving.save(&saved)).await?;
        let (events, _) = broadcast::channel(256);
        let inner = Inner {
            repo,
            store,
            state: Mutex::new(state),
            wake: Notify::new(),
            events,
            integrator,
            limits,
        };
        Ok((
            Self {
                inner: Arc::new(inner),
            },
            failed,
        ))
    }

    /// The repo this queue lands entries in.
    #[must_use]
    pub fn repo(&self) -> &RepoPaths {
        &self.inner.repo
    }

    /// Hear what changes from now on.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<QueueEvent> {
        self.inner.events.subscribe()
    }

    /// Every entry, oldest first.
    pub async fn entries(&self) -> Vec<MergeEntry> {
        self.inner.state.lock().await.entries.clone()
    }

    /// Queue `request` and wake the processor.
    ///
    /// # Errors
    /// The queue can't be saved; the request is not queued then.
    pub async fn enqueue(&self, request: MergeRequest) -> Result<Enqueued, QueueError> {
        let mut state = self.inner.state.lock().await;
        let mut next = state.clone();
        let enqueued = next.enqueue(request, Utc::now());
        self.save(&next).await?;
        *state = next;
        drop(state);
        self.inner.wake.notify_one();
        let _ = self.inner.events.send(QueueEvent::Changed);
        Ok(enqueued)
    }

    /// Run the oldest queued entry to its end. Returns it finished; `None`
    /// when none waits.
    ///
    /// # Errors
    /// The queue can't be saved.
    pub async fn process_next(&self) -> Result<Option<MergeEntry>, QueueError> {
        let Some(entry) = self.change(|state| state.claim_next(Utc::now())).await? else {
            return Ok(None);
        };
        let _ = self.inner.events.send(QueueEvent::Changed);
        tracing::info!(entry = %entry.id, branch = %entry.branch, head = %entry.short_head(), "Merging");
        let limit = self.inner.limits.entry_timeout;
        let outcome = tokio::time::timeout(limit, self.inner.integrator.integrate(&entry))
            .await
            .unwrap_or_else(|_| {
                RunOutcome::Failed(format!("took longer than {} minutes", limit.as_secs() / 60))
            });
        let keep = self.inner.limits.keep_finished;
        let finished = self
            .change(|state| {
                let finished = state.finish(&entry.id, outcome, Utc::now());
                state.prune(keep);
                finished
            })
            .await?;
        if let Some(finished) = &finished {
            tracing::info!(entry = %finished.id, status = finished.status.label(), "Merge ended");
            let _ = self
                .inner
                .events
                .send(QueueEvent::Finished(Box::new(finished.clone())));
        }
        Ok(finished)
    }

    /// Process entries until `cancel` fires: one at a time, waiting for
    /// pushes while none is queued. Returns once cancelled; an entry it
    /// was running is recovered the next time the queue opens.
    pub async fn run(&self, cancel: CancellationToken) {
        let Some(lock) = self.wait_for_lock(&cancel).await else {
            return;
        };
        loop {
            if let Err(error) = lock.touch() {
                tracing::warn!(%error, "Could not refresh the merge queue lock");
            }
            self.expire().await;
            let step = tokio::select! {
                () = cancel.cancelled() => return,
                step = self.process_next() => step,
            };
            match step {
                Ok(Some(_)) => {}
                Ok(None) => tokio::select! {
                    () = cancel.cancelled() => return,
                    () = self.inner.wake.notified() => {}
                    () = tokio::time::sleep(HEARTBEAT) => {}
                },
                Err(error) => {
                    tracing::error!(%error, "The merge queue could not save its state");
                    tokio::select! {
                        () = cancel.cancelled() => return,
                        () = tokio::time::sleep(SAVE_RETRY) => {}
                    }
                }
            }
        }
    }

    async fn wait_for_lock(&self, cancel: &CancellationToken) -> Option<ProcessorLock> {
        loop {
            let dir = self.inner.repo.queue_dir();
            match blocking(move || ProcessorLock::acquire(&dir)).await {
                Ok(Some(lock)) => return Some(lock),
                Ok(None) => tracing::info!("Another processor runs this merge queue; waiting"),
                Err(error) => tracing::warn!(%error, "Could not take the merge queue lock"),
            }
            tokio::select! {
                () = cancel.cancelled() => return None,
                () = tokio::time::sleep(HEARTBEAT) => {}
            }
        }
    }

    async fn expire(&self) {
        let ttl = self.inner.limits.queued_ttl;
        match self.change(|state| state.expire(Utc::now(), ttl)).await {
            Ok(expired) => {
                for entry in expired {
                    let _ = self
                        .inner
                        .events
                        .send(QueueEvent::Finished(Box::new(entry)));
                }
            }
            Err(error) => tracing::warn!(%error, "Could not expire old merge queue entries"),
        }
    }

    /// Apply `edit` to the queue and save it; the queue changes only once
    /// it is saved.
    async fn change<T>(&self, edit: impl FnOnce(&mut QueueState) -> T) -> Result<T, QueueError> {
        let mut state = self.inner.state.lock().await;
        let mut next = state.clone();
        let result = edit(&mut next);
        if next != *state {
            self.save(&next).await?;
            *state = next;
        }
        Ok(result)
    }

    async fn save(&self, state: &QueueState) -> Result<(), QueueError> {
        let store = self.inner.store.clone();
        let state = state.clone();
        blocking(move || store.save(&state)).await
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
