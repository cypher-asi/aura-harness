//! [`RecordLog`] — append-only per-agent record log abstraction.
//!
//! Phase 2 carve-out of the `append_entry_*` / `get_head_seq` family
//! that previously lived only as inherent methods on
//! `aura-store::WriteStore`/`ReadStore`. Hoisting the API into a
//! dedicated trait lets `aura-agent-kernel` (Phase 6a) and
//! `aura-fleet-spawn` (Phase 7a) bind to a small `Arc<dyn RecordLog>`
//! surface instead of pulling the whole [`crate::RecordEntry`]-aware
//! store.
//!
//! Section 4 of the architecture plan sketches a richer trait with
//! `read_window` and `replay_from` cursors. Phase 2 ships only the
//! two methods needed by the kernel hot path — [`RecordLog::append`]
//! and [`RecordLog::head_seq`] — so the trait surface stays minimal
//! while the layered crates are still being broken out. Phase 6b
//! grows the trait with the `scan` method that replay needs.
//!
//! ## Invariants (per `.cursor/rules.md` §13)
//!
//! - **Per-agent linearisability.** Concurrent appends for the same
//!   `AgentId` are serialised by the implementor; the trait places no
//!   ordering requirements across distinct agents.
//! - **Strict sequence monotonicity.** Calls to [`RecordLog::append`]
//!   MUST receive `entry.seq` values that are exactly `head_seq + 1`
//!   for the target agent — no gaps, no duplicates, no out-of-order.
//! - **Caller-owned next-seq computation.** The kernel — or, post
//!   Phase 7a, the per-parent audit-append lease — decides the next
//!   sequence number by reading [`RecordLog::head_seq`] and adding
//!   one. Implementors validate the invariant and refuse out-of-order
//!   writes via [`RecordLogError::SeqOutOfOrder`].
//! - **No partial writes.** Backends that wrap multi-step storage
//!   commits MUST only return `Ok(())` after the full record write
//!   has been durably persisted (matches today's
//!   `RocksStore::append_entry_atomic`).
//!
//! ## Failure modes
//!
//! - [`RecordLogError::Backend`] — storage backend reported an error
//!   (I/O, corruption, transient unavailability). The message carries
//!   operation + context (agent id, seq) for diagnostics; the
//!   concrete-impl crate retains its strongly-typed error for the
//!   consumer that wants the original.
//! - [`RecordLogError::SeqOutOfOrder`] — the caller computed the
//!   wrong next sequence. Indicates a logic bug; implementations
//!   MUST NOT silently accept the entry.

use std::sync::Arc;

use aura_core_types::AgentId;
use thiserror::Error;

use crate::RecordEntry;

/// Errors surfaced by [`RecordLog`] implementations.
#[derive(Debug, Error)]
pub enum RecordLogError {
    /// Storage backend failure (I/O, corruption, transient unavailability).
    #[error("record-log backend error: {0}")]
    Backend(String),
    /// Caller-computed sequence number did not match the next expected
    /// slot for the agent.
    #[error("expected seq {expected}, got {actual} for agent {agent_id}")]
    SeqOutOfOrder {
        /// Agent whose record log was being appended to.
        agent_id: AgentId,
        /// Sequence number the implementation expected (`head_seq + 1`).
        expected: u64,
        /// Sequence number the caller actually supplied.
        actual: u64,
    },
}

/// Append-only per-agent record log.
///
/// See the module-level documentation for invariants, assumptions,
/// and failure modes.
pub trait RecordLog: Send + Sync {
    /// Append a record entry for `agent_id`.
    ///
    /// # Errors
    ///
    /// - [`RecordLogError::SeqOutOfOrder`] if `entry.seq` is not the
    ///   next expected sequence for the agent.
    /// - [`RecordLogError::Backend`] if the storage backend reports a
    ///   write failure.
    fn append(&self, agent_id: &AgentId, entry: &RecordEntry) -> Result<(), RecordLogError>;

    /// Returns the current head sequence number for `agent_id`. An
    /// agent with no record entries reports `0`.
    ///
    /// # Errors
    ///
    /// Returns [`RecordLogError::Backend`] if the storage backend
    /// reports a read failure.
    fn head_seq(&self, agent_id: &AgentId) -> Result<u64, RecordLogError>;

    /// Forward seq-ordered scan of `agent_id`'s record log.
    ///
    /// Phase 6b carve-out: the replay consumer in `aura-agent-kernel`
    /// needs to walk the historical record entries in monotonic
    /// `seq` order starting at `from_seq`. We keep the API minimal —
    /// no cursors, no descending order, no random access — because
    /// every replay path today is a single forward sweep over the
    /// recorded turn. Richer iteration (cursors / chunked replay)
    /// is left for the eventual `replay_from` cursor in the
    /// architecture plan §4.
    ///
    /// `limit` caps the number of returned entries so a bounded
    /// memory budget is preserved when the consumer chooses to
    /// stream a long agent log in batches. A `limit` of `0` returns
    /// the empty vector. Implementations MUST return entries in
    /// strictly ascending `seq` order with no gaps relative to what
    /// is durably persisted; if an entry is missing inside the
    /// requested range, the implementation truncates the result at
    /// the gap rather than fabricating placeholders.
    ///
    /// # Errors
    ///
    /// Returns [`RecordLogError::Backend`] when the storage backend
    /// reports a read failure.
    fn scan(
        &self,
        agent_id: &AgentId,
        from_seq: u64,
        limit: usize,
    ) -> Result<Vec<RecordEntry>, RecordLogError>;
}

/// Blanket impl so any `Arc<T: RecordLog + ?Sized>` (most importantly
/// `Arc<RocksStore>` and `Arc<dyn RecordLog>`) is itself a
/// [`RecordLog`]. Defined in this crate to satisfy Rust's orphan
/// rules — downstream backends like `aura-store-db` cannot add this
/// impl themselves because both `Arc` and `RecordLog` would be
/// foreign types from their point of view.
impl<T: RecordLog + ?Sized> RecordLog for Arc<T> {
    fn append(&self, agent_id: &AgentId, entry: &RecordEntry) -> Result<(), RecordLogError> {
        (**self).append(agent_id, entry)
    }

    fn head_seq(&self, agent_id: &AgentId) -> Result<u64, RecordLogError> {
        (**self).head_seq(agent_id)
    }

    fn scan(
        &self,
        agent_id: &AgentId,
        from_seq: u64,
        limit: usize,
    ) -> Result<Vec<RecordEntry>, RecordLogError> {
        (**self).scan(agent_id, from_seq, limit)
    }
}
