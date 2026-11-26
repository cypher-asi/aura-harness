//! System-shaped record writes that bypass a live `Kernel`.

use aura_core_types::{AgentId, RecordEntry};
use aura_store_db::Store;
use std::sync::Arc;

/// Phase 6a consolidation: write a System-shaped `RecordEntry`
/// through the kernel crate.
///
/// Previously `aura-node::tool_permissions::append_agent_tool_permissions_entry`
/// constructed and appended this entry inline, bypassing the kernel
/// and violating the "kernel owns every `RecordEntry` write"
/// invariant. The body of the build-and-write now lives here; the
/// runtime retains responsibility for the scheduler's
/// `processing_claim` (a scheduler-side lock) which the kernel crate
/// would not be able to see without a layering violation.
///
/// On-disk shape: identical to the prior runtime path — same
/// `seq = head_seq + 1`, same window-hashed `context_hash`, same
/// `append_entry_direct` write path. Replay and audit consumers see
/// no diff.
///
/// # Errors
///
/// Returns [`KernelError::Store`] for any backend failure.
pub fn write_system_record(
    store: &Arc<dyn Store>,
    agent_id: AgentId,
    tx: &aura_core_types::Transaction,
) -> Result<u64, crate::KernelError> {
    write_system_entry(store, agent_id, tx).map(|entry| entry.seq)
}

/// [`write_system_record`], returning the stored entry so the caller can
/// hand it to a [`RecordObserver`].
///
/// # Errors
///
/// Returns [`KernelError::Store`] for any backend failure.
pub fn write_system_entry(
    store: &Arc<dyn Store>,
    agent_id: AgentId,
    tx: &aura_core_types::Transaction,
) -> Result<RecordEntry, crate::KernelError> {
    // The `seq` for a System record is derived from a `get_head_seq`
    // read that is NOT serialized with the actual `append_entry_direct`
    // write at the store level. When another writer for the same agent
    // commits between our read and our append (e.g. a SubagentSpawn
    // audit record racing the parent's own turn writes), the store's
    // pre-flight `assert_next_seq` rejects us with
    // [`StoreError::SequenceMismatch`]. That is a transient,
    // recompute-and-retry condition rather than a hard failure, so we
    // re-read the head and rebuild the entry on a bounded retry loop.
    const MAX_RETRIES: u32 = 16;
    for attempt in 0..=MAX_RETRIES {
        match append_system_record_once(store, agent_id, tx) {
            Ok(entry) => return Ok(entry),
            Err(SystemRecordError::SequenceMismatch) if attempt < MAX_RETRIES => {
                // Brief backoff so the racing writer can land before we
                // recompute. Capped low because the audit append is on
                // the subagent spawn hot path.
                std::thread::sleep(std::time::Duration::from_millis(
                    u64::from(attempt + 1).min(8),
                ));
            }
            Err(SystemRecordError::SequenceMismatch) => {
                return Err(crate::KernelError::Store(format!(
                    "append_entry_direct: sequence mismatch unresolved after {MAX_RETRIES} retries"
                )));
            }
            Err(SystemRecordError::Other(err)) => return Err(err),
        }
    }
    unreachable!("write_system_record retry loop always returns")
}

/// Internal error discriminator for [`write_system_record`]'s retry
/// loop: `SequenceMismatch` is the retryable case, everything else is
/// terminal.
enum SystemRecordError {
    SequenceMismatch,
    Other(crate::KernelError),
}

/// Single attempt to append a System record: read the head, hash the
/// transaction against the trailing window, and direct-append at
/// `head + 1`. Returns [`SystemRecordError::SequenceMismatch`] when the
/// store rejects the computed sequence so the caller can retry.
fn append_system_record_once(
    store: &Arc<dyn Store>,
    agent_id: AgentId,
    tx: &aura_core_types::Transaction,
) -> Result<RecordEntry, SystemRecordError> {
    let head = store.get_head_seq(agent_id).map_err(|e| {
        SystemRecordError::Other(crate::KernelError::Store(format!("get_head_seq: {e}")))
    })?;
    let from_seq = head.saturating_sub(49).max(1);
    let window = if head == 0 {
        Vec::new()
    } else {
        store.scan_record(agent_id, from_seq, 50).map_err(|e| {
            SystemRecordError::Other(crate::KernelError::Store(format!("scan_record: {e}")))
        })?
    };
    let context_hash = crate::context::hash_tx_with_window(tx, &window).map_err(|e| {
        SystemRecordError::Other(crate::KernelError::Internal(format!("context hash: {e}")))
    })?;
    let seq = head + 1;
    let entry = RecordEntry::builder(seq, tx.clone())
        .context_hash(context_hash)
        .build();
    match store.append_entry_direct(agent_id, seq, &entry) {
        Ok(()) => Ok(entry),
        Err(aura_store_db::StoreError::SequenceMismatch { .. }) => {
            Err(SystemRecordError::SequenceMismatch)
        }
        Err(e) => Err(SystemRecordError::Other(crate::KernelError::Store(
            format!("append_entry_direct: {e}"),
        ))),
    }
}
