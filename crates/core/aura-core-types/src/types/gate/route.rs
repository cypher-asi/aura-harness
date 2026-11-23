//! Where a prompt's work runs, and why.

use serde::{Deserialize, Serialize};

/// What a routing decision covers.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum RouteLevel {
    /// The main conversation loop for a whole turn.
    #[default]
    Turn,
    /// A piece of work delegated with the `task` tool.
    Subtask,
}

impl RouteLevel {
    /// The level's name as stored, e.g. `turn`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Turn => "turn",
            Self::Subtask => "subtask",
        }
    }
}

impl std::fmt::Display for RouteLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why the work runs on the model it runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RouteReason {
    /// The user (or an agent definition) chose the model.
    #[default]
    Pinned,
    /// Gate's default for the category.
    Default,
    /// The cheapest model that has proven itself on the category.
    Proven,
    /// Gate tried a cheaper model to learn how it does.
    Explore,
    /// A run of `/compare`: Gate's pick with exploration off, or the
    /// model Gate is compared with.
    Compare,
    /// A subtask retried on a stronger model after its first attempt
    /// failed. Recorded, but not counted in Gate's table: it only ever
    /// sees work that already failed once.
    Escalate,
}

impl RouteReason {
    /// The reason's name as stored, e.g. `proven`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pinned => "pinned",
            Self::Default => "default",
            Self::Proven => "proven",
            Self::Explore => "explore",
            Self::Compare => "compare",
            Self::Escalate => "escalate",
        }
    }
}

impl std::fmt::Display for RouteReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The model a prompt's work runs on, and why.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Route {
    /// Whether this routes a turn or a subtask.
    pub level: RouteLevel,
    /// Model that runs the work, route prefix included.
    pub model: String,
    /// Why it runs there.
    pub reason: RouteReason,
    /// Whether Gate explored instead of taking its pick.
    pub explored: bool,
    /// Probability of this choice under Gate's policy, 1.0 when it was
    /// not a random choice.
    pub propensity: f64,
    /// Reasoning effort Gate chose for the work (`low`, `high`, `max`,
    /// ...); `None` when it chose none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

impl Route {
    /// A turn on `model`, chosen for `reason` without exploring.
    #[must_use]
    pub fn turn(model: impl Into<String>, reason: RouteReason) -> Self {
        Self {
            level: RouteLevel::Turn,
            model: model.into(),
            reason,
            explored: false,
            propensity: 1.0,
            effort: None,
        }
    }

    /// The same route, at reasoning effort `effort`.
    #[must_use]
    pub fn at_effort(self, effort: impl Into<String>) -> Self {
        Self {
            effort: Some(effort.into()),
            ..self
        }
    }

    /// A subtask on `model`, chosen for `reason` without exploring.
    #[must_use]
    pub fn subtask(model: impl Into<String>, reason: RouteReason) -> Self {
        Self {
            level: RouteLevel::Subtask,
            ..Self::turn(model, reason)
        }
    }
}

impl Default for Route {
    fn default() -> Self {
        Self::turn(String::new(), RouteReason::Pinned)
    }
}
