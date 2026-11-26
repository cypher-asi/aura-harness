//! UI events that edit the queue of work waiting for an agent.

use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::delivery::send_queued_now;
use crate::event_loop::{hud, queue, LoopState};
use aura_core_types::AgentId;
use aura_ui::{UiCommand, UiEvent};
use tracing::debug;

/// Queue edits.
pub(super) async fn handle(state: &mut LoopState<'_>, event: UiEvent) {
    match event {
        UiEvent::SendQueuedNow(agent_id) => {
            let agent_id = state.agent_or_active(&agent_id);
            send_queued_now(state, agent_id).await;
        }
        UiEvent::RemoveQueued { agent_id, id } => {
            let agent_id = state.agent_or_active(&agent_id);
            if !state.queued.remove(agent_id, id) {
                debug!(%agent_id, id, "Queued entry to remove is gone");
            }
            hud::send_queued(state, agent_id).await;
        }
        UiEvent::EditQueued { agent_id, id, text } => {
            let agent_id = state.agent_or_active(&agent_id);
            let outcome = state.queued.edit(agent_id, id, text);
            if outcome != queue::EditOutcome::Applied {
                debug!(%agent_id, id, ?outcome, "Queued entry was not edited");
            }
            hud::send_queued(state, agent_id).await;
            if let Some(warning) = outcome.warning() {
                AgentUi::new(state.commands.clone(), agent_id)
                    .send(UiCommand::ShowWarning(warning.to_string()))
                    .await;
            }
        }
        UiEvent::MoveQueued { agent_id, id, to } => {
            let agent_id = state.agent_or_active(&agent_id);
            if !state.queued.move_to(agent_id, id, to) {
                debug!(%agent_id, id, "Queued entry to move is gone");
            }
            hud::send_queued(state, agent_id).await;
        }
        UiEvent::SendQueuedItemNow { agent_id, id } => {
            let agent_id = state.agent_or_active(&agent_id);
            send_item_now(state, agent_id, id).await;
        }
        // `domain_of` routes no other event here.
        _ => {}
    }
}

/// Move queued entry `id` of `agent_id` to the front and send it now.
async fn send_item_now(state: &mut LoopState<'_>, agent_id: AgentId, id: u64) {
    if state.queued.move_to(agent_id, id, 0) {
        hud::send_queued(state, agent_id).await;
        send_queued_now(state, agent_id).await;
    } else {
        debug!(%agent_id, id, "Queued entry to send now is gone");
        hud::send_queued(state, agent_id).await;
        AgentUi::new(state.commands.clone(), agent_id)
            .send(UiCommand::ShowWarning(
                "That message already started or was removed; nothing was sent now".to_string(),
            ))
            .await;
    }
}
