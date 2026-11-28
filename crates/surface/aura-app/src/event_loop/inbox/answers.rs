//! What an agent was asked, answered when the turn that saw it ends:
//! with the agent's last words, or with why its work stopped and what it
//! said before.

use super::*;

/// Longest reply, in characters.
const REPLY_CHARS: usize = 6000;

/// Longest last report `list_agents` and `get_agent_state` show, in
/// characters.
const REPORT_CHARS: usize = 300;

/// Longest reason a reply gives for work that stopped, in characters:
/// room for a failed build's first compiler errors.
const STOPPED_CHARS: usize = 1200;

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

/// Answer each request `agent_id` owes that the turn which ended well
/// with `messages`, its conversation, answers ([`take_answered`]), with
/// the last thing it said. Requests no turn has started on stay owed.
pub(crate) fn answer_seen(hub: &AgentHub, agent_id: AgentId, messages: &[Message]) {
    answer_turn(hub, agent_id, messages, None);
}

/// [`answer_seen`] for a cloud turn, whose reply streamed here as `said`
/// (its last text, after its last tool call): those are its last words,
/// whatever the copy of its conversation, which can be the one an
/// earlier turn left when this turn's copy failed, holds.
pub(crate) fn answer_turn(
    hub: &AgentHub,
    agent_id: AgentId,
    messages: &[Message],
    said: Option<&str>,
) {
    let streamed = said.filter(|said| !said.trim().is_empty());
    for (owed, at) in take_answered(hub, agent_id, messages) {
        let said = streamed
            .map(str::to_string)
            .or_else(|| words_since(messages, at))
            .unwrap_or_else(|| "(finished without a word)".to_string());
        reply(hub, agent_id, &owed, true, &said);
    }
}

/// Tell the senders of the requests `agent_id` owes that no answer is
/// coming, its work having stopped `why`: those whose messages are in
/// `messages`, its conversation, else (without one) the oldest, which
/// was the one that ran. Each reply carries the last thing the agent
/// said after the request, so work done before the stop isn't lost.
pub(crate) fn give_up_seen(
    hub: &AgentHub,
    agent_id: AgentId,
    messages: Option<&[Message]>,
    why: &str,
) {
    let dropped: Vec<(Owed, Option<String>)> = match messages {
        Some(messages) => take_answered(hub, agent_id, messages)
            .into_iter()
            .map(|(owed, at)| (owed, words_since(messages, at)))
            .collect(),
        None => take_oldest(hub, agent_id)
            .into_iter()
            .map(|owed| (owed, None))
            .collect(),
    };
    let why = capped(why, STOPPED_CHARS);
    for (owed, said) in dropped {
        reply(hub, agent_id, &owed, false, &stopped(&why, said.as_deref()));
    }
}

/// Tell the sender of every request `agent_id` owes that no answer is
/// coming, its work having stopped `why` before any of it ran. Work its
/// sender took back is dropped without a word: the sender knows.
pub(crate) fn give_up_all(hub: &AgentHub, agent_id: AgentId, why: &str) {
    let why = capped(why, STOPPED_CHARS);
    let owed = lock(&hub.inbox.state)
        .owed
        .remove(&agent_id)
        .unwrap_or_default();
    for owed in owed.iter().filter(|owed| !owed.withdrawn) {
        reply(hub, agent_id, owed, false, &stopped(&why, None));
    }
}

/// Tell the senders of `owed`, requests `agent_id` owed, that no answer
/// is coming, `why`, nothing of them having run. Work its sender took
/// back is dropped without a word: the sender knows.
pub(super) fn give_up_owed(hub: &AgentHub, agent_id: AgentId, owed: &[Owed], why: &str) {
    let why = capped(why, STOPPED_CHARS);
    for owed in owed.iter().filter(|owed| !owed.withdrawn) {
        reply(hub, agent_id, owed, false, &stopped(&why, None));
    }
}

/// Why an archived agent's owed work won't be answered.
pub(crate) const ARCHIVED: &str = "the agent was archived";

/// After `agent_id`'s turn ended: when it was archived meanwhile, give up
/// what it still owes ([`give_up_all`]), its queued work having been
/// dropped.
pub(crate) fn give_up_if_archived(hub: &AgentHub, agent_id: AgentId) {
    let archived = lock(&hub.registry)
        .find(agent_id)
        .is_some_and(|agent| agent.archived);
    if archived {
        give_up_all(hub, agent_id, ARCHIVED);
    }
}

/// The reply of work that stopped `why`, having said `said` last.
fn stopped(why: &str, said: Option<&str>) -> String {
    match said {
        Some(said) => {
            format!("Stopped before finishing: {why}\n\nWhat it said last:\n{said}")
        }
        None => format!("Stopped before finishing: {why}"),
    }
}

/// Take the requests `agent_id` owes that its turn ending with
/// `messages` answers, each with where its message is, if it is still
/// there: those a turn started on ([`super::take_up`]), and those whose
/// message is in `messages`. A taken one's message may be gone: a
/// compaction summarizes older messages away, and a cloud turn's copy
/// may be an earlier turn's.
fn take_answered(
    hub: &AgentHub,
    agent_id: AgentId,
    messages: &[Message],
) -> Vec<(Owed, Option<usize>)> {
    let mut state = lock(&hub.inbox.state);
    let Some(owed) = state.owed.get_mut(&agent_id) else {
        return Vec::new();
    };
    let mut seen = Vec::new();
    owed.retain(|request| {
        let at = asked_at(messages, &inbox::marker(request.kind, &request.id));
        if at.is_none() && !request.taken {
            return true;
        }
        seen.push((request.clone(), at));
        false
    });
    if owed.is_empty() {
        state.owed.remove(&agent_id);
    }
    seen
}

/// Take the oldest request `agent_id` owes.
fn take_oldest(hub: &AgentHub, agent_id: AgentId) -> Option<Owed> {
    let mut state = lock(&hub.inbox.state);
    let owed = state.owed.get_mut(&agent_id)?;
    // One taken back never started, so it isn't the one that ran.
    let oldest = owed
        .iter()
        .position(|request| !request.withdrawn)
        .map(|at| owed.remove(at));
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

/// The last thing the model said after its request's message at `at`,
/// else (the message is gone) after the turn's last prompt: never what
/// an earlier turn said.
fn words_since(messages: &[Message], at: Option<usize>) -> Option<String> {
    let from = at.or_else(|| last_prompt_at(messages))?;
    messages.get(from + 1..).and_then(last_words)
}

/// Where the last user message with text is in `messages`: the prompt
/// the turn worked on last (tool results carry no text).
fn last_prompt_at(messages: &[Message]) -> Option<usize> {
    messages
        .iter()
        .rposition(|m| m.role == Role::User && !m.text_content().trim().is_empty())
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
    if super::saved::send(hub, notice).is_err() {
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
