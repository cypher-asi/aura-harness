//! Post-session memory consolidation: forgetting, compression, and evolution.
//!
//! Runs three phases over an agent's curated memories:
//! 1. **Forget** — deterministic pruning of low-value facts and underperforming procedures.
//! 2. **Compress** — LLM-assisted episodic event compression.
//! 3. **Evolve** — LLM-assisted fact merging, contradiction resolution, and insight creation.

mod compress;
mod evolve;
mod forget;
mod parse;

use crate::error::MemoryError;
use crate::store::MemoryStoreApi;
use crate::types::{MemoryContinuity, MemoryProvenance, MemorySensitivity, MemoryStatus};
use crate::DEFAULT_MEMORY_MODEL;
use aura_core_types::AgentId;
use aura_model::{Message, ModelProvider, ModelRequest, ThinkingEffort};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::info;

// ============================================================================
// Configuration
// ============================================================================

/// Configuration for the memory consolidation process.
#[derive(Debug, Clone)]
pub struct ConsolidationConfig {
    /// Model identifier for LLM-assisted consolidation steps.
    pub model: String,
    /// Optional auth token for the model provider proxy.
    pub auth_token: Option<String>,
    /// Trigger event compression when event count exceeds this threshold.
    pub max_events_before_compression: usize,
    /// Maximum age (days) for events before they become compression candidates.
    pub max_event_age_days: i64,
    /// Facts with importance below this and zero access are forgetting candidates.
    pub importance_forget_threshold: f32,
    /// Facts not accessed within this many days are forgetting candidates.
    pub access_forget_days: i64,
    /// Procedures with success rate below this are forgetting candidates.
    pub procedure_forget_threshold: f32,
    /// Minimum execution count before a procedure can be forgotten for low success.
    pub procedure_min_executions: u32,
}

impl Default for ConsolidationConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_MEMORY_MODEL.to_string(),
            auth_token: None,
            max_events_before_compression: 100,
            max_event_age_days: 30,
            importance_forget_threshold: 0.1,
            access_forget_days: 60,
            procedure_forget_threshold: 0.2,
            procedure_min_executions: 3,
        }
    }
}

// ============================================================================
// Report
// ============================================================================

/// Summary of changes applied during a consolidation run.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsolidationReport {
    /// Facts merged into richer entries.
    pub facts_merged: usize,
    /// Facts updated with new evidence.
    pub facts_evolved: usize,
    /// Consolidated summary events created.
    pub events_compressed: usize,
    /// Original events deleted during compression.
    pub events_deleted: usize,
    /// Low-value facts pruned.
    pub facts_forgotten: usize,
    /// Underperforming procedures pruned.
    pub procedures_forgotten: usize,
    /// New cross-session insight facts created.
    pub insights_created: usize,
}

// ============================================================================
// Consolidator
// ============================================================================

/// Post-session consolidator that prunes, compresses, and evolves agent memories.
///
/// LLM calls for compression and fact evolution go through the injected
/// [`ModelProvider`]. Production wiring MUST pass a recording-capable
/// provider (e.g. `aura_agent::KernelModelGateway`) so Invariant §3
/// ("Every LLM Call Is Recorded") holds. The context layer accepts the
/// abstract trait object to avoid an upward edge into `aura-agent`; the
/// runtime composition root in `aura_node::node` is the single place
/// responsible for picking the right impl.
pub struct MemoryConsolidator {
    store: Arc<dyn MemoryStoreApi>,
    provider: Arc<dyn ModelProvider + Send + Sync>,
    config: ConsolidationConfig,
}

impl MemoryConsolidator {
    /// Create a new consolidator backed by the given store and model
    /// provider.
    ///
    /// `provider` must be a recording-capable [`ModelProvider`] (in
    /// production, `aura_agent::KernelModelGateway`) so the LLM calls
    /// the compress / evolve phases issue are persisted in the kernel's
    /// append-only log per Invariant §3.
    #[must_use]
    pub fn new(
        store: Arc<dyn MemoryStoreApi>,
        provider: Arc<dyn ModelProvider + Send + Sync>,
        config: ConsolidationConfig,
    ) -> Self {
        Self {
            store,
            provider,
            config,
        }
    }

    /// Run the full consolidation pipeline for an agent.
    ///
    /// Executes forget → compress → evolve in sequence and returns a report
    /// summarising all changes.
    ///
    /// # Errors
    /// Returns error on store I/O or model provider failure.
    pub async fn consolidate(&self, agent_id: AgentId) -> Result<ConsolidationReport, MemoryError> {
        let mut report = ConsolidationReport::default();

        self.forget(agent_id, &mut report).await?;
        self.compress_events(agent_id, &mut report).await?;
        self.evolve_facts(agent_id, &mut report).await?;

        info!(
            %agent_id,
            facts_forgotten = report.facts_forgotten,
            procedures_forgotten = report.procedures_forgotten,
            events_compressed = report.events_compressed,
            events_deleted = report.events_deleted,
            facts_merged = report.facts_merged,
            facts_evolved = report.facts_evolved,
            insights_created = report.insights_created,
            "Consolidation complete"
        );

        Ok(report)
    }

    /// Send one prompt to the consolidation model and return its text reply.
    async fn complete_text(
        &self,
        system_prompt: &str,
        prompt: String,
    ) -> Result<String, MemoryError> {
        let request = ModelRequest::builder(&self.config.model, system_prompt)
            .messages(vec![Message::user(prompt)])
            .thinking_effort(Some(ThinkingEffort::Off))
            .auth_token(self.config.auth_token.clone())
            .try_build()
            .map_err(|e| MemoryError::Provider(e.to_string()))?;

        let response = self
            .provider
            .complete(request)
            .await
            .map_err(|e| MemoryError::Provider(e.to_string()))?;

        Ok(response.message.text_content())
    }
}

/// Run blocking store work off the async runtime.
async fn run_blocking<T, F>(work: F) -> Result<T, MemoryError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, MemoryError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| MemoryError::BlockingTaskFailed(e.to_string()))?
}

fn consolidatable(continuity: &MemoryContinuity) -> bool {
    continuity.status == MemoryStatus::Active && continuity.sensitivity == MemorySensitivity::Normal
}

fn consolidated_continuity(source: &MemoryContinuity) -> MemoryContinuity {
    MemoryContinuity {
        scope: source.scope,
        status: MemoryStatus::Active,
        sensitivity: MemorySensitivity::Normal,
        pinned: false,
        provenance: MemoryProvenance {
            project_id: source.provenance.project_id.clone(),
            user_id: source.provenance.user_id.clone(),
            ..MemoryProvenance::default()
        },
        superseded_by: None,
    }
}

#[cfg(test)]
mod tests;
