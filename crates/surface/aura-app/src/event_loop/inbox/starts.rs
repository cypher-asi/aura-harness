//! Work that is slow to start or can't start: the agents waiting on it
//! hear so, rather than waiting on an agent that sits idle.

use super::*;
use crate::event_loop::standing::Standing;

/// Tell each agent that `agent` owes an answer how its work stands,
/// `note`, leaving what it owes as it is.
pub(crate) fn tell_waiting(hub: &AgentHub, agent: AgentId, note: &str) {
    let owed = lock(&hub.inbox.state)
        .owed
        .get(&agent)
        .cloned()
        .unwrap_or_default();
    let Some(me) = lock(&hub.registry).find(agent).map(Named::of) else {
        return;
    };
    let mut told = HashSet::new();
    for owed in owed.iter().filter(|owed| told.insert(owed.from)) {
        let notice = Notice {
            to: owed.from,
            shown: format!("{}: {note}", me.short()),
            prompt: format!(
                "{}, which owes you a report on \"{}\": {note}",
                me.full(),
                owed.subject
            ),
            inbox: None,
        };
        if hub.notices.send(notice).is_err() {
            warn!(agent = %agent.to_hex(), "A start notice after the event loop ended; dropped");
        }
    }
}

/// `agent`'s work could not start, `why`: unless other work of its runs,
/// moves or waits, give up everything it owes, telling each sender why,
/// and show it as failed to start. Otherwise that other work answers.
pub(in crate::event_loop) fn give_up_unstarted(state: &LoopState<'_>, agent: AgentId, why: &str) {
    let busy = state.hub.is_working(agent)
        || state.cloud.is_running(agent)
        || state.cloud.is_moving(agent)
        || state.queued.len(agent) > 0;
    if busy {
        return;
    }
    let given = answers::give_up_all(&state.hub, agent, why);
    if given > 0 {
        warn!(agent = %agent.to_hex(), %why, given, "Work could not start; its reports are given up");
        state
            .hub
            .standing
            .set(agent, Standing::FailedToStart(why.to_string()));
    }
}
