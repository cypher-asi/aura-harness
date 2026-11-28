//! The software factory's flow for an agent given a task in a channel:
//! plan, then build the plan, then ship it, with no one in between.
//!
//! ```text
//! assignment ─> Assigned ─its work starts─> Planning ─plan written─> Building
//!                                              │                        │
//!                      asked instead (no plan) ┘          done, pushed? ┤
//!                                                     yes: done   no: Shipping
//!                                                                 (asked to git_ship)
//! ```
//!
//! The stages are pure state here; the event loop applies them
//! (see `super::hooks`). "Plan written" means the turn itself ran
//! `create_plan`, so a plan from an earlier task is never taken for this
//! one's, and a rewrite of the same plan file counts. A failed or stopped
//! turn leaves the flow, as the work it was in is given up; one that was
//! planning says so, so its agent leaves plan mode.

use aura_core_types::AgentId;
use std::collections::HashMap;
use std::sync::Mutex;

/// Where an agent is in the flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    /// A task came; the work it starts plans.
    Assigned,
    /// Its turns run in plan mode until one writes a plan.
    Planning,
    /// Its turn builds the plan it wrote.
    Building,
    /// It was asked to ship what it built.
    Shipping,
}

/// How an agent's turn ended, as far as the flow cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TurnEnd<'a> {
    /// It ended well (not failed or stopped)
    pub ok: bool,
    /// The plan the turn wrote with `create_plan`, if it did
    pub wrote_plan: Option<&'a str>,
    /// The turn pushed its work (a push tool succeeded in it)
    pub shipped: bool,
}

/// What the loop does after the turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Next {
    /// Nothing more: answer what the turn was asked, as always.
    Normal,
    /// The flow was left while planning: back out of plan mode, then
    /// answer (or give up) as always.
    LeftPlanning,
    /// Build the plan at this path next; the answer waits for the build.
    Build(String),
    /// Ask the agent to ship; the answer waits for that turn.
    AskToShip,
}

impl Next {
    /// Whether the turn's answer waits for a later turn of the flow.
    pub(crate) const fn defers_answer(&self) -> bool {
        matches!(self, Self::Build(_) | Self::AskToShip)
    }
}

/// The stage after a turn of an agent at `stage` ended as `end`, and what
/// to do next.
pub(crate) fn advance(stage: Option<Stage>, end: TurnEnd<'_>) -> (Option<Stage>, Next) {
    match (stage, end.ok) {
        (None, _) => (None, Next::Normal),
        // The turn that ended was not the task's: it still waits.
        (Some(Stage::Assigned), _) => (Some(Stage::Assigned), Next::Normal),
        (Some(Stage::Planning), false) => (None, Next::LeftPlanning),
        (Some(_), false) => (None, Next::Normal),
        (Some(Stage::Planning), true) => match end.wrote_plan {
            Some(plan) => (Some(Stage::Building), Next::Build(plan.to_string())),
            // It asked instead of planning: its answer is the question,
            // and the reply comes back to a turn still planning.
            None => (Some(Stage::Planning), Next::Normal),
        },
        (Some(Stage::Building), true) if end.shipped => (None, Next::Normal),
        (Some(Stage::Building), true) => (Some(Stage::Shipping), Next::AskToShip),
        (Some(Stage::Shipping), true) => (None, Next::Normal),
    }
}

/// Every agent's stage. Agents not in the flow have none.
#[derive(Debug, Default)]
pub(crate) struct Flows {
    stages: Mutex<HashMap<AgentId, Stage>>,
    /// The text each agent's latest turn was started with, which finds
    /// where that turn starts in its conversation.
    prompts: Mutex<HashMap<AgentId, String>>,
}

impl Flows {
    fn stages(&self) -> std::sync::MutexGuard<'_, HashMap<AgentId, Stage>> {
        self.stages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `agent`'s turn starts with `prompt`.
    pub(crate) fn note_prompt(&self, agent: AgentId, prompt: &str) {
        self.prompts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(agent, prompt.to_string());
    }

    /// The text `agent`'s latest turn started with, if known.
    pub(crate) fn prompt_of(&self, agent: AgentId) -> Option<String> {
        self.prompts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&agent)
            .cloned()
    }

    /// `agent` got a new task: the work it starts plans first.
    pub(crate) fn assign(&self, agent: AgentId) {
        self.stages().insert(agent, Stage::Assigned);
    }

    /// `agent`'s work starts: a task waiting for it plans now. Returns
    /// whether this turn plans.
    pub(crate) fn begin(&self, agent: AgentId) -> bool {
        let mut stages = self.stages();
        match stages.get_mut(&agent) {
            Some(stage @ Stage::Assigned) => {
                *stage = Stage::Planning;
                true
            }
            Some(Stage::Planning) => true,
            _ => false,
        }
    }

    /// `agent`'s stage, if it is in the flow.
    pub(crate) fn stage(&self, agent: AgentId) -> Option<Stage> {
        self.stages().get(&agent).copied()
    }

    /// `agent` leaves the flow (its build could not start).
    pub(crate) fn leave(&self, agent: AgentId) {
        self.stages().remove(&agent);
    }

    /// Record that `agent`'s turn ended as `end`; returns what to do next.
    pub(crate) fn turn_ended(&self, agent: AgentId, end: TurnEnd<'_>) -> Next {
        let mut stages = self.stages();
        let (stage, next) = advance(stages.remove(&agent), end);
        if let Some(stage) = stage {
            stages.insert(agent, stage);
        }
        next
    }
}

#[cfg(test)]
mod tests;
