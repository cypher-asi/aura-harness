//! The members' replies to the channel messages they were asked: posted
//! from their conversation when their work ends well, dropped when it
//! doesn't.

use super::{append, lock, send_post, LoopState};
use aura_core_types::time::now_ms;
use aura_core_types::AgentId;
use aura_model::{Message, Role};
use aura_store_db::{NewPost, PostAuthor};
use tracing::warn;

/// After `agent_id`'s work ended well: post the answers it gave to the
/// channel messages it was asked ([`post_replies_from`] its session).
pub(crate) async fn post_replies(state: &LoopState<'_>, agent_id: AgentId) {
    if let Some(session) = state.idle.get(&agent_id) {
        post_replies_from(state, agent_id, &session.messages).await;
    }
}

/// Post the answers `agent_id` gave in `messages`, its conversation after
/// work that ended well, to the channel messages it was asked, each
/// replying to its user post. A question still waiting in the agent's
/// queue stays pending.
pub(crate) async fn post_replies_from(
    state: &LoopState<'_>,
    agent_id: AgentId,
    messages: &[Message],
) {
    let answered = {
        let mut pending = lock(&state.hub.channel_chat.pending);
        let Some(queue) = pending.get_mut(&agent_id) else {
            return;
        };
        let mut answered = Vec::new();
        // Each asked message answers one question: two identical questions
        // are not both answered by the first one's reply.
        let mut used = Vec::new();
        while let Some((index, reply)) = queue
            .front()
            .and_then(|asked| answer_to(messages, &asked.prompt, &used))
        {
            used.push(index);
            if let Some(asked) = queue.pop_front() {
                answered.push((asked, reply));
            }
        }
        if queue.is_empty() {
            pending.remove(&agent_id);
        }
        answered
    };
    for (asked, text) in answered {
        let new = NewPost {
            author: PostAuthor::Agent(agent_id),
            text,
            mentions: Vec::new(),
            reply_to: Some(asked.seq),
            at_ms: now_ms(),
        };
        if let Some(post) = append(&state.hub, &asked.channel_id, new).await {
            send_post(state.commands, &state.hub, asked.coordinator, &post).await;
        }
    }
}

/// After `agent_id`'s work was cancelled or failed: drop the questions
/// its session shows it was given ([`drop_unanswered_from`]).
pub(crate) fn drop_unanswered(state: &LoopState<'_>, agent_id: AgentId) {
    let messages = state
        .idle
        .get(&agent_id)
        .map(|session| session.messages.as_slice());
    drop_unanswered_from(state, agent_id, messages);
}

/// Drop the questions whose message `agent_id` had already been given,
/// as `messages`, its conversation after work that was cancelled or
/// failed, shows: no answer to them will come. Left pending, a later,
/// unrelated turn's reply would be posted as their answer. Questions
/// still waiting in its queue stay.
pub(crate) fn drop_unanswered_from(
    state: &LoopState<'_>,
    agent_id: AgentId,
    messages: Option<&[Message]>,
) {
    let mut pending = lock(&state.hub.channel_chat.pending);
    let Some(queue) = pending.get_mut(&agent_id) else {
        return;
    };
    let Some(messages) = messages else {
        // Without the conversation nothing tells which question ran; the
        // work that ended was the front one's.
        queue.pop_front();
        if queue.is_empty() {
            pending.remove(&agent_id);
        }
        return;
    };
    let mut used = Vec::new();
    while let Some(index) = queue
        .front()
        .and_then(|asked| asked_at(messages, &asked.prompt, &used))
    {
        used.push(index);
        if let Some(asked) = queue.pop_front() {
            warn!(
                agent = %agent_id.to_hex(),
                channel = %asked.channel_id,
                post = asked.seq,
                "A channel question got no answer: the member's turn ended early"
            );
        }
    }
    if queue.is_empty() {
        pending.remove(&agent_id);
    }
}

/// Index of the latest user message starting with `prompt` (file
/// mentions may have been appended to it) that is not in `used`.
pub(super) fn asked_at(messages: &[Message], prompt: &str, used: &[usize]) -> Option<usize> {
    messages.iter().enumerate().rev().find_map(|(index, m)| {
        (m.role == Role::User && !used.contains(&index) && m.text_content().starts_with(prompt))
            .then_some(index)
    })
}

/// Where `prompt` was asked ([`asked_at`]) and the last assistant text
/// that answered it before the next user message, if the model answered.
pub(super) fn answer_to(
    messages: &[Message],
    prompt: &str,
    used: &[usize],
) -> Option<(usize, String)> {
    let asked = asked_at(messages, prompt, used)?;
    let after = messages.get(asked + 1..)?;
    let end = after
        .iter()
        .position(|m| m.role == Role::User)
        .unwrap_or(after.len());
    after[..end]
        .iter()
        .rev()
        .filter(|m| m.role == Role::Assistant)
        .map(Message::text_content)
        .find(|text| !text.trim().is_empty())
        .map(|text| (asked, text))
}
