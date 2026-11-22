//! Whether a turn is led, its work handed to subtasks, or done by the
//! model it is routed to, from what Gate has learned of both ways.

use aura_config::GateConfig;
use aura_core_types::{Difficulty, PromptCategory};
use aura_gate_route::stats::{GateStats, Row};
use aura_gate_route::Random;
use aura_model::model_catalog::{infer_tier, ModelTier};

/// How a turn is done: led, or by its model itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Way {
    pub led: bool,
    /// Whether it was taken to learn about it rather than as the best
    /// known way
    pub explored: bool,
}

/// How a turn asking for `category` on `model` is done, given what
/// `stats` has learned of both ways on the category and model, judged on
/// success and on the cost of the whole work, the subtasks' included. A
/// turn of known `difficulty` goes by the turns of that difficulty alone
/// ([`GateStats::way_row_of`]), as leading cheap work costs little and
/// says nothing of what leading hard work costs; one of unknown
/// difficulty goes by every turn ([`GateStats::way_row`]):
///
/// 1. the cheaper of the two ways once both are proven: at least
///    `min_tries` outcomes with a success rate of at least `target`;
/// 2. else by the model itself, unless its outcomes rule that out
///    ([`Row::can_still_reach`]) and leading is proven or not ruled out
///    either. Leading proven alone does not win: the model doing the work
///    itself gets its tries until it is proven too, or ruled out;
/// 3. with probability `explore_rate`, the other way instead, unless its
///    outcomes rule it out.
///
/// Leading is unproven by default because the lead's own reading,
/// planning and checking cost about what the model doing the work itself
/// does, before any subtask runs. On a frontier model it is taken only
/// once proven cheaper, never to explore or as a fallback. Never led for
/// work that can't be handed to subtasks: a question, a small change or
/// prose.
pub fn choose_way(
    stats: &GateStats,
    (category, difficulty): (PromptCategory, Option<Difficulty>),
    model: &str,
    settings: &GateConfig,
    rng: &mut impl Random,
) -> Way {
    if !may_lead(category) {
        return Way {
            led: false,
            explored: false,
        };
    }
    let row = |led| match difficulty {
        Some(difficulty) => stats.way_row_of(category, difficulty, model, led),
        None => stats.way_row(category, model, led),
    };
    let learned = Learned {
        direct: row(false),
        led: row(true),
        frontier: infer_tier(model) == ModelTier::Frontier,
        settings,
    };
    let led = learned.best_is_led();
    let rate = settings.explore_rate.clamp(0.0, 1.0);
    if rate > 0.0 && learned.open(!led) && rng.unit() < rate {
        return Way {
            led: !led,
            explored: true,
        };
    }
    Way {
        led,
        explored: false,
    }
}

/// Whether a turn asking for `category` may be led at all: work that can
/// be handed to subtasks. A question, a small change or prose is always
/// done by the model it is routed to.
const fn may_lead(category: PromptCategory) -> bool {
    matches!(
        category,
        PromptCategory::MultiFileChange
            | PromptCategory::Planning
            | PromptCategory::Debugging
            | PromptCategory::Research
    )
}

/// What Gate has learned of both ways of doing one category's turns on
/// one model.
struct Learned<'a> {
    /// The model's outcomes doing the work itself
    direct: Option<&'a Row>,
    /// Its outcomes leading
    led: Option<&'a Row>,
    /// Whether it is a frontier model, which leads only once leading is
    /// proven cheaper
    frontier: bool,
    settings: &'a GateConfig,
}

impl Learned<'_> {
    fn row(&self, led: bool) -> Option<&Row> {
        if led {
            self.led
        } else {
            self.direct
        }
    }

    /// The mean cost of the whole work done the `led` way, once that way
    /// is proven: at least `min_tries` outcomes, with a success rate of at
    /// least `target`.
    fn proven_cost(&self, led: bool) -> Option<f64> {
        let row = self
            .row(led)
            .filter(|row| row.settled(self.settings.min_tries))?;
        let (rate, cost) = (row.success_rate()?, row.mean_total_cost_usd()?);
        (rate >= self.settings.target).then_some(cost)
    }

    /// Whether the `led` way may still be taken: it isn't leading on a
    /// frontier model, and its outcomes don't rule it out.
    fn open(&self, led: bool) -> bool {
        let (target, min_tries) = (self.settings.target, self.settings.min_tries);
        !(led && self.frontier)
            && self
                .row(led)
                .is_none_or(|row| row.can_still_reach(target, min_tries))
    }

    /// Whether the best known way is leading, by rules 1 and 2 of
    /// [`choose_way`].
    fn best_is_led(&self) -> bool {
        match (self.proven_cost(true), self.proven_cost(false)) {
            (Some(led), Some(direct)) if led != direct => led < direct,
            (Some(_), None) => !self.open(false),
            (None, Some(_)) => false,
            _ => !self.open(false) && self.open(true),
        }
    }
}

#[cfg(test)]
mod tests;
