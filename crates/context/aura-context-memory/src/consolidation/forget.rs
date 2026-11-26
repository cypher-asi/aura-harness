//! Phase 1 — Forget (deterministic, no LLM).

use super::{run_blocking, ConsolidationConfig, ConsolidationReport, MemoryConsolidator};
use crate::error::MemoryError;
use crate::store::MemoryStoreApi;
use aura_core_types::AgentId;
use chrono::Utc;
use std::sync::Arc;
use tracing::debug;

impl MemoryConsolidator {
    /// Deterministic pruning of low-value facts and underperforming procedures.
    pub(super) async fn forget(
        &self,
        agent_id: AgentId,
        report: &mut ConsolidationReport,
    ) -> Result<(), MemoryError> {
        let store = Arc::clone(&self.store);
        let config = self.config.clone();
        let (facts_forgotten, procedures_forgotten) = run_blocking(move || {
            let facts_forgotten = forget_facts(&*store, &config, agent_id)?;
            let procedures_forgotten = forget_procedures(&*store, &config, agent_id)?;
            Ok((facts_forgotten, procedures_forgotten))
        })
        .await?;

        report.facts_forgotten = facts_forgotten;
        report.procedures_forgotten = procedures_forgotten;
        debug!(%agent_id, facts_forgotten, procedures_forgotten, "Forget phase complete");
        Ok(())
    }
}

/// Delete unimportant, never-accessed facts older than the access cutoff.
fn forget_facts(
    store: &dyn MemoryStoreApi,
    config: &ConsolidationConfig,
    agent_id: AgentId,
) -> Result<usize, MemoryError> {
    let now = Utc::now();
    let access_cutoff_secs = config.access_forget_days * 86_400;
    let mut forgotten = 0usize;

    let facts = store.list_facts(agent_id)?;
    for fact in &facts {
        let age_secs = (now - fact.updated_at).num_seconds();
        if fact.importance < config.importance_forget_threshold
            && fact.access_count == 0
            && age_secs > access_cutoff_secs
        {
            store.delete_fact(agent_id, fact.fact_id)?;
            forgotten += 1;
        }
    }
    Ok(forgotten)
}

/// Delete procedures that have run enough times to judge and keep failing.
fn forget_procedures(
    store: &dyn MemoryStoreApi,
    config: &ConsolidationConfig,
    agent_id: AgentId,
) -> Result<usize, MemoryError> {
    let mut forgotten = 0usize;

    let procedures = store.list_procedures(agent_id)?;
    for proc in &procedures {
        if proc.success_rate < config.procedure_forget_threshold
            && proc.execution_count >= config.procedure_min_executions
        {
            store.delete_procedure(agent_id, proc.procedure_id)?;
            forgotten += 1;
        }
    }
    Ok(forgotten)
}
