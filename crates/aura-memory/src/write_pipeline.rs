//! Orchestrates Stage 1 (heuristic) -> Stage 2 (LLM) -> dedup -> write.

use crate::error::MemoryError;
use crate::extraction::HeuristicExtractor;
use crate::refinement::LlmRefiner;
use crate::store::MemoryStore;
use crate::types::{AgentEvent, CandidateType, Fact, FactSource, RefinedCandidate};
use aura_agent::AgentLoopResult;
use aura_core::{AgentEventId, AgentId, FactId};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, info};

pub struct MemoryWritePipeline {
    store: Arc<MemoryStore>,
    extractor: HeuristicExtractor,
    refiner: LlmRefiner,
    config: WriteConfig,
}

#[derive(Debug, Clone)]
pub struct WriteConfig {
    pub confidence_floor: f32,
    pub max_facts_per_agent: usize,
    pub max_events_per_agent: usize,
}

impl Default for WriteConfig {
    fn default() -> Self {
        Self {
            confidence_floor: 0.5,
            max_facts_per_agent: 100,
            max_events_per_agent: 500,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WriteReport {
    pub candidates_extracted: usize,
    pub candidates_refined: usize,
    pub facts_written: usize,
    pub facts_updated: usize,
    pub events_written: usize,
    pub candidates_dropped: usize,
}

impl MemoryWritePipeline {
    #[must_use]
    pub const fn new(store: Arc<MemoryStore>, refiner: LlmRefiner, config: WriteConfig) -> Self {
        Self {
            store,
            extractor: HeuristicExtractor,
            refiner,
            config,
        }
    }

    /// Ingest an `AgentLoopResult` through the two-stage pipeline.
    ///
    /// # Errors
    /// Returns error on extraction, refinement, or storage failure.
    pub async fn ingest(
        &self,
        agent_id: AgentId,
        result: &AgentLoopResult,
    ) -> Result<WriteReport, MemoryError> {
        let mut report = WriteReport::default();

        // Stage 1: Heuristic extraction
        let candidates = self.extractor.extract(result);
        report.candidates_extracted = candidates.len();

        if candidates.is_empty() {
            debug!("No memory candidates extracted, skipping refinement");
            return Ok(report);
        }

        // Stage 2: LLM refinement
        let refined = self.refiner.refine(candidates).await?;
        report.candidates_refined = refined.len();

        for candidate in &refined {
            if !candidate.keep {
                report.candidates_dropped += 1;
                continue;
            }
            if candidate.confidence < self.config.confidence_floor {
                report.candidates_dropped += 1;
                continue;
            }

            match candidate.candidate_type {
                CandidateType::Fact => {
                    self.write_fact(agent_id, candidate, &mut report)?;
                }
                CandidateType::Event => {
                    self.write_event(agent_id, candidate, &mut report)?;
                }
                CandidateType::Procedure => {
                    // Procedural memory write — deferred to a later phase.
                }
            }
        }

        self.enforce_capacity(agent_id)?;

        info!(
            extracted = report.candidates_extracted,
            refined = report.candidates_refined,
            facts = report.facts_written,
            updated = report.facts_updated,
            events = report.events_written,
            dropped = report.candidates_dropped,
            "Memory write pipeline complete"
        );

        Ok(report)
    }

    fn write_fact(
        &self,
        agent_id: AgentId,
        candidate: &RefinedCandidate,
        report: &mut WriteReport,
    ) -> Result<(), MemoryError> {
        let now = Utc::now();

        if let Ok(Some(mut existing)) = self.store.get_fact_by_key(agent_id, &candidate.key) {
            existing.value = candidate.value.clone();
            existing.confidence = candidate.confidence;
            existing.importance = candidate.importance;
            existing.updated_at = now;
            self.store.put_fact(&existing)?;
            report.facts_updated += 1;
        } else {
            let fact = Fact {
                fact_id: FactId::generate(),
                agent_id,
                key: candidate.key.clone(),
                value: candidate.value.clone(),
                confidence: candidate.confidence,
                source: FactSource::Extracted,
                importance: candidate.importance,
                access_count: 0,
                last_accessed: now,
                created_at: now,
                updated_at: now,
            };
            self.store.put_fact(&fact)?;
            report.facts_written += 1;
        }

        Ok(())
    }

    fn write_event(
        &self,
        agent_id: AgentId,
        candidate: &RefinedCandidate,
        report: &mut WriteReport,
    ) -> Result<(), MemoryError> {
        let now = Utc::now();

        let event = AgentEvent {
            event_id: AgentEventId::generate(),
            agent_id,
            event_type: "task_run".to_string(),
            summary: candidate.summary.clone().unwrap_or_default(),
            metadata: candidate.value.clone(),
            importance: candidate.importance,
            access_count: 0,
            last_accessed: now,
            timestamp: now,
        };
        self.store.put_event(&event)?;
        report.events_written += 1;

        Ok(())
    }

    fn enforce_capacity(&self, agent_id: AgentId) -> Result<(), MemoryError> {
        let mut facts = self.store.list_facts(agent_id)?;
        if facts.len() > self.config.max_facts_per_agent {
            facts.sort_by(|a, b| {
                a.importance
                    .partial_cmp(&b.importance)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let to_remove = facts.len() - self.config.max_facts_per_agent;
            for fact in facts.iter().take(to_remove) {
                self.store.delete_fact(agent_id, fact.fact_id)?;
            }
        }

        let overflow_buffer = 100;
        let events = self
            .store
            .list_events(agent_id, self.config.max_events_per_agent + overflow_buffer)?;
        if events.len() > self.config.max_events_per_agent {
            for event in events.iter().skip(self.config.max_events_per_agent) {
                self.store.delete_event(agent_id, event.event_id)?;
            }
        }

        Ok(())
    }
}
