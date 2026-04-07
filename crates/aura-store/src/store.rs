//! Store trait definition.

use crate::error::StoreError;
use aura_core::{AgentId, AgentStatus, RecordEntry, RuntimeCapabilityInstall, Transaction};

/// Opaque token produced by [`Store::dequeue_tx`] and consumed exactly once
/// by [`Store::append_entry_atomic`] or [`Store::append_entry_dequeued`].
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

/// Storage trait for the Aura system.
///
/// All implementations must provide atomic commit semantics.
pub trait Store: Send + Sync {
    /// Enqueue a transaction to an agent's inbox.
    ///
    /// This is a durable write - the transaction is persisted before returning.
    ///
    /// # Errors
    /// Returns error if the write fails.
    fn enqueue_tx(&self, tx: &Transaction) -> Result<(), StoreError>;

    /// Dequeue a transaction from an agent's inbox.
    ///
    /// Returns a [`DequeueToken`] and the transaction, or `None` if inbox is empty.
    /// Does NOT delete the transaction — that happens when the token is consumed
    /// by [`append_entry_atomic`](Store::append_entry_atomic) or
    /// [`append_entry_dequeued`](Store::append_entry_dequeued).
    ///
    /// # Errors
    /// Returns error if the read fails.
    fn dequeue_tx(&self, agent_id: AgentId) -> Result<Option<(DequeueToken, Transaction)>, StoreError>;

    /// Get the current head sequence number for an agent.
    ///
    /// Returns 0 if the agent has no record entries yet.
    ///
    /// # Errors
    /// Returns error if the read fails.
    fn get_head_seq(&self, agent_id: AgentId) -> Result<u64, StoreError>;

    /// Atomically append a record entry coupled with an inbox dequeue.
    ///
    /// This commits in a single `WriteBatch`:
    /// 1. Put record entry at `next_seq`
    /// 2. Update `head_seq` to `next_seq`
    /// 3. Delete inbox entry referenced by `dequeued_inbox_seq`
    /// 4. Update `inbox_head` cursor
    ///
    /// Compatibility wrapper — prefer [`append_entry_dequeued`](Store::append_entry_dequeued)
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
    /// Semantically identical to [`append_entry_atomic`](Store::append_entry_atomic)
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
}
