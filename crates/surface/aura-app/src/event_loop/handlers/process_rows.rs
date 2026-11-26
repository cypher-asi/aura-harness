//! Processes tab handlers: turn rows and killing a row's work.

use crate::event_loop::processes::warn_kill_failed;
use crate::event_loop::LoopState;
use aura_core_types::time::now_ms;
use aura_core_types::AgentId;
use aura_ui::events::{ProcessKind, ProcessStatus, ProcessSummary};
use aura_ui::UiCommand;
use tracing::info;

/// Processes tab row for a turn of `agent_id` on `model`, answering
/// `prompt` (or doing `prompt`, for work that isn't a model reply).
pub(in crate::event_loop) fn turn_summary(
    agent_id: AgentId,
    prompt: &str,
    model: &str,
) -> ProcessSummary {
    let agent = agent_id.to_hex();
    ProcessSummary {
        id: crate::event_loop::processes::turn_id(agent_id),
        kind: ProcessKind::Turn,
        label: prompt.lines().next().unwrap_or_default().to_string(),
        detail: prompt.to_string(),
        status: ProcessStatus::Running,
        jump_agent_id: agent.clone(),
        owner_agent_id: agent,
        started_ms: now_ms(),
        model: Some(model.to_string()),
        killable: true,
        ..ProcessSummary::default()
    }
}

/// Kill what the Processes tab row `id` runs. A turn or task waiting for
/// an approval is denied, so it sees the cancellation.
pub(in crate::event_loop) async fn handle_kill_process(state: &LoopState<'_>, id: &str) {
    let command = match state.processes.kill(id) {
        Ok(killed) => {
            info!(process = %id, kind = killed.kind.label(), owner = %killed.owner, "Killed from the Processes tab");
            let agent = id
                .strip_prefix("turn:")
                .or_else(|| id.strip_prefix("task:"))
                .and_then(|hex| AgentId::from_hex(hex).ok());
            if let Some(agent) = agent {
                state.approvals.cancel_agent(agent).await;
            }
            UiCommand::ShowSuccess(format!("Stopped {}", killed.label))
        }
        Err(problem) => {
            warn_kill_failed(id, &problem);
            UiCommand::ShowWarning(problem)
        }
    };
    let _ = state.commands.send(command).await;
}
