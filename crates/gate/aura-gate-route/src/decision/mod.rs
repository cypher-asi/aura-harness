//! Where Gate sends a piece of work, and why: what a turn's prompt
//! records of its route, and how the route reads.

use aura_core_types::{Route, RouteLevel, RouteReason};
use aura_model::ThinkingEffort;

use crate::Candidate;

/// Where Gate sends a piece of work, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// The model it runs on.
    pub model: Candidate,
    /// How hard the model reasons on it.
    pub effort: ThinkingEffort,
    /// Why: [`RouteReason::Proven`], [`RouteReason::Default`] or
    /// [`RouteReason::Explore`], [`RouteReason::Escalate`] after a
    /// failed attempt, and [`RouteReason::Compare`] on an arm of
    /// `/compare` with a model per difficulty.
    pub reason: RouteReason,
    /// Whether Gate explored instead of taking its pick.
    pub explored: bool,
    /// Probability that Gate's policy makes this choice.
    pub propensity: f64,
}

impl Decision {
    /// The model, who serves it and its effort, e.g.
    /// `claude-sonnet-4-6 (claude plan) at high effort`, or
    /// `claude-haiku-5-5 (claude plan) with thinking off`.
    #[must_use]
    pub fn label(&self) -> String {
        let model = self.model.route_label();
        match self.effort.reasoning_effort_wire() {
            Some(effort) => format!("{model} at {effort} effort"),
            None => format!("{model} with thinking off"),
        }
    }

    /// The decision as recorded on the prompt of work at `level`.
    #[must_use]
    pub fn route(&self, level: RouteLevel) -> Route {
        Route {
            level,
            model: self.model.id.clone(),
            reason: self.reason,
            explored: self.explored,
            propensity: self.propensity,
            effort: self.effort.reasoning_effort_wire().map(str::to_string),
        }
    }
}

#[cfg(test)]
mod tests;
