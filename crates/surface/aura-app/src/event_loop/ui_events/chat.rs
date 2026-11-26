//! UI events that send an agent work or stop it.

use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::delivery::{deliver_with_images, submit};
use crate::event_loop::steering::SteerSource;
use crate::event_loop::turns::Work;
use crate::event_loop::{channels, LoopState};
use aura_core_types::AgentId;
use aura_ui::{UiCommand, UiEvent};
use tracing::debug;

/// Messages, commands, compaction, builds and cancelling.
pub(super) async fn handle(state: &mut LoopState<'_>, event: UiEvent) {
    match event {
        UiEvent::UserMessage {
            agent_id,
            text,
            shown,
            images,
        } => {
            let agent_id = state.agent_or_active(&agent_id);
            if !channels::route_user_message(state, agent_id, &text, &images, shown).await {
                deliver_with_images(state, agent_id, SteerSource::User, text, images, shown).await;
            }
        }
        UiEvent::RunCommand {
            agent_id,
            command,
            cols,
        } => {
            let agent_id = state.agent_or_active(&agent_id);
            submit(state, agent_id, Work::Command { command, cols }).await;
        }
        UiEvent::Cancel(agent_id) => {
            let agent_id = state.agent_or_active(&agent_id);
            cancel(state, agent_id).await;
        }
        UiEvent::StopCommand(agent_id) => {
            let agent_id = state.agent_or_active(&agent_id);
            stop_command(state, agent_id).await;
        }
        UiEvent::Compact { agent_id, focus } => {
            let agent_id = state.agent_or_active(&agent_id);
            submit(state, agent_id, Work::Compact { focus }).await;
        }
        UiEvent::BuildPlan { agent_id, path } => {
            let agent_id = state.agent_or_active(&agent_id);
            submit(state, agent_id, Work::Build { path }).await;
        }
        // `domain_of` routes no other event here.
        _ => {}
    }
}

/// Cancel the work of `agent_id`.
async fn cancel(state: &mut LoopState<'_>, agent_id: AgentId) {
    debug!(%agent_id, "Cancel received");
    // A call waiting for an answer holds the turn; deny it so the
    // turn sees the cancellation.
    state.approvals.cancel_agent(agent_id).await;
    if state.cloud.cancel(agent_id) {
        return;
    }
    if !state.hub.cancel(agent_id) {
        AgentUi::new(state.commands.clone(), agent_id)
            .send(UiCommand::SetStatus("Cancelled".to_string()))
            .await;
    }
}

/// Stop the commands `agent_id` is running.
async fn stop_command(state: &LoopState<'_>, agent_id: AgentId) {
    let stopped = aura_exec_tools::stop_commands(&agent_id.to_hex());
    debug!(%agent_id, stopped, "Stop command received");
    let status = match stopped {
        0 => "No command running".to_string(),
        1 => "Stopping the command".to_string(),
        n => format!("Stopping {n} commands"),
    };
    AgentUi::new(state.commands.clone(), agent_id)
        .send(UiCommand::SetStatus(status))
        .await;
}
