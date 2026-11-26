//! Per-scenario scoring and per-agent aggregation.

use std::collections::{HashMap, HashSet};

use aura_context_memory::{MemoryScope, MemorySensitivity, MemoryStatus};
use serde::Serialize;

use crate::suite::{EvalSuite, ExternalComparison, ExternalRun, Scenario};

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ScenarioResult {
    scenario_id: String,
    recalled_ids: Vec<String>,
    expected_ids: Vec<String>,
    relevant_selected: usize,
    forbidden_selected: Vec<String>,
    estimated_tokens: usize,
    duration_ms: u64,
}

#[derive(Debug, Serialize)]
pub(crate) struct AgentReport {
    agent: String,
    scenario_count: usize,
    pub(crate) recall_at_k: f64,
    pub(crate) precision_at_k: f64,
    mean_reciprocal_rank: f64,
    pub(crate) forbidden_recall_rate: f64,
    pub(crate) average_estimated_tokens: f64,
    p95_duration_ms: u64,
    runs: Vec<ScenarioResult>,
}

fn forbidden_ids(scenario: &Scenario) -> HashSet<String> {
    scenario
        .records
        .iter()
        .filter(|record| {
            record.status != MemoryStatus::Active
                || record.sensitivity == MemorySensitivity::Sensitive
                || (record.scope == MemoryScope::User && !scenario.allow_user_scope)
                || (record.scope == MemoryScope::Project && !scenario.allow_project_scope)
        })
        .map(|record| record.id.clone())
        .collect()
}

pub(crate) fn score_scenario(
    scenario: &Scenario,
    recalled_ids: Vec<String>,
    estimated_tokens: usize,
    duration_ms: u64,
) -> ScenarioResult {
    let expected: HashSet<&str> = scenario.expected_ids.iter().map(String::as_str).collect();
    let forbidden = forbidden_ids(scenario);
    ScenarioResult {
        scenario_id: scenario.id.clone(),
        relevant_selected: recalled_ids
            .iter()
            .filter(|id| expected.contains(id.as_str()))
            .count(),
        forbidden_selected: recalled_ids
            .iter()
            .filter(|id| forbidden.contains(id.as_str()))
            .cloned()
            .collect(),
        recalled_ids,
        expected_ids: scenario.expected_ids.clone(),
        estimated_tokens,
        duration_ms,
    }
}

pub(crate) fn aggregate(agent: String, runs: Vec<ScenarioResult>) -> AgentReport {
    let scenario_count = runs.len();
    let expected_count: usize = runs.iter().map(|run| run.expected_ids.len()).sum();
    let relevant_count: usize = runs.iter().map(|run| run.relevant_selected).sum();
    let selected_count: usize = runs.iter().map(|run| run.recalled_ids.len()).sum();
    let forbidden_count: usize = runs.iter().map(|run| run.forbidden_selected.len()).sum();
    let reciprocal_rank_sum: f64 = runs
        .iter()
        .map(|run| {
            run.recalled_ids
                .iter()
                .position(|id| run.expected_ids.contains(id))
                .map_or(0.0, |rank| 1.0 / (rank + 1) as f64)
        })
        .sum();
    let mut durations: Vec<u64> = runs.iter().map(|run| run.duration_ms).collect();
    durations.sort_unstable();
    let p95_index = durations.len().saturating_sub(1) * 95 / 100;

    AgentReport {
        agent,
        scenario_count,
        recall_at_k: relevant_count as f64 / expected_count.max(1) as f64,
        precision_at_k: relevant_count as f64 / selected_count.max(1) as f64,
        mean_reciprocal_rank: reciprocal_rank_sum / scenario_count.max(1) as f64,
        forbidden_recall_rate: forbidden_count as f64 / selected_count.max(1) as f64,
        average_estimated_tokens: runs.iter().map(|run| run.estimated_tokens).sum::<usize>() as f64
            / scenario_count.max(1) as f64,
        p95_duration_ms: durations.get(p95_index).copied().unwrap_or_default(),
        runs,
    }
}

pub(crate) fn score_external(suite: &EvalSuite, comparison: ExternalComparison) -> AgentReport {
    let by_scenario: HashMap<String, ExternalRun> = comparison
        .runs
        .into_iter()
        .map(|run| (run.scenario_id.clone(), run))
        .collect();
    let runs = suite
        .scenarios
        .iter()
        .map(|scenario| {
            let run = by_scenario.get(&scenario.id);
            score_scenario(
                scenario,
                run.map(|item| item.recalled_ids.clone())
                    .unwrap_or_default(),
                run.map_or(0, |item| item.estimated_tokens),
                run.map_or(0, |item| item.duration_ms),
            )
        })
        .collect();
    aggregate(comparison.agent, runs)
}
