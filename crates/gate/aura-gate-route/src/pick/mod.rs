//! The model Gate sends a piece of work to, given what it has learned.
//!
//! [`pick`] chooses among the candidates for one category: the cheapest
//! model that has proven itself there, or the model the category starts
//! on when that is cheaper ([`crate::start`]), and now and then the next
//! cheaper model instead, to learn how it does. Until Gate has learned
//! per category, every turn goes to [`fixed_pick`]; the subtasks of a
//! lead on a frontier model are first routed below its tier
//! ([`below_lead`]).

use aura_config::GateConfig;
use aura_core_types::RouteReason;
use aura_model::model_catalog::{infer_tier, ModelTier};

use crate::candidate::is_current;
use crate::effort::effort_on;
use crate::judged::{judge, Judged};
use crate::start::{start_floor, start_index, start_tier};
use crate::stats::GateStats;
use crate::{current, Candidate, Decision, Random, Work};

/// Where `work` goes, among `candidates` (as
/// [`candidates`](crate::candidates) returns them), given what `stats`
/// has learned:
///
/// 1. the cheapest model with at least `min_tries` counted outcomes on
///    the work and a success rate of at least `target`;
/// 2. or, when it is cheaper than that (or nothing is proven), the model
///    the work starts on: the cheapest model of the tier it starts on,
///    or of the next tier up while the work's outcomes on that model
///    send it up, ending at the strongest;
/// 3. with probability `explore_rate`, a cheaper model than that
///    instead, when there is one: the nearest the work's outcomes
///    have not ruled out, else the nearest. So a model that was ruled
///    out is tried again once nothing cheaper is more promising, and
///    comes back when those tries prove it.
///
/// Models are judged by their outcomes on the work, on work of the same
/// difficulty where they have done any, and ordered by
/// [`by_expected_cost`](crate::by_expected_cost). `None` when there are
/// no candidates. Work that is not easy starts no lower than the
/// standard tier where a model that strong is there, whether proven, by
/// default or explored, and no work goes to a model a later version
/// supersedes ([`current`]). The work runs at the effort of its
/// difficulty on the model it goes to.
pub fn pick(
    stats: &GateStats,
    work: &Work<'_>,
    candidates: &[Candidate],
    settings: &GateConfig,
    rng: &mut impl Random,
) -> Option<Decision> {
    let order = judge(stats, work, current(candidates), settings.min_tries);
    let floor = start_floor(work.difficulty, order.iter().map(|c| c.candidate.tier));
    let (index, reason) = choose(&order, work, floor, settings)?;
    let decide = |at: usize, reason, explored, propensity| Decision {
        model: order[at].candidate.clone(),
        effort: effort_on(work, order[at].candidate),
        reason,
        explored,
        propensity,
    };
    let cheaper: Vec<usize> = (0..index)
        .filter(|&at| order[at].may_start(floor))
        .collect();
    if cheaper.is_empty() {
        return Some(decide(index, reason, false, 1.0));
    }
    let rate = settings.explore_rate.clamp(0.0, 1.0);
    Some(match explore(&order, &cheaper, rate, settings, rng) {
        Some(at) => decide(at, RouteReason::Explore, true, rate),
        None => decide(index, reason, false, 1.0 - rate),
    })
}

/// Gate's pick in `order` before it explores, and why: the cheapest
/// proven model work starting no lower than `floor` may go to, when it is
/// no dearer than the model the work starts on ([`start_index`]); else
/// that one. `None` when `order` is empty.
fn choose(
    order: &[Judged<'_>],
    work: &Work<'_>,
    floor: Option<ModelTier>,
    settings: &GateConfig,
) -> Option<(usize, RouteReason)> {
    let start = start_index(order, start_tier(work.category, work.difficulty), settings)?;
    let proven = order
        .iter()
        .position(|c| c.may_start(floor) && c.proven(settings));
    Some(match proven {
        Some(proven) if proven <= start => (proven, RouteReason::Proven),
        _ => (start, RouteReason::Default),
    })
}

/// Where exploring sends the work instead, with probability `rate`: of
/// the `cheaper` positions in `order`, cheapest first, the nearest whose
/// outcomes have not ruled it out, else the nearest. `None` when Gate
/// keeps its pick. Draws from `rng` only when `cheaper` is not empty and
/// `rate` is above 0.
fn explore(
    order: &[Judged<'_>],
    cheaper: &[usize],
    rate: f64,
    settings: &GateConfig,
    rng: &mut impl Random,
) -> Option<usize> {
    let &nearest = cheaper.last()?;
    (rate > 0.0 && rng.unit() < rate).then(|| {
        cheaper
            .iter()
            .rev()
            .copied()
            .find(|&at| order[at].open(settings))
            .unwrap_or(nearest)
    })
}

/// The model every turn goes to until Gate learns per category: the
/// cheapest standard-tier candidate, else the strongest one (the cheapest
/// of that tier), leaving out those a later version supersedes
/// ([`current`]). `candidates` must be ordered as
/// [`candidates`](crate::candidates) returns them.
#[must_use]
pub fn fixed_pick(candidates: &[Candidate]) -> Option<&Candidate> {
    let kept = || candidates.iter().filter(|c| is_current(c, candidates));
    kept()
        .find(|c| c.tier == ModelTier::Standard)
        .or_else(|| kept().rev().max_by_key(|c| c.tier))
}

/// What a subtask is first routed among when the agent dispatching it
/// works on `lead`: the `candidates` below the frontier tier when `lead`
/// is a frontier model and a standard one is among them, so the most
/// expensive model plans the work and a cheaper one does it. Every
/// candidate otherwise: a lead on a cheaper model already costs little,
/// and with nothing between frontier and fast the work would drop two
/// tiers. A failed attempt still escalates among all `candidates`.
#[must_use]
pub fn below_lead(candidates: &[Candidate], lead: Option<&str>) -> Vec<Candidate> {
    let frontier = lead.is_some_and(|lead| {
        candidates
            .iter()
            .find(|c| c.id == lead)
            .map_or_else(|| infer_tier(lead), |c| c.tier)
            == ModelTier::Frontier
    });
    let standard = candidates.iter().any(|c| c.tier == ModelTier::Standard);
    if frontier && standard {
        candidates
            .iter()
            .filter(|c| c.tier < ModelTier::Frontier)
            .cloned()
            .collect()
    } else {
        candidates.to_vec()
    }
}

#[cfg(test)]
mod tests;
