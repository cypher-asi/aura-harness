//! Offline replay: how another way of routing would have done on the
//! work Gate has recorded, estimated from the recorded routes alone.
//!
//! Each counted prompt says which model did the work, how likely the
//! router was to pick that model (its propensity) and how it went. A
//! policy is judged on the prompts where it would have picked the same
//! model, each weighted by one over its propensity, so work rarely sent
//! to a model also stands for the times it went elsewhere
//! (self-normalised inverse propensity weighting). Work no recorded
//! route matched says nothing about the policy: [`Estimate::matched`]
//! says how much of the work the estimate rests on. `/compare --learn`
//! and exploring widen it.

use aura_core_types::{Difficulty, PromptCategory, RouteLevel};

use crate::Work;

/// Smallest propensity weighed, so one unlikely pick can't outweigh the
/// rest.
const MIN_PROPENSITY: f64 = 0.01;

/// One counted prompt as the replay sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Logged {
    pub level: RouteLevel,
    pub category: PromptCategory,
    pub difficulty: Option<Difficulty>,
    /// The permanent agent a subtask went to
    pub kind: Option<String>,
    /// Model that did the work
    pub model: String,
    /// How likely the router was to pick `model`
    pub propensity: f64,
    pub success: bool,
    /// List-price cost, its subtasks' included, in USD
    pub cost_usd: f64,
}

impl Logged {
    /// The work this prompt was, as the router saw it.
    #[must_use]
    pub fn work(&self) -> Work<'_> {
        Work::new(self.level, self.category, self.kind.as_deref()).of(self.difficulty)
    }

    fn weight(&self) -> f64 {
        if self.propensity.is_finite() {
            1.0 / self.propensity.clamp(MIN_PROPENSITY, 1.0)
        } else {
            1.0
        }
    }
}

/// What a policy is estimated to do on the recorded work.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Estimate {
    /// Prompts where the policy picks the model that did the work
    pub matched: usize,
    /// Prompts replayed
    pub of: usize,
    pub success_rate: f64,
    /// Mean cost per prompt, in USD
    pub mean_cost_usd: f64,
}

/// How the recorded routes did; `None` when there are none.
#[must_use]
pub fn as_recorded<'a>(logged: impl IntoIterator<Item = &'a Logged>) -> Option<Estimate> {
    weighed(logged, |_| Some(1.0))
}

/// What a policy would have done on `logged`, given whether it picks
/// the model that did each prompt's work; `None` when it picks none.
#[must_use]
pub fn estimate<'a>(
    logged: impl IntoIterator<Item = &'a Logged>,
    mut picks: impl FnMut(&Logged) -> bool,
) -> Option<Estimate> {
    weighed(logged, |prompt| picks(prompt).then(|| prompt.weight()))
}

fn weighed<'a>(
    logged: impl IntoIterator<Item = &'a Logged>,
    mut weight_of: impl FnMut(&Logged) -> Option<f64>,
) -> Option<Estimate> {
    let (mut of, mut matched) = (0, 0);
    let (mut weight, mut successes, mut cost) = (0.0, 0.0, 0.0);
    for prompt in logged {
        of += 1;
        let Some(w) = weight_of(prompt) else {
            continue;
        };
        matched += 1;
        weight += w;
        successes += w * f64::from(u8::from(prompt.success));
        cost += w * prompt.cost_usd;
    }
    (matched > 0).then(|| Estimate {
        matched,
        of,
        success_rate: successes / weight,
        mean_cost_usd: cost / weight,
    })
}

#[cfg(test)]
mod tests;
