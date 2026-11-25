//! Read side of the per-agent store.
//!
//! [`ReadStore`] holds every read plus the inbox / agent-metadata writes
//! that non-kernel callers may make. The sealed record-append family
//! (`WriteStore`) lives with the concrete backend in `aura-store-db`, so
//! crates that only read or enqueue depend on this crate instead of the
//! backend.

use crate::error::StoreError;
use aura_core_types::{
    AgentId, AgentStatus, ContextHash, RecordEntry, RuntimeCapabilityInstall, Transaction,
    UserToolDefaults,
};

/// Opaque token produced by [`ReadStore::dequeue_tx`] and consumed exactly once
/// by `WriteStore::append_entry_atomic` or `WriteStore::append_entry_dequeued`.
///
/// Encapsulates the inbox sequence number so callers never manipulate it directly.
#[derive(Debug, Clone)]
pub struct DequeueToken {
    inbox_seq: u64,
}

impl DequeueToken {
    /// Mint a token for `inbox_seq`. Only store implementations call this,
    /// from their [`ReadStore::dequeue_tx`].
    #[doc(hidden)]
    #[must_use]
    pub const fn from_inbox_seq(inbox_seq: u64) -> Self {
        Self { inbox_seq }
    }

    /// The inbox sequence this token represents (read-only for diagnostics/logging).
    #[must_use]
    pub const fn inbox_seq(&self) -> u64 {
        self.inbox_seq
    }
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
    /// by `WriteStore::append_entry_atomic` or
    /// `WriteStore::append_entry_dequeued`.
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

    /// The seq and context hash of each entry [`Self::scan_record`]
    /// returns. Implementations may skip decoding the rest of each entry.
    ///
    /// # Errors
    /// Returns error if the scan fails.
    fn scan_context_hashes(
        &self,
        agent_id: AgentId,
        from_seq: u64,
        limit: usize,
    ) -> Result<Vec<(u64, ContextHash)>, StoreError> {
        Ok(self
            .scan_record(agent_id, from_seq, limit)?
            .into_iter()
            .map(|entry| (entry.seq, entry.context_hash))
            .collect())
    }

    /// Scan record entries for an agent in descending sequence order.
    ///
    /// Returns entries starting at `from_seq` and walking backwards up to
    /// `limit` entries. Implementations may override this with a native
    /// reverse iterator; the default preserves a bounded read path by issuing
    /// at most `limit` point lookups.
    ///
    /// # Errors
    /// Returns error if any bounded lookup fails.
    fn scan_record_descending(
        &self,
        agent_id: AgentId,
        from_seq: u64,
        limit: usize,
    ) -> Result<Vec<RecordEntry>, StoreError> {
        if from_seq == 0 || limit == 0 {
            return Ok(Vec::new());
        }

        let window = u64::try_from(limit).unwrap_or(u64::MAX);
        let min_seq = from_seq.saturating_sub(window.saturating_sub(1)).max(1);
        let mut entries = Vec::with_capacity(limit);

        for seq in (min_seq..=from_seq).rev() {
            entries.push(self.get_record_entry(agent_id, seq)?);
        }

        Ok(entries)
    }

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
