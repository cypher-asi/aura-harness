//! UI events that answer approvals and set modes and policies.

use super::answer_approval;
use crate::event_loop::approvals::Answer;
use crate::event_loop::{modes, policies, LoopState};
use aura_ui::{PermissionMode, UiEvent};

/// Approvals, modes and policies.
pub(super) async fn handle(state: &mut LoopState<'_>, event: UiEvent) {
    match event {
        UiEvent::Approve(id) => answer_approval(state, &id, Answer::Once).await,
        UiEvent::ApproveAlways(id) => answer_approval(state, &id, Answer::Always).await,
        UiEvent::Deny(id) => answer_approval(state, &id, Answer::Deny).await,
        UiEvent::SetMode { agent_id, mode } => {
            let agent_id = state.agent_or_active(&agent_id);
            modes::set_mode(state, agent_id, mode).await;
        }
        UiEvent::SetApproval { agent_id, level } => {
            let agent_id = state.agent_or_active(&agent_id);
            modes::set_approval(state, agent_id, level).await;
        }
        UiEvent::CycleApproval(agent_id) => {
            let agent_id = state.agent_or_active(&agent_id);
            let next = state.hub.permissions_of(agent_id).approval.next();
            modes::set_approval(state, agent_id, next).await;
        }
        UiEvent::TogglePlan(agent_id) => {
            let agent_id = state.agent_or_active(&agent_id);
            let next = if state.hub.mode_of(agent_id) == PermissionMode::Plan {
                state.hub.exit_plan_mode(agent_id)
            } else {
                PermissionMode::Plan
            };
            modes::set_mode(state, agent_id, next).await;
        }
        UiEvent::RequestPolicies(agent_id) => {
            let agent_id = state.agent_or_active(&agent_id);
            policies::send_policies_of(state, agent_id).await;
        }
        // `domain_of` routes no other event here.
        _ => {}
    }
}
