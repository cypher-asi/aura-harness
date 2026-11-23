use crate::kernel::{Kernel, KernelConfig};
use crate::ExecutorRouter;
use async_trait::async_trait;
use aura_core_types::{AgentId, Transaction};
use aura_model::{MockProvider, ModelProvider, ModelRequest, ModelResponse, ReasonerError};
use aura_store_db::{RocksStore, Store};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::Barrier;
use tokio::task::JoinSet;

/// Model calls answered together in each round.
const AT_ONCE: usize = 8;
/// Rounds of calls answered together.
const ROUNDS: usize = 6;

/// A provider that holds each call until [`AT_ONCE`] of them wait, then
/// answers them all together, so their records race for the agent's
/// next sequence numbers.
struct Together {
    barrier: Barrier,
    answer: MockProvider,
}

#[async_trait]
impl ModelProvider for Together {
    fn name(&self) -> &'static str {
        "together"
    }

    async fn complete(&self, request: ModelRequest) -> Result<ModelResponse, ReasonerError> {
        self.barrier.wait().await;
        self.answer.complete(request).await
    }

    async fn health_check(&self) -> bool {
        true
    }
}

fn request() -> ModelRequest {
    ModelRequest::builder("test-model", "system")
        .message(aura_model::Message::user("test"))
        .try_build()
        .unwrap()
}

/// A kernel on `store` for `agent_id` whose model calls come back
/// together.
fn kernel(store: &Arc<dyn Store>, agent_id: AgentId, workspace: &TempDir) -> Arc<Kernel> {
    let provider = Arc::new(Together {
        barrier: Barrier::new(AT_ONCE),
        answer: MockProvider::simple_response("answer"),
    });
    let config = KernelConfig {
        workspace_base: workspace.path().to_path_buf(),
        ..KernelConfig::default()
    };
    let kernel = Kernel::new(
        Arc::clone(store),
        provider,
        ExecutorRouter::new(),
        config,
        agent_id,
    );
    Arc::new(kernel.unwrap())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_made_at_once_on_one_kernel_each_get_their_own_number() {
    let (db, workspace) = (TempDir::new().unwrap(), TempDir::new().unwrap());
    let store: Arc<dyn Store> = Arc::new(RocksStore::open(db.path(), false).unwrap());
    let agent_id = AgentId::generate();
    let kernel = kernel(&store, agent_id, &workspace);
    let mut failed = Vec::new();
    for _ in 0..ROUNDS {
        let mut calls = JoinSet::new();
        for _ in 0..AT_ONCE {
            let reasoning = Arc::clone(&kernel);
            calls.spawn(async move { reasoning.reason(request()).await.map(|_| ()) });
            let prompting = Arc::clone(&kernel);
            calls.spawn(async move {
                let prompt = Transaction::user_prompt(agent_id, "prompt");
                prompting.process_direct(prompt).await.map(|_| ())
            });
        }
        while let Some(call) = calls.join_next().await {
            if let Err(error) = call.expect("the call ran to its end") {
                failed.push(error.to_string());
            }
        }
    }
    assert_eq!(failed, Vec::<String>::new(), "records that failed");
    let total = 2 * AT_ONCE * ROUNDS;
    let seqs: Vec<u64> = (store.scan_record(agent_id, 1, total).unwrap().iter())
        .map(|entry| entry.seq)
        .collect();
    assert_eq!(seqs, (1..=total as u64).collect::<Vec<_>>());
    assert_eq!(store.get_head_seq(agent_id).unwrap(), total as u64);
}
