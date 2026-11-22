//! The model work goes on with after a failed attempt: the cheapest one
//! a tier up, else, once a judge found it unfinished, the same model at a
//! higher effort.

use aura_core_types::{OutcomeSignal, RouteLevel, RouteReason};
use aura_model::model_catalog::infer_tier;
use aura_model::ThinkingEffort;

use crate::cost::expected_costs;
use crate::effort::raise_effort;
use crate::stats::GateStats;
use crate::{current, Candidate, Decision};

/// Where work at `level` that failed on `failed` at `effort` goes next:
/// the cheapest model (by [`by_expected_cost`](crate::by_expected_cost))
/// of the lowest tier above `failed`'s, among those no later version
/// supersedes ([`current`]), at the same effort, or at medium when it ran
/// with thinking off (not every stronger model can turn thinking off).
/// With no stronger model, the same model one effort higher, so max is
/// the last step on the strongest model, and only when a judge found the
/// work unfinished (`signal` is [`OutcomeSignal::JudgedFail`]). A failing
/// test run or an error does not send the strongest model over its own
/// work again: its last test run often fails for reasons the change did
/// not cause, and the second pass costs several times the first. `effort`
/// is `None` when Gate does not choose the effort (the user pinned one):
/// then only the model moves. `None` when nothing is left to try.
#[must_use]
pub fn escalate(
    stats: &GateStats,
    level: RouteLevel,
    failed: (&str, Option<ThinkingEffort>),
    signal: OutcomeSignal,
    candidates: &[Candidate],
) -> Option<Decision> {
    let (model, effort) = next_attempt(stats, level, failed, signal, candidates)?;
    Some(Decision {
        model: model.clone(),
        effort,
        reason: RouteReason::Escalate,
        explored: false,
        propensity: 1.0,
    })
}

/// Whether work at `level` that `ran` on a model, at an effort as
/// [`escalate()`] takes it, has a stronger model to go on to among
/// `candidates`. One effort higher on the same model is not one: work on
/// the strongest model that checked nothing gets only the judge's
/// verdict (`aura-gate-followup`).
#[must_use]
pub fn has_stronger_model(
    stats: &GateStats,
    level: RouteLevel,
    ran: (&str, Option<ThinkingEffort>),
    candidates: &[Candidate],
) -> bool {
    next_attempt(stats, level, ran, OutcomeSignal::JudgedFail, candidates)
        .is_some_and(|(next, _)| next.id != ran.0)
}

/// The model and effort [`escalate()`] goes on with.
fn next_attempt<'a>(
    stats: &GateStats,
    level: RouteLevel,
    (failed, effort): (&str, Option<ThinkingEffort>),
    signal: OutcomeSignal,
    candidates: &'a [Candidate],
) -> Option<(&'a Candidate, ThinkingEffort)> {
    let ran = candidates.iter().find(|c| c.id == failed);
    let tier = ran.map_or_else(|| infer_tier(failed), |c| c.tier);
    let stronger = expected_costs(stats, level, current(candidates))
        .into_iter()
        .map(|(model, _)| model)
        .filter(|model| model.tier > tier)
        .min_by_key(|model| model.tier);
    match stronger {
        Some(model) => {
            let effort = effort.filter(|&effort| effort != ThinkingEffort::Off);
            Some((model, effort.unwrap_or(ThinkingEffort::Medium)))
        }
        None if signal == OutcomeSignal::JudgedFail => Some((ran?, raise_effort(effort?, failed)?)),
        None => None,
    }
}

#[cfg(test)]
mod tests;
