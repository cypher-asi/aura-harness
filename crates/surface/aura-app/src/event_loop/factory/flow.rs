//! The software factory's flow for an agent given a task in a channel:
//! plan, then build the plan, then ship it, with no one in between.
//!
//! ```text
//! assignment ──> Planning ──plan written──> Building ──done──> ship
//!                   │                                      (local: the hub runs git_ship;
//!                   └─ no plan (it asked) ─> Planning        cloud: Shipping, one turn
//!                                                             asking the agent to ship)
//! ```
//!
//! The stages are pure state here; the event loop applies them
//! (see `super::hooks`). A failed or stopped turn leaves the flow, as the
//! work it was in is given up.

use aura_core_types::AgentId;
use std::collections::HashMap;
use std::sync::Mutex;

/// Where an agent is in the flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Its turns run in plan mode until one writes a plan; `before` is
    /// the plan it had when the task came.
    Planning { before: Option<String> },
    /// Its turn builds the plan it wrote.
    Building,
    /// A cloud agent was asked to ship what it built.
    Shipping,
}

/// How an agent's turn ended, as far as the flow cares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TurnEnd<'a> {
    /// It ended well (not failed or stopped)
    pub ok: bool,
    /// The plan the agent has now, if any
    pub plan: Option<&'a str>,
    /// The turn pushed its work (a push marker was in its results)
    pub shipped: bool,
    /// The agent runs in the cloud, where the hub can't run git for it
    pub cloud: bool,
}

/// What the loop does after the turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Next {
    /// Nothing more: answer what the turn was asked, as always.
    Normal,
    /// Build the plan at this path next; the answer waits for the build.
    Build(String),
    /// Ship the agent's workspace now, then answer.
    Ship,
    /// Ask the cloud agent to ship; the answer waits for that turn.
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
    let Some(stage) = stage else {
        return (None, Next::Normal);
    };
    if !end.ok {
        return (None, Next::Normal);
    }
    match stage {
        Stage::Planning { before } => match end.plan {
            Some(plan) if before.as_deref() != Some(plan) => {
                (Some(Stage::Building), Next::Build(plan.to_string()))
            }
            // It asked instead of planning: its answer is the question,
            // and the reply comes back to a turn still planning.
            _ => (Some(Stage::Planning { before }), Next::Normal),
        },
        Stage::Building if end.shipped => (None, Next::Normal),
        Stage::Building if end.cloud => (Some(Stage::Shipping), Next::AskToShip),
        Stage::Building => (None, Next::Ship),
        Stage::Shipping => (None, Next::Normal),
    }
}

/// Every agent's stage. Agents not in the flow have none.
#[derive(Debug, Default)]
pub(crate) struct Flows {
    stages: Mutex<HashMap<AgentId, Stage>>,
}

impl Flows {
    fn stages(&self) -> std::sync::MutexGuard<'_, HashMap<AgentId, Stage>> {
        self.stages
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `agent` got a new task: it plans first. `before` is the plan it has.
    pub(crate) fn start(&self, agent: AgentId, before: Option<String>) {
        self.stages().insert(agent, Stage::Planning { before });
    }

    /// Whether `agent`'s next turn plans.
    pub(crate) fn is_planning(&self, agent: AgentId) -> bool {
        matches!(self.stages().get(&agent), Some(Stage::Planning { .. }))
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
mod tests {
    use super::*;

    const fn end(plan: Option<&str>) -> TurnEnd<'_> {
        TurnEnd {
            ok: true,
            plan,
            shipped: false,
            cloud: false,
        }
    }

    #[test]
    fn a_written_plan_is_built_next_and_the_answer_waits() {
        let flows = Flows::default();
        let agent = AgentId::generate();
        flows.start(agent, None);
        assert!(flows.is_planning(agent));
        let next = flows.turn_ended(agent, end(Some(".aura/plans/x.plan.md")));
        assert_eq!(next, Next::Build(".aura/plans/x.plan.md".to_string()));
        assert!(next.defers_answer());
        assert!(!flows.is_planning(agent));
    }

    #[test]
    fn a_built_plan_ships_and_leaves_the_flow() {
        let flows = Flows::default();
        let agent = AgentId::generate();
        flows.start(agent, None);
        flows.turn_ended(agent, end(Some("p")));
        assert_eq!(flows.turn_ended(agent, end(Some("p"))), Next::Ship);
        assert_eq!(flows.turn_ended(agent, end(Some("p"))), Next::Normal);
    }

    #[test]
    fn a_turn_that_asked_instead_of_planning_keeps_planning() {
        let flows = Flows::default();
        let agent = AgentId::generate();
        flows.start(agent, Some("old".to_string()));
        assert_eq!(flows.turn_ended(agent, end(Some("old"))), Next::Normal);
        assert!(flows.is_planning(agent), "the reply comes back to planning");
        assert_eq!(
            flows.turn_ended(agent, end(Some("new"))),
            Next::Build("new".to_string())
        );
    }

    #[test]
    fn a_cloud_agent_is_asked_to_ship_unless_it_did() {
        let built = (Some(Stage::Shipping), Next::AskToShip);
        let cloud = TurnEnd {
            cloud: true,
            ..end(None)
        };
        assert_eq!(advance(Some(Stage::Building), cloud), built);
        let shipped = TurnEnd {
            shipped: true,
            ..cloud
        };
        assert_eq!(
            advance(Some(Stage::Building), shipped),
            (None, Next::Normal)
        );
        assert_eq!(advance(Some(Stage::Shipping), cloud), (None, Next::Normal));
    }

    #[test]
    fn a_failed_turn_leaves_the_flow() {
        let failed = TurnEnd {
            ok: false,
            ..end(Some("p"))
        };
        for stage in [
            Stage::Planning { before: None },
            Stage::Building,
            Stage::Shipping,
        ] {
            assert_eq!(advance(Some(stage), failed), (None, Next::Normal));
        }
    }

    #[test]
    fn agents_outside_the_flow_are_left_alone() {
        assert_eq!(advance(None, end(Some("p"))), (None, Next::Normal));
    }
}
