//! How each agent's work went, for its rows: failed, and why, starting
//! on a cloud machine of its own, or waiting for the turn already under
//! way on it.
//!
//! A failure is shown on the agent's row in red until a new start or turn
//! begins. Rows and peers read the same state ([`AgentHub::live_state`]),
//! so a row never says `Failed` while peers hear `starting`. Nothing here
//! is stored: a restart shows every agent as it is.

use super::{lock, AgentHub};
use aura_core_types::AgentId;
use aura_ui::events::{AgentSummary, StartStep};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tracing::warn;

/// Longest failure reason a row carries, in characters.
const MAX_ERROR_CHARS: usize = 600;

/// The agents whose work failed, and the ones starting in the cloud.
#[derive(Default)]
pub(crate) struct AgentStatus {
    failed: Mutex<HashMap<AgentId, String>>,
    starting: Mutex<HashSet<AgentId>>,
    /// Cloud agents whose turn waits for the one already under way on
    /// their machine ([`aura_engine_cloud::hub::WAITING`])
    waiting: Mutex<HashSet<AgentId>>,
    /// Cloud agents whose running turn the app reconnects to
    /// ([`aura_engine_cloud::hub::RECONNECTING`])
    reconnecting: Mutex<HashSet<AgentId>>,
    /// The steps of each start under way, and the one it is at
    steps: Mutex<HashMap<AgentId, StartSteps>>,
    /// Members whose last start failed for good, why, and when a start
    /// last tried ([`Self::blocked_start`])
    blocked: Mutex<HashMap<AgentId, (String, u64)>>,
}

/// A start's plan, by stage key and label, and the step it is at.
#[derive(Default)]
struct StartSteps {
    plan: Vec<String>,
    at: Option<StartStep>,
}

impl AgentStatus {
    /// Note that `agent`'s work failed because of `why`.
    pub(crate) fn fail(&self, agent: AgentId, why: &str) {
        warn!(agent = %agent.to_hex(), %why, "Agent failed");
        let why: String = why.trim().chars().take(MAX_ERROR_CHARS).collect();
        lock(&self.failed).insert(agent, why);
        lock(&self.starting).remove(&agent);
        lock(&self.waiting).remove(&agent);
        lock(&self.steps).remove(&agent);
    }

    /// `agent` begins a new start on a cloud machine: what failed before
    /// is behind it, and its row says it starts from now.
    pub(crate) fn begin_start(&self, agent: AgentId) {
        lock(&self.failed).remove(&agent);
        lock(&self.steps).remove(&agent);
        lock(&self.starting).insert(agent);
    }

    /// The stages `agent`'s start runs, in order, by key. Returns whether
    /// that changed what its row shows.
    pub(crate) fn plan_start(&self, agent: AgentId, plan: Vec<String>) -> bool {
        if !self.is_starting(agent) {
            return false;
        }
        let mut steps = lock(&self.steps);
        let start = steps.entry(agent).or_default();
        start.plan = plan;
        let Some(at) = start.at.as_mut() else {
            return false;
        };
        let total = start.plan.len().max(at.index);
        std::mem::replace(&mut at.total, total) != total
    }

    /// `agent`'s start began stage `key`, `label`, at `now_ms`. Returns
    /// whether that changed what its row shows.
    pub(crate) fn start_stage(
        &self,
        agent: AgentId,
        (key, label): (&str, &str),
        now_ms: u64,
    ) -> bool {
        if !self.is_starting(agent) {
            return false;
        }
        let mut steps = lock(&self.steps);
        let start = steps.entry(agent).or_default();
        if !start.plan.iter().any(|known| known == key) {
            start.plan.push(key.to_string());
        }
        let index = start
            .plan
            .iter()
            .position(|known| known == key)
            .unwrap_or(0)
            + 1;
        if start
            .at
            .as_ref()
            .is_some_and(|at| at.index == index && at.label == label)
        {
            return false;
        }
        start.at = Some(StartStep {
            label: label.to_string(),
            index,
            total: start.plan.len(),
            since_ms: now_ms,
        });
        true
    }

    /// Note that `agent`'s start, tried at `now_ms`, failed `why` for a
    /// reason a new try won't fix by itself.
    pub(crate) fn block_start(&self, agent: AgentId, why: &str, now_ms: u64) {
        lock(&self.blocked).insert(agent, (why.to_string(), now_ms));
    }

    /// `agent` may start again: it started, or the user retries it.
    pub(crate) fn unblock_start(&self, agent: AgentId) {
        lock(&self.blocked).remove(&agent);
    }

    /// Why `agent` must not start at `now_ms`, if its last start failed
    /// for good less than `after_ms` ago. Past that, one try is let
    /// through and the clock starts again.
    pub(crate) fn blocked_start(
        &self,
        agent: AgentId,
        now_ms: u64,
        after_ms: u64,
    ) -> Option<String> {
        let mut blocked = lock(&self.blocked);
        let (why, tried) = blocked.get_mut(&agent)?;
        if now_ms.saturating_sub(*tried) >= after_ms {
            *tried = now_ms;
            return None;
        }
        Some(why.clone())
    }

    /// The step `agent`'s start is at, while it starts.
    pub(crate) fn start_step(&self, agent: AgentId) -> Option<StartStep> {
        lock(&self.steps)
            .get(&agent)
            .and_then(|start| start.at.clone())
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
            lock(&self.steps).remove(&agent);
        }
    }

    /// Whether `agent` is starting on a cloud machine.
    pub(crate) fn is_starting(&self, agent: AgentId) -> bool {
        lock(&self.starting).contains(&agent)
    }

    /// Note whether `agent`'s turn waits for the one under way on its
    /// machine. Returns whether that changed.
    pub(crate) fn set_waiting(&self, agent: AgentId, waiting: bool) -> bool {
        let mut set = lock(&self.waiting);
        if waiting {
            set.insert(agent)
        } else {
            set.remove(&agent)
        }
    }

    /// Whether `agent`'s turn waits for the one under way on its machine.
    pub(crate) fn is_waiting(&self, agent: AgentId) -> bool {
        lock(&self.waiting).contains(&agent)
    }

    /// Whether the app reconnects to `agent`'s running cloud turn.
    pub(crate) fn is_reconnecting(&self, agent: AgentId) -> bool {
        lock(&self.reconnecting).contains(&agent)
    }

    /// Note whether the app reconnects to `agent`'s running cloud turn.
    /// Returns whether that changed.
    pub(crate) fn set_reconnecting(&self, agent: AgentId, on: bool) -> bool {
        let mut set = lock(&self.reconnecting);
        if on {
            set.insert(agent)
        } else {
            set.remove(&agent)
        }
    }
}

impl AgentHub {
    /// Mark `row`, `agent_id`'s, with what it is doing now: the same
    /// [`Self::live_state`] its peers are told.
    pub(super) fn mark_status(&self, agent_id: AgentId, row: &mut AgentSummary) {
        let live = self.live_state(agent_id);
        row.error = None;
        row.starting = false;
        row.start_step = None;
        row.waiting = false;
        row.reconnecting = false;
        row.waiting_for_sign_in = false;
        match live {
            LiveState::Failed { why, .. } => row.error = Some(why),
            LiveState::Starting(step) => {
                row.starting = true;
                row.start_step = step;
            }
            LiveState::Waiting => row.waiting = true,
            LiveState::Reconnecting => row.reconnecting = true,
            LiveState::SignIn(_) => row.waiting_for_sign_in = true,
            LiveState::Working | LiveState::Queued(_) | LiveState::Idle => {}
        }
    }

    /// What `agent` is doing now, for its rows and its peers alike: one
    /// answer, in this order. A start or a turn under way comes before an
    /// earlier failure, which only shows while nothing else goes on.
    pub(crate) fn live_state(&self, agent: AgentId) -> LiveState {
        let status = &self.status;
        if status.is_waiting(agent) {
            return LiveState::Waiting;
        }
        if status.is_reconnecting(agent) {
            return LiveState::Reconnecting;
        }
        if self.is_working(agent) {
            return LiveState::Working;
        }
        if status.is_starting(agent) || lock(&self.cloud_starts).contains(&agent) {
            return LiveState::Starting(status.start_step(agent));
        }
        if let Some(why) = self.recovery.sign_in_wait(agent) {
            return LiveState::SignIn(why);
        }
        if let Some(why) = status.failure(agent) {
            let stalled = self.standing.is_stalled(agent);
            return LiveState::Failed { why, stalled };
        }
        match self.standing.queued(agent) {
            Some(count) => LiveState::Queued(count),
            None => LiveState::Idle,
        }
    }
}

/// What an agent is doing now ([`AgentHub::live_state`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LiveState {
    /// Its turn waits for the one already under way on its cloud machine
    Waiting,
    /// Aura reconnects to the turn running on its cloud machine
    Reconnecting,
    /// A turn of it runs
    Working,
    /// It starts on a cloud machine of its own, at this step if known
    Starting(Option<StartStep>),
    /// The model refused its sign-in, for this reason: it waits for one
    /// that works, its work kept
    SignIn(String),
    /// Its work failed, or stalled, because of `why`
    Failed {
        /// Why
        why: String,
        /// Whether it stalled rather than failed
        stalled: bool,
    },
    /// Work waits for it to be free
    Queued(usize),
    /// Nothing
    Idle,
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
        status.fail(agent, &format!("  no machine: {}", "x".repeat(900)));
        assert!(!status.is_starting(agent), "a failed start isn't starting");
        let why = status.failure(agent).unwrap();
        assert!(why.starts_with("no machine: x"), "{why}");
        assert_eq!(why.chars().count(), MAX_ERROR_CHARS);
        assert!(status.clear(agent));
        assert_eq!(status.failure(agent), None);
        assert!(!status.clear(agent));
    }

    #[test]
    fn a_new_start_clears_the_failure_and_follows_its_steps() {
        let status = AgentStatus::default();
        let agent = AgentId::generate();
        assert!(
            !status.start_stage(agent, ("connect", "Connecting"), 1),
            "not starting"
        );
        status.fail(agent, "the old reason");
        status.begin_start(agent);
        assert_eq!(status.failure(agent), None);
        assert!(status.start_stage(agent, ("connect", "Connecting"), 1));
        assert!(
            !status.start_stage(agent, ("connect", "Connecting"), 2),
            "the same step"
        );
        assert!(status.plan_start(agent, vec!["connect".into(), "snapshot".into()]));
        assert!(status.start_stage(agent, ("snapshot", "Preparing project snapshot"), 3));
        let step = status.start_step(agent).unwrap();
        assert_eq!((step.index, step.total, step.since_ms), (2, 2, 3));
        status.set_starting(agent, false);
        assert_eq!(
            status.start_step(agent),
            None,
            "a finished start has no step"
        );
    }
}
