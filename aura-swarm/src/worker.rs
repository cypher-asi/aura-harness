//! Worker for processing agent transactions.

use aura_core::AgentId;
use aura_kernel::Kernel;
use aura_reasoner::Reasoner;
use aura_store::Store;
use std::sync::Arc;
use tracing::{debug, info, instrument, warn};

/// Process all pending transactions for an agent.
///
/// This function should be called while holding the agent lock.
#[instrument(skip(store, kernel), fields(agent_id = %agent_id))]
pub async fn process_agent<S, R>(
    agent_id: AgentId,
    store: Arc<S>,
    kernel: Arc<Kernel<S, R>>,
) -> anyhow::Result<u64>
where
    S: Store + 'static,
    R: Reasoner + 'static,
{
    let mut processed = 0u64;

    loop {
        // Dequeue next transaction
        let Some((inbox_seq, tx)) = store.dequeue_tx(agent_id)? else {
            debug!(processed, "Inbox empty, worker done");
            break;
        };

        // Get current head_seq
        let head_seq = store.get_head_seq(agent_id)?;
        let next_seq = head_seq + 1;

        debug!(
            inbox_seq,
            head_seq,
            next_seq,
            hash = %tx.hash,
            "Processing transaction"
        );

        // Process transaction through kernel
        let result = kernel.process(tx, next_seq).await?;

        // Atomic commit
        store.append_entry_atomic(agent_id, next_seq, &result.entry, inbox_seq)?;

        if result.had_failures {
            warn!(seq = next_seq, "Transaction processed with failures");
        } else {
            info!(seq = next_seq, "Transaction committed");
        }

        processed += 1;
    }

    Ok(processed)
}
