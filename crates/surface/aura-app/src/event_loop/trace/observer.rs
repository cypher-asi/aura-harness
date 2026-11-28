//! Hosted agents' turns, each recorded in a run of its own.

use aura_agent::{AgentLoopEvent, AgentLoopResult};
use aura_agent_trace::{RunMeta, RunSource};
use aura_core_types::{AgentId, SubagentExit};
use aura_engine::{HostedTurn, TurnObserver};

use super::{lock, refresh, Traces};

/// Records hosted agents' turns in their own runs.
impl TurnObserver for Traces {
    fn turn_started(&self, turn: &HostedTurn<'_>) {
        let mut meta = RunMeta::new(turn.model, RunSource::Tui);
        meta.mode = "spawned".into();
        meta.spawned_by = turn.parent.and_then(|parent| self.run_id_of(parent));
        let Some(rec) = self.start(turn.agent_id, meta) else {
            return;
        };
        if let Some(run) = lock(&self.live).get_mut(&turn.agent_id) {
            run.in_turn = true;
        }
        rec.begin_turn(turn.prompt);
    }

    fn on_event(&self, agent_id: AgentId, event: &AgentLoopEvent) {
        if let Some(rec) = self.recorder(agent_id) {
            rec.on_event(event);
        }
    }

    fn turn_ended(&self, agent_id: AgentId, result: Option<&AgentLoopResult>, exit: &SubagentExit) {
        let Some(rec) = self.recorder(agent_id) else {
            return;
        };
        match result {
            Some(result) => rec.end_turn(result),
            None => rec.end_turn(&AgentLoopResult {
                llm_error: Some(exit_reason(exit)),
                ..AgentLoopResult::default()
            }),
        }
        refresh(&rec);
        let removed = lock(&self.live).get_mut(&agent_id).is_some_and(|run| {
            run.in_turn = false;
            run.removed
        });
        if removed {
            self.forget(agent_id);
        }
    }

    fn agent_removed(&self, agent_id: AgentId) {
        let in_turn = lock(&self.live).get_mut(&agent_id).is_some_and(|run| {
            run.removed = true;
            run.in_turn
        });
        if !in_turn {
            self.forget(agent_id);
        }
    }
}

impl Traces {
    /// Drop a hosted agent's run, already finished after its last turn.
    fn forget(&self, agent: AgentId) {
        let run = lock(&self.live).remove(&agent);
        if let Some(router) = &self.router {
            router.detach(agent);
        }
        drop(run);
    }
}

fn exit_reason(exit: &SubagentExit) -> String {
    match exit {
        SubagentExit::Completed => "completed".into(),
        SubagentExit::Failed { reason } => reason.clone(),
        SubagentExit::Cancelled => "cancelled".into(),
        SubagentExit::Timeout => "timed out".into(),
        SubagentExit::Rejected { reason } => format!("rejected: {reason}"),
    }
}
