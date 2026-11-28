//! Queued work the user removed or edited: what it carried from the
//! inbox is settled, so the sender hears no answer is coming and the
//! stall sweep never asks for it back.
//!
//! # Invariant
//!
//! A request stays owed while some queued work still carries its marker:
//! an edit that keeps the marker keeps the request, and its report still
//! answers it.

use super::*;

/// Why the sender of work the user took out of a queue gets no answer.
pub(crate) const REMOVED_BY_USER: &str = "the user took it out of the agent's queue";

/// `agent`'s queued work read `before` and now reads `after` (empty once
/// it was removed): give up, `why`, each request it owes whose marker
/// `before` carried and `after` doesn't. Returns how many were settled.
pub(crate) fn settle_dropped(
    hub: &AgentHub,
    agent: AgentId,
    (before, after): (&str, &str),
    why: &str,
) -> usize {
    let dropped = take_dropped(hub, agent, before, after);
    if !dropped.is_empty() {
        info!(agent = %agent.to_hex(), settled = dropped.len(), "Queued inbox work was taken out; its reports are settled");
        answers::give_up_owed(hub, agent, &dropped, why);
    }
    dropped.len()
}

/// Take the requests `agent` owes whose marker is in `before` but not in
/// `after`.
fn take_dropped(hub: &AgentHub, agent: AgentId, before: &str, after: &str) -> Vec<Owed> {
    let mut state = lock(&hub.inbox.state);
    let Some(owed) = state.owed.get_mut(&agent) else {
        return Vec::new();
    };
    let mut dropped = Vec::new();
    owed.retain(|request| {
        let marker = inbox::marker(request.kind, &request.id);
        let gone = before.contains(&marker) && !after.contains(&marker);
        if gone {
            dropped.push(request.clone());
        }
        !gone
    });
    if owed.is_empty() {
        state.owed.remove(&agent);
    }
    dropped
}
