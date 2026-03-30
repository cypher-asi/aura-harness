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

        let prompt = String::from_utf8(tx.payload.to_vec())
            .map_err(|e| anyhow::anyhow!("Transaction payload is not valid UTF-8: {e}"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core::Transaction;
    use aura_core::TransactionType;
    use aura_reasoner::MockProvider;
    use aura_store::RocksStore;
    use bytes::Bytes;

    #[test]
    fn test_compute_context_hash_deterministic() {
        let agent_id = AgentId::generate();
        let tx = Transaction::new_chained(
            agent_id,
            TransactionType::UserPrompt,
            Bytes::from("hello"),
            None,
        );
        let h1 = compute_context_hash(1, &tx);
        let h2 = compute_context_hash(1, &tx);
        assert_eq!(h1, h2, "Same inputs must produce same hash");
    }

    #[test]
    fn test_compute_context_hash_different_seq() {
        let agent_id = AgentId::generate();
        let tx = Transaction::new_chained(
            agent_id,
            TransactionType::UserPrompt,
            Bytes::from("hello"),
            None,
        );
        let h1 = compute_context_hash(1, &tx);
        let h2 = compute_context_hash(2, &tx);
        assert_ne!(h1, h2, "Different seq should produce different hash");
    }

    #[test]
    fn test_compute_context_hash_different_tx() {
        let agent_id = AgentId::generate();
        let tx1 = Transaction::new_chained(
            agent_id,
            TransactionType::UserPrompt,
            Bytes::from("hello"),
            None,
        );
        let tx2 = Transaction::new_chained(
            agent_id,
            TransactionType::UserPrompt,
            Bytes::from("world"),
            None,
        );
        let h1 = compute_context_hash(1, &tx1);
        let h2 = compute_context_hash(1, &tx2);
        assert_ne!(h1, h2, "Different tx should produce different hash");
    }

    #[tokio::test]
    async fn test_process_agent_empty_inbox() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("response"));
        let agent_id = AgentId::generate();

        let ws_dir = dir.path().join("workspaces");
        std::fs::create_dir_all(&ws_dir).unwrap();

        let config = aura_agent::AgentLoopConfig::default();
        let agent_loop = AgentLoop::new(config);
        let router = aura_kernel::ExecutorRouter::new();
        let executor = aura_agent::KernelToolExecutor::new(router, agent_id, ws_dir.join("test"));

        let count = process_agent(agent_id, store, provider, &agent_loop, &executor, &[])
            .await
            .unwrap();

        assert_eq!(count, 0, "Empty inbox should process 0 transactions");
    }

    #[tokio::test]
    async fn test_process_agent_single_tx() {
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn Store> =
            Arc::new(RocksStore::open(dir.path().join("db"), false).unwrap());
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("I processed your request."));

        let agent_id = AgentId::generate();
        let tx = Transaction::new_chained(
            agent_id,
            TransactionType::UserPrompt,
            Bytes::from("test prompt"),
            None,
        );
        store.enqueue_tx(&tx).unwrap();

        let ws_dir = dir.path().join("workspaces");
        std::fs::create_dir_all(&ws_dir).unwrap();

        let config = aura_agent::AgentLoopConfig::default();
        let agent_loop = AgentLoop::new(config);
        let router = aura_kernel::ExecutorRouter::new();
        let executor = aura_agent::KernelToolExecutor::new(router, agent_id, ws_dir.join("agent"));

        let count = process_agent(
            agent_id,
            store.clone(),
            provider,
            &agent_loop,
            &executor,
            &[],
        )
        .await
        .unwrap();

        assert_eq!(count, 1, "Should process exactly 1 transaction");
        assert_eq!(
            store.get_head_seq(agent_id).unwrap(),
            1,
            "Head should advance to 1"
        );
    }

    #[test]
    fn test_agent_loop_timeout_constant() {
        assert_eq!(AGENT_LOOP_TIMEOUT, Duration::from_secs(300));
    }
}
