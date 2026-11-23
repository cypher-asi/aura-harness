//! How [`pick`](crate::pick()) weighs each candidate for a piece of work:
//! by which of its outcomes, and at what cost.

use aura_config::GateConfig;
use aura_core_types::RouteLevel;
use aura_model::model_catalog::ModelTier;

use crate::cost::expected_costs;
use crate::stats::{GateStats, Row};
use crate::{Candidate, Work};

/// A candidate as [`pick`](crate::pick()) weighs it for a piece of work.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Judged<'a> {
    pub(crate) candidate: &'a Candidate,
    /// The outcomes it is judged by ([`judged`]).
    pub(crate) row: Option<Row>,
    /// What the work costs on it: what it has cost there, else what any
    /// work at its level is expected to.
    pub(crate) cost: Option<f64>,
}

impl Judged<'_> {
    /// Whether its outcomes prove it: at least `min_tries` of them, with
    /// a success rate of at least `target`.
    pub(crate) fn proven(&self, settings: &GateConfig) -> bool {
        self.row.is_some_and(|row| {
            row.tries >= settings.min_tries
                && row
                    .success_rate()
                    .is_some_and(|rate| rate >= settings.target)
        })
    }

    /// Whether its outcomes have not ruled it out: it has none, or can
    /// still reach the target ([`Row::can_still_reach`]).
    pub(crate) fn open(&self, settings: &GateConfig) -> bool {
        self.row
            .is_none_or(|row| row.can_still_reach(settings.target, settings.min_tries))
    }

    /// Whether work that starts no lower than `floor` may go to it.
    pub(crate) fn may_start(&self, floor: Option<ModelTier>) -> bool {
        floor.is_none_or(|floor| self.candidate.tier >= floor)
    }
}

/// `candidates` ordered by what `work` is expected to cost on each
/// ([`expected_costs`]), with the outcomes each is judged by.
pub(crate) fn judge<'a>(
    stats: &GateStats,
    work: &Work<'_>,
    candidates: impl IntoIterator<Item = &'a Candidate>,
    min_tries: u32,
) -> Vec<Judged<'a>> {
    expected_costs(stats, work.level, candidates)
        .into_iter()
        .map(|(candidate, expected)| {
            let row = judged(stats, work, &candidate.id, min_tries);
            let cost = row.and_then(|row| row.mean_total_cost_usd()).or(expected);
            Judged {
                candidate,
                row,
                cost,
            }
        })
        .collect()
}

/// The outcomes [`pick`](crate::pick()) judges `model` by for `work`: for
/// a subtask, those of subtasks for its permanent agent once they number
/// `min_tries`, as a model can suit one agent's work and not another's,
/// else every subtask on the category; for a turn, the turns it did
/// itself ([`GateStats::unled_row`]), since a led turn that failed may
/// have failed in its subtasks. A turn the classifier gave a difficulty
/// is judged by the turns of that difficulty when any has a known
/// result: failing an easy rename says little of hard work, and passing
/// one says less.
pub(crate) fn judged(
    stats: &GateStats,
    work: &Work<'_>,
    model: &str,
    min_tries: u32,
) -> Option<Row> {
    let Work {
        level,
        category,
        difficulty,
        kind,
    } = *work;
    match level {
        RouteLevel::Turn => difficulty
            .and_then(|difficulty| stats.unled_work_row(category, difficulty, model))
            .filter(|row| row.tries > 0)
            .or_else(|| stats.unled_row(category, model)),
        RouteLevel::Subtask => kind
            .and_then(|kind| stats.kind_row(category, kind, model))
            .filter(|row| row.settled(min_tries))
            .or_else(|| stats.row(level, category, model))
            .copied(),
    }
}

#[cfg(test)]
mod tests;
