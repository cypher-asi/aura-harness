//! A cloud turn or move ending, as the loop hears it ([`Ended`]).

use super::*;
use crate::event_loop::{agents, channels, inbox};

/// Let the agent `ended` names take work again: settle what its turn was
/// asked, as a turn here does, then start what it queued meanwhile where
/// it runs now.
pub(in crate::event_loop) async fn handle_ended(state: &mut LoopState<'_>, ended: Ended) {
    let turn_ended = matches!(ended, Ended::Turn(..));
    let (agent, outcome) = state.cloud.free(ended);
    let interrupted = turn_ended && state.hub.inbox.take_interrupted(agent);
    if let Some(outcome) = outcome {
        lock(&state.hub.cloud_turns).remove(&agent);
        settle(state, agent, outcome, interrupted).await;
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
/// or tell their senders none is coming. A turn the user stopped
/// (`interrupted`) to carry on leaves its inbox requests owed, for the
/// next turn that ends well.
async fn settle(state: &LoopState<'_>, agent: AgentId, outcome: TurnOutcome, interrupted: bool) {
    let messages = outcome.messages.as_deref();
    match (outcome.failure, messages) {
        (None, Some(messages)) => {
            channels::post_replies_from(state, agent, messages).await;
            inbox::answer_seen(&state.hub, agent, messages);
        }
        // Nothing tells what the turn answered: it still owes it all, as
        // an agent here whose session is gone does.
        (None, None) => {
            warn!(%agent, "A cloud turn's conversation was not copied here; its answers wait");
        }
        // Once the user stopped the turn, the failure it ends with is that
        // stop, whatever it was doing then.
        (Some(_), messages) if interrupted => {
            channels::drop_unanswered_from(state, agent, messages);
            info!(%agent, "The user stopped the cloud turn; its inbox answers wait for the next one");
        }
        (Some(why), messages) => {
            channels::drop_unanswered_from(state, agent, messages);
            inbox::give_up_seen(&state.hub, agent, messages, &why);
        }
    }
}
