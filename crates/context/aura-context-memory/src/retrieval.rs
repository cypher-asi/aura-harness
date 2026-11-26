//! Query-aware retrieval of memory for system-prompt injection.

use crate::error::MemoryError;
use crate::safety::unsafe_for_prompt;
use crate::salience;
use crate::store::MemoryStoreApi;
use crate::types::{
    AgentEvent, Fact, MemoryAccessContext, MemoryPacket, MemoryRetrievalMode, MemoryRetrievalTrace,
    MemoryScope, MemorySelection, MemorySensitivity, MemoryStatus, Procedure,
};
use aura_core_types::AgentId;
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Instant;

/// Current-turn signal used to choose relevant memories.
#[derive(Debug, Clone, Default)]
pub struct MemoryQueryContext {
    pub text: String,
    pub active_skills: Vec<String>,
    pub allow_user_scope: bool,
    pub allow_project_scope: bool,
    pub access: MemoryAccessContext,
}

/// Retrieves and ranks agent memory for system-prompt injection.
pub struct MemoryRetriever {
    store: Arc<dyn MemoryStoreApi>,
    config: RetrievalConfig,
}

/// Configuration for memory retrieval, scoring, and budget enforcement.
#[derive(Debug, Clone)]
pub struct RetrievalConfig {
    pub max_facts: usize,
    pub max_events: usize,
    pub max_procedures: usize,
    pub min_confidence: f32,
    /// Maximum estimated tokens for the memory injection.
    pub token_budget: usize,
}

impl Default for RetrievalConfig {
    fn default() -> Self {
        Self {
            max_facts: 12,
            max_events: 6,
            max_procedures: 4,
            min_confidence: 0.3,
            token_budget: 800,
        }
    }
}

impl MemoryRetriever {
    #[must_use]
    pub fn new(store: Arc<dyn MemoryStoreApi>, config: RetrievalConfig) -> Self {
        Self { store, config }
    }

    /// Backward-compatible salience retrieval without a turn query.
    pub async fn retrieve(&self, agent_id: AgentId) -> Result<MemoryPacket, MemoryError> {
        self.retrieve_with_query(
            agent_id,
            MemoryQueryContext::default(),
            MemoryRetrievalMode::Salience,
        )
        .await
    }

    /// Retrieve a budgeted packet using current-turn relevance plus durable
    /// salience. The trace contains IDs and scores but never the query text.
    pub async fn retrieve_with_query(
        &self,
        agent_id: AgentId,
        query: MemoryQueryContext,
        mode: MemoryRetrievalMode,
    ) -> Result<MemoryPacket, MemoryError> {
        let store = Arc::clone(&self.store);
        let config = self.config.clone();
        tokio::task::spawn_blocking(move || {
            retrieve_blocking(store.as_ref(), &config, agent_id, &query, mode)
        })
        .await
        .map_err(|e| MemoryError::BlockingTaskFailed(e.to_string()))?
    }
}

/// A memory item with its final score and its lexical relevance.
type Scored<T> = (T, f32, f32);

/// The current-turn signal derived from a query, shared by every scorer.
struct QuerySignal {
    tokens: HashSet<String>,
    query_aware: bool,
    active_skills: HashSet<String>,
}

impl QuerySignal {
    fn new(query: &MemoryQueryContext, mode: MemoryRetrievalMode) -> Self {
        let tokens = tokenize(&query.text);
        let query_aware = mode == MemoryRetrievalMode::QueryAware && !tokens.is_empty();
        let active_skills = query
            .active_skills
            .iter()
            .map(|skill| skill.to_ascii_lowercase())
            .collect();
        Self {
            tokens,
            query_aware,
            active_skills,
        }
    }

    fn relevance(&self, document: &str) -> f32 {
        if self.query_aware {
            lexical_relevance(&self.tokens, document)
        } else {
            0.0
        }
    }
}

/// Memories of every kind, as loaded from the readable partitions.
struct MemorySet {
    facts: Vec<Fact>,
    events: Vec<AgentEvent>,
    procedures: Vec<Procedure>,
}

impl MemorySet {
    fn len(&self) -> usize {
        self.facts.len() + self.events.len() + self.procedures.len()
    }

    /// Drop memories that are below the confidence floor, not visible to
    /// this query, or unsafe to put in a prompt.
    fn retain_eligible(&mut self, config: &RetrievalConfig, query: &MemoryQueryContext) {
        self.facts.retain(|fact| {
            fact.confidence >= config.min_confidence
                && eligible(&fact.continuity, query)
                && !unsafe_for_prompt(&fact_text(fact))
        });
        self.events.retain(|event| {
            eligible(&event.continuity, query) && !unsafe_for_prompt(&event_text(event))
        });
        self.procedures.retain(|procedure| {
            eligible(&procedure.continuity, query) && !unsafe_for_prompt(&procedure_text(procedure))
        });
    }
}

/// Ranked, per-kind candidates before the token budget is applied.
struct RankedMemories {
    facts: Vec<Scored<Fact>>,
    events: Vec<Scored<AgentEvent>>,
    procedures: Vec<Scored<Procedure>>,
}

/// The memories that fit the token budget, plus why each was chosen.
struct BudgetedSelection {
    memories: MemorySet,
    selections: Vec<MemorySelection>,
    remaining_budget: usize,
}

fn retrieve_blocking(
    store: &dyn MemoryStoreApi,
    config: &RetrievalConfig,
    agent_id: AgentId,
    query: &MemoryQueryContext,
    mode: MemoryRetrievalMode,
) -> Result<MemoryPacket, MemoryError> {
    let started = Instant::now();
    let now = Utc::now();
    let signal = QuerySignal::new(query, mode);

    let mut memories = load_memories(store, config, agent_id, query)?;
    let candidate_count = memories.len();
    memories.retain_eligible(config, query);

    let ranked = rank_memories(memories, &signal, config, now);
    let selected = select_within_budget(ranked, config.token_budget);

    for fact in &selected.memories.facts {
        store.touch_fact(fact.agent_id, fact.fact_id)?;
    }

    let estimated_tokens = config
        .token_budget
        .saturating_sub(selected.remaining_budget);
    let trace = MemoryRetrievalTrace {
        candidate_count,
        selected_count: selected.selections.len(),
        estimated_tokens,
        duration_ms: started.elapsed().as_millis().try_into().unwrap_or(u64::MAX),
        query_aware: signal.query_aware,
        selections: selected.selections,
    };

    Ok(MemoryPacket {
        facts: selected.memories.facts,
        events: selected.memories.events,
        procedures: selected.memories.procedures,
        trace: Some(trace),
    })
}

fn load_memories(
    store: &dyn MemoryStoreApi,
    config: &RetrievalConfig,
    agent_id: AgentId,
    query: &MemoryQueryContext,
) -> Result<MemorySet, MemoryError> {
    let partitions = query.access.readable_partitions(
        agent_id,
        query.allow_user_scope,
        query.allow_project_scope,
    );
    let mut memories = MemorySet {
        facts: Vec::new(),
        events: Vec::new(),
        procedures: Vec::new(),
    };
    for partition in partitions {
        memories.facts.extend(store.list_facts(partition)?);
        memories
            .events
            .extend(store.list_events(partition, config.max_events.saturating_mul(10).max(50))?);
        memories
            .procedures
            .extend(store.list_procedures(partition)?);
    }
    Ok(memories)
}

fn rank_memories(
    memories: MemorySet,
    signal: &QuerySignal,
    config: &RetrievalConfig,
    now: DateTime<Utc>,
) -> RankedMemories {
    let mut facts: Vec<_> = memories
        .facts
        .into_iter()
        .filter_map(|fact| score_fact(fact, signal, now))
        .collect();
    facts.sort_by(score_order);
    facts.truncate(config.max_facts);

    let mut events: Vec<_> = memories
        .events
        .into_iter()
        .filter_map(|event| score_event(event, signal, now))
        .collect();
    events.sort_by(score_order);
    events.truncate(config.max_events);

    let mut procedures: Vec<_> = memories
        .procedures
        .into_iter()
        .filter_map(|procedure| score_procedure(procedure, signal, now))
        .collect();
    procedures.sort_by(score_order);
    procedures.truncate(config.max_procedures);

    RankedMemories {
        facts,
        events,
        procedures,
    }
}

fn score_fact(fact: Fact, signal: &QuerySignal, now: DateTime<Utc>) -> Option<Scored<Fact>> {
    let relevance = signal.relevance(&fact_text(&fact));
    if signal.query_aware && relevance == 0.0 && !fact.continuity.pinned {
        return None;
    }
    let base = salience::score_fact(&fact, now);
    let pinned = if fact.continuity.pinned { 0.75 } else { 0.0 };
    let score = if signal.query_aware {
        relevance.mul_add(0.65, base * 0.35) + pinned
    } else {
        base + pinned
    };
    Some((fact, score, relevance))
}

fn score_event(
    event: AgentEvent,
    signal: &QuerySignal,
    now: DateTime<Utc>,
) -> Option<Scored<AgentEvent>> {
    let relevance = signal.relevance(&event_text(&event));
    if signal.query_aware && relevance == 0.0 && event.importance < 0.85 {
        return None;
    }
    let base = salience::score_event(&event, now);
    let score = if signal.query_aware {
        relevance.mul_add(0.7, base * 0.3)
    } else {
        base
    };
    Some((event, score, relevance))
}

fn score_procedure(
    procedure: Procedure,
    signal: &QuerySignal,
    now: DateTime<Utc>,
) -> Option<Scored<Procedure>> {
    let mut relevance = signal.relevance(&procedure_text(&procedure));
    if procedure
        .skill_name
        .as_ref()
        .is_some_and(|skill| signal.active_skills.contains(&skill.to_ascii_lowercase()))
    {
        relevance = (relevance + 0.5).min(1.0);
    }
    if signal.query_aware && relevance == 0.0 && !procedure.continuity.pinned {
        return None;
    }
    let base = salience::score_procedure(&procedure, now);
    let pinned = if procedure.continuity.pinned {
        0.75
    } else {
        0.0
    };
    let score = if signal.query_aware {
        relevance.mul_add(0.7, base * 0.3) + pinned
    } else {
        base + pinned
    };
    Some((procedure, score, relevance))
}

/// Fill the token budget with facts first, then events, then procedures.
fn select_within_budget(ranked: RankedMemories, token_budget: usize) -> BudgetedSelection {
    let mut budget = token_budget;
    let mut selections = Vec::new();
    let facts = select_budgeted(
        ranked.facts,
        &mut budget,
        salience::estimate_fact_tokens,
        "fact",
        |fact| fact.fact_id.to_hex(),
        |fact| fact.continuity.scope,
        &mut selections,
    );
    let events = select_budgeted(
        ranked.events,
        &mut budget,
        salience::estimate_event_tokens,
        "event",
        |event| event.event_id.to_hex(),
        |event| event.continuity.scope,
        &mut selections,
    );
    let procedures = select_budgeted(
        ranked.procedures,
        &mut budget,
        salience::estimate_procedure_tokens,
        "procedure",
        |procedure| procedure.procedure_id.to_hex(),
        |procedure| procedure.continuity.scope,
        &mut selections,
    );
    BudgetedSelection {
        memories: MemorySet {
            facts,
            events,
            procedures,
        },
        selections,
        remaining_budget: budget,
    }
}

fn eligible(continuity: &crate::types::MemoryContinuity, query: &MemoryQueryContext) -> bool {
    continuity.status == MemoryStatus::Active
        && continuity.sensitivity == MemorySensitivity::Normal
        && match continuity.scope {
            MemoryScope::Agent => true,
            MemoryScope::User => query.allow_user_scope,
            MemoryScope::Project => query.allow_project_scope,
        }
}

fn score_order<T>(a: &Scored<T>, b: &Scored<T>) -> std::cmp::Ordering {
    b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
}

fn select_budgeted<T>(
    items: Vec<Scored<T>>,
    budget: &mut usize,
    estimator: fn(&T) -> usize,
    kind: &str,
    id: fn(&T) -> String,
    scope_of: fn(&T) -> MemoryScope,
    selections: &mut Vec<MemorySelection>,
) -> Vec<T> {
    let mut result = Vec::new();
    for (item, score, relevance) in items {
        let tokens = estimator(&item);
        if tokens > *budget {
            continue;
        }
        *budget -= tokens;
        selections.push(MemorySelection {
            memory_id: id(&item),
            kind: kind.to_string(),
            score,
            relevance,
            reason: if relevance > 0.0 {
                "current_request".to_string()
            } else {
                "durable_salience".to_string()
            },
            scope: scope_of(&item),
        });
        result.push(item);
    }
    result
}

fn fact_text(fact: &Fact) -> String {
    format!("{} {}", fact.key, fact.value)
}

fn event_text(event: &AgentEvent) -> String {
    format!("{} {} {}", event.event_type, event.summary, event.metadata)
}

fn procedure_text(procedure: &Procedure) -> String {
    format!(
        "{} {} {} {}",
        procedure.name,
        procedure.trigger,
        procedure.steps.join(" "),
        procedure.skill_name.as_deref().unwrap_or_default()
    )
}

#[allow(clippy::cast_precision_loss)] // token sets are bounded far below f32's exact integer range
fn lexical_relevance(query: &HashSet<String>, document: &str) -> f32 {
    if query.is_empty() {
        return 0.0;
    }
    let document = tokenize(document);
    if document.is_empty() {
        return 0.0;
    }
    let overlap = query.intersection(&document).count() as f32;
    overlap / ((query.len() as f32 * document.len() as f32).sqrt().max(1.0))
}

fn tokenize(text: &str) -> HashSet<String> {
    const STOP_WORDS: &[&str] = &[
        "a", "an", "and", "are", "as", "at", "be", "by", "for", "from", "i", "in", "is", "it",
        "my", "of", "on", "or", "that", "the", "this", "to", "we", "what", "when", "with", "you",
    ];
    // Memory keys and procedure names commonly use snake_case/kebab-case,
    // while user requests use spaces. Split all punctuation so those forms
    // share the same semantic tokens.
    text.split(|c: char| !c.is_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|token| token.len() > 1 && !STOP_WORDS.contains(&token.as_str()))
        .collect()
}

#[cfg(test)]
mod query_tests {
    use super::*;

    #[test]
    fn lexical_relevance_prefers_matching_meaning_tokens() {
        let query = tokenize("Which test command should I run?");
        let matching = lexical_relevance(&query, "test_command npm test");
        let unrelated = lexical_relevance(&query, "favorite color blue");
        assert!(matching > unrelated);
    }

    #[test]
    fn tokenizer_removes_common_stop_words() {
        let tokens = tokenize("What is the build command for this project?");
        assert!(tokens.contains("build"));
        assert!(tokens.contains("command"));
        assert!(!tokens.contains("the"));
    }
}
