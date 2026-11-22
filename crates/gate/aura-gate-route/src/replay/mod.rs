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

use crate::pick::Work;

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
mod tests {
    use super::*;

    fn logged(model: &str, propensity: f64, success: bool, cost_usd: f64) -> Logged {
        Logged {
            level: RouteLevel::Turn,
            category: PromptCategory::Debugging,
            difficulty: None,
            kind: None,
            model: model.to_string(),
            propensity,
            success,
            cost_usd,
        }
    }

    #[test]
    fn the_recorded_routes_count_as_they_went() {
        let log = [logged("a", 0.5, true, 1.0), logged("a", 1.0, false, 3.0)];
        assert_eq!(
            as_recorded(&log),
            Some(Estimate {
                matched: 2,
                of: 2,
                success_rate: 0.5,
                mean_cost_usd: 2.0,
            })
        );
    }

    #[test]
    fn rarely_picked_work_stands_for_the_times_it_was_not() {
        // "b" was explored on one in ten prompts: its one success weighs
        // like ten, against "a"'s nine.
        let mut log = vec![logged("b", 0.1, true, 0.1)];
        log.extend((0..9).map(|_| logged("a", 0.9, false, 1.0)));
        let all = estimate(&log, |_| true).unwrap();
        assert!((all.success_rate - 0.5).abs() < 1e-9, "{all:?}");
        let on_b = estimate(&log, |p| p.model == "b").unwrap();
        assert_eq!((on_b.matched, on_b.of), (1, 10));
        assert!((on_b.mean_cost_usd - 0.1).abs() < 1e-9);
    }

    #[test]
    fn a_policy_that_matches_nothing_has_no_estimate() {
        let log = [logged("a", 1.0, true, 1.0)];
        assert_eq!(estimate(&log, |p| p.model == "z"), None);
    }

    #[test]
    fn an_unlikely_pick_is_capped() {
        let log = [logged("a", 0.0, true, 1.0), logged("a", 1.0, false, 1.0)];
        let estimate = estimate(&log, |_| true).unwrap();
        assert!((estimate.success_rate - 100.0 / 101.0).abs() < 1e-9);
    }
}
