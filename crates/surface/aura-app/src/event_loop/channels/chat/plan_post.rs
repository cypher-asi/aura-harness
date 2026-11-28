//! The user's `/plan` request in a channel, kept as a post of its own.
//!
//! A request typed with `/plan` goes to a new member, the planner, in its
//! own chat; the channel's chat shows nothing of it. So that the
//! channel's HISTORY still lists it, after a restart too, it is stored as
//! a user post that names its planner ([`ChannelPost::planner`]). Such a
//! post is not part of the channel's conversation: the UI leaves it out of
//! the chat, and the coordinator's digest leaves it out of its turns.
//!
//! # Failure modes
//!
//! A request that cannot be stored is logged and still goes to the
//! planner; HISTORY just doesn't list it.

use super::{append, coordinated_channel, send_post};
use crate::event_loop::LoopState;
use aura_core_types::time::now_ms;
use aura_core_types::AgentId;
use aura_store_db::{NewPost, PostAuthor};

/// Store `text`, the user's `/plan` request planned by `planner` in the
/// channel `coordinator` runs, as a plan post; send it to the
/// coordinator's chat (which files it for HISTORY only) and ask for its
/// summary. Nothing for an empty request or an agent that coordinates no
/// channel.
pub(crate) async fn post_plan_request(
    state: &LoopState<'_>,
    coordinator: AgentId,
    planner: AgentId,
    text: &str,
) {
    if text.trim().is_empty() {
        return;
    }
    let Some((channel, _)) = coordinated_channel(&state.hub, coordinator).await else {
        return;
    };
    let new = NewPost {
        author: PostAuthor::User,
        text: text.to_string(),
        mentions: vec![planner],
        reply_to: None,
        at_ms: now_ms(),
        planner: Some(planner),
    };
    let Some(post) = append(&state.hub, &channel.id, new).await else {
        return;
    };
    send_post(state.commands, &state.hub, coordinator, &post).await;
    let mine = vec![(post.seq, post.text.clone())];
    super::super::summaries::spawn_for(state, coordinator, &channel.id, mine);
}
