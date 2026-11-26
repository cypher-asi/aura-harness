//! Benchmark inputs: the scenario suite and external comparison runs.

use aura_context_memory::{MemoryScope, MemorySensitivity, MemoryStatus};
use serde::Deserialize;

pub(crate) const DEFAULT_SUITE: &str =
    include_str!("../../../../../evals/memory_continuity/scenarios.json");

#[derive(Debug, Deserialize)]
pub(crate) struct EvalSuite {
    pub(crate) version: u32,
    pub(crate) scenarios: Vec<Scenario>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Scenario {
    pub(crate) id: String,
    pub(crate) query: String,
    pub(crate) active_skills: Vec<String>,
    pub(crate) allow_user_scope: bool,
    #[serde(alias = "allow_workspace_scope")]
    pub(crate) allow_project_scope: bool,
    pub(crate) expected_ids: Vec<String>,
    pub(crate) records: Vec<EvalRecord>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct EvalRecord {
    pub(crate) kind: String,
    pub(crate) id: String,
    pub(crate) key: String,
    pub(crate) value: String,
    #[serde(default)]
    pub(crate) trigger: String,
    #[serde(default)]
    pub(crate) steps: Vec<String>,
    #[serde(default)]
    pub(crate) skill_name: Option<String>,
    pub(crate) importance: f32,
    pub(crate) confidence: f32,
    pub(crate) status: MemoryStatus,
    pub(crate) sensitivity: MemorySensitivity,
    pub(crate) scope: MemoryScope,
    pub(crate) pinned: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ExternalComparison {
    pub(crate) agent: String,
    pub(crate) runs: Vec<ExternalRun>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ExternalRun {
    pub(crate) scenario_id: String,
    pub(crate) recalled_ids: Vec<String>,
    #[serde(default)]
    pub(crate) estimated_tokens: usize,
    #[serde(default)]
    pub(crate) duration_ms: u64,
}
