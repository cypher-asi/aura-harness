//! Run traces for TUI conversations.
//!
//! Each agent's conversation is one run under `AURA_HOME/runs/`. The
//! recorder starts on the agent's first turn and is finished when the
//! session is reset or the TUI quits; a scorecard is written after
//! every turn so `aura runs` sees live data.

use std::sync::Arc;

use aura_agent::{AgentError, AgentLoopResult};
use aura_agent_trace::{RunMeta, RunRecorder, RunSource};
use aura_core_types::AgentId;

use super::LoopState;

/// Recorder for the active agent, started on first use. `None` when
/// tracing is off or the bundle can't be created.
pub(super) fn recorder(state: &mut LoopState<'_>, model: &str) -> Option<Arc<RunRecorder>> {
    let agent_id = state.session.agent_id();
    if let Some(rec) = state.traces.get(&agent_id) {
        return Some(Arc::clone(rec));
    }
    let mut meta = RunMeta::new(model, RunSource::Tui);
    meta.workspace = state.session.kernel.agent_workspace().display().to_string();
    meta.mode = state.session.definition.clone().unwrap_or_default();
    meta.task_id = Some(format!("tui:{}", agent_id.to_hex()));
    let rec = RunRecorder::start_default(meta)?;
    state.traces.insert(agent_id, Arc::clone(&rec));
    Some(rec)
}

/// Record how a turn ended and refresh the run's scorecard.
pub(super) fn end_turn(
    rec: &RunRecorder,
    result: &Result<AgentLoopResult, AgentError>,
    cancelled: bool,
) {
    if cancelled {
        rec.interrupted();
    }
    match result {
        Ok(result) => rec.end_turn(result),
        Err(e) => rec.end_turn(&AgentLoopResult {
            llm_error: Some(e.to_string()),
            ..AgentLoopResult::default()
        }),
    }
    rec.finish();
}

/// Finish the run of `agent_id`, so its next turn starts a new run.
pub(super) fn close(state: &mut LoopState<'_>, agent_id: AgentId) {
    if let Some(rec) = state.traces.remove(&agent_id) {
        rec.finish();
    }
}

/// Finish every open run.
pub(super) fn close_all(state: &mut LoopState<'_>) {
    for (_, rec) in state.traces.drain() {
        rec.finish();
    }
}
