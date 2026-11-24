//! Store trait definition.
//!
//! Invariant §10 (Append-Only Record) requires a narrow, audited set of
//! write APIs. The trait hierarchy below splits the record-append family off
//! into a sealed [`WriteStore`] trait so that only the kernel can produce
//! new implementations of the write path:
//!
//! * [`ReadStore`] — public, contains every read-only operation plus the
//!   inbox-level writes (`enqueue_tx`, `dequeue_tx`, `set_agent_status`)
//!   that non-kernel callers (HTTP `/tx`, automaton bridge, schedulers) are
//!   allowed to invoke.
//! * [`WriteStore`] — public for call-site bindings, but sealed against new
//!   implementations through [`sealed::Sealed`]. Covers the atomic commit
//!   family (`append_entry_atomic`, `append_entry_direct`,
//!   `append_entries_batch`, plus their `*_with_runtime_capabilities`
//!   siblings). Only implementors declared inside `aura-store` can satisfy
//!   the `Sealed` bound, and by convention only `aura-kernel` ever asks for
//!   `Arc<dyn WriteStore>` — non-kernel crates must bind to
//!   `Arc<dyn ReadStore>` instead.
//! * [`Store`] — convenience combined trait, blanket-implemented for every
//!   `ReadStore + WriteStore`. Existing call sites that say
//!   `Arc<dyn Store>` continue to work; new non-kernel code should prefer
//!   `Arc<dyn ReadStore>`.
//!
//! TODO(p10-determinism-tests): add a compile-fail test that enforces
//! "only the `aura-kernel` crate may import and implement `WriteStore`"
//! once the workspace has `trybuild` wired into CI.

use crate::error::StoreError;
use aura_core_types::{AgentId, RecordEntry, RuntimeCapabilityInstall};
pub use aura_store_record::{DequeueToken, ReadStore};

pub(crate) mod sealed {
    /// Marker bound that seals [`super::WriteStore`] against new
    /// implementations outside of `aura-store`. Crates that only *use*
    /// `WriteStore` (currently only `aura-kernel`) do not need to name this
    /// trait; it is a pure implementation detail.
    pub trait Sealed {}
}

/// Sealed record-append family (Invariant §10).
///
/// Implementations of this trait are restricted to types declared inside
/// `aura-store` via the `Sealed` marker bound. `aura-kernel` is the only
/// caller that binds `Arc<dyn WriteStore>`; any new external bind site
/// should be treated as a bug and rerouted through a kernel method.
pub trait WriteStore: ReadStore + sealed::Sealed {
    /// Atomically append a record entry coupled with an inbox dequeue.
    ///
    /// This commits in a single `WriteBatch`:
    /// 1. Put record entry at `next_seq`
    /// 2. Update `head_seq` to `next_seq`
    /// 3. Delete inbox entry referenced by `dequeued_inbox_seq`
    /// 4. Update `inbox_head` cursor
    ///
    /// Compatibility wrapper — prefer [`append_entry_dequeued`](Self::append_entry_dequeued)
    /// for new code using [`DequeueToken`].
    ///
    /// # Errors
    /// Returns error if the write fails (nothing is committed).
    fn append_entry_atomic(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
        dequeued_inbox_seq: u64,
    ) -> Result<(), StoreError>;

    /// Append a record entry coupled with an inbox dequeue, using a [`DequeueToken`].
    ///
    /// Semantically identical to [`append_entry_atomic`](Self::append_entry_atomic)
    /// but accepts a typed token instead of a raw inbox sequence.
    ///
    /// # Errors
    /// Returns error if the write fails (nothing is committed).
    fn append_entry_dequeued(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
        token: DequeueToken,
    ) -> Result<(), StoreError> {
        self.append_entry_atomic(agent_id, next_seq, entry, token.inbox_seq())
    }

    /// Append a record entry coupled with an inbox dequeue while atomically
    /// updating the persisted runtime capability ledger.
    fn append_entry_dequeued_with_runtime_capabilities(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
        token: DequeueToken,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
        clear_runtime_capabilities: bool,
    ) -> Result<(), StoreError>;

    /// Append a record entry without touching the inbox (direct / non-inbox callers).
    ///
    /// Commits in a single `WriteBatch`:
    /// 1. Put record entry at `next_seq`
    /// 2. Update `head_seq` to `next_seq`
    ///
    /// # Errors
    /// Returns error if the write fails or sequence is non-contiguous.
    fn append_entry_direct(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
    ) -> Result<(), StoreError>;

    /// Append a record entry without touching the inbox, while atomically
    /// updating the persisted runtime capability ledger.
    ///
    /// When `clear_runtime_capabilities` is true, any prior ledger snapshot is
    /// removed in the same write. When `runtime_capabilities` is present, it is
    /// written as the new authoritative snapshot.
    fn append_entry_direct_with_runtime_capabilities(
        &self,
        agent_id: AgentId,
        next_seq: u64,
        entry: &RecordEntry,
        runtime_capabilities: Option<&RuntimeCapabilityInstall>,
        clear_runtime_capabilities: bool,
    ) -> Result<(), StoreError>;

    /// Atomically append multiple record entries in a single `WriteBatch`.
    ///
    /// Entries must have contiguous sequence numbers starting from `base_seq`.
    /// Updates `head_seq` to the last entry's sequence.
    ///
    /// # Errors
    /// Returns error if the write fails or sequences are non-contiguous.
    fn append_entries_batch(
        &self,
        agent_id: AgentId,
        base_seq: u64,
        entries: &[RecordEntry],
    ) -> Result<(), StoreError>;

    /// Append entries another node's kernel committed, verbatim, in one
    /// atomic batch: the cloud record mirror's only write path.
    ///
    /// This is a declared exception to "only the kernel appends"
    /// (`docs/invariants.md`, *Declared Exceptions*): while an agent runs
    /// on a cloud machine, that machine's kernel is its single writer and
    /// the local store only receives copies. Callers must have checked
    /// the page (`aura_engine_cloud::replica::verify_page`); this method
    /// re-checks that every entry's `seq` continues from `base_seq` and
    /// that `base_seq` is the local head + 1, so a gap or a duplicate
    /// can't be written.
    ///
    /// # Errors
    ///
    /// [`StoreError::SequenceMismatch`] for non-contiguous entries;
    /// otherwise as [`Self::append_entries_batch`]. Nothing is written
    /// on error.
    fn append_replica_batch(
        &self,
        agent_id: AgentId,
        base_seq: u64,
        entries: &[RecordEntry],
    ) -> Result<(), StoreError> {
        for (offset, entry) in (0u64..).zip(entries) {
            let expected = base_seq + offset;
            if entry.seq != expected {
                return Err(StoreError::SequenceMismatch {
                    expected,
                    actual: entry.seq,
                });
            }
        }
        self.append_entries_batch(agent_id, base_seq, entries)
    }
}

/// Convenience combined trait for storage implementations. Blanket-impl'd for
/// every type satisfying both halves. External crates should bind to
/// [`ReadStore`] unless they legitimately belong on the kernel write path.
pub trait Store: ReadStore + WriteStore {}
impl<T: ReadStore + WriteStore + ?Sized> Store for T {}
