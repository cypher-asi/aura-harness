//! Phase 3 — Evolve facts (LLM-assisted).

use super::parse::{
    extract_float_value, extract_quoted_value, parse_merge_indices, parse_single_index,
};
use super::{
    consolidatable, consolidated_continuity, run_blocking, ConsolidationReport, MemoryConsolidator,
};
use crate::error::MemoryError;
use crate::store::MemoryStoreApi;
use crate::types::{AgentEvent, Fact, FactSource};
use aura_core_types::{AgentId, FactId};
use chrono::{DateTime, Utc};
use std::fmt::Write;
use std::sync::Arc;
use tracing::debug;

impl MemoryConsolidator {
    /// LLM-assisted fact analysis: merging, contradiction resolution, insights.
    pub(super) async fn evolve_facts(
        &self,
        agent_id: AgentId,
        report: &mut ConsolidationReport,
    ) -> Result<(), MemoryError> {
        // Read phase — blocking
        let store = Arc::clone(&self.store);
        let (mut facts, mut recent_events) = {
            let s = Arc::clone(&store);
            run_blocking(move || {
                let facts = s.list_facts(agent_id)?;
                let events = s.list_events(agent_id, 50)?;
                Ok((facts, events))
            })
            .await?
        };
        facts.retain(|fact| consolidatable(&fact.continuity));
        recent_events.retain(|event| consolidatable(&event.continuity));

        if facts.len() < 2 {
            return Ok(());
        }

        // LLM phase — async
        let prompt = Self::build_evolve_prompt(&facts, &recent_events);
        let text = self.complete_text(EVOLVE_SYSTEM_PROMPT, prompt).await?;

        // Write phase — blocking (apply_evolution logic inlined)
        let s = Arc::clone(&store);
        let (merged, evolved, insights) =
            run_blocking(move || apply_evolution(&*s, &text, agent_id, &facts)).await?;

        report.facts_merged = merged;
        report.facts_evolved = evolved;
        report.insights_created = insights;

        debug!(
            %agent_id,
            merged = report.facts_merged,
            evolved = report.facts_evolved,
            insights = report.insights_created,
            "Evolve phase complete"
        );

        Ok(())
    }

    fn build_evolve_prompt(facts: &[Fact], recent_events: &[AgentEvent]) -> String {
        let mut prompt = String::from("Current facts:\n");

        for (i, fact) in facts.iter().enumerate() {
            let val = match &fact.value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let _ = writeln!(
                prompt,
                "{}. key=\"{}\" value=\"{}\" confidence={:.2} importance={:.2}",
                i + 1,
                fact.key,
                val,
                fact.confidence,
                fact.importance
            );
        }

        if !recent_events.is_empty() {
            prompt.push_str("\nRecent events:\n");
            for event in recent_events.iter().take(20) {
                let _ = writeln!(
                    prompt,
                    "- [{}] {}: {}",
                    event.timestamp.format("%Y-%m-%d"),
                    event.event_type,
                    event.summary
                );
            }
        }

        prompt.push_str(
            "\nAnalyze these facts and respond with actions, one per line:\n\
             - MERGE <N> <M> key=\"merged_key\" value=\"merged_value\" — merge fact N into M\n\
             - EVOLVE <N> value=\"new_value\" confidence=X.X — update a fact with new evidence\n\
             - INSIGHT key=\"key\" value=\"value\" — create a new cross-session insight\n\
             Only suggest changes that are clearly beneficial. It is fine to suggest nothing.",
        );

        prompt
    }
}

/// Apply LLM-recommended evolution actions to the fact store.
/// Returns (facts_merged, facts_evolved, insights_created).
fn apply_evolution(
    store: &dyn MemoryStoreApi,
    response: &str,
    agent_id: AgentId,
    facts: &[Fact],
) -> Result<(usize, usize, usize), MemoryError> {
    let mut applier = EvolutionApplier {
        store,
        agent_id,
        facts,
        now: Utc::now(),
        to_delete: Vec::new(),
        evolved_indices: Vec::new(),
        merged: 0,
        evolved: 0,
        insights: 0,
    };

    for line in response.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("MERGE ") {
            applier.merge(rest)?;
        } else if let Some(rest) = trimmed.strip_prefix("EVOLVE ") {
            applier.evolve(rest)?;
        } else if let Some(rest) = trimmed.strip_prefix("INSIGHT ") {
            applier.insight(rest)?;
        }
    }

    applier.delete_merged_sources()?;
    Ok((applier.merged, applier.evolved, applier.insights))
}

/// Running state while applying one evolution response, line by line.
struct EvolutionApplier<'a> {
    store: &'a dyn MemoryStoreApi,
    agent_id: AgentId,
    facts: &'a [Fact],
    now: DateTime<Utc>,
    /// Indices of facts merged into another fact; deleted at the end.
    to_delete: Vec<usize>,
    evolved_indices: Vec<usize>,
    merged: usize,
    evolved: usize,
    insights: usize,
}

impl EvolutionApplier<'_> {
    /// `MERGE <N> <M> key=".." value=".."`: fold fact N into fact M.
    fn merge(&mut self, rest: &str) -> Result<(), MemoryError> {
        let Some((src, dst)) = parse_merge_indices(rest, self.facts.len()) else {
            return Ok(());
        };
        if self.to_delete.contains(&src) || self.to_delete.contains(&dst) {
            return Ok(());
        }
        let Some(target) = self.facts.get(dst) else {
            return Ok(());
        };
        self.to_delete.push(src);
        let mut target = target.clone();
        if let Some(val) = extract_quoted_value(rest, "value=") {
            target.value = serde_json::Value::String(val);
        }
        if let Some(key) = extract_quoted_value(rest, "key=") {
            target.key = key;
        }
        target.source = FactSource::Consolidated;
        target.updated_at = self.now;
        self.store.put_fact(&target)?;
        self.merged += 1;
        Ok(())
    }

    /// `EVOLVE <N> value=".." confidence=X.X`: update fact N in place.
    fn evolve(&mut self, rest: &str) -> Result<(), MemoryError> {
        let Some(idx) = parse_single_index(rest, self.facts.len()) else {
            return Ok(());
        };
        if self.evolved_indices.contains(&idx) || self.to_delete.contains(&idx) {
            return Ok(());
        }
        let Some(fact) = self.facts.get(idx) else {
            return Ok(());
        };
        self.evolved_indices.push(idx);
        let mut fact = fact.clone();
        if let Some(val) = extract_quoted_value(rest, "value=") {
            fact.value = serde_json::Value::String(val);
        }
        if let Some(conf) = extract_float_value(rest, "confidence=") {
            fact.confidence = conf;
        }
        fact.source = FactSource::Consolidated;
        fact.updated_at = self.now;
        self.store.put_fact(&fact)?;
        self.evolved += 1;
        Ok(())
    }

    /// `INSIGHT key=".." value=".."`: create a new cross-session fact.
    fn insight(&mut self, rest: &str) -> Result<(), MemoryError> {
        let key = extract_quoted_value(rest, "key=");
        let value = extract_quoted_value(rest, "value=");
        let (Some(k), Some(v)) = (key, value) else {
            return Ok(());
        };
        // The caller only applies evolution with at least two facts.
        let Some(first) = self.facts.first() else {
            return Ok(());
        };
        let fact = Fact {
            fact_id: FactId::generate(),
            agent_id: self.agent_id,
            key: k,
            value: serde_json::Value::String(v),
            confidence: 0.7,
            source: FactSource::Consolidated,
            importance: 0.5,
            access_count: 0,
            last_accessed: self.now,
            created_at: self.now,
            updated_at: self.now,
            continuity: consolidated_continuity(&first.continuity),
        };
        self.store.put_fact(&fact)?;
        self.insights += 1;
        Ok(())
    }

    fn delete_merged_sources(&self) -> Result<(), MemoryError> {
        for fact in self.to_delete.iter().filter_map(|&idx| self.facts.get(idx)) {
            self.store.delete_fact(self.agent_id, fact.fact_id)?;
        }
        Ok(())
    }
}

const EVOLVE_SYSTEM_PROMPT: &str = "\
You are a memory analyst for an AI agent. Given the agent's current facts and \
recent events, identify: (1) facts that should be merged because they describe \
the same thing, (2) facts that should be updated based on new evidence, and \
(3) new insights derivable from cross-session patterns. Respond with one action \
per line using the exact formats specified. Be conservative — only suggest \
changes with clear benefit.";
