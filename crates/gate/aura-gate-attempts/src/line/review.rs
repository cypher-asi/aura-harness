//! A `review` line: what Gate's judge made of an attempt that was about
//! to stand.

use aura_gate_evidence::Flag;
use aura_gate_judge::criteria::Checked;
use aura_gate_judge::turn::Answer;
use serde::{Deserialize, Serialize};

/// What Gate's judge made of an attempt that was about to stand.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Review {
    /// When the line was written, in Unix milliseconds
    pub at_ms: u64,
    /// The attempt's agent, in hex
    pub agent: String,
    /// The attempt's prompt, in hex
    pub prompt_tx: String,
    /// Why it was reviewed: `hard`, or the names of the signs and
    /// suspicions that called for it
    pub why: Vec<String>,
    /// Every sign the attempt's evidence raised when it was reviewed
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signs: Vec<Flag>,
    /// What the judge answered
    pub verdict: Answer,
    /// What it found
    pub findings: String,
    /// The model that reviewed
    pub judge: String,
    /// List-price cost of the review in USD
    pub cost_usd: f64,
    /// How the judge was asked, e.g. `plain`, `thinking`, or
    /// `plain+criteria` when it was given the attempt's acceptance
    /// criteria
    pub variant: String,
    /// The acceptance criteria it was given, each with what it found
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub criteria: Vec<Checked>,
    /// Whether Gate acted on the verdict: sent work found unfinished
    /// back to the model or on to a stronger one
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub acted: bool,
    /// Whether `aura gate replay` asked, after the attempt had ended
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replayed: bool,
}
