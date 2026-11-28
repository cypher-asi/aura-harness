//! Showing a channel's posts in its coordinator's chat and the digest of
//! channel activity a coordinator's turn starts with.

use super::*;

/// Send every post of the channel `agent_id` coordinates to its chat,
/// as its conversation loads. Nothing for other agents. Returns the
/// channel's id and the user's posts (`seq` and text), whose summaries
/// the HISTORY tab wants.
pub(crate) async fn send_posts_of(
    commands: &tokio::sync::mpsc::Sender<UiCommand>,
    hub: &AgentHub,
    agent_id: AgentId,
) -> Option<(String, Vec<(u64, String)>)> {
    let (channel, _) = coordinated_channel(hub, agent_id).await?;
    let posts = read_posts(hub, &channel.id, None).await?;
    let mine = posts
        .iter()
        .filter(|post| post.author == PostAuthor::User)
        .map(|post| (post.seq, post.text.clone()))
        .collect();
    let views = {
        let registry = lock(&hub.registry);
        posts
            .iter()
            .map(|post| post_view(&registry, post))
            .collect()
    };
    AgentUi::new(commands.clone(), agent_id)
        .send(UiCommand::SetChannelPosts {
            channel_id: channel.id.clone(),
            posts: views,
        })
        .await;
    Some((channel.id, mine))
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
    // A `/plan` request went to its planner, not the coordinator.
    let posts: Vec<_> = posts
        .into_iter()
        .filter(|p| p.at_ms <= now && p.planner.is_none())
        .collect();
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
