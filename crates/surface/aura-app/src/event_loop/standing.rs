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
//! * Working always wins over why an agent would be idle.
//! * Starting a turn clears a stall: the agent is no longer idle.

use super::subagents::{lock, AgentHub};
use aura_core_types::AgentId;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

/// What peers are told of an agent's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StateView {
    pub state: &'static str,
    pub why: Option<String>,
}

/// Queue lengths, stalls and the stall sweep's counts.
#[derive(Default)]
pub(crate) struct Standings {
    queued: Mutex<HashMap<AgentId, usize>>,
    stalled: Mutex<HashSet<AgentId>>,
    /// Sweeps in a row each agent owing answers was seen idle, and
    /// whether it was retried already.
    strikes: Mutex<HashMap<AgentId, (u8, bool)>>,
}

impl Standings {
    /// `count` pieces of work wait in `agent`'s queue.
    pub(crate) fn set_queued(&self, agent: AgentId, count: usize) {
        let mut queued = lock(&self.queued);
        if count == 0 {
            queued.remove(&agent);
        } else {
            queued.insert(agent, count);
        }
    }

    fn queued(&self, agent: AgentId) -> Option<usize> {
        lock(&self.queued).get(&agent).copied()
    }

    /// The stall sweep gave up on `agent`.
    pub(crate) fn stall(&self, agent: AgentId) {
        lock(&self.stalled).insert(agent);
    }

    /// `agent`'s turn starts: it is not stalled any more.
    pub(crate) fn started(&self, agent: AgentId) {
        lock(&self.stalled).remove(&agent);
    }

    fn is_stalled(&self, agent: AgentId) -> bool {
        lock(&self.stalled).contains(&agent)
    }

    /// One more sweep saw `agent` idle while it owes answers: how many in
    /// a row now, and whether it was retried already.
    pub(crate) fn strike(&self, agent: AgentId) -> (u8, bool) {
        let mut strikes = lock(&self.strikes);
        let entry = strikes.entry(agent).or_default();
        entry.0 = entry.0.saturating_add(1);
        *entry
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
        lock(&self.strikes).insert(agent, (0, true));
    }

    /// Forget the counts of agents not in `owing`, which owe nothing now.
    pub(crate) fn keep_strikes(&self, owing: &HashSet<AgentId>) {
        lock(&self.strikes).retain(|agent, _| owing.contains(agent));
    }
}

/// What `agent`'s peers are told of it: `working`, `starting in the
/// cloud`, `stalled` or `failed` (with why), `queued` or `idle`.
pub(crate) fn state_of(hub: &AgentHub, agent: AgentId) -> StateView {
    let view = |state, why| StateView { state, why };
    if hub.is_working(agent) {
        return view("working", None);
    }
    if hub.status.is_starting(agent) || lock(&hub.cloud_starts).contains(&agent) {
        return view(
            "starting in the cloud",
            Some("its work waits until its machine is up".to_string()),
        );
    }
    if let Some(why) = hub.status.failure(agent) {
        let state = if hub.standing.is_stalled(agent) {
            "stalled"
        } else {
            "failed"
        };
        return view(state, Some(why));
    }
    match hub.standing.queued(agent) {
        Some(count) => view(
            "queued",
            Some(format!("{count} piece(s) of work wait for it to be free")),
        ),
        None => view("idle", None),
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
}
