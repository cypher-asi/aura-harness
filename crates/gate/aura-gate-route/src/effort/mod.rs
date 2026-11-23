//! How hard the model reasons on the work Gate sends it.
//!
//! Gate picks the reasoning effort with the model: work starts at the
//! effort its difficulty calls for on the model it goes to
//! ([`effort_on`]), and only work on the strongest model that a judge
//! found unfinished is tried again at a higher one, up to max
//! ([`raise_effort`]). At max one call can think for minutes, so it is
//! kept for work nothing less got right.

use aura_core_types::Difficulty;
use aura_gate_evidence::verifiable;
use aura_model::model_catalog::{has_max_effort, ModelTier};
use aura_model::ThinkingEffort;

use crate::{Candidate, Work};

/// The reasoning effort work of `difficulty` starts at, on whichever
/// model it goes to: low for easy work, high for hard work, medium for
/// the rest and when the classifier did not say. Never max: that is
/// reached only after a failed attempt ([`raise_effort`]).
const fn start_effort(difficulty: Option<Difficulty>) -> ThinkingEffort {
    match difficulty {
        Some(Difficulty::Easy) => ThinkingEffort::Low,
        Some(Difficulty::Hard) => ThinkingEffort::High,
        Some(Difficulty::Medium) | None => ThinkingEffort::Medium,
    }
}

/// The reasoning effort `work` starts at on `model`: low for easy work,
/// high for hard work and medium for the rest, except on the fast tier,
/// where code work ([`verifiable`]) reasons at medium effort or above and
/// other work does not think. At low effort a model may skip thinking on steps it
/// finds simple, and code is where that costs a failed attempt.
#[must_use]
pub const fn effort_on(work: &Work<'_>, model: &Candidate) -> ThinkingEffort {
    let effort = start_effort(work.difficulty);
    match model.tier {
        ModelTier::Fast if !verifiable(work.category) => ThinkingEffort::Off,
        ModelTier::Fast => match effort {
            ThinkingEffort::Off | ThinkingEffort::Minimal | ThinkingEffort::Low => {
                ThinkingEffort::Medium
            }
            effort => effort,
        },
        _ => effort,
    }
}

/// The effort after `effort` on `model`'s ladder (low, medium, high,
/// max), when Gate may still raise it; `None` at the top. Max is a step
/// only on a model that reasons harder there than at high
/// ([`has_max_effort`]).
pub(crate) fn raise_effort(effort: ThinkingEffort, model: &str) -> Option<ThinkingEffort> {
    match effort {
        ThinkingEffort::Off | ThinkingEffort::Minimal | ThinkingEffort::Low => {
            Some(ThinkingEffort::Medium)
        }
        ThinkingEffort::Medium => Some(ThinkingEffort::High),
        ThinkingEffort::High | ThinkingEffort::XHigh => {
            has_max_effort(model).then_some(ThinkingEffort::Max)
        }
        ThinkingEffort::Max => None,
    }
}

#[cfg(test)]
mod tests;
