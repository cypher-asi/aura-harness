use super::*;

/// A channel that is not archived, as read from the store.
struct ChannelRead {
    channel: ChannelRecord,
    members: Vec<MemberRecord>,
}

/// Every channel that is not archived, with its members.
async fn read_channels(store: &ChannelStore) -> anyhow::Result<Vec<ChannelRead>> {
    blocking(store, |store| {
        let mut out = Vec::new();
        for channel in store.list()? {
            if channel.archived {
                continue;
            }
            let members = store.members(&channel.id)?;
            out.push(ChannelRead { channel, members });
        }
        Ok(out)
    })
    .await
    .context("reading the channels")
}

/// The channels list: every channel that is not archived, in the order
/// they were made, each with its project and its members. Member
/// rows are the Agents panel's rows of those agents, nested under the
/// coordinator by who brought them in. Empty without a channel store.
///
/// # Errors
/// Returns an error if the channel store cannot be read.
pub(crate) async fn channel_summaries(hub: &AgentHub) -> anyhow::Result<Vec<ChannelSummary>> {
    if hub.channels.is_none() {
        return Ok(Vec::new());
    }
    summaries_with(hub, hub.member_rows()).await
}

/// [`channel_summaries`] with the member rows already laid out.
async fn summaries_with(
    hub: &AgentHub,
    mut rows: HashMap<AgentId, AgentSummary>,
) -> anyhow::Result<Vec<ChannelSummary>> {
    let Some(store) = hub.channels.as_ref() else {
        return Ok(Vec::new());
    };
    let reads = read_channels(store).await?;
    let registry = lock(&hub.registry);
    Ok(reads
        .into_iter()
        .map(|read| summarize(&registry, read, &mut rows))
        .collect())
}

/// The summary of one channel. Members without a row (archived agents)
/// are left out; each row is used once, as an agent is in one channel.
fn summarize(
    registry: &AgentRegistry,
    read: ChannelRead,
    rows: &mut HashMap<AgentId, AgentSummary>,
) -> ChannelSummary {
    let ChannelRead { channel, members } = read;
    let project = channel
        .project_id
        .as_deref()
        .map(|project_id| project_view(registry, project_id));
    let members = members
        .into_iter()
        .filter_map(|member| {
            let row = rows.remove(&member.agent_id)?;
            Some((row, member.parent.map(|parent| parent.to_hex())))
        })
        .collect();
    let coordinator_id = channel.coordinator.to_hex();
    ChannelSummary {
        members: nest_members(&coordinator_id, members),
        id: channel.id,
        slug: channel.slug,
        title: channel.title,
        description: channel.description,
        coordinator_id,
        project,
        archived: channel.archived,
        placement: placement_view(channel.placement),
    }
}

/// What INFO calls a channel's project that the registry doesn't list.
pub(super) const MISSING_PROJECT: &str = "(missing project)";

/// How the channel project `project_id` shows. A project the registry
/// doesn't list (unknown or removed) still shows, as
/// [`MISSING_PROJECT`] with its id and no folder, so the user sees the
/// channel has lost its project and can change it.
pub(super) fn project_view(registry: &AgentRegistry, project_id: &str) -> ChannelProjectView {
    match registry.project(project_id).filter(|p| !p.removed) {
        Some(project) => ChannelProjectView {
            id: project_id.to_string(),
            name: project.name(),
            folder: project.folder.display().to_string(),
        },
        None => ChannelProjectView {
            id: project_id.to_string(),
            name: MISSING_PROJECT.to_string(),
            folder: String::new(),
        },
    }
}

/// Send the channels list to the UI. Nothing is sent without a channel
/// store, or when it cannot be read (logged): the UI keeps the list it
/// has rather than losing every channel.
pub(crate) async fn send_channels(hub: &AgentHub) {
    if hub.channels.is_some() {
        send_channels_with(hub, hub.member_rows(), true).await;
    }
}

/// [`send_channels`] with the member rows already laid out. Unless
/// `even_empty`, an empty list is not sent: the agent list refreshes it
/// on every change, and most sessions have no channels.
pub(crate) async fn send_channels_with(
    hub: &AgentHub,
    rows: HashMap<AgentId, AgentSummary>,
    even_empty: bool,
) {
    if hub.channels.is_none() {
        return;
    }
    match summaries_with(hub, rows).await {
        Ok(summaries) if summaries.is_empty() && !even_empty => {}
        Ok(summaries) => {
            let _ = hub.commands().send(UiCommand::SetChannels(summaries)).await;
        }
        Err(error) => warn!(error = %format!("{error:#}"), "Could not list the channels"),
    }
}

/// The channel `channel_id`, unless it is missing or archived.
///
/// # Errors
/// Returns an error if there is no such open channel or the store cannot
/// be read.
pub(crate) async fn open_channel_record(
    hub: &AgentHub,
    channel_id: &str,
) -> anyhow::Result<ChannelRecord> {
    let store = store_of(hub)?;
    let id = channel_id.to_string();
    let channel = blocking(store, move |store| store.get(&id))
        .await
        .with_context(|| format!("reading channel {channel_id}"))?;
    match channel {
        Some(channel) if !channel.archived => Ok(channel),
        Some(channel) => bail!("#{} is archived", channel.slug),
        None => bail!("there is no channel {channel_id}"),
    }
}

/// The agents in channel `channel_id`, in the order they joined.
///
/// # Errors
/// Returns an error if the channel store cannot be read.
pub(crate) async fn member_ids(hub: &AgentHub, channel_id: &str) -> anyhow::Result<Vec<AgentId>> {
    let store = store_of(hub)?;
    let id = channel_id.to_string();
    let members = blocking(store, move |store| store.members(&id))
        .await
        .with_context(|| format!("reading the members of channel {channel_id}"))?;
    Ok(members.into_iter().map(|member| member.agent_id).collect())
}
/// The first channel that is not archived and keeps project
/// `project_id` in use: it works in the project, or it has a project and
/// one of its members is in this one. `None` if there is none, or
/// without a channel store.
///
/// The members of a channel without a project sit, hidden, in the launch
/// project; they don't keep it in use, since removing a project keeps
/// channel members (see [`AgentRegistry::remove_project`]).
///
/// # Errors
/// Returns an error if the channel store cannot be read.
pub(crate) async fn channel_in_project(
    hub: &AgentHub,
    project_id: &str,
) -> anyhow::Result<Option<ChannelRecord>> {
    let Some(store) = hub.channels.as_ref() else {
        return Ok(None);
    };
    let reads = read_channels(store).await?;
    let registry = lock(&hub.registry);
    Ok(reads
        .into_iter()
        .find(|read| uses_project(&registry, read, project_id))
        .map(|read| read.channel))
}

/// Whether the channel `read` works in project `project_id`, or has a
/// project and a member in this one.
fn uses_project(registry: &AgentRegistry, read: &ChannelRead, project_id: &str) -> bool {
    let Some(own) = read.channel.project_id.as_deref() else {
        return false;
    };
    own == project_id
        || read.members.iter().any(|member| {
            registry
                .find(member.agent_id)
                .is_some_and(|agent| agent.project_id.as_deref() == Some(project_id))
        })
}
