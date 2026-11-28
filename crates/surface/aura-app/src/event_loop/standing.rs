//! Why an agent that isn't working is idle, for its peers: what
//! `list_agents` and `get_agent_state` show as its `state` ([`Standings`]).
//!
//! The loop writes it: a new member of a cloud channel is starting in
//! the cloud while it moves to a machine of its own, an agent whose work
//! could not start failed to start, and one the stall sweep gave up on
//! stalled. How much work waits in its queue is noted as the queue
//! changes ([`super::hud::send_queued`]).
//!
//! # Invariants
//!
//! * A standing says why an agent is idle; working always wins over it.
//! * Starting work clears a failure or a stall: it is no longer idle.

use super::subagents::lock;
use aura_core_types::time::now_ms;
use aura_core_types::AgentId;
use std::collections::HashMap;
use std::sync::Mutex;

/// Why an idle agent is idle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Standing {
    /// Moving to a cloud machine of its own before its first work: the
    /// boot stage it reached, and what that stage waits on.
    Starting {
        stage: Option<String>,
        waiting: Option<String>,
    },
    /// Its work could not start, and why.
    FailedToStart(String),
    /// It owed reports with nothing running or queued, and why.
    Stalled(String),
}

/// What peers are told of an agent's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StateView {
    pub state: &'static str,
    pub why: Option<String>,
    pub since_ms: Option<u64>,
}

/// Each agent's [`Standing`], since when, and its queue's length.
#[derive(Default)]
pub(crate) struct Standings {
    standing: Mutex<HashMap<AgentId, (Standing, u64)>>,
    queued: Mutex<HashMap<AgentId, usize>>,
    /// The stall sweep's count of sweeps in a row each agent owing
    /// answers was seen idle, and whether it was retried already
    /// ([`super::stalls`]).
    strikes: Mutex<HashMap<AgentId, (u8, bool)>>,
}

impl Standings {
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
    pub(crate) fn keep_strikes(&self, owing: &std::collections::HashSet<AgentId>) {
        lock(&self.strikes).retain(|agent, _| owing.contains(agent));
    }
    /// `agent` stands as `standing` from now on.
    pub(crate) fn set(&self, agent: AgentId, standing: Standing) {
        lock(&self.standing).insert(agent, (standing, now_ms()));
    }

    /// `agent`, starting in the cloud, reached boot `stage` or waits on
    /// `waiting` now; since when it started stays as it was.
    pub(crate) fn note_boot(&self, agent: AgentId, stage: Option<&str>, waiting: Option<&str>) {
        let mut standings = lock(&self.standing);
        let Some((
            Standing::Starting {
                stage: at,
                waiting: on,
            },
            _,
        )) = standings.get_mut(&agent)
        else {
            return;
        };
        if let Some(stage) = stage {
            *at = Some(stage.to_string());
            *on = None;
        }
        if let Some(waiting) = waiting {
            *on = Some(waiting.to_string()).filter(|what| !what.is_empty());
        }
    }

    /// `agent` stands as nothing in particular any more.
    pub(crate) fn clear(&self, agent: AgentId) {
        lock(&self.standing).remove(&agent);
    }

    /// Clear a failure or a stall of `agent`, whose work starts now.
    pub(crate) fn started(&self, agent: AgentId) {
        let mut standings = lock(&self.standing);
        if matches!(
            standings.get(&agent),
            Some((Standing::FailedToStart(_) | Standing::Stalled(_), _))
        ) {
            standings.remove(&agent);
        }
    }

    /// How `agent` stands, and since when.
    pub(crate) fn get(&self, agent: AgentId) -> Option<(Standing, u64)> {
        lock(&self.standing).get(&agent).cloned()
    }

    /// `count` pieces of work wait in `agent`'s queue.
    pub(crate) fn set_queued(&self, agent: AgentId, count: usize) {
        let mut queued = lock(&self.queued);
        if count == 0 {
            queued.remove(&agent);
        } else {
            queued.insert(agent, count);
        }
    }

    /// What peers are told of `agent`, `working` or not.
    pub(crate) fn view(&self, agent: AgentId, working: bool) -> StateView {
        if working {
            return StateView {
                state: "working",
                why: None,
                since_ms: None,
            };
        }
        if let Some((standing, since)) = self.get(agent) {
            let (state, why) = match standing {
                Standing::Starting { stage, waiting } => {
                    ("starting in the cloud", Some(starting_why(stage, waiting)))
                }
                Standing::FailedToStart(why) => ("failed to start", Some(why)),
                Standing::Stalled(why) => ("stalled", Some(why)),
            };
            return StateView {
                state,
                why,
                since_ms: Some(since),
            };
        }
        match lock(&self.queued).get(&agent).copied() {
            Some(count) => StateView {
                state: "queued",
                why: Some(format!("{count} piece(s) of work wait for it to be free")),
                since_ms: None,
            },
            None => StateView {
                state: "idle",
                why: None,
                since_ms: None,
            },
        }
    }
}

/// What a start in the cloud is doing, from the boot `stage` it reached
/// and what that stage waits on.
fn starting_why(stage: Option<String>, waiting: Option<String>) -> String {
    match (stage, waiting) {
        (Some(stage), Some(waiting)) => format!("{stage}: {waiting}"),
        (Some(stage), None) => stage,
        (None, Some(waiting)) => waiting,
        (None, None) => "connecting to the cloud".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_wins_then_a_standing_then_the_queue() {
        let standings = Standings::default();
        let agent = AgentId::new([1; 32]);
        assert_eq!(standings.view(agent, false).state, "idle");
        standings.set_queued(agent, 2);
        assert_eq!(standings.view(agent, false).state, "queued");
        standings.set(
            agent,
            Standing::Starting {
                stage: None,
                waiting: None,
            },
        );
        standings.note_boot(agent, Some("Project snapshot"), None);
        standings.note_boot(agent, None, Some("boxd is building it"));
        let view = standings.view(agent, false);
        assert_eq!(view.state, "starting in the cloud");
        assert_eq!(
            view.why.as_deref(),
            Some("Project snapshot: boxd is building it")
        );
        assert!(view.since_ms.is_some());
        assert_eq!(standings.view(agent, true).state, "working");

        standings.started(agent);
        assert_eq!(
            standings.view(agent, false).state,
            "starting in the cloud",
            "a start goes on until its move ends"
        );
        standings.set(agent, Standing::FailedToStart("no model".into()));
        assert_eq!(standings.view(agent, false).state, "failed to start");
        standings.started(agent);
        standings.set_queued(agent, 0);
        assert_eq!(standings.view(agent, false).state, "idle");
    }
}
