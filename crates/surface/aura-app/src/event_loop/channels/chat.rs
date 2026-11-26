//! Talking in a channel from its coordinator's chat: `@CODE` mentions of
//! members, the posts they leave, the members' replies, and the digest
//! of channel activity a coordinator's next turn starts with.
//!
//! # Invariants
//!
//! * A message that mentions a member never starts a coordinator turn
//!   unless it also mentions `@coordinator` (or the coordinator's code).
//! * Channel store calls run off the async runtime ([`super::blocking`]).
//! * Posting a reply never fails the member's turn: failures are logged.
//!
//! # Failure modes
//!
//! A user post that cannot be stored is logged and the message is still
//! delivered to the members mentioned, without a reply to link back to.

use super::blocking;
use crate::agent_registry::AgentRegistry;
use crate::event_loop::agent_events::AgentUi;
use crate::event_loop::steering::SteerSource;
use crate::event_loop::subagents::{lock, AgentHub};
use crate::event_loop::LoopState;
use aura_core_types::time::now_ms;
use aura_core_types::AgentId;
use aura_model::Role;
use aura_store_db::{ChannelPost, ChannelRecord, MemberRecord, NewPost, PostAuthor};
use aura_ui_terminal::events::{ChannelPostAuthor, ChannelPostView, ImageAttachment};
use aura_ui_terminal::UiCommand;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use tracing::{info, warn};

/// The word that addresses the coordinator itself.
const COORDINATOR_WORD: &str = "coordinator";

/// Characters a mention may be followed by that are not part of it.
const TRAILING_PUNCTUATION: &[char] = &[',', '.', ':', ';', '!', '?', ')', '"', '\''];

/// Longest post text, in characters, a digest line quotes.
const DIGEST_LINE_CHARS: usize = 300;

/// Largest digest, in bytes; the newest lines are kept.
const DIGEST_BYTES: usize = 4096;

/// What starts a digest of channel activity.
pub(crate) const DIGEST_HEADER: &str = "Channel activity since your last turn:";

/// State of the channels' conversation kept on the hub.
#[derive(Default)]
pub(crate) struct ChannelChat {
    /// Replies each member owes, oldest first. A queue rather than one
    /// entry: a member may be asked twice before its first answer.
    pending: Mutex<HashMap<AgentId, VecDeque<PendingReply>>>,
    /// When each coordinator last started a turn for the user, in Unix
    /// milliseconds; a cache of what the channel store keeps
    /// ([`aura_store_db::ChannelStore::seen`]), so a restart doesn't
    /// repeat what the coordinator already read.
    last_seen_ms: Mutex<HashMap<AgentId, u64>>,
}

/// A user post a member was asked to answer.
#[derive(Debug, Clone)]
struct PendingReply {
    channel_id: String,
    coordinator: AgentId,
    /// `seq` of the user post the reply answers
    seq: u64,
    /// The prompt the member got, to find its answer in the conversation
    prompt: String,
}

/// The members `text` mentions as `@CODE` (case-insensitive, trailing
/// punctuation ignored), in the order first mentioned, and whether it
/// mentions `@coordinator`. Other `@words` are not mentions of members:
/// they stay for the file mentions of [`crate::event_loop::attachments`].
pub(crate) fn parse_member_mentions(
    text: &str,
    members: &[(AgentId, Option<String>)],
) -> (Vec<AgentId>, bool) {
    let mut mentioned = Vec::new();
    let mut coordinator = false;
    for word in text.split_whitespace() {
        let Some(name) = word.strip_prefix('@') else {
            continue;
        };
        let name = name.trim_end_matches(TRAILING_PUNCTUATION);
        if name.is_empty() {
            continue;
        }
        if name.eq_ignore_ascii_case(COORDINATOR_WORD) {
            coordinator = true;
            continue;
        }
        let member = members.iter().find(|(_, code)| {
            code.as_deref()
                .is_some_and(|code| code.eq_ignore_ascii_case(name))
        });
        if let Some((id, _)) = member {
            if !mentioned.contains(id) {
                mentioned.push(*id);
            }
        }
    }
    (mentioned, coordinator)
}

/// The channel `agent_id` coordinates, with its members.
async fn coordinated_channel(
    hub: &AgentHub,
    agent_id: AgentId,
) -> Option<(ChannelRecord, Vec<MemberRecord>)> {
    let store = hub.channels.as_ref()?;
    if !lock(&hub.channel_members).contains(&agent_id) {
        return None;
    }
    let read = blocking(store, move |store| {
        let Some(channel_id) = store.channel_of(&agent_id)? else {
            return Ok(None);
        };
        let Some(channel) = store.get(&channel_id)? else {
            return Ok(None);
        };
        if channel.coordinator != agent_id || channel.archived {
            return Ok(None);
        }
        let members = store.members(&channel.id)?;
        Ok(Some((channel, members)))
    })
    .await;
    match read {
        Ok(found) => found,
        Err(error) => {
            warn!(
                agent = %agent_id.to_hex(),
                error = %format!("{error:#}"),
                "Could not read the channel of a coordinator"
            );
            None
        }
    }
}

/// `UiEvent::UserMessage` to the coordinator `coordinator`: when `text`
/// mentions members of its channel, post it to the channel and give it
/// to each of them, and to the coordinator only when it is mentioned
/// too. Returns `false`, having done nothing, when it mentions no member
/// (or `coordinator` coordinates no channel): the coordinator takes the
/// message as usual.
pub(crate) async fn route_user_message(
    state: &mut LoopState<'_>,
    coordinator: AgentId,
    text: &str,
    images: &[ImageAttachment],
    shown: bool,
) -> bool {
    let Some((channel, members)) = coordinated_channel(&state.hub, coordinator).await else {
        return false;
    };
    let coded = member_codes(&lock(&state.hub.registry), &members);
    let (mut mentioned, mut to_coordinator) = parse_member_mentions(text, &coded);
    if mentioned.contains(&coordinator) {
        mentioned.retain(|id| *id != coordinator);
        to_coordinator = true;
    }
    if mentioned.is_empty() {
        return false;
    }
    let seq = post_from_user(state, &channel, text, &mentioned).await;
    let header = context_header(&channel);
    for member in &mentioned {
        let prompt = format!("{header}\n\n{text}");
        if let Some(seq) = seq {
            lock(&state.hub.channel_chat.pending)
                .entry(*member)
                .or_default()
                .push_back(PendingReply {
                    channel_id: channel.id.clone(),
                    coordinator,
                    seq,
                    prompt: prompt.clone(),
                });
        }
        info!(
            channel_id = %channel.id,
            member = %member.to_hex(),
            "Routing a channel message to a member"
        );
        crate::event_loop::deliver_with_images(
            state,
            *member,
            SteerSource::User,
            prompt,
            images.to_vec(),
            false,
        )
        .await;
    }
    if to_coordinator {
        crate::event_loop::deliver_with_images(
            state,
            coordinator,
            SteerSource::User,
            text.to_string(),
            images.to_vec(),
            shown,
        )
        .await;
    } else if !state.hub.is_running(coordinator) {
        // The chat started waiting for a reply when the message was sent;
        // none is coming from the coordinator.
        AgentUi::new(state.commands.clone(), coordinator)
            .send(UiCommand::Complete)
            .await;
    }
    true
}

/// Each member with its code, for [`parse_member_mentions`].
fn member_codes(
    registry: &AgentRegistry,
    members: &[MemberRecord],
) -> Vec<(AgentId, Option<String>)> {
    members
        .iter()
        .map(|member| {
            let code = registry.find(member.agent_id).and_then(|a| a.code.clone());
            (member.agent_id, code)
        })
        .collect()
}

/// What a member's prompt starts with: where the message came from.
fn context_header(channel: &ChannelRecord) -> String {
    let mut header = format!(
        "[#{} channel] message from the user via the coordinator's chat:\nChannel: {}",
        channel.slug, channel.title
    );
    if !channel.description.is_empty() {
        header.push_str(" - ");
        header.push_str(&channel.description);
    }
    header
}

/// Store the user's `text` in `channel`, mentioning `mentioned`, and show
/// it in the coordinator's chat. Returns its `seq`; `None` when it could
/// not be stored (logged).
async fn post_from_user(
    state: &LoopState<'_>,
    channel: &ChannelRecord,
    text: &str,
    mentioned: &[AgentId],
) -> Option<u64> {
    let new = NewPost {
        author: PostAuthor::User,
        text: text.to_string(),
        mentions: mentioned.to_vec(),
        reply_to: None,
        at_ms: now_ms(),
    };
    let post = append(&state.hub, &channel.id, new).await?;
    send_post(state.commands, &state.hub, channel.coordinator, &post).await;
    Some(post.seq)
}

/// Append `new` to channel `channel_id`; `None` when it fails (logged).
async fn append(hub: &AgentHub, channel_id: &str, new: NewPost) -> Option<ChannelPost> {
    let store = hub.channels.as_ref()?;
    let id = channel_id.to_string();
    match blocking(store, move |store| store.append_post(&id, new)).await {
        Ok(post) => Some(post),
        Err(error) => {
            warn!(
                %channel_id,
                error = %format!("{error:#}"),
                "Could not store a channel post"
            );
            None
        }
    }
}

/// Show `post` in the chat of `coordinator`.
async fn send_post(
    commands: &tokio::sync::mpsc::Sender<UiCommand>,
    hub: &AgentHub,
    coordinator: AgentId,
    post: &ChannelPost,
) {
    let view = post_view(&lock(&hub.registry), post);
    AgentUi::new(commands.clone(), coordinator)
        .send(UiCommand::ChannelPost {
            channel_id: post.channel_id.clone(),
            post: view,
        })
        .await;
}

/// `post` as the UI shows it, its author named from `registry`.
fn post_view(registry: &AgentRegistry, post: &ChannelPost) -> ChannelPostView {
    let author = match post.author {
        PostAuthor::User => ChannelPostAuthor::User,
        PostAuthor::Agent(id) => {
            let agent = registry.find(id);
            ChannelPostAuthor::Agent {
                id: id.to_hex(),
                code: agent.and_then(|a| a.code.clone()),
                name: agent.map_or_else(|| id.to_hex(), |a| a.display_name().into_owned()),
            }
        }
    };
    ChannelPostView {
        seq: post.seq,
        author,
        text: post.text.clone(),
        mentions: post.mentions.iter().map(AgentId::to_hex).collect(),
        reply_to: post.reply_to,
        at_ms: post.at_ms,
    }
}

/// After `agent_id`'s work ended well: post the answers it gave to the
/// channel messages it was asked, each replying to its user post. A
/// question still waiting in the agent's queue stays pending.
pub(crate) async fn post_replies(state: &LoopState<'_>, agent_id: AgentId) {
    let Some(session) = state.idle.get(&agent_id) else {
        return;
    };
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
            .and_then(|asked| answer_to(&session.messages, &asked.prompt, &used))
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
/// whose message it had already been given, as no answer to them will
/// come. Left pending, a later, unrelated turn's reply would be posted as
/// their answer. Questions still waiting in its queue stay.
pub(crate) fn drop_unanswered(state: &LoopState<'_>, agent_id: AgentId) {
    let mut pending = lock(&state.hub.channel_chat.pending);
    let Some(queue) = pending.get_mut(&agent_id) else {
        return;
    };
    let Some(session) = state.idle.get(&agent_id) else {
        // Without the session nothing tells which question ran; the work
        // that ended was the front one's.
        queue.pop_front();
        if queue.is_empty() {
            pending.remove(&agent_id);
        }
        return;
    };
    let mut used = Vec::new();
    while let Some(index) = queue
        .front()
        .and_then(|asked| asked_at(&session.messages, &asked.prompt, &used))
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
fn asked_at(messages: &[aura_model::Message], prompt: &str, used: &[usize]) -> Option<usize> {
    messages.iter().enumerate().rev().find_map(|(index, m)| {
        (m.role == Role::User && !used.contains(&index) && m.text_content().starts_with(prompt))
            .then_some(index)
    })
}

/// Where `prompt` was asked ([`asked_at`]) and the last assistant text
/// that answered it before the next user message, if the model answered.
fn answer_to(
    messages: &[aura_model::Message],
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
        .map(aura_model::Message::text_content)
        .find(|text| !text.trim().is_empty())
        .map(|text| (asked, text))
}

/// Send every post of the channel `agent_id` coordinates to its chat,
/// as its conversation loads. Nothing for other agents.
pub(crate) async fn send_posts_of(
    commands: &tokio::sync::mpsc::Sender<UiCommand>,
    hub: &AgentHub,
    agent_id: AgentId,
) {
    let Some((channel, _)) = coordinated_channel(hub, agent_id).await else {
        return;
    };
    let Some(posts) = read_posts(hub, &channel.id, None).await else {
        return;
    };
    let views = {
        let registry = lock(&hub.registry);
        posts
            .iter()
            .map(|post| post_view(&registry, post))
            .collect()
    };
    AgentUi::new(commands.clone(), agent_id)
        .send(UiCommand::SetChannelPosts {
            channel_id: channel.id,
            posts: views,
        })
        .await;
}

/// Posts of channel `channel_id` after `since_ms`; `None` when they cannot
/// be read (logged).
async fn read_posts(
    hub: &AgentHub,
    channel_id: &str,
    since_ms: Option<u64>,
) -> Option<Vec<ChannelPost>> {
    let store = hub.channels.as_ref()?;
    let id = channel_id.to_string();
    match blocking(store, move |store| store.posts(&id, since_ms)).await {
        Ok(posts) => Some(posts),
        Err(error) => {
            warn!(
                %channel_id,
                error = %format!("{error:#}"),
                "Could not read the channel posts"
            );
            None
        }
    }
}

/// When the coordinator of `channel_id` last read its activity, as stored
/// before this run of the app; `None` when never, or unreadable (logged).
async fn stored_seen(hub: &AgentHub, channel_id: &str) -> Option<u64> {
    let store = hub.channels.as_ref()?;
    let id = channel_id.to_string();
    match blocking(store, move |store| store.seen(&id)).await {
        Ok(seen) => seen,
        Err(error) => {
            warn!(%channel_id, error = %format!("{error:#}"), "Could not read when the channel was last read");
            None
        }
    }
}

/// Store that the coordinator of `channel_id` read its activity up to
/// `at_ms`, so a restart doesn't repeat it. A failure is only logged: the
/// next digest then repeats some lines.
async fn save_seen(hub: &AgentHub, channel_id: &str, at_ms: u64) {
    let Some(store) = hub.channels.as_ref() else {
        return;
    };
    let id = channel_id.to_string();
    if let Err(error) = blocking(store, move |store| store.set_seen(&id, at_ms)).await {
        warn!(%channel_id, error = %format!("{error:#}"), "Could not store when the channel was read");
    }
}

/// For a turn the coordinator `agent_id` starts for the user: the digest
/// of the channel's posts since its last such turn, if there were any,
/// and from now on nothing older. `None` for other agents.
pub(crate) async fn take_digest(hub: &AgentHub, agent_id: AgentId) -> Option<String> {
    let (channel, _) = coordinated_channel(hub, agent_id).await?;
    let now = now_ms();
    let cached = lock(&hub.channel_chat.last_seen_ms).insert(agent_id, now);
    let since = match cached {
        Some(since) => Some(since),
        None => stored_seen(hub, &channel.id).await,
    };
    save_seen(hub, &channel.id, now).await;
    let posts = read_posts(hub, &channel.id, since).await?;
    let posts: Vec<_> = posts.into_iter().filter(|p| p.at_ms <= now).collect();
    let lines: Vec<String> = {
        let registry = lock(&hub.registry);
        posts
            .iter()
            .map(|post| digest_line(&registry, post))
            .collect()
    };
    render_digest(&lines)
}

/// One digest line: who wrote `post` and what, on one line.
fn digest_line(registry: &AgentRegistry, post: &ChannelPost) -> String {
    let who = match post.author {
        PostAuthor::User => "user".to_string(),
        PostAuthor::Agent(id) => match registry.find(id) {
            Some(agent) => match agent.code.as_deref() {
                Some(code) => format!("{code} {}", agent.display_name()),
                None => agent.display_name().into_owned(),
            },
            None => id.to_hex(),
        },
    };
    let flat = post.text.split_whitespace().collect::<Vec<_>>().join(" ");
    let text: String = if flat.chars().count() > DIGEST_LINE_CHARS {
        let cut: String = flat.chars().take(DIGEST_LINE_CHARS).collect();
        format!("{cut}...")
    } else {
        flat
    };
    format!("{who}: {text}")
}

/// The digest of `lines`, oldest first, keeping the newest that fit in
/// [`DIGEST_BYTES`]; `None` without lines.
pub(crate) fn render_digest(lines: &[String]) -> Option<String> {
    let mut budget = DIGEST_BYTES.saturating_sub(DIGEST_HEADER.len() + 1);
    let mut kept = Vec::new();
    for line in lines.iter().rev() {
        let cost = line.len() + 1;
        if cost > budget {
            break;
        }
        budget -= cost;
        kept.push(line.as_str());
    }
    if kept.is_empty() {
        return None;
    }
    kept.reverse();
    Some(format!("{DIGEST_HEADER}\n{}", kept.join("\n")))
}

#[cfg(test)]
impl ChannelChat {
    /// How many replies the members still owe.
    pub(crate) fn pending_replies(&self) -> usize {
        lock(&self.pending).values().map(VecDeque::len).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::{answer_to, asked_at};
    use aura_model::Message;

    #[test]
    fn an_answer_comes_before_the_next_user_message() {
        let messages = [
            Message::user("[#site] status?"),
            Message::assistant("Building."),
            Message::user("unrelated direct question"),
            Message::assistant("Unrelated answer."),
        ];
        assert_eq!(
            answer_to(&messages, "[#site] status?", &[]),
            Some((0, "Building.".to_string()))
        );

        // A question the model never answered gets nothing, not the reply
        // to what came after it.
        let unanswered = [
            Message::user("[#site] status?"),
            Message::user("unrelated direct question"),
            Message::assistant("Unrelated answer."),
        ];
        assert_eq!(answer_to(&unanswered, "[#site] status?", &[]), None);
    }

    #[test]
    fn identical_questions_each_take_their_own_answer() {
        let messages = [
            Message::user("[#site] status?"),
            Message::assistant("First."),
            Message::user("[#site] status?"),
            Message::assistant("Second."),
        ];
        let first = answer_to(&messages, "[#site] status?", &[]).unwrap();
        let second = answer_to(&messages, "[#site] status?", &[first.0]).unwrap();
        assert_ne!(first.0, second.0);
        assert_ne!(first.1, second.1);
        assert_eq!(asked_at(&messages, "[#site] status?", &[0, 2]), None);
    }
}
