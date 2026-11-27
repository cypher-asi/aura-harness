//! A cloud turn or move ending, as the loop hears it ([`Ended`]).

use super::*;
use crate::event_loop::{agents, channels, inbox};

/// Let the agent `ended` names take work again: settle what its turn was
/// asked, as a turn here does, then start what it queued meanwhile where
/// it runs now.
pub(in crate::event_loop) async fn handle_ended(state: &mut LoopState<'_>, ended: Ended) {
    let (agent, outcome) = state.cloud.free(ended);
    if let Some(outcome) = outcome {
        lock(&state.hub.cloud_turns).remove(&agent);
        let ok = outcome.failure.is_none();
        let messages = outcome.messages.as_deref().unwrap_or_default();
        let waits = crate::event_loop::factory::hooks::after_cloud_turn(state, agent, ok, messages);
        settle(state, agent, outcome, waits).await;
        agents::send_agent_list(state).await;
    }
    // The turn may have written or worked through a plan.
    if agent == state.active {
        refresh_machine_plans(state, agent);
    }
    crate::event_loop::start_next(state, agent).await;
}

/// Settle what `agent`'s cloud turn was asked, as `outcome` says: post
/// its answers to the channel questions and inbox requests the turn saw,
/// or tell their senders none is coming.
async fn settle(state: &LoopState<'_>, agent: AgentId, outcome: TurnOutcome, waits: bool) {
    let messages = outcome.messages.as_deref();
    match (outcome.failure, messages) {
        (None, Some(messages)) => {
            channels::post_replies_from(state, agent, messages).await;
            // A plan turn's answer waits for the turns the factory adds.
            if !waits {
                inbox::answer_seen(&state.hub, agent, messages);
            }
        }
        // Nothing tells what the turn answered: it still owes it all, as
        // an agent here whose session is gone does.
        (None, None) => {
            warn!(%agent, "A cloud turn's conversation was not copied here; its answers wait");
        }
        (Some(why), messages) => {
            channels::drop_unanswered_from(state, agent, messages);
            inbox::give_up_seen(&state.hub, agent, messages, &why);
        }
    }
}
