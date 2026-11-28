//! Work that can't start: the agents waiting on it hear so, rather than
//! waiting on an agent that sits idle.

use super::*;

/// `agent`'s work could not start, `why`: unless other work of its runs,
/// moves or waits, give up everything it owes, telling each sender why,
/// and fail it, as its row and its peers then show. Otherwise that other
/// work answers.
pub(in crate::event_loop) fn give_up_unstarted(state: &LoopState<'_>, agent: AgentId, why: &str) {
    let busy = state.hub.is_working(agent)
        || state.cloud.is_running(agent)
        || state.cloud.is_moving(agent)
        || state.queued.len(agent) > 0;
    if busy || state.hub.inbox.owed_by(agent) == 0 {
        return;
    }
    warn!(agent = %agent.to_hex(), %why, "Work could not start; its reports are given up");
    answers::give_up_all(&state.hub, agent, why);
    state.hub.status.fail(agent, why);
}
