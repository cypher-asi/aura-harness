//! What work is expected to cost on each candidate: what it has cost on
//! the model, else its list price scaled by what the tried models cost.

use std::cmp::Ordering;

use aura_core_types::RouteLevel;
use aura_model::model_catalog::{ModelBackend, ModelTier};

use crate::stats::GateStats;
use crate::Candidate;

/// A tried model's tier, the mean cost observed on it and its list price.
type Tried = (ModelTier, f64, f64);

/// Known costs first, cheapest first, and unknown ones last, as equals.
pub(crate) fn cheaper_first(a: Option<f64>, b: Option<f64>) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) => a.total_cmp(&b),
        _ => a.is_none().cmp(&b.is_none()),
    }
}

/// How far apart two tiers are, in tiers.
const fn tiers_apart(a: ModelTier, b: ModelTier) -> u8 {
    const fn rank(tier: ModelTier) -> u8 {
        match tier {
            ModelTier::Local => 0,
            ModelTier::Fast => 1,
            ModelTier::Standard => 2,
            ModelTier::Frontier => 3,
        }
    }
    rank(a).abs_diff(rank(b))
}

/// How observed costs compare with list prices among the `tried` models
/// of `tier`, or of the nearest tiers when none of its own was tried.
/// `None` when nothing was.
fn price_scale(tried: &[Tried], tier: ModelTier) -> Option<f64> {
    let nearest = tried.iter().map(|(t, ..)| tiers_apart(*t, tier)).min();
    let (seen, listed) = tried
        .iter()
        .filter(|(t, ..)| Some(tiers_apart(*t, tier)) == nearest)
        .fold((0.0, 0.0), |(seen, listed), (_, o, l)| {
            (seen + o, listed + l)
        });
    (seen > 0.0 && listed > 0.0).then(|| seen / listed)
}

/// The mean cost of work at `level` on `model`, its subtasks' included:
/// `None` for a local model, and for one not tried there.
fn observed_cost(stats: &GateStats, level: RouteLevel, model: &Candidate) -> Option<f64> {
    if model.backend == ModelBackend::Local {
        return None;
    }
    stats.mean_cost_usd(level, &model.id)
}

/// What work is expected to cost on `model`, whose work has cost
/// `observed` on average: nothing on a local model, else `observed`, or
/// its list price scaled as the `tried` models' costs say.
fn expected_cost(model: &Candidate, observed: Option<f64>, tried: &[Tried]) -> Option<f64> {
    if model.backend == ModelBackend::Local {
        return Some(0.0);
    }
    observed.or_else(|| {
        model
            .cost_usd
            .map(|price| price * price_scale(tried, model.tier).unwrap_or(1.0))
    })
}

/// [`by_expected_cost`], with what work at `level` is expected to cost on
/// each candidate.
pub(crate) fn expected_costs<'a>(
    stats: &GateStats,
    level: RouteLevel,
    candidates: impl IntoIterator<Item = &'a Candidate>,
) -> Vec<(&'a Candidate, Option<f64>)> {
    let observed: Vec<(&Candidate, Option<f64>)> = candidates
        .into_iter()
        .map(|model| (model, observed_cost(stats, level, model)))
        .collect();
    let tried: Vec<Tried> = observed
        .iter()
        .filter_map(|&(model, seen)| {
            Some((model.tier, seen?, model.cost_usd.filter(|&p| p > 0.0)?))
        })
        .collect();
    let mut order: Vec<(&Candidate, Option<f64>)> = observed
        .into_iter()
        .map(|(model, seen)| (model, expected_cost(model, seen, &tried)))
        .collect();
    order.sort_by(|a, b| cheaper_first(a.1, b.1));
    order
}

/// `candidates` ordered by what work at `level` is expected to cost on
/// each: local models first (they cost nothing), then by the mean cost
/// observed at `level`, or for a model not tried there yet, its list
/// price scaled by how observed costs compare with list prices for the
/// tried models of its tier, or of the nearest tiers when none of its own
/// was tried. Heavier work goes to stronger models, so what one tier's
/// work costs says less about another's the further apart they are.
/// Models with no price known come last; ties keep the order of
/// `candidates`.
#[must_use]
pub fn by_expected_cost<'a>(
    stats: &GateStats,
    level: RouteLevel,
    candidates: &'a [Candidate],
) -> Vec<&'a Candidate> {
    expected_costs(stats, level, candidates)
        .into_iter()
        .map(|(model, _)| model)
        .collect()
}

#[cfg(test)]
mod tests;
