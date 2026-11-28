//! Why an agent that isn't working is idle, for its peers: what
//! `list_agents` and `get_agent_state` show as its `state`
//! ([`state_of`]).
//!
//! Most of it is read where it is kept: a start in the cloud and a
//! failure from the agent's status ([`super::subagents::AgentStatus`]),
//! a new member of a cloud channel not yet started from the hub's cloud
//! starts. Kept here: how much work waits in each queue, noted as the
//! queue changes ([`super::hud::send_queued`]), the agents the stall
//! sweep gave up on, and the sweep's counts ([`super::stalls`]).
//!
//! # Invariants
//!
//! * Working always wins over why an agent would be idle, but a turn
//!   waiting on its machine says so: nothing of it runs yet.
//! * Starting a turn clears a stall: the agent is no longer idle.

use super::subagents::{lock, AgentHub, LiveState};
use aura_core_types::AgentId;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

/// What peers are told of an agent's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StateView {
    pub state: &'static str,
    pub why: Option<String>,
}

/// How long each agent's queue is, shared with the queues that keep it
/// ([`super::queue::WorkQueues::counted`]).
pub(crate) type QueueCounts = Arc<Mutex<HashMap<AgentId, usize>>>;

/// Where an agent the stall sweep saw idle is with its one retry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Retry {
    /// Not asked to carry on yet
    #[default]
    No,
    /// Asked; the turn that asks has not started yet
    Asked,
    /// The turn that asked started: the retry is spent
    Spent,
}

/// Queue lengths, stalls and the stall sweep's counts.
#[derive(Default)]
pub(crate) struct Standings {
    queued: QueueCounts,
    stalled: Mutex<HashSet<AgentId>>,
    /// Sweeps in a row each agent owing answers was seen idle, and how
    /// far it is with its retry.
    strikes: Mutex<HashMap<AgentId, (u8, Retry)>>,
}

impl Standings {
    /// The queue lengths, for the queues to keep as they change.
    pub(crate) fn counts(&self) -> QueueCounts {
        Arc::clone(&self.queued)
    }

    /// `count` pieces of work wait in `agent`'s queue, for a test with no
    /// queues behind it.
    #[cfg(test)]
    pub(crate) fn set_queued(&self, agent: AgentId, count: usize) {
        let mut queued = lock(&self.queued);
        if count == 0 {
            queued.remove(&agent);
        } else {
            queued.insert(agent, count);
        }
    }

    pub(crate) fn queued(&self, agent: AgentId) -> Option<usize> {
        lock(&self.queued).get(&agent).copied()
    }

    /// The stall sweep gave up on `agent`.
    pub(crate) fn stall(&self, agent: AgentId) {
        lock(&self.stalled).insert(agent);
    }

    /// `agent`'s turn starts: it is not stalled any more. The turn that
    /// carries the stall sweep's retry spends it; any other turn is new
    /// work, which gets a retry of its own if it stalls too.
    pub(crate) fn started(&self, agent: AgentId) {
        lock(&self.stalled).remove(&agent);
        let mut strikes = lock(&self.strikes);
        match strikes.get_mut(&agent) {
            Some((count, retry @ Retry::Asked)) => {
                *count = 0;
                *retry = Retry::Spent;
            }
            Some(_) => {
                strikes.remove(&agent);
            }
            None => {}
        }
    }

    pub(crate) fn is_stalled(&self, agent: AgentId) -> bool {
        lock(&self.stalled).contains(&agent)
    }

    /// One more sweep saw `agent` idle while it owes answers: how many in
    /// a row now, and whether it was retried already.
    pub(crate) fn strike(&self, agent: AgentId) -> (u8, bool) {
        let mut strikes = lock(&self.strikes);
        let entry = strikes.entry(agent).or_default();
        entry.0 = entry.0.saturating_add(1);
        (entry.0, entry.1 != Retry::No)
    }

    /// A sweep saw `agent` busy: its count starts again, keeping whether
    /// it was retried.
    pub(crate) fn unstrike(&self, agent: AgentId) {
        if let Some(entry) = lock(&self.strikes).get_mut(&agent) {
            entry.0 = 0;
        }
    }

    /// `agent` was retried; its count starts again.
    pub(crate) fn retried(&self, agent: AgentId) {
        lock(&self.strikes).insert(agent, (0, Retry::Asked));
    }

    /// Forget the counts of agents not in `owing`, which owe nothing now.
    pub(crate) fn keep_strikes(&self, owing: &HashSet<AgentId>) {
        lock(&self.strikes).retain(|agent, _| owing.contains(agent));
    }
}

/// What peers are told of an agent whose turn waits for the one already
/// under way on its cloud machine, and why.
pub(crate) const WAITING_STATE: &str = "waiting for its machine";
const WAITING_WHY: &str =
    "its machine is still running an earlier turn, which must end before this one starts";

/// What `agent`'s peers are told of it: `waiting for its machine`,
/// `working`, `starting in the cloud` (at which step), `stalled` or
/// `failed` (with why), `queued` or `idle`. Its rows show the same
/// [`AgentHub::live_state`].
pub(crate) fn state_of(hub: &AgentHub, agent: AgentId) -> StateView {
    let view = |state, why| StateView { state, why };
    match hub.live_state(agent) {
        // Its turn counts as working, but nothing of it runs yet.
        LiveState::Waiting => view(WAITING_STATE, Some(WAITING_WHY.to_string())),
        LiveState::Reconnecting => view(
            "working",
            Some("Aura reconnects to the turn running on its machine".to_string()),
        ),
        LiveState::Working => view("working", None),
        LiveState::SignIn(why) => view(
            "waiting for sign-in",
            Some(format!("{why}; it carries on once a sign-in works again")),
        ),
        LiveState::Starting(step) => {
            let at = step.map_or_else(String::new, |step| {
                format!(
                    " (at {}, step {} of {})",
                    step.label, step.index, step.total
                )
            });
            view(
                "starting in the cloud",
                Some(format!("its work waits until its machine is up{at}")),
            )
        }
        LiveState::Failed { why, stalled } => {
            view(if stalled { "stalled" } else { "failed" }, Some(why))
        }
        LiveState::Queued(count) => view(
            "queued",
            Some(format!("{count} piece(s) of work wait for it to be free")),
        ),
        LiveState::Idle => view("idle", None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strikes_count_sweeps_in_a_row_and_remember_a_retry() {
        let standings = Standings::default();
        let agent = AgentId::new([1; 32]);
        assert_eq!(standings.strike(agent), (1, false));
        assert_eq!(standings.strike(agent), (2, false));
        standings.retried(agent);
        standings.unstrike(agent);
        assert_eq!(standings.strike(agent), (1, true));
        standings.keep_strikes(&HashSet::new());
        assert_eq!(
            standings.strike(agent),
            (1, false),
            "forgotten once it owes nothing"
        );
    }

    #[test]
    fn the_retrys_own_turn_spends_it_and_new_work_clears_it() {
        let standings = Standings::default();
        let agent = AgentId::new([1; 32]);
        standings.strike(agent);
        standings.retried(agent);
        standings.started(agent);
        assert_eq!(standings.strike(agent), (1, true), "the retry's turn ran");
        standings.started(agent);
        assert_eq!(
            standings.strike(agent),
            (1, false),
            "a turn of new work gets its own retry"
        );
    }
}
