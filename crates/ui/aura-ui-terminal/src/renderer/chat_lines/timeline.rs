//! Which messages a frame shows, in which blocks, and where each block
//! falls in its turn.

use super::TurnPlace;
use crate::components::{Message, MessageRole};
use crate::events::{
    merge_timeline, ChannelPostAuthor, ChannelPostView, MessageData, TimelineItem,
};
use std::hash::{Hash, Hasher};
use std::ops::Range;

/// Messages grouped into blocks: consecutive agent steps share one, and so
/// do consecutive notices.
pub(super) fn block_runs(messages: &[&Message]) -> Vec<Range<usize>> {
    let mut runs: Vec<Range<usize>> = Vec::new();
    let is_activity = |i: usize| messages[i].activity_step().is_some();
    let is_notice = |i: usize| is_notice(messages[i]);
    for i in 0..messages.len() {
        match runs.last_mut() {
            Some(run)
                if (is_activity(i) && is_activity(run.end - 1))
                    || (is_notice(i) && is_notice(run.end - 1)) =>
            {
                run.end = i + 1;
            }
            _ => runs.push(i..i + 1),
        }
    }
    runs
}

/// A one-line system message such as `policy: read_file allowed by …`.
pub(super) fn is_notice(message: &Message) -> bool {
    message.role() == MessageRole::System
        && message.activity_step().is_none()
        && message.shell_run().is_none()
        && message.boot_console().is_none()
        && !message.content().trim().is_empty()
        && !message.content().contains('\n')
}

/// Each block's place in its turn. A prompt starts a turn; the agent's
/// last block in it carries the footer of the turn's latest reply, and
/// steps are numbered in order across the turn.
pub(super) fn turn_places<'a>(shown: &[&'a Message], runs: &[Range<usize>]) -> Vec<TurnPlace<'a>> {
    let mut places: Vec<TurnPlace<'a>> = Vec::with_capacity(runs.len());
    let mut last_agent: Option<usize> = None;
    let mut meta: Option<&'a Message> = None;
    let mut steps = 0;
    let mut started_ms = shown.first().map_or(0, |m| m.timestamp_ms());
    for run in runs {
        let first = shown[run.start];
        // A post neither starts the agent's turn nor ends it.
        if first.author().is_some() {
            places.push(TurnPlace {
                meta: None,
                first_step: steps + 1,
                started_ms,
            });
            continue;
        }
        if first.role() == MessageRole::User {
            close_turn(&mut places, last_agent.take(), meta.take());
            steps = 0;
            started_ms = first.timestamp_ms();
        }
        let agent = first.role() == MessageRole::Assistant;
        if agent {
            last_agent = Some(places.len());
        }
        places.push(TurnPlace {
            meta: None,
            first_step: steps + 1,
            started_ms,
        });
        steps += shown[run.clone()]
            .iter()
            .filter(|m| m.activity_step().is_some())
            .count();
        if agent && first.activity_step().is_none() && first.model().is_some() {
            meta = Some(first);
        }
    }
    close_turn(&mut places, last_agent, meta);
    places
}

/// Set on the id of a channel post's message, so it never meets the ids
/// of the conversation's messages, which count up from 0.
const POST_ID: u64 = 1 << 63;

/// The id of the message channel post `seq` is drawn as, which keys its
/// block in the chat.
pub(crate) const fn post_block_key(seq: u64) -> u64 {
    POST_ID | seq
}

/// The message a channel post is drawn as: the user's as a prompt, an
/// agent's as a reply, each under its author's label. Its id comes from
/// the post's `seq` and its revision from what it shows, so the lines laid
/// out for it are reused until it changes.
pub(super) fn post_message(post: &ChannelPostView) -> Message {
    let (role, author) = match &post.author {
        ChannelPostAuthor::User => (MessageRole::User, "You"),
        ChannelPostAuthor::Agent { code, name, .. } => (
            MessageRole::Assistant,
            code.as_deref()
                .filter(|code| !code.is_empty())
                .unwrap_or(name),
        ),
    };
    let label = if post.reply_to.is_some() {
        format!("{author} ↩")
    } else {
        author.to_string()
    };
    let mut hasher = std::hash::DefaultHasher::new();
    (&label, &post.text, post.at_ms).hash(&mut hasher);
    let at = i64::try_from(post.at_ms)
        .ok()
        .and_then(chrono::DateTime::from_timestamp_millis)
        .unwrap_or_default();
    Message::post(
        (POST_ID | post.seq, hasher.finish()),
        role,
        label,
        (&post.text, at),
    )
}

/// `history` with `post_messages`, the messages of `posts`, among it in
/// the order [`merge_timeline`] puts them.
pub(super) fn interleave_posts<'a>(
    history: Vec<&'a Message>,
    posts: &[ChannelPostView],
    post_messages: &'a [Message],
) -> Vec<&'a Message> {
    if posts.is_empty() {
        return history;
    }
    // Only the times are merged on; each list keeps its order, so the
    // merged entries are taken from the front of their own list.
    let stamps: Vec<MessageData> = history
        .iter()
        .map(|message| MessageData {
            timestamp_ms: Some(message.timestamp_ms()),
            ..MessageData::new(message.role(), String::new())
        })
        .collect();
    let mut history = history.into_iter();
    let mut post_messages = post_messages.iter();
    merge_timeline(&stamps, posts)
        .into_iter()
        .filter_map(|item| match item {
            TimelineItem::Message(_) => history.next(),
            TimelineItem::Post(_) => post_messages.next(),
        })
        .collect()
}

/// Put the footer of a turn's latest reply, `meta`, under its last block
/// from the agent, at `last`.
fn close_turn<'a>(places: &mut [TurnPlace<'a>], last: Option<usize>, meta: Option<&'a Message>) {
    if let Some(place) = last.and_then(|at| places.get_mut(at)) {
        place.meta = meta;
    }
}
