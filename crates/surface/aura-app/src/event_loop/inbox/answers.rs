//! What an agent was asked, answered when the turn that saw it ends:
//! with the agent's last words, or with why its work stopped.

use super::*;

/// Longest reply, in characters.
const REPLY_CHARS: usize = 6000;

/// Longest last report `list_agents` and `get_agent_state` show, in
/// characters.
const REPORT_CHARS: usize = 300;

/// Longest reason a reply gives for work that stopped, in characters.
const STOPPED_CHARS: usize = 300;

/// After `agent_id`'s work ended well: answer what it was asked
/// ([`answer_seen`]).
pub(in crate::event_loop) fn answer(state: &LoopState<'_>, agent_id: AgentId) {
    if let Some(session) = state.idle.get(&agent_id) {
        answer_seen(&state.hub, agent_id, &session.messages);
    }
}

/// After `agent_id`'s work failed or was stopped, `why`: tell the senders
/// of what it was asked that no answer is coming ([`give_up_seen`]).
pub(in crate::event_loop) fn give_up(state: &LoopState<'_>, agent_id: AgentId, why: &str) {
    let messages = state
        .idle
        .get(&agent_id)
        .map(|session| session.messages.as_slice());
    give_up_seen(&state.hub, agent_id, messages, why);
}

/// Answer each request `agent_id` owes whose message is in `messages`,
/// its conversation after a turn that ended well, with the last thing it
/// said after it. Requests whose messages it hasn't seen yet stay owed.
pub(crate) fn answer_seen(hub: &AgentHub, agent_id: AgentId, messages: &[Message]) {
    for (owed, at) in take_seen(hub, agent_id, messages) {
        let said = messages
            .get(at + 1..)
            .and_then(last_words)
            .unwrap_or_else(|| "(finished without a word)".to_string());
        reply(hub, agent_id, &owed, true, &said);
    }
}

/// Tell the senders of the requests `agent_id` owes that no answer is
/// coming, its work having stopped `why`: those whose messages are in
/// `messages`, its conversation, else (without one) the oldest, which
/// was the one that ran.
pub(crate) fn give_up_seen(
    hub: &AgentHub,
    agent_id: AgentId,
    messages: Option<&[Message]>,
    why: &str,
) {
    let dropped: Vec<Owed> = match messages {
        Some(messages) => take_seen(hub, agent_id, messages)
            .into_iter()
            .map(|(owed, _)| owed)
            .collect(),
        None => take_oldest(hub, agent_id).into_iter().collect(),
    };
    for owed in dropped {
        let why = capped(why, STOPPED_CHARS);
        reply(
            hub,
            agent_id,
            &owed,
            false,
            &format!("Stopped before finishing: {why}"),
        );
    }
}

/// Take the requests `agent_id` owes whose messages are in `messages`,
/// each with where its message is.
fn take_seen(hub: &AgentHub, agent_id: AgentId, messages: &[Message]) -> Vec<(Owed, usize)> {
    let mut state = lock(&hub.inbox.state);
    let Some(owed) = state.owed.get_mut(&agent_id) else {
        return Vec::new();
    };
    let mut seen = Vec::new();
    owed.retain(
        |request| match asked_at(messages, &inbox::marker(request.kind, &request.id)) {
            Some(at) => {
                seen.push((request.clone(), at));
                false
            }
            None => true,
        },
    );
    if owed.is_empty() {
        state.owed.remove(&agent_id);
    }
    seen
}

/// Take the oldest request `agent_id` owes.
fn take_oldest(hub: &AgentHub, agent_id: AgentId) -> Option<Owed> {
    let mut state = lock(&hub.inbox.state);
    let owed = state.owed.get_mut(&agent_id)?;
    let oldest = (!owed.is_empty()).then(|| owed.remove(0));
    if owed.is_empty() {
        state.owed.remove(&agent_id);
    }
    oldest
}

/// Where the user message carrying `marker` is in `messages`.
fn asked_at(messages: &[Message], marker: &str) -> Option<usize> {
    messages
        .iter()
        .rposition(|m| m.role == Role::User && m.text_content().contains(marker))
}

/// The last thing the model said in `messages`, if anything.
fn last_words(messages: &[Message]) -> Option<String> {
    messages
        .iter()
        .rev()
        .filter(|m| m.role == Role::Assistant)
        .map(Message::text_content)
        .find(|text| !text.trim().is_empty())
}

/// Send `agent_id`'s answer `said` to `owed`, `ok` when it finished the
/// work, closing the sender's todo when it named one.
fn reply(hub: &AgentHub, agent_id: AgentId, owed: &Owed, ok: bool, said: &str) {
    let (from, to) = {
        let registry = lock(&hub.registry);
        (
            registry.find(agent_id).map(Named::of),
            registry.find(owed.from).map(Named::of),
        )
    };
    let (Some(from), Some(to)) = (from, to) else {
        warn!(agent = %agent_id.to_hex(), "An inbox reply's agents are gone; not sent");
        return;
    };
    let id = new_id();
    let header = inbox::header(
        InboxKind::Reply,
        &id,
        from.short(),
        &from.name,
        &owed.subject,
    );
    let mut prompt = format!("{header}\n\n{}", capped(said, REPLY_CHARS));
    if let Some(todo) = &owed.todo {
        prompt.push('\n');
        prompt.push_str(&task_report_line(todo, ok));
    }
    lock(&hub.inbox.state)
        .reports
        .insert(agent_id, (ok, capped(said, REPORT_CHARS)));
    hub.post_mail(mail_view(
        InboxKind::Reply,
        &owed.thread,
        &owed.subject,
        (&from, &to),
        said,
    ));
    let notice = Notice {
        to: owed.from,
        shown: shown(&header),
        prompt,
        inbox: Some(InboxKind::Reply),
    };
    if hub.notices.send(notice).is_err() {
        warn!(agent = %agent_id.to_hex(), "Inbox reply after the event loop ended; dropped");
    }
}

/// `text` trimmed and cut to `max` characters, marked when cut.
pub(super) fn capped(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{cut}… [cut at {max} characters]")
}
