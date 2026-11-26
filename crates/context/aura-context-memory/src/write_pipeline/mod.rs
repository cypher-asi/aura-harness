//! Orchestrates Stage 1 (heuristic) -> Stage 2 (LLM extraction + refinement) -> write.

mod candidate;
mod capacity;
mod writers;

use crate::error::MemoryError;
use crate::extraction::{ConversationTurn, HeuristicExtractor};
use crate::refinement::{LlmRefiner, RefinementRequestContext};
use crate::store::MemoryStoreApi;
use crate::turn_summary::TurnSummary;
use crate::types::{CandidateType, MemoryAccessContext, MemoryStatus, RefinedCandidate};
use aura_core_types::AgentId;
use candidate::{candidate_is_sensitive, candidate_is_unsafe, WriteOrigin};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, info};

pub struct MemoryWritePipeline {
    store: Arc<dyn MemoryStoreApi>,
    extractor: HeuristicExtractor,
    refiner: LlmRefiner,
    config: WriteConfig,
}

#[derive(Debug, Clone)]
pub struct WriteConfig {
    pub confidence_floor: f32,
    pub max_facts_per_agent: usize,
    pub max_events_per_agent: usize,
    pub max_procedures_per_agent: usize,
}

impl Default for WriteConfig {
    fn default() -> Self {
        Self {
            confidence_floor: 0.5,
            max_facts_per_agent: 100,
            max_events_per_agent: 500,
            max_procedures_per_agent: 50,
        }
    }
}

/// Where an ingested turn came from, and the status its memories start
/// in.
#[derive(Debug, Clone, Copy)]
pub struct IngestProvenance<'a> {
    /// Session the turn was taken from, kept in each memory's provenance.
    pub source_session_id: Option<&'a str>,
    pub initial_status: MemoryStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WriteReport {
    pub candidates_extracted: usize,
    pub candidates_refined: usize,
    pub facts_written: usize,
    pub facts_updated: usize,
    pub events_written: usize,
    pub procedures_written: usize,
    pub candidates_dropped: usize,
    pub sensitive_dropped: usize,
    pub unsafe_dropped: usize,
    pub conflicts_pending: usize,
}

impl MemoryWritePipeline {
    #[must_use]
    pub fn new(store: Arc<dyn MemoryStoreApi>, refiner: LlmRefiner, config: WriteConfig) -> Self {
        Self {
            store,
            extractor: HeuristicExtractor,
            refiner,
            config,
        }
    }

    /// Ingest a [`TurnSummary`] through the pipeline.
    ///
    /// Stage 1: free heuristic extraction on assistant text.
    /// Stage 2: LLM call (Haiku) that sees the full conversation turn and
    ///          refines heuristic candidates + extracts new facts.
    ///
    /// # Errors
    /// Returns error on extraction, refinement, or storage failure.
    pub async fn ingest(
        &self,
        agent_id: AgentId,
        summary: &TurnSummary,
        turn: Option<&ConversationTurn>,
    ) -> Result<WriteReport, MemoryError> {
        self.ingest_with_token(agent_id, summary, turn, None).await
    }

    pub async fn ingest_with_token(
        &self,
        agent_id: AgentId,
        summary: &TurnSummary,
        turn: Option<&ConversationTurn>,
        auth_token: Option<String>,
    ) -> Result<WriteReport, MemoryError> {
        self.ingest_with_context(agent_id, summary, turn, auth_token, &[])
            .await
    }

    pub async fn ingest_with_context(
        &self,
        agent_id: AgentId,
        summary: &TurnSummary,
        turn: Option<&ConversationTurn>,
        auth_token: Option<String>,
        active_skills: &[String],
    ) -> Result<WriteReport, MemoryError> {
        self.ingest_with_provenance(
            agent_id,
            summary,
            turn,
            auth_token,
            active_skills,
            IngestProvenance {
                source_session_id: None,
                initial_status: MemoryStatus::Active,
            },
        )
        .await
    }

    pub async fn ingest_with_provenance(
        &self,
        agent_id: AgentId,
        summary: &TurnSummary,
        turn: Option<&ConversationTurn>,
        auth_token: Option<String>,
        active_skills: &[String],
        provenance: IngestProvenance<'_>,
    ) -> Result<WriteReport, MemoryError> {
        self.ingest_with_provenance_and_request_context(
            agent_id,
            summary,
            turn,
            RefinementRequestContext {
                auth_token,
                ..Default::default()
            },
            active_skills,
            provenance,
        )
        .await
    }

    pub async fn ingest_with_provenance_and_request_context(
        &self,
        agent_id: AgentId,
        summary: &TurnSummary,
        turn: Option<&ConversationTurn>,
        request_context: RefinementRequestContext,
        active_skills: &[String],
        provenance: IngestProvenance<'_>,
    ) -> Result<WriteReport, MemoryError> {
        let mut report = WriteReport::default();

        // Stage 1: Heuristic extraction (free, no LLM)
        let candidates = self.extractor.extract(summary);
        report.candidates_extracted = candidates.len();

        if candidates.is_empty() && turn.is_none() {
            debug!("No memory candidates and no conversation turn, skipping");
            return Ok(report);
        }

        // Stage 2: LLM extraction + refinement in one call
        let memory_context = MemoryAccessContext {
            project_id: request_context.aura_project_id.clone(),
            user_id: request_context.user_id.clone(),
            include_legacy: false,
        };
        let refined = self
            .refiner
            .extract_and_refine_with_skills_and_context(
                candidates,
                turn,
                request_context,
                active_skills,
            )
            .await?;
        report.candidates_refined = refined.len();

        let origin = WriteOrigin {
            contributor_agent_id: agent_id,
            source_session_id: provenance.source_session_id,
            initial_status: provenance.initial_status,
            memory_context: &memory_context,
            extractor_model: self.refiner.model_name(),
        };
        let touched_partitions = self.write_candidates(&refined, origin, &mut report)?;
        for partition in touched_partitions {
            self.enforce_capacity(partition)?;
        }

        log_report(&report);
        Ok(report)
    }

    /// Writes every admitted candidate and returns the partitions touched.
    fn write_candidates(
        &self,
        refined: &[RefinedCandidate],
        origin: WriteOrigin<'_>,
        report: &mut WriteReport,
    ) -> Result<HashSet<AgentId>, MemoryError> {
        let mut touched_partitions = HashSet::new();
        for candidate in refined {
            if !self.admit(candidate, report) {
                continue;
            }

            let write = origin.candidate_write(candidate);
            touched_partitions.insert(write.storage_id);
            match candidate.candidate_type {
                CandidateType::Fact => self.write_fact(&write, report)?,
                CandidateType::Event => self.write_event(&write, report)?,
                CandidateType::Procedure => self.write_procedure(&write, report)?,
            }
        }
        Ok(touched_partitions)
    }

    /// Applies the drop filters in order, counting each drop in the report.
    fn admit(&self, candidate: &RefinedCandidate, report: &mut WriteReport) -> bool {
        if candidate_is_sensitive(candidate) {
            report.sensitive_dropped += 1;
            return false;
        }
        if candidate_is_unsafe(candidate) {
            report.unsafe_dropped += 1;
            return false;
        }
        if !candidate.keep || candidate.confidence < self.config.confidence_floor {
            report.candidates_dropped += 1;
            return false;
        }
        true
    }
}

fn log_report(report: &WriteReport) {
    info!(
        extracted = report.candidates_extracted,
        refined = report.candidates_refined,
        facts = report.facts_written,
        updated = report.facts_updated,
        events = report.events_written,
        procedures = report.procedures_written,
        dropped = report.candidates_dropped,
        sensitive_dropped = report.sensitive_dropped,
        unsafe_dropped = report.unsafe_dropped,
        conflicts_pending = report.conflicts_pending,
        "Memory write pipeline complete"
    );
}
