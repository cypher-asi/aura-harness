use super::arm::CompareArm;
use super::run::is_zero_calls;
use serde::{Deserialize, Serialize};

/// How the judge rated an attempt against the reference: what the user
/// kept from the session that was replayed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RefVerdict {
    Better,
    AsGood,
    Worse,
}

impl RefVerdict {
    /// `better`, `as good`, `worse`.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Better => "better",
            Self::AsGood => "as good",
            Self::Worse => "worse",
        }
    }
}

/// The blind judge's verdict on one run of arm A paired with one run of
/// arm B.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompareJudgment {
    /// Name of the arm A run.
    #[serde(alias = "gate_run")]
    pub a_run: String,
    /// Name of the arm B run.
    #[serde(alias = "other_run")]
    pub b_run: String,
    /// Whether the judge saw the arm A run as attempt A.
    #[serde(alias = "gate_first")]
    pub a_first: bool,
    /// The better arm; `None` for a tie. Meaningless when not `parsed`.
    pub winner: Option<CompareArm>,
    /// Whether the judge gave a verdict that could be read.
    pub parsed: bool,
    /// How the arm A run compares with the reference, when there is one
    /// and the judge said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub a_vs_reference: Option<RefVerdict>,
    /// How the arm B run compares with the reference.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b_vs_reference: Option<RefVerdict>,
    /// Model that judged.
    pub model: String,
    /// List-price cost of the judge call; 0 when its price is unknown,
    /// which `unpriced_calls` says.
    pub cost_usd: f64,
    /// Judge calls with no known list price, which `cost_usd` leaves
    /// out: 1 or 0; 0 in records written before they were counted.
    #[serde(default, skip_serializing_if = "is_zero_calls")]
    pub unpriced_calls: u32,
    /// The judge's reasoning, cut short.
    pub reasoning: String,
}

impl CompareJudgment {
    /// How `arm`'s run compares with the reference.
    #[must_use]
    pub const fn vs_reference(&self, arm: CompareArm) -> Option<RefVerdict> {
        match arm {
            CompareArm::A => self.a_vs_reference,
            CompareArm::B => self.b_vs_reference,
        }
    }
}
