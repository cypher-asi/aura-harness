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
//!
//! The stages outlive the app: they are saved with the inbox and put back
//! when it starts ([`super::saved`]). [`Flows::records`] and
//! [`Flows::restore`] are that boundary.

use aura_core_types::AgentId;
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, MutexGuard, PoisonError};

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
    /// The plan each building agent builds
    plans: Mutex<HashMap<AgentId, String>>,
    /// Agents whose cut-off factory work a restore queued again, until
    /// their next turn starts
    resumed: Mutex<HashSet<AgentId>>,
}

/// One agent's place in the flow, as it is saved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FlowRecord {
    pub agent: AgentId,
    pub stage: Stage,
    /// The plan it builds, while building or shipping
    pub plan: Option<String>,
    /// The text its latest turn started with
    pub prompt: Option<String>,
    /// Factory work of its stage is in flight or queued, and only the
    /// factory can start it again: a planning turn under way, the build,
    /// the ship
    pub pending: bool,
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Flows {
    fn stages(&self) -> MutexGuard<'_, HashMap<AgentId, Stage>> {
        locked(&self.stages)
    }

    /// `agent`'s turn starts with `prompt`. A restored agent's work runs
    /// again: it is no longer the restore's to start.
    pub(crate) fn note_prompt(&self, agent: AgentId, prompt: &str) {
        locked(&self.prompts).insert(agent, prompt.to_string());
        locked(&self.resumed).remove(&agent);
    }

    /// `agent`'s turn starts without a prompt of its own (a plan's build).
    pub(crate) fn note_started(&self, agent: AgentId) {
        locked(&self.resumed).remove(&agent);
    }

    /// The text `agent`'s latest turn started with, if known.
    pub(crate) fn prompt_of(&self, agent: AgentId) -> Option<String> {
        locked(&self.prompts).get(&agent).cloned()
    }

    /// Whether a restore queued `agent`'s cut-off factory work again and
    /// it hasn't started yet. A turn resumed on reopen skips such an
    /// agent: its work is queued already.
    // The hook RUS203's turn resume on reopen calls; until it lands only
    // the tests do.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn resumes_after_restart(&self, agent: AgentId) -> bool {
        locked(&self.resumed).contains(&agent)
    }

    /// The plan `agent` builds, while building or shipping.
    pub(crate) fn plan_of(&self, agent: AgentId) -> Option<String> {
        locked(&self.plans).get(&agent).cloned()
    }

    /// Every agent's place in the flow, oldest id first; `running` says
    /// whether an agent has a turn under way.
    pub(crate) fn records(&self, running: impl Fn(AgentId) -> bool) -> Vec<FlowRecord> {
        let stages: Vec<(AgentId, Stage)> = self.stages().iter().map(|(a, s)| (*a, *s)).collect();
        let resumed = locked(&self.resumed).clone();
        let mut records: Vec<FlowRecord> = stages
            .into_iter()
            .map(|(agent, stage)| FlowRecord {
                agent,
                stage,
                plan: self.plan_of(agent),
                prompt: self.prompt_of(agent),
                pending: match stage {
                    Stage::Assigned => false,
                    Stage::Planning => running(agent) || resumed.contains(&agent),
                    Stage::Building | Stage::Shipping => true,
                },
            })
            .collect();
        records.sort_by_cached_key(|record| record.agent.to_hex());
        records
    }

    /// Put `records` back, as the app starts. Those whose work is
    /// `pending` are marked resumed until their next turn starts.
    pub(crate) fn restore(&self, records: &[FlowRecord]) {
        for record in records {
            self.stages().insert(record.agent, record.stage);
            if let Some(plan) = &record.plan {
                locked(&self.plans).insert(record.agent, plan.clone());
            }
            if let Some(prompt) = &record.prompt {
                locked(&self.prompts).insert(record.agent, prompt.clone());
            }
            if record.pending {
                locked(&self.resumed).insert(record.agent);
            }
        }
    }

    /// Forget every stage, as a new process starts.
    #[cfg(test)]
    pub(crate) fn forget_all(&self) {
        self.stages().clear();
        locked(&self.prompts).clear();
        locked(&self.plans).clear();
        locked(&self.resumed).clear();
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
        self.forget_extras(agent);
    }

    fn forget_extras(&self, agent: AgentId) {
        locked(&self.plans).remove(&agent);
        locked(&self.resumed).remove(&agent);
    }

    /// Record that `agent`'s turn ended as `end`; returns what to do next.
    pub(crate) fn turn_ended(&self, agent: AgentId, end: TurnEnd<'_>) -> Next {
        let (stage, next) = {
            let mut stages = self.stages();
            let (stage, next) = advance(stages.remove(&agent), end);
            if let Some(stage) = stage {
                stages.insert(agent, stage);
            }
            (stage, next)
        };
        match (&stage, &next) {
            (_, Next::Build(plan)) => {
                locked(&self.plans).insert(agent, plan.clone());
            }
            (None, _) => self.forget_extras(agent),
            _ => {}
        }
        next
    }
}

#[cfg(test)]
mod tests;
