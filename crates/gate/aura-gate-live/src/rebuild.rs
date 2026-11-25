//! The table rebuilt from the record at startup.

use crate::record::for_each_batch;
use crate::runtime::in_background;
use crate::table::{lock, GateLive};
use aura_core_types::{AgentId, RecordEntry, TransactionType};
use aura_store_db::{ReadStore, StoreError};
use rayon::prelude::*;
use std::sync::Arc;
use tracing::{info, warn};

impl GateLive {
    /// Take in the records of `agents` in `store`, oldest first. Returns
    /// how many entries were read.
    ///
    /// The records are read and decoded in parallel; each agent's
    /// prompts and outcomes are then taken in, in order, one agent at a
    /// time, as live entries are.
    pub fn rebuild<S: ReadStore + ?Sized>(&self, store: &S, agents: &[AgentId]) -> usize {
        let scans: Vec<(AgentId, usize, Result<Vec<RecordEntry>, StoreError>)> = agents
            .par_iter()
            .map(|&agent_id| {
                let mut read = 0;
                let mut kept = Vec::new();
                let scanned = for_each_batch(store, agent_id, |entries| {
                    read += entries.len();
                    kept.extend(entries.into_iter().filter(counts));
                });
                (agent_id, read, scanned.map(|()| kept))
            })
            .collect();
        let mut read = 0;
        for (agent_id, scanned, kept) in scans {
            read += scanned;
            match kept {
                Ok(entries) => {
                    let mut stats = lock(&self.stats);
                    for entry in &entries {
                        stats.observe(entry);
                    }
                }
                Err(e) => warn!(%agent_id, error = %e, "Gate could not read a record"),
            }
        }
        read
    }

    /// [`Self::rebuild`] on a blocking thread, so startup doesn't wait
    /// for the scan. Without a Tokio runtime it runs here.
    pub fn rebuild_in_background(
        self: &Arc<Self>,
        store: Arc<dyn ReadStore>,
        agents: Vec<AgentId>,
    ) {
        let live = Arc::clone(self);
        in_background(move || {
            let read = live.rebuild(store.as_ref(), &agents);
            info!(
                agents = agents.len(),
                entries = read,
                "Gate rebuilt its table"
            );
        });
    }
}

/// Whether `entry` is one Gate's table takes in: a prompt or an outcome.
fn counts(entry: &RecordEntry) -> bool {
    matches!(
        entry.tx.tx_type,
        TransactionType::UserPrompt | TransactionType::Outcome
    )
}
