use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a reward scores.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum RewardTarget {
    /// One step.
    Step {
        /// Step sequence number.
        seq: u64,
    },
    /// The whole run.
    Run,
}

/// Kind of reward signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewardKind {
    /// Task verification result.
    Outcome,
    /// Local progress signal (edit applied, build fixed, redundant read).
    Process,
    /// Money or time spent.
    Cost,
    /// Tokens spent.
    Tokens,
    /// Whether a harness decision changed behavior.
    DecisionEffect,
    /// Explicit human label.
    HumanExplicit,
    /// Inferred from human behavior (revert, interrupt, commit).
    HumanImplicit,
}

impl RewardKind {
    /// Stable snake-case name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Outcome => "outcome",
            Self::Process => "process",
            Self::Cost => "cost",
            Self::Tokens => "tokens",
            Self::DecisionEffect => "decision_effect",
            Self::HumanExplicit => "human_explicit",
            Self::HumanImplicit => "human_implicit",
        }
    }
}

/// A scalar label on a step or run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reward {
    /// What is scored.
    pub target: RewardTarget,
    /// Signal kind.
    pub kind: RewardKind,
    /// Short name of the specific signal, e.g. `edit_applied`.
    pub name: String,
    /// Score; positive is good.
    pub value: f64,
    /// Labeler that produced it.
    pub source: String,
    /// Unix milliseconds.
    pub at_ms: u64,
    /// Supporting data.
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub evidence: Value,
}
