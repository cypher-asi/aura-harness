//! UI events for sessions, models and session tools.

use crate::event_loop::{bench, compare, handlers, log_view, rewind, LoopState};
use aura_ui::{UiCommand, UiEvent};

/// Sessions, models and session tools.
pub(super) async fn handle(state: &mut LoopState<'_>, event: UiEvent) {
    match event {
        UiEvent::Clear => {
            if handlers::handle_new_session(state).await {
                let _ = state.commands.send(UiCommand::ClearConversation).await;
            }
        }
        UiEvent::NewSession => {
            handlers::handle_new_session(state).await;
        }
        UiEvent::SelectModel(id) => handlers::handle_select_model(state, &id).await,
        UiEvent::SetThinking(level) => {
            handlers::handle_set_thinking(state, level.as_deref()).await;
        }
        UiEvent::Resume { agent_id, index } => {
            let agent_id = state.agent_or_active(&agent_id);
            handlers::handle_resume(state, agent_id, index).await;
        }
        UiEvent::InitProject { agent_id } => {
            let agent_id = state.agent_or_active(&agent_id);
            handlers::handle_init_project(state, agent_id).await;
        }
        UiEvent::Rewind {
            agent_id,
            turn,
            scope,
        } => {
            let agent_id = state.agent_or_active(&agent_id);
            rewind::handle_rewind(state, agent_id, turn, scope).await;
        }
        UiEvent::Diff { agent_id, session } => {
            let agent_id = state.agent_or_active(&agent_id);
            rewind::handle_diff(state, agent_id, session).await;
        }
        UiEvent::Compare { agent_id, args } => {
            let agent_id = state.agent_or_active(&agent_id);
            compare::handle(state, agent_id, &args).await;
        }
        UiEvent::Bench { agent_id, request } => {
            let agent_id = state.agent_or_active(&agent_id);
            bench::handle(state, agent_id, request).await;
        }
        UiEvent::Log { agent_id, request } => {
            let agent_id = state.agent_or_active(&agent_id);
            log_view::handle(state, agent_id, request).await;
        }
        UiEvent::KillProcess(id) => handlers::handle_kill_process(state, &id).await,
        // `domain_of` routes no other event here.
        _ => {}
    }
}
