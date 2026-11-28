//! A cloud turn or move ending, as the loop hears it ([`Ended`]).

use super::*;
use crate::event_loop::{agents, channels, inbox};

/// Let the agent `ended` names take work again: settle what its turn was
/// asked, as a turn here does, then start what it queued meanwhile where
/// it runs now.
pub(in crate::event_loop) async fn handle_ended(state: &mut LoopState<'_>, ended: Ended) {
    let turn_ended = matches!(ended, Ended::Turn(..));
    let (agent, outcome) = state.cloud.free(ended);
    // A new member whose machine didn't start fails: nothing it was sent
    // runs here instead.
    if !turn_ended && super::member_start::settle_start(state, agent).await {
        return;
    }
    let interrupted = turn_ended && state.hub.inbox.take_interrupted(agent);
    if let Some(outcome) = outcome {
        lock(&state.hub.cloud_turns).remove(&agent);
        let ok = outcome.failure.is_none();
        let messages = outcome.messages.as_deref().unwrap_or_default();
        let waits = crate::event_loop::factory::hooks::after_cloud_turn(
            state,
            agent,
            crate::event_loop::factory::hooks::TurnState { ok, interrupted },
            messages,
        )
        .await;
        settle(state, agent, outcome, Settle { interrupted, waits }).await;
        agents::send_agent_list(state).await;
    }
    // The turn may have written or worked through a plan.
    if agent == state.active {
        refresh_machine_plans(state, agent);
    }
    crate::event_loop::start_next(state, agent).await;
}

/// How a cloud turn's asks are settled.
#[derive(Debug, Clone, Copy)]
struct Settle {
    /// The user stopped the turn to carry on
    interrupted: bool,
    /// The factory's next turn answers for it
    waits: bool,
}

/// Settle what `agent`'s cloud turn was asked, as `outcome` says: post
/// its answers to the channel questions and inbox requests the turn saw,
/// or tell their senders none is coming. A turn the user stopped
/// (`interrupted`) to carry on leaves its inbox requests owed, for the
/// next turn that ends well. A turn of the software factory whose
/// answer `waits` for the turns it adds answers nothing yet.
async fn settle(state: &LoopState<'_>, agent: AgentId, outcome: TurnOutcome, how: Settle) {
    let Settle { interrupted, waits } = how;
    let messages = outcome.messages.as_deref();
    let said = outcome.said.as_deref();
    match (outcome.failure, messages) {
        (None, Some(messages)) => {
            channels::post_replies_from(state, agent, messages).await;
            // A plan turn's answer waits for the turns the factory adds.
            if !waits {
                inbox::answer_turn(&state.hub, agent, messages, said);
            }
        }
        // Without a copy, what the turn was started on is still known,
        // and its reply streamed here: that answers it. The rest waits.
        (None, None) => {
            warn!(%agent, "A cloud turn's conversation was not copied here; answering from its stream");
            if !waits && said.is_some() {
                inbox::answer_turn(&state.hub, agent, &[], said);
            }
        }
        // Once the user stopped the turn, the failure it ends with is that
        // stop, whatever it was doing then.
        (Some(_), messages) if interrupted => {
            channels::drop_unanswered_from(state, agent, messages);
            info!(%agent, "The user stopped the cloud turn; its inbox answers wait for the next one");
            state.hub.inbox.park(agent);
        }
        (Some(why), messages) => {
            if why != super::submit::STOPPED {
                state
                    .hub
                    .status
                    .fail(agent, &format!("Its turn failed: {why}"));
            }
            channels::drop_unanswered_from(state, agent, messages);
            inbox::give_up_seen(&state.hub, agent, messages, &why);
            inbox::give_up_if_archived(&state.hub, agent);
        }
    }
}
