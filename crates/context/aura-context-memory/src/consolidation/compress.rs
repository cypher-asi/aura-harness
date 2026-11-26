//! Phase 2 — Compress events (LLM-assisted).

use super::{
    consolidatable, consolidated_continuity, run_blocking, ConsolidationReport, MemoryConsolidator,
};
use crate::error::MemoryError;
use crate::store::MemoryStoreApi;
use crate::types::{AgentEvent, MemoryContinuity};
use aura_core_types::{AgentEventId, AgentId};
use chrono::Utc;
use std::fmt::Write;
use std::sync::Arc;
use tracing::debug;

/// The oldest events past the compression threshold.
struct CompressionBatch {
    oldest: Vec<AgentEvent>,
    overflow: usize,
    summary_continuity: MemoryContinuity,
}

impl MemoryConsolidator {
    /// LLM-assisted compression of oldest episodic events into summaries.
    pub(super) async fn compress_events(
        &self,
        agent_id: AgentId,
        report: &mut ConsolidationReport,
    ) -> Result<(), MemoryError> {
        // Read phase — blocking
        let Some(batch) = self.load_compression_batch(agent_id).await? else {
            return Ok(());
        };

        // LLM phase — async
        let prompt = Self::build_compress_prompt(&batch.oldest.iter().collect::<Vec<_>>());
        let text = self.complete_text(COMPRESS_SYSTEM_PROMPT, prompt).await?;
        let summaries = Self::parse_compress_response(&text);

        if summaries.is_empty() {
            return Ok(());
        }

        // Write phase — blocking
        let store = Arc::clone(&self.store);
        let (compressed, deleted) =
            run_blocking(move || write_compressed(&*store, agent_id, &summaries, &batch)).await?;

        report.events_compressed = compressed;
        report.events_deleted = deleted;
        debug!(%agent_id, compressed, deleted, "Compress phase complete");
        Ok(())
    }

    /// Load the consolidatable events and take the oldest ones past the
    /// threshold; `None` when there is nothing to compress.
    async fn load_compression_batch(
        &self,
        agent_id: AgentId,
    ) -> Result<Option<CompressionBatch>, MemoryError> {
        let store = Arc::clone(&self.store);
        let threshold = self.config.max_events_before_compression;
        let mut all_events = run_blocking(move || store.list_events(agent_id, 100_000)).await?;
        all_events.retain(|event| consolidatable(&event.continuity));

        if all_events.len() <= threshold {
            return Ok(None);
        }

        let overflow = all_events.len() - threshold;
        let oldest: Vec<AgentEvent> = all_events.into_iter().rev().take(overflow).collect();
        let Some(first) = oldest.first() else {
            return Ok(None);
        };
        let summary_continuity = consolidated_continuity(&first.continuity);
        Ok(Some(CompressionBatch {
            oldest,
            overflow,
            summary_continuity,
        }))
    }

    pub(super) fn build_compress_prompt(events: &[&AgentEvent]) -> String {
        let mut prompt = String::from(
            "Summarize the following episodic events into a few higher-level summaries.\n\
             Each summary should capture the key outcome or pattern from a group of related events.\n\
             Respond with one summary per line, prefixed with \"SUMMARY: \".\n\nEvents:\n",
        );

        for (i, event) in events.iter().enumerate() {
            let _ = writeln!(
                prompt,
                "{}. [{}] {}: {}",
                i + 1,
                event.timestamp.format("%Y-%m-%d"),
                event.event_type,
                event.summary
            );
        }

        prompt
    }

    pub(super) fn parse_compress_response(response: &str) -> Vec<String> {
        response
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                trimmed
                    .strip_prefix("SUMMARY: ")
                    .or_else(|| trimmed.strip_prefix("SUMMARY:"))
                    .map(|s| s.trim().to_string())
            })
            .filter(|s| !s.is_empty())
            .collect()
    }
}

/// Store the summary events and delete the events they replace.
/// Returns (summaries written, events deleted).
fn write_compressed(
    store: &dyn MemoryStoreApi,
    agent_id: AgentId,
    summaries: &[String],
    batch: &CompressionBatch,
) -> Result<(usize, usize), MemoryError> {
    let now = Utc::now();
    for summary in summaries {
        let event = AgentEvent {
            event_id: AgentEventId::generate(),
            agent_id,
            event_type: "consolidated_summary".to_string(),
            summary: summary.clone(),
            metadata: serde_json::json!({
                "source": "consolidation",
                "original_count": batch.overflow
            }),
            importance: 0.6,
            access_count: 0,
            last_accessed: now,
            timestamp: now,
            continuity: batch.summary_continuity.clone(),
        };
        store.put_event(&event)?;
    }
    for event in &batch.oldest {
        store.delete_event_direct(agent_id, event.timestamp, event.event_id)?;
    }
    Ok((summaries.len(), batch.oldest.len()))
}

const COMPRESS_SYSTEM_PROMPT: &str = "\
You are a memory consolidator for an AI agent. Given a list of episodic events \
from past sessions, compress them into a smaller set of higher-level summaries \
that preserve key outcomes, patterns, and lessons learned. Respond with one \
summary per line, each prefixed with \"SUMMARY: \". Focus on actionable \
knowledge and significant outcomes.";
