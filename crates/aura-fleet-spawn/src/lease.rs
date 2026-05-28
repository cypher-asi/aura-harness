//! [`ParentLeaseRegistry`] — per-parent audit-append lease.
//!
//! Replaces the legacy single `spawn_lock` in
//! `crates/aura-runtime/src/subagent_dispatch.rs` (deleted in Phase
//! 7a). The old lock serialised EVERY spawn across the entire
//! daemon, so two unrelated parents could not spawn concurrently.
//! The per-parent lease keeps the in-order parent-side
//! `RecordEntry` append guarantee (concurrent spawns from one parent
//! still serialise) without the cross-parent contention.
//!
//! # Invariants
//!
//! - Each parent agent id maps to AT MOST ONE `Arc<Mutex<()>>`
//!   handle for the duration the registry observes any spawn for
//!   that parent. Phase 7b adds an idle-eviction policy; Phase 7a
//!   intentionally retains handles for the daemon lifetime to keep
//!   the implementation a single dashmap insert/get.
//! - Holding a [`ParentLease`] guarantees mutual exclusion against
//!   any other spawn call for the same parent. The lock is held
//!   across the entire derive → quota → audit → registry → dispatch
//!   sequence so the parent's audit-record appends are linearised.
//! - The async lock is held across `.await` points by design — that
//!   is the whole point of the lease. Phase 7a sequences the lease
//!   acquire BEFORE any work so the only awaited operations inside
//!   the lease are the kernel record write and (for `SpawnMode::Wait`)
//!   the child runner future.

use std::collections::HashMap;
use std::sync::Arc;

use aura_core::AgentId;
use parking_lot::Mutex as SyncMutex;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

/// RAII handle proving the holder has the parent's audit-append
/// lease. Dropping the handle releases the lease.
pub struct ParentLease {
    /// Held for the lease's lifetime; `_guard` is intentionally
    /// unread — its `Drop` is what releases the lock.
    _guard: OwnedMutexGuard<()>,
}

/// Per-parent lease pool. Cloneable via `Arc` so multiple call
/// sites (task tool today; mailbox + dispatch in Phase 7b) share
/// one registry.
#[derive(Debug, Default)]
pub struct ParentLeaseRegistry {
    /// `AgentId → shared lock`. Each parent gets its own lock; the
    /// dashmap-like outer lock is a `parking_lot::Mutex` only over
    /// the map mutation (fast — never held across `await`).
    inner: SyncMutex<HashMap<AgentId, Arc<AsyncMutex<()>>>>,
}

impl ParentLeaseRegistry {
    /// Construct an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Acquire the lease for `parent`. Returns a [`ParentLease`]
    /// RAII guard that releases the lease on drop.
    ///
    /// Concurrent acquires for the same parent serialise; acquires
    /// for distinct parents proceed in parallel.
    pub async fn acquire(&self, parent: AgentId) -> ParentLease {
        let lock = {
            let mut map = self.inner.lock();
            map.entry(parent)
                .or_insert_with(|| Arc::new(AsyncMutex::new(())))
                .clone()
        };
        let guard = lock.lock_owned().await;
        ParentLease { _guard: guard }
    }

    /// Snapshot count of distinct parents observed so far. Useful
    /// for tests + observability.
    #[must_use]
    pub fn known_parents(&self) -> usize {
        self.inner.lock().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[tokio::test]
    async fn acquire_same_parent_serialises() {
        let registry = Arc::new(ParentLeaseRegistry::new());
        let parent = AgentId::generate();

        let a = registry.clone();
        let b = registry.clone();
        let start = Instant::now();
        let handle_a = tokio::spawn(async move {
            let _lease = a.acquire(parent).await;
            tokio::time::sleep(Duration::from_millis(60)).await;
        });
        let handle_b = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let _lease = b.acquire(parent).await;
            tokio::time::sleep(Duration::from_millis(60)).await;
        });
        handle_a.await.unwrap();
        handle_b.await.unwrap();
        let elapsed = start.elapsed();
        // Both must serialise: total >= 60 + 60 = 120ms
        assert!(
            elapsed >= Duration::from_millis(110),
            "expected serial execution (>=110ms), got {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn acquire_distinct_parents_parallelises() {
        let registry = Arc::new(ParentLeaseRegistry::new());
        let parent_a = AgentId::generate();
        let parent_b = AgentId::generate();

        let a = registry.clone();
        let b = registry.clone();
        let start = Instant::now();
        let handle_a = tokio::spawn(async move {
            let _lease = a.acquire(parent_a).await;
            tokio::time::sleep(Duration::from_millis(80)).await;
        });
        let handle_b = tokio::spawn(async move {
            let _lease = b.acquire(parent_b).await;
            tokio::time::sleep(Duration::from_millis(80)).await;
        });
        handle_a.await.unwrap();
        handle_b.await.unwrap();
        let elapsed = start.elapsed();
        // Parallel: total should be ~80ms, well under 150ms
        assert!(
            elapsed < Duration::from_millis(150),
            "expected parallel execution (<150ms), got {elapsed:?}"
        );
    }
}
