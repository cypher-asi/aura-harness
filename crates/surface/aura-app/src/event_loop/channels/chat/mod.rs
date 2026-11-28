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
use aura_store_db::{ChannelPost, ChannelRecord, MemberRecord, NewPost, PostAuthor};
use aura_ui::events::{prompt_key, ChannelPostAuthor, ChannelPostView, ImageAttachment};
use aura_ui::UiCommand;
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
/// too. Returns `false` when it mentions no member (or `coordinator`
/// coordinates no channel): the coordinator takes the message as usual,
/// and in a channel only its HISTORY summary is asked for.
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
        // The coordinator takes it, and it stays in its conversation;
        // HISTORY lists it from there, summarized like a post.
        let typed = vec![(prompt_key(text), text.to_string())];
        super::summaries::spawn_for(state, coordinator, &channel.id, typed);
        return false;
    }
    let seq = post_from_user(state, &channel, text, &mentioned).await;
    let header = context_header(&channel);
    for member in &mentioned {
        let prompt = format!("{header}\n\n{text}");
        if let Some(seq) = seq {
            expect_reply(state, &channel, coordinator, *member, seq, &prompt);
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
    finish_for_coordinator(state, coordinator, text, images, shown, to_coordinator).await;
    true
}

/// Note that `member` owes a reply to the user's post `seq` in `channel`.
fn expect_reply(
    state: &LoopState<'_>,
    channel: &ChannelRecord,
    coordinator: AgentId,
    member: AgentId,
    seq: u64,
    prompt: &str,
) {
    lock(&state.hub.channel_chat.pending)
        .entry(member)
        .or_default()
        .push_back(PendingReply {
            channel_id: channel.id.clone(),
            coordinator,
            seq,
            prompt: prompt.to_string(),
        });
}

/// Give the message to `coordinator` when it was mentioned, else end the
/// wait for its reply.
async fn finish_for_coordinator(
    state: &mut LoopState<'_>,
    coordinator: AgentId,
    text: &str,
    images: &[ImageAttachment],
    shown: bool,
    to_coordinator: bool,
) {
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

/// What the first line of a member's prompt from the coordinator's chat
/// ends with, after `[#slug`.
const RELAYED: &str = " channel] message from the user via the coordinator's chat:";

/// What a member's prompt starts with: where the message came from.
fn context_header(channel: &ChannelRecord) -> String {
    let mut header = format!("[#{}{RELAYED}\nChannel: {}", channel.slug, channel.title);
    if !channel.description.is_empty() {
        header.push_str(" - ");
        header.push_str(&channel.description);
    }
    header
}

/// Whether `prompt` is a message [`route_user_message`] gave a member:
/// the user's, from its coordinator's chat rather than its own.
pub(crate) fn is_relayed(prompt: &str) -> bool {
    prompt
        .lines()
        .next()
        .is_some_and(|line| line.starts_with("[#") && line.ends_with(RELAYED))
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
        planner: None,
    };
    let post = append(&state.hub, &channel.id, new).await?;
    send_post(state.commands, &state.hub, channel.coordinator, &post).await;
    let mine = vec![(post.seq, post.text.clone())];
    super::summaries::spawn_for(state, channel.coordinator, &channel.id, mine);
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
        planner: post.planner.map(|id| id.to_hex()),
    }
}

#[cfg(test)]
impl ChannelChat {
    /// How many replies the members still owe.
    pub(crate) fn pending_replies(&self) -> usize {
        lock(&self.pending).values().map(VecDeque::len).sum()
    }
}

mod digest;
mod plan_post;
mod replies;
#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) use digest::render_digest;
pub(crate) use digest::{send_posts_of, take_digest};
pub(crate) use plan_post::post_plan_request;
#[cfg(test)]
use replies::{answer_to, asked_at};
pub(crate) use replies::{drop_unanswered, drop_unanswered_from, post_replies, post_replies_from};
