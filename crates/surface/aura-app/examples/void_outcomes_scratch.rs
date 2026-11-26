//! Scratch: void failed outcomes by appending a `judged_excused` one for
//! the same prompt, as the judge does. Usage:
//! `void_outcomes_scratch <db> <agent_hex>:<seq>... [--write]`

// Example code: it may unwrap/expect.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use aura_agent_kernel::{Kernel, KernelConfig};
use aura_core_types::{AgentId, OutcomeSignal, Transaction};
use aura_model::{MockProvider, ModelProvider};
use aura_store_db::{ReadStore, RocksStore, Store};
use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let write = args.iter().any(|a| a == "--write");
    let mut args = args.into_iter().filter(|a| a != "--write");
    let db = args.next().expect("db path");
    let store = Arc::new(RocksStore::open(&db, true)?);
    for target in args {
        let (agent, seq) = target.split_once(':').expect("agent:seq");
        let agent_id = AgentId::from_hex(agent)?;
        let seq: u64 = seq.parse()?;
        let entries = store.scan_record(agent_id, seq, 10_000)?;
        let entry = entries.first().filter(|e| e.seq == seq).expect("entry");
        let failed = entry.tx.outcome_payload().expect("an outcome");
        anyhow::ensure!(
            failed.signal == OutcomeSignal::ChecksFailed,
            "{target}: {} is not checks_failed",
            failed.signal
        );
        let voided = entries.iter().skip(1).any(|e| {
            e.tx.outcome_payload()
                .is_some_and(|o| o.prompt_tx == failed.prompt_tx && o.signal.voids())
        });
        println!(
            "{target}: {} {} {} cost {:.4}{}",
            failed.model,
            failed.category,
            failed.signal,
            failed.cost_usd,
            if voided { " (already voided)" } else { "" }
        );
        if voided || !write {
            continue;
        }
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("unused"));
        let (executor, _) = aura_agent::session_bootstrap::build_executor_router();
        let kernel = Kernel::new(
            Arc::clone(&store) as Arc<dyn Store>,
            provider,
            executor,
            KernelConfig::default(),
            agent_id,
        )?;
        let excused = failed.with_signal(OutcomeSignal::JudgedExcused);
        let result = kernel
            .process_direct(Transaction::outcome(agent_id, &excused))
            .await?;
        println!("  appended judged_excused at seq {}", result.entry.seq);
    }
    Ok(())
}
