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
use aura_core::{
    AgentId, AgentStatus, RecordEntry, RuntimeCapabilityInstall, Transaction, UserToolDefaults,
};

/// Opaque token produced by [`ReadStore::dequeue_tx`] and consumed exactly once
/// by [`WriteStore::append_entry_atomic`] or [`WriteStore::append_entry_dequeued`].
///
/// Encapsulates the inbox sequence number so callers never manipulate it directly.
#[derive(Debug, Clone)]
pub struct DequeueToken {
    pub(crate) inbox_seq: u64,
}

impl DequeueToken {
    /// The inbox sequence this token represents (read-only for diagnostics/logging).
    #[must_use]
    pub const fn inbox_seq(&self) -> u64 {
        self.inbox_seq
    }
}

pub(crate) mod sealed {
    /// Marker bound that seals [`super::WriteStore`] against new
    /// implementations outside of `aura-store`. Crates that only *use*
    /// `WriteStore` (currently only `aura-kernel`) do not need to name this
    /// trait; it is a pure implementation detail.
    pub trait Sealed {}
}

/// Read side of the store, plus the inbox / agent-metadata writes that are
/// explicitly **not** part of the sealed record-append family (Invariant §10).
///
/// This is the trait that non-kernel crates should bind to (`Arc<dyn ReadStore>`).
pub trait ReadStore: Send + Sync {
    /// Enqueue a transaction to an agent's inbox.
    ///
    /// This is a durable write — the transaction is persisted before returning.
    /// Explicitly allowed for non-kernel callers (HTTP `/tx` handler, scheduler,
    /// automaton bridge) which enqueue externally-originated transactions that
    /// the kernel will later dequeue and process.
    ///
    /// # Errors
    /// Returns error if the write fails.
    fn enqueue_tx(&self, tx: &Transaction) -> Result<(), StoreError>;

    /// Dequeue a transaction from an agent's inbox.
    ///
    /// Returns a [`DequeueToken`] and the transaction, or `None` if inbox is empty.
    /// Does NOT delete the transaction — that happens when the token is consumed
    /// by [`WriteStore::append_entry_atomic`] or
    /// [`WriteStore::append_entry_dequeued`].
    ///
    /// # Errors
    /// Returns error if the read fails.
    fn dequeue_tx(
        &self,
        agent_id: AgentId,
    ) -> Result<Option<(DequeueToken, Transaction)>, StoreError>;

    /// Get the current head sequence number for an agent.
    ///
    /// Returns 0 if the agent has no record entries yet.
    ///
    /// # Errors
    /// Returns error if the read fails.
    fn get_head_seq(&self, agent_id: AgentId) -> Result<u64, StoreError>;

    /// Scan record entries for an agent.
    ///
    /// Returns entries starting from `from_seq` up to `limit` entries.
    ///
    /// # Errors
    /// Returns error if the scan fails.
    fn scan_record(
        &self,
        agent_id: AgentId,
        from_seq: u64,
        limit: usize,
    ) -> Result<Vec<RecordEntry>, StoreError>;

    /// Get a single record entry.
    ///
    /// # Errors
    /// Returns error if the entry is not found or read fails.
    fn get_record_entry(&self, agent_id: AgentId, seq: u64) -> Result<RecordEntry, StoreError>;

    /// Get agent status.
    ///
    /// Returns `Active` if not explicitly set.
    ///
    /// # Errors
    /// Returns error if the read fails.
    fn get_agent_status(&self, agent_id: AgentId) -> Result<AgentStatus, StoreError>;

    /// Get the current persisted runtime capability snapshot for an agent.
    ///
    /// Returns `None` if no snapshot has been recorded or if the current
    /// session boundary cleared the ledger.
    fn get_runtime_capabilities(
        &self,
        agent_id: AgentId,
    ) -> Result<Option<RuntimeCapabilityInstall>, StoreError>;

    /// Set agent status.
    ///
    /// # Errors
    /// Returns error if the write fails.
    fn set_agent_status(&self, agent_id: AgentId, status: AgentStatus) -> Result<(), StoreError>;

    /// Check if agent has pending transactions in inbox.
    ///
    /// # Errors
    /// Returns error if the read fails.
    fn has_pending_tx(&self, agent_id: AgentId) -> Result<bool, StoreError>;

    /// Get inbox depth (number of pending transactions).
    ///
    /// # Errors
    /// Returns error if the read fails.
    fn get_inbox_depth(&self, agent_id: AgentId) -> Result<u64, StoreError>;

    /// Load the persisted [`UserToolDefaults`] for a user, if any.
    ///
    /// Returns `None` when no entry exists (first-run users); callers
    /// substitute [`UserToolDefaults::default`] (`FullAccess`) in that
    /// case so every agent the user owns defaults to "all tools on".
    ///
    /// # Errors
    /// Returns error if the read or deserialisation fails.
    fn get_user_tool_defaults(&self, user_id: &str)
        -> Result<Option<UserToolDefaults>, StoreError>;

    /// Replace the persisted [`UserToolDefaults`] for a user.
    ///
    /// # Errors
    /// Returns error if the write fails.
    fn put_user_tool_defaults(
        &self,
        user_id: &str,
        defaults: &UserToolDefaults,
    ) -> Result<(), StoreError>;

    /// Delete the persisted [`UserToolDefaults`] for a user. After
    /// deletion, reads fall back to [`UserToolDefaults::default`].
    ///
    /// # Errors
    /// Returns error if the write fails.
    fn delete_user_tool_defaults(&self, user_id: &str) -> Result<(), StoreError>;

    /// Attempt to claim exclusive processing rights for an agent.
    ///
    /// Returns `Ok(true)` only when the claim transitioned from absent to
    /// present. Returns `Ok(false)` when another worker already owns the claim.
    ///
    /// # Errors
    /// Returns error if the compare-and-set read/write fails.
    fn try_claim_agent_processing(&self, agent_id: AgentId) -> Result<bool, StoreError>;

    /// Release a previously acquired processing claim for an agent.
    ///
    /// This operation is idempotent: releasing an absent claim succeeds.
    ///
    /// # Errors
    /// Returns error if the delete fails.
    fn release_agent_processing(&self, agent_id: AgentId) -> Result<(), StoreError>;

    /// Check whether an agent currently has a processing claim.
    ///
    /// # Errors
    /// Returns error if the read fails.
    fn is_agent_processing(&self, agent_id: AgentId) -> Result<bool, StoreError>;
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
        self.append_entry_atomic(agent_id, next_seq, entry, token.inbox_seq)
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
}

/// Convenience combined trait for storage implementations. Blanket-impl'd for
/// every type satisfying both halves. External crates should bind to
/// [`ReadStore`] unless they legitimately belong on the kernel write path.
pub trait Store: ReadStore + WriteStore {}
impl<T: ReadStore + WriteStore + ?Sized> Store for T {}
