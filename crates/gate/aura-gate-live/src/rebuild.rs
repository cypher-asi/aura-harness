//! The table rebuilt from the record at startup.

use crate::record::for_each_batch;
use crate::runtime::in_background;
use crate::table::{lock, GateLive};
use aura_core_types::AgentId;
use aura_store_db::ReadStore;
use std::sync::Arc;
use tracing::{info, warn};

impl GateLive {
    /// Take in the records of `agents` in `store`, oldest first. Returns
    /// how many entries were read.
    pub fn rebuild<S: ReadStore + ?Sized>(&self, store: &S, agents: &[AgentId]) -> usize {
        let mut read = 0;
        for &agent_id in agents {
            let scanned = for_each_batch(store, agent_id, |entries| {
                let mut stats = lock(&self.stats);
                for entry in &entries {
                    stats.observe(entry);
                }
                read += entries.len();
            });
            if let Err(e) = scanned {
                warn!(%agent_id, error = %e, "Gate could not read a record");
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
