//! How many idle sessions the event loop keeps.
//!
//! Every agent the loop opened, and every spawn that finished, leaves an
//! [`AgentSession`] in [`LoopState::idle`]: its whole model conversation
//! and its kernel. With many agents that is a copy of every agent's
//! conversation held for the whole run. None of it is the only copy: a
//! session is opened again from its checkpoint and the record beyond it
//! (`KernelFactory::open`), as [`super::start_work`] and
//! `agents::activate_agent` already do for an agent with none.
//!
//! So the loop keeps the [`IDLE_SESSIONS_KEPT`] sessions most recently
//! left idle, and lets go of older ones once they are settled
//! ([`is_settled`]), saving each one's checkpoint first so it reopens as
//! it was. An archived agent's sessions go at once
//! ([`release_sessions`]).
//!
//! # Invariants
//!
//! * The agent on screen, an agent at work (here or in the cloud), one
//!   with work queued, one building a plan, one that owes reports through
//!   its inbox (`inbox::answer` and `give_up` read its messages to tell
//!   which it saw), one in a factory flow (`factory::hooks::after_turn`
//!   reads its messages), and a session without a checkpoint store are
//!   never let go of.
//! * A session is let go of only after its checkpoint is saved: a failed
//!   save is logged and the record still has the conversation.

use super::agents::AgentSession;
use super::LoopState;
use aura_core_types::AgentId;
use tracing::debug;

/// Idle sessions kept, the most recently left idle first.
pub(crate) const IDLE_SESSIONS_KEPT: usize = 16;

/// Note that `agent_id` was just left idle, the most recent of them, then
/// let go of the oldest settled sessions past [`IDLE_SESSIONS_KEPT`].
pub(super) async fn left_idle(state: &mut LoopState<'_>, agent_id: AgentId) {
    state.idle_order.retain(|id| *id != agent_id);
    state.idle_order.push_front(agent_id);
    trim(state).await;
}

/// Let go of the oldest settled idle sessions while more than
/// [`IDLE_SESSIONS_KEPT`] are held.
pub(super) async fn trim(state: &mut LoopState<'_>) {
    let idle = &state.idle;
    state.idle_order.retain(|id| idle.contains_key(id));
    if state.idle.len() <= IDLE_SESSIONS_KEPT {
        return;
    }
    // Oldest first: sessions the order never saw, then the order's tail.
    let mut oldest: Vec<AgentId> = state
        .idle
        .keys()
        .filter(|id| !state.idle_order.contains(id))
        .copied()
        .collect();
    oldest.extend(state.idle_order.iter().rev().copied());
    for agent_id in oldest {
        if state.idle.len() <= IDLE_SESSIONS_KEPT {
            break;
        }
        if is_settled(state, agent_id) {
            release(state, agent_id).await;
        }
    }
}

/// Let go of the sessions of `agents`, which were archived.
pub(super) async fn release_sessions(state: &mut LoopState<'_>, agents: &[AgentId]) {
    for &agent_id in agents {
        if state.idle.get(&agent_id).is_some_and(has_checkpoints) {
            release(state, agent_id).await;
        }
    }
}

/// Whether the idle session of `agent_id` can be let go of and opened
/// again later as it is.
fn is_settled(state: &LoopState<'_>, agent_id: AgentId) -> bool {
    let Some(session) = state.idle.get(&agent_id) else {
        return false;
    };
    agent_id != state.active
        && !state.hub.is_running(agent_id)
        && !state.cloud.is_running(agent_id)
        && state.queued.len(agent_id) == 0
        && state.hub.inbox.owed_by(agent_id) == 0
        && state.hub.factory_flows.stage(agent_id).is_none()
        && !state.hub.factory_flows.resumes_after_restart(agent_id)
        && !session.building_plan
        && has_checkpoints(session)
}

/// Whether `session` is checkpointed, so it reopens with its todos and
/// usage and not only its conversation.
const fn has_checkpoints(session: &AgentSession) -> bool {
    session.checkpoints.is_some()
}

/// Save the idle session of `agent_id` and let go of it.
async fn release(state: &mut LoopState<'_>, agent_id: AgentId) {
    let Some(session) = state.idle.remove(&agent_id) else {
        return;
    };
    state.idle_order.retain(|id| *id != agent_id);
    session.save_checkpoint().await;
    debug!(%agent_id, held = state.idle.len(), "Let go of an idle session");
}
