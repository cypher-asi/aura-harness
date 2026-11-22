//! What a comparison costs at list prices: a model call's price, and the
//! estimate shown before anything runs.

#[cfg(test)]
mod tests;

use aura_model::model_catalog::{backend_for, estimate_cost_usd, ModelBackend};
use aura_model::Usage;

/// Tokens of a typical agentic run (several model calls over a growing
/// context), for the estimate shown before a comparison starts.
const REFERENCE_RUN: Usage = Usage {
    input_tokens: 150_000,
    output_tokens: 6_000,
    cache_creation_input_tokens: None,
    cache_read_input_tokens: None,
};

/// Tokens of one judge call: the task, two diffs and two replies.
const REFERENCE_JUDGE: Usage = Usage {
    input_tokens: 12_000,
    output_tokens: 600,
    cache_creation_input_tokens: None,
    cache_read_input_tokens: None,
};

/// List price of `usage` on `model`: 0 for local models, `None` when the
/// price is unknown.
#[must_use]
pub fn list_price(model: &str, usage: &Usage) -> Option<f64> {
    if backend_for(model) == ModelBackend::Local {
        return Some(0.0);
    }
    estimate_cost_usd(model, usage)
}

/// What a comparison is expected to cost at list prices, before it runs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// Every run of arm A; `None` when the model's price is unknown.
    pub a_usd: Option<f64>,
    /// Every run of arm B.
    pub b_usd: Option<f64>,
    /// Every judge call.
    pub judge_usd: Option<f64>,
}

impl Estimate {
    /// The parts whose price is known, added up.
    #[must_use]
    pub fn known_usd(&self) -> f64 {
        [self.a_usd, self.b_usd, self.judge_usd]
            .into_iter()
            .flatten()
            .sum()
    }

    /// Whether every part has a price.
    #[must_use]
    pub const fn fully_priced(&self) -> bool {
        self.a_usd.is_some() && self.b_usd.is_some() && self.judge_usd.is_some()
    }
}

/// The estimate for `repeats` runs per arm of `prompts` prompts each,
/// with arm A on `a_model` (Gate's expected pick when Gate routes it),
/// arm B on `b_model`, and one judge call per pair on `judge_model`, each
/// prompt's turn the size of a typical agentic run.
#[must_use]
pub fn estimate(
    a_model: &str,
    b_model: &str,
    judge_model: &str,
    repeats: u32,
    prompts: usize,
) -> Estimate {
    let runs = f64::from(repeats);
    let turns = runs * f64::from(u32::try_from(prompts.max(1)).unwrap_or(u32::MAX));
    Estimate {
        a_usd: list_price(a_model, &REFERENCE_RUN).map(|usd| usd * turns),
        b_usd: list_price(b_model, &REFERENCE_RUN).map(|usd| usd * turns),
        judge_usd: list_price(judge_model, &REFERENCE_JUDGE).map(|usd| usd * runs),
    }
}
