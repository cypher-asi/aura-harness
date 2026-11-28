//! How each agent's work went, for its rows: failed, and why, or starting
//! on a cloud machine of its own.
//!
//! A failure is shown on the agent's row in red until its next turn
//! starts. Nothing here is stored: a restart shows every agent as it is.

use super::{lock, AgentHub};
use aura_core_types::AgentId;
use aura_ui::events::AgentSummary;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tracing::warn;

/// Longest failure reason a row carries, in characters.
const MAX_ERROR_CHARS: usize = 200;

/// The agents whose work failed, and the ones starting in the cloud.
#[derive(Default)]
pub(crate) struct AgentStatus {
    failed: Mutex<HashMap<AgentId, String>>,
    starting: Mutex<HashSet<AgentId>>,
}

impl AgentStatus {
    /// Note that `agent`'s work failed because of `why`.
    pub(crate) fn fail(&self, agent: AgentId, why: &str) {
        warn!(agent = %agent.to_hex(), %why, "Agent failed");
        let why: String = why.trim().chars().take(MAX_ERROR_CHARS).collect();
        lock(&self.failed).insert(agent, why);
        lock(&self.starting).remove(&agent);
    }

    /// Forget `agent`'s failure: it works again. Returns whether it had
    /// one.
    pub(crate) fn clear(&self, agent: AgentId) -> bool {
        lock(&self.failed).remove(&agent).is_some()
    }

    /// Why `agent` failed, if it did.
    pub(crate) fn failure(&self, agent: AgentId) -> Option<String> {
        lock(&self.failed).get(&agent).cloned()
    }

    /// Note whether `agent` is starting on a cloud machine.
    pub(crate) fn set_starting(&self, agent: AgentId, starting: bool) {
        let mut set = lock(&self.starting);
        if starting {
            set.insert(agent);
        } else {
            set.remove(&agent);
        }
    }

    /// Whether `agent` is starting on a cloud machine.
    pub(crate) fn is_starting(&self, agent: AgentId) -> bool {
        lock(&self.starting).contains(&agent)
    }
}

impl AgentHub {
    /// Mark `row`, `agent_id`'s, with how its work went.
    pub(super) fn mark_status(&self, agent_id: AgentId, row: &mut AgentSummary) {
        row.error = self.status.failure(agent_id);
        row.starting = row.error.is_none() && self.status.is_starting(agent_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failure_shows_until_cleared_and_ends_a_start() {
        let status = AgentStatus::default();
        let agent = AgentId::generate();
        status.set_starting(agent, true);
        assert!(status.is_starting(agent));
        status.fail(agent, &format!("  no machine: {}", "x".repeat(400)));
        assert!(!status.is_starting(agent), "a failed start isn't starting");
        let why = status.failure(agent).unwrap();
        assert!(why.starts_with("no machine: x"), "{why}");
        assert_eq!(why.chars().count(), MAX_ERROR_CHARS);
        assert!(status.clear(agent));
        assert_eq!(status.failure(agent), None);
        assert!(!status.clear(agent));
    }
}
