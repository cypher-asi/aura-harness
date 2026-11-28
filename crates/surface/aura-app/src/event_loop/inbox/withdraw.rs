//! Work a coordinator takes back before it started, by withdrawing it
//! (`assign` with `withdraw`) or by handing its todo to another member.
//!
//! # Invariant
//!
//! An assignment taken back never starts. It stays owed, marked
//! withdrawn, until the member's queue reaches it: the event loop then
//! drops it unrun and forgets it ([`take_withdrawn`]). One already
//! running is not stopped; it still reports as usual.

use super::*;

/// Mark as withdrawn the assignments `coordinator` gave `member` that
/// are still owed. Returns how many were marked.
pub(crate) fn withdraw(hub: &AgentHub, coordinator: AgentId, member: AgentId) -> usize {
    let mut state = lock(&hub.inbox.state);
    let Some(owed) = state.owed.get_mut(&member) else {
        return 0;
    };
    let mut marked = 0;
    for request in owed.iter_mut().filter(|request| {
        request.kind == InboxKind::Assignment && request.from == coordinator && !request.withdrawn
    }) {
        request.withdrawn = true;
        marked += 1;
    }
    marked
}

/// Mark as withdrawn the assignments `coordinator` gave for its todo
/// `todo` to members other than `keep`, which it now went to. Returns the
/// members they were taken from.
pub(crate) fn withdraw_todo(
    hub: &AgentHub,
    coordinator: AgentId,
    keep: AgentId,
    todo: &str,
) -> Vec<AgentId> {
    let mut state = lock(&hub.inbox.state);
    let mut taken = Vec::new();
    for (member, owed) in state.owed.iter_mut().filter(|(member, _)| **member != keep) {
        let mut took = false;
        for request in owed.iter_mut().filter(|request| {
            request.kind == InboxKind::Assignment
                && request.from == coordinator
                && request.todo.as_deref() == Some(todo)
                && !request.withdrawn
        }) {
            request.withdrawn = true;
            took = true;
        }
        if took {
            taken.push(*member);
        }
    }
    taken
}

/// Whether `text`, work about to start for `agent`, is an assignment its
/// coordinator took back. When it is, it is forgotten: the caller drops
/// the work unrun.
pub(in crate::event_loop) fn take_withdrawn(hub: &AgentHub, agent: AgentId, text: &str) -> bool {
    let mut state = lock(&hub.inbox.state);
    let Some(owed) = state.owed.get_mut(&agent) else {
        return false;
    };
    let Some(at) = owed.iter().position(|request| {
        request.withdrawn && text.contains(&inbox::marker(request.kind, &request.id))
    }) else {
        return false;
    };
    let request = owed.remove(at);
    if owed.is_empty() {
        state.owed.remove(&agent);
    }
    info!(
        agent = %agent.to_hex(),
        id = %request.id,
        "Dropped an assignment its coordinator took back before it started"
    );
    true
}
