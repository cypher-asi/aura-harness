//! Worker for processing agent transactions via `AgentLoop`.

use aura_agent::{AgentLoop, KernelToolExecutor};
use aura_core::AgentId;
use aura_reasoner::{Message, ModelProvider, ToolDefinition};
use aura_store::Store;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, instrument, warn};

const AGENT_LOOP_TIMEOUT: Duration = Duration::from_secs(300);

/// Process all pending transactions for an agent using `AgentLoop`.
///
/// Each dequeued transaction is converted to a user message, run through the
/// agent loop, and the result is recorded as a new entry in the agent's record.
///
/// This function should be called while holding the agent lock.
#[instrument(skip(store, provider, agent_loop, executor, tools), fields(agent_id = %agent_id))]
pub async fn process_agent(
    agent_id: AgentId,
    store: Arc<dyn Store>,
    provider: Arc<dyn ModelProvider + Send + Sync>,
    agent_loop: &AgentLoop,
    executor: &KernelToolExecutor,
    tools: &[ToolDefinition],
) -> anyhow::Result<u64> {
    let mut processed = 0u64;

    loop {
        let Some((inbox_seq, tx)) = store.dequeue_tx(agent_id)? else {
            debug!(processed, "Inbox empty, worker done");
            break;
        };

        let head_seq = store.get_head_seq(agent_id)?;
        let next_seq = head_seq + 1;

        debug!(
            inbox_seq,
            head_seq,
            next_seq,
            hash = %tx.hash,
            "Processing transaction"
        );

        let prompt = String::from_utf8(tx.payload.to_vec()).map_err(|e| {
            anyhow::anyhow!("Transaction payload is not valid UTF-8: {e}")
        })?;
        let messages = vec![Message::user(prompt)];

        let result = tokio::time::timeout(
            AGENT_LOOP_TIMEOUT,
            agent_loop.run(provider.as_ref(), executor, messages, tools.to_vec()),
        )
        .await
        .map_err(|_| anyhow::anyhow!("Agent loop timed out after {AGENT_LOOP_TIMEOUT:?}"))??;

        let context_hash = compute_context_hash(next_seq, &tx);
        let entry = aura_core::RecordEntry::builder(next_seq, tx)
            .context_hash(context_hash)
            .build();

        store.append_entry_atomic(agent_id, next_seq, &entry, inbox_seq)?;

        if result.llm_error.is_some() {
            warn!(seq = next_seq, "Transaction processed with LLM error");
        } else {
            info!(
                seq = next_seq,
                iterations = result.iterations,
                "Transaction committed via AgentLoop"
            );
        }

        processed += 1;
    }

    Ok(processed)
}

fn compute_context_hash(seq: u64, tx: &aura_core::Transaction) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    hasher.update(&seq.to_be_bytes());
    hasher.update(&tx.hash.0);
    *hasher.finalize().as_bytes()
}
