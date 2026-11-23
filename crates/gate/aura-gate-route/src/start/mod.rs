//! The model work starts on before a model has proven itself on it.
//!
//! Work starts cheap ([`start_tier`]), and work that is not easy never
//! starts below the standard tier ([`start_floor`]). A category moves up
//! a tier once its start model has failed too often to reach the target,
//! and from the standard tier up only once starting there has come to
//! cost more than starting on the next ([`start_index`]).

use aura_config::GateConfig;
use aura_core_types::{Difficulty, PromptCategory};
use aura_model::model_catalog::ModelTier;

use crate::judged::Judged;
use crate::stats::Row;

/// The tier work asking for `category`, of `difficulty` when the
/// classifier said, starts on before Gate has learned better.
///
/// Only work the classifier found easy starts on the fast tier, plans
/// excepted; everything else starts on the standard one: plans, whatever
/// their difficulty, and medium, hard or unrated work of any kind, small
/// changes included. Work that failed moves up a tier
/// ([`escalate`](crate::escalate())), and work whose start model has
/// shown it is the dearer place to start starts higher ([`start_index`]).
/// The frontier tier is reached that way, not by default: on the work it
/// is asked for, it costs several times a standard model for results the
/// checks rarely tell apart.
pub(crate) const fn start_tier(
    category: PromptCategory,
    difficulty: Option<Difficulty>,
) -> ModelTier {
    match (category, difficulty) {
        (PromptCategory::Planning, _) => ModelTier::Standard,
        (_, Some(Difficulty::Easy)) => ModelTier::Fast,
        _ => ModelTier::Standard,
    }
}

/// The weakest tier work of `difficulty` may start on, among candidates of
/// `tiers`: the standard tier for all but easy work (unrated work counts
/// as medium) when a model at least that strong is there, else any. Below
/// it, a model's own test passes on work it got wrong (tests it wrote for
/// its own code, or tests that miss the change), so the work is never
/// moved up.
pub(crate) fn start_floor(
    difficulty: Option<Difficulty>,
    mut tiers: impl Iterator<Item = ModelTier>,
) -> Option<ModelTier> {
    (difficulty != Some(Difficulty::Easy) && tiers.any(|tier| tier >= ModelTier::Standard))
        .then_some(ModelTier::Standard)
}

/// Position in `order` of the model work starts on: the cheapest model of
/// its `start` tier ([`start_tier`]), or of the lowest tier above it
/// whose cheapest model the work's outcomes do not send higher
/// ([`keeps_work`]). The cheapest model of the strongest tier when every
/// tier below is sent up or none is at or above the start tier.
pub(crate) fn start_index(
    order: &[Judged<'_>],
    start: ModelTier,
    settings: &GateConfig,
) -> Option<usize> {
    let top = order.iter().map(|c| c.candidate.tier).max()?;
    let firsts = cheapest_of_each_tier(order, start);
    let next = |at: usize| firsts.get(at + 1).map(|&next| &order[next]);
    firsts
        .iter()
        .enumerate()
        .find(|&(at, &first)| keeps_work(&order[first], next(at), settings))
        .map(|(_, &first)| first)
        .or_else(|| order.iter().position(|c| c.candidate.tier == top))
}

/// Positions in `order` of the cheapest model of each tier from `start`
/// up, weakest tier first.
fn cheapest_of_each_tier(order: &[Judged<'_>], start: ModelTier) -> Vec<usize> {
    let mut tiers: Vec<ModelTier> = order
        .iter()
        .map(|c| c.candidate.tier)
        .filter(|tier| *tier >= start)
        .collect();
    tiers.sort_unstable();
    tiers.dedup();
    tiers
        .into_iter()
        .filter_map(|tier| order.iter().position(|c| c.candidate.tier == tier))
        .collect()
}

/// Whether work starts on `first`, the cheapest model of its tier, rather
/// than going on to `next`, the cheapest of the next tier up (`None` when
/// there is none).
///
/// A model is sent up once it can no longer reach the target
/// ([`Row::can_still_reach`]): two failures in its first ten tries.
/// Below the standard tier that is enough, as moving up from there costs
/// cents. From the standard tier up the next tier costs several times as
/// much, so the model's outcomes must also show that starting on it
/// costs more than starting on the next ([`dearer_start`]): its failures
/// go on to the next model anyway, which is how the frontier tier is
/// reached. Where either cost is unknown, the target decides alone.
fn keeps_work(first: &Judged<'_>, next: Option<&Judged<'_>>, settings: &GateConfig) -> bool {
    let Some(row) = first.row else {
        return true;
    };
    if row.can_still_reach(settings.target, settings.min_tries) {
        return true;
    }
    if first.candidate.tier < ModelTier::Standard {
        return false;
    }
    let Some(next) = next else {
        return true;
    };
    match (first.cost, next.cost) {
        (Some(cost), Some(next)) if cost > 0.0 && next > 0.0 => {
            !dearer_start(&row, cost, next, settings.min_tries)
        }
        _ => false,
    }
}

/// Whether work costs more started on a model with the outcomes `row`,
/// where it costs `cost`, than on the next model up, where it costs
/// `next`. With the share `rate` of its attempts succeeding, starting on
/// it costs `cost + (1 - rate) * next` once the failures have gone on to
/// the next model, against `next` for starting there: no less when
/// `cost >= rate * next`. Before the row has `min_tries` tries, the ones
/// it lacks count as successes, as in [`Row::can_still_reach`], so a
/// model is sent up early only by failures no later success can make up
/// for.
fn dearer_start(row: &Row, cost: f64, next: f64, min_tries: u32) -> bool {
    let lacking = min_tries.saturating_sub(row.tries);
    let counted = row.tries.max(min_tries).max(1);
    let rate = f64::from(row.successes + lacking) / f64::from(counted);
    cost >= rate * next
}

#[cfg(test)]
mod tests;
