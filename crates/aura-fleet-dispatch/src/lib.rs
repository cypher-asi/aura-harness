//! # aura-fleet-dispatch
//!
//! Layer: fleet
//!
//! Takes a stream of [`AgentJob`] items and routes each one into the
//! correct [`aura_fleet_spawn::FleetSpawner::spawn`] call.
//!
//! ## Phase 7a scope
//!
//! Phase 7a only needs to route ONE source of [`AgentJob`]s: the
//! task-tool compatibility adapter that today builds a single spawn
//! request per `task` tool call. Phase 7b adds the mailbox + the
//! detached/batch dispatch surface; until then `FleetDispatcher::run`
//! is a thin async wrapper that drains a stream of jobs and awaits
//! each spawn sequentially.
//!
//! ## Invariants (per `.cursor/rules.md` §13)
//!
//! - The dispatcher is **stateless** in Phase 7a — every
//!   [`AgentJob`] is converted into a [`SpawnRequest`] and handed
//!   to the shared [`FleetSpawner`]. Concurrency is the spawner's
//!   responsibility (per-parent lease; cross-parent parallelism).
//! - The dispatcher **does not enqueue or persist jobs** in Phase
//!   7a. Phase 7b introduces the durable mailbox + a job queue
//!   with retry semantics.
//!
//! ## Failure modes
//!
//! - [`DispatchError::Spawn`] — the spawner rejected a job.
//! - [`DispatchError::Lagged`] — the input stream surfaced a
//!   `Lagged` error from a `tokio::sync::broadcast` (reserved for
//!   Phase 7b mailbox wiring).

#![forbid(unsafe_code)]
#![warn(clippy::all)]

use std::sync::Arc;

use aura_core::SubagentResult;
use aura_core_modes::SpawnMode;
use aura_fleet_spawn::{FleetSpawner, SpawnError, SpawnHandle, SpawnRequest};
use futures_util::{Stream, StreamExt};
use thiserror::Error;
use tracing::instrument;

/// A job the dispatcher routes into a spawn call.
///
/// Phase 7a wraps a single [`SpawnRequest`] plus the resolved
/// [`SpawnMode`] (which today is always [`SpawnMode::Wait`]). Phase
/// 7b extends this with `DispatchHandle`, priority, deadline,
/// idempotency key, etc.
#[derive(Debug)]
pub struct AgentJob {
    /// The spawn request derived from a parent tool call.
    pub request: SpawnRequest,
    /// Resolved spawn mode. Phase 7a expects [`SpawnMode::Wait`];
    /// any other value is rejected by [`FleetSpawner::spawn`] with
    /// [`SpawnError::NotImplementedInPhase7a`].
    pub mode: SpawnMode,
}

/// Errors surfaced by [`FleetDispatcher::run`].
#[derive(Debug, Error)]
pub enum DispatchError {
    /// Underlying spawner rejected the job.
    #[error("dispatch failed: {0}")]
    Spawn(#[from] SpawnError),

    /// Input stream signalled a lag (reserved for Phase 7b
    /// broadcast wiring).
    #[error("dispatch input stream lagged: {0}")]
    Lagged(String),
}

/// Phase 7a dispatcher — wraps a shared [`FleetSpawner`] and
/// streams jobs through `spawn`.
pub struct FleetDispatcher {
    spawner: Arc<FleetSpawner>,
}

impl FleetDispatcher {
    /// Construct a dispatcher around an existing spawner.
    #[must_use]
    pub fn new(spawner: Arc<FleetSpawner>) -> Self {
        Self { spawner }
    }

    /// Drain a stream of [`AgentJob`] items, awaiting each spawn
    /// to completion. The dispatcher returns the [`SubagentResult`]
    /// for each job in input order. A single spawn failure aborts
    /// the loop and surfaces the [`DispatchError`].
    ///
    /// # Errors
    ///
    /// Returns [`DispatchError::Spawn`] on the first spawner
    /// rejection.
    #[instrument(skip(self, jobs))]
    pub async fn run<S>(&self, mut jobs: S) -> Result<Vec<SubagentResult>, DispatchError>
    where
        S: Stream<Item = AgentJob> + Unpin + Send,
    {
        let mut results = Vec::new();
        while let Some(job) = jobs.next().await {
            match self.spawn_one(job).await? {
                SpawnHandle::Completed(result) => results.push(result),
            }
        }
        Ok(results)
    }

    /// Spawn a single job. Useful for the task-tool adapter that
    /// constructs one job per tool call without building a stream.
    ///
    /// # Errors
    ///
    /// Surfaces any [`SpawnError`] from the underlying spawner.
    #[instrument(skip(self, job))]
    pub async fn spawn_one(&self, job: AgentJob) -> Result<SpawnHandle, DispatchError> {
        let handle = self.spawner.spawn(job.request, job.mode).await?;
        Ok(handle)
    }
}
