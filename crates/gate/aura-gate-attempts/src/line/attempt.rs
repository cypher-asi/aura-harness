//! An `attempt` line: one model's work on a prompt, and what it left
//! behind.

use aura_core_types::{Difficulty, OutcomePayload, OutcomeSignal, PromptCategory};
use aura_gate_evidence::Evidence;
use serde::de::value::StrDeserializer;
use serde::{Deserialize, Deserializer, Serialize};

/// One model's work on a prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Attempt {
    /// When the line was written, in Unix milliseconds
    pub at_ms: u64,
    /// The agent that ran it, in hex
    pub agent: String,
    /// Hash of the prompt it answers, in hex: the user's, or the one
    /// recorded when Gate moved the turn to this model
    pub prompt_tx: String,
    /// What the prompt asked for
    pub category: PromptCategory,
    /// How hard the classifier judged it, when it said
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<Difficulty>,
    /// Whether the agent led it, handing its work to subtasks
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub led: Option<bool>,
    /// Whether Gate chose the model
    pub routed: bool,
    /// The model that did the work
    pub model: String,
    /// What Gate recorded of how it went, after any follow-up
    #[serde(deserialize_with = "stored_signal")]
    pub signal: OutcomeSignal,
    /// Whether that counts as success; absent when nothing said
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub success: Option<bool>,
    /// List-price cost of its model calls in USD
    pub cost_usd: f64,
    /// List-price cost of its subtasks' model calls in USD
    #[serde(default)]
    pub subtask_cost_usd: f64,
    /// Its wall time
    pub latency_ms: u64,
    /// What it left behind
    pub evidence: Evidence,
    /// Whether the line was rebuilt from the record and a saved session
    /// rather than written as the attempt ended
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub backfilled: bool,
}

/// A stored attempt's signal. Older logs wrote `none` for work that
/// ended with nothing to say how it went, which reads as
/// [`OutcomeSignal::Unavailable`]: it counts neither way.
fn stored_signal<'de, D>(deserializer: D) -> Result<OutcomeSignal, D::Error>
where
    D: Deserializer<'de>,
{
    let name = String::deserialize(deserializer)?;
    if name == "none" {
        return Ok(OutcomeSignal::Unavailable);
    }
    OutcomeSignal::deserialize(StrDeserializer::new(&name))
}

/// What an attempt ran under, the part its `Outcome` does not hold.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Terms {
    /// How hard the classifier judged the prompt
    pub difficulty: Option<Difficulty>,
    /// Whether the agent led it
    pub led: Option<bool>,
    /// Whether Gate chose the model
    pub routed: bool,
}

impl Attempt {
    /// The attempt of `agent` (in hex) whose outcome is `outcome`.
    #[must_use]
    pub fn new(
        at_ms: u64,
        agent: String,
        outcome: &OutcomePayload,
        terms: Terms,
        evidence: Evidence,
    ) -> Self {
        Self {
            at_ms,
            agent,
            prompt_tx: outcome.prompt_tx.to_hex(),
            category: outcome.category,
            difficulty: terms.difficulty,
            led: terms.led,
            routed: terms.routed,
            model: outcome.model.clone(),
            signal: outcome.signal,
            success: outcome.success,
            cost_usd: outcome.cost_usd,
            subtask_cost_usd: outcome.subtask_cost_usd,
            latency_ms: outcome.latency_ms,
            evidence,
            backfilled: false,
        }
    }

    /// Whether Gate let the attempt stand: it did not count it as failed.
    pub(crate) fn stood(&self) -> bool {
        self.success != Some(false)
    }
}
