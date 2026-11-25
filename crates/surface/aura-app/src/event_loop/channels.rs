//! Channels on the event loop: making a channel together with its
//! coordinator, the `<channel>` block a coordinator's turns get, the
//! channels list the UI shows, and the UI's channel actions.
//!
//! # Invariants
//!
//! * A channel and its coordinator's agent row land in one batch
//!   ([`ChannelStore::create`]); the coordinator joins the in-memory
//!   registry only after that batch landed, so a failed create leaves the
//!   registry as it was.
//! * The registry lock is never held across an `await`.

use super::subagents::{lock, AgentHub};
use super::LoopState;
use crate::agent_definitions::COORDINATOR_DEFINITION;
use crate::agent_registry::{AgentRegistry, RegisteredAgent};
use anyhow::{bail, Context};
use aura_core_types::AgentId;
use aura_store_db::agents::derive_agent_id;
use aura_store_db::{
    valid_slug, ChannelRecord, ChannelStore, ChannelUpdate, MemberRecord, MemberRole, NewChannel,
    ProjectLink, RegistryDiff, WriteBatch,
};
use aura_ui_terminal::events::{nest_members, AgentSummary, ChannelProjectView, ChannelSummary};
use aura_ui_terminal::UiCommand;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::PathBuf;
use tracing::{info, warn};

/// Directory under the data directory holding each channel's folder.
const CHANNELS_DIR: &str = "channels";

/// Make the channel `new` with a fresh coordinator working in
/// `<data_dir>/channels/<slug>/`, and register the coordinator.
///
/// # Errors
/// Returns an error if the slug is invalid or taken, the hub has no
/// channel store, the folder cannot be made, or the channel or registry
/// cannot be written. Nothing is added to the registry on error.
pub(crate) async fn create_channel(
    hub: &AgentHub,
    new: NewChannel,
) -> anyhow::Result<ChannelRecord> {
    let new = trimmed(new);
    let slug = new.slug.clone();
    if !valid_slug(&slug) {
        bail!("invalid channel name `{slug}`: use 1-40 characters of a-z, 0-9 and -");
    }
    let Some(store) = hub.channels.clone() else {
        bail!("channels are not available without the agent database");
    };
    if blocking(&store, {
        let slug = slug.clone();
        move |store| store.by_slug(&slug)
    })
    .await
    .with_context(|| format!("checking whether #{slug} exists"))?
    .is_some()
    {
        bail!("a channel named #{slug} already exists");
    }

    let folder = lock(&hub.registry)
        .data_dir()
        .join(CHANNELS_DIR)
        .join(&slug);
    tokio::fs::create_dir_all(&folder)
        .await
        .with_context(|| format!("creating channel folder {}", folder.display()))?;

    let (draft, row) = {
        let mut registry = lock(&hub.registry);
        let draft = registry.coordinator_draft(&slug, folder.clone());
        let row = registry.record_for(&draft);
        (draft, row)
    };
    let created = blocking(&store, move |store| {
        store.create(new, &row, chrono::Utc::now())
    })
    .await
    .with_context(|| format!("creating channel #{slug}"));
    let channel = match created {
        Ok(channel) => channel,
        Err(error) => {
            remove_empty_folder(folder).await;
            return Err(error);
        }
    };
    let coordinator = {
        let mut registry = lock(&hub.registry);
        let coordinator = registry
            .adopt(draft)
            .with_context(|| format!("registering the coordinator of #{slug}"))?;
        registry.mark_channel_members([channel.coordinator]);
        coordinator
    };
    lock(&hub.channel_members).insert(channel.coordinator);
    info!(
        channel_id = %channel.id,
        slug = %channel.slug,
        coordinator = %coordinator.agent_id().to_hex(),
        "Created channel"
    );
    Ok(channel)
}

/// `new` with its fields trimmed and the title defaulting to the slug.
fn trimmed(new: NewChannel) -> NewChannel {
    let slug = new.slug.trim().to_string();
    let title = match new.title.trim() {
        "" => slug.clone(),
        title => title.to_string(),
    };
    NewChannel {
        title,
        description: new.description.trim().to_string(),
        slug,
    }
}

/// Run `read` on a clone of `store` off the async runtime.
async fn blocking<T: Send + 'static>(
    store: &ChannelStore,
    read: impl FnOnce(&ChannelStore) -> Result<T, aura_store_db::ChannelError> + Send + 'static,
) -> anyhow::Result<T> {
    let store = store.clone();
    let result = tokio::task::spawn_blocking(move || read(&store))
        .await
        .context("channel store task failed")?;
    Ok(result?)
}

/// Remove the folder a failed create made, if nothing was put in it.
async fn remove_empty_folder(folder: PathBuf) {
    if let Err(error) = tokio::fs::remove_dir(&folder).await {
        warn!(
            folder = %folder.display(),
            %error,
            "Could not remove the folder of a channel that was not created"
        );
    }
}

/// Every agent in a channel in `store`, for [`AgentHub::channel_members`].
/// Empty without a store, or when the store cannot be read (logged): the
/// set only hides rows from the Projects tree, so a failed read shows
/// members there rather than failing the start.
///
/// Reads synchronously: it runs once, while the hub is built at start-up.
pub(crate) fn load_members(store: Option<&ChannelStore>) -> HashSet<AgentId> {
    let Some(store) = store else {
        return HashSet::new();
    };
    match store.all_members() {
        Ok(members) => members.into_iter().map(|(_, agent)| agent).collect(),
        Err(error) => {
            warn!(%error, "Could not read the channel members");
            HashSet::new()
        }
    }
}

/// Make `child`, just registered under `parent`, a member of the channel
/// `parent` is in, if any: an agent of it when `parent` coordinates the
/// channel, else a subagent. Returns the channel's id.
///
/// The child's agent row and its member row land in one batch, so a
/// member never lacks its agent row. The registry lock is held while the
/// batch is staged and written, so the row written is the registry's
/// current one and no registry save can land between; like the
/// registry's own saves, the write is synchronous: one small batch.
///
/// # Errors
/// Returns an error if the channel store cannot be read or written, or
/// `child` is not in the registry. Callers log it: a failed join must
/// not fail the spawn.
pub(crate) fn join_parent_channel(
    hub: &AgentHub,
    parent: AgentId,
    child: &RegisteredAgent,
) -> anyhow::Result<Option<String>> {
    let Some(store) = hub.channels.as_ref() else {
        return Ok(None);
    };
    let child_id = child.agent_id();
    let Some(channel_id) = store
        .channel_of(&parent)
        .with_context(|| format!("reading the channel of {}", parent.to_hex()))?
    else {
        return Ok(None);
    };
    let channel = store
        .get(&channel_id)
        .with_context(|| format!("reading channel {channel_id}"))?
        .with_context(|| format!("channel {channel_id} of {} is missing", parent.to_hex()))?;
    let role = if channel.coordinator == parent {
        MemberRole::Agent
    } else {
        MemberRole::Subagent
    };
    let member = MemberRecord {
        agent_id: child_id,
        role,
        parent: Some(parent),
        joined_at: chrono::Utc::now(),
    };
    let workspace = {
        let mut registry = lock(&hub.registry);
        let row = registry
            .row_of(child_id)
            .with_context(|| format!("{} is not registered", child_id.to_hex()))?;
        let diff = RegistryDiff {
            agents_put: vec![row],
            ..RegistryDiff::default()
        };
        let mut batch = WriteBatch::default();
        registry
            .store()
            .stage(&diff, &mut batch)
            .with_context(|| format!("staging the agent row of {}", child_id.to_hex()))?;
        store
            .stage_member(&channel_id, &member, &mut batch)
            .with_context(|| format!("staging {} in channel {channel_id}", child_id.to_hex()))?;
        store
            .write(batch)
            .with_context(|| format!("adding {} to channel {channel_id}", child_id.to_hex()))?;
        registry.mark_channel_members([child_id]);
        registry.project_folder(child_id)
    };
    lock(&hub.channel_members).insert(child_id);
    if let Some(workspace) = workspace {
        hub.factory
            .tasks
            .join_channel(&channel_id, child_id, workspace);
    }
    info!(
        channel_id = %channel_id,
        agent = %child_id.to_hex(),
        parent = %parent.to_hex(),
        ?role,
        "Agent joined its parent's channel"
    );
    Ok(Some(channel_id))
}

/// [`join_parent_channel`] for a spawn path, with the parent `child` was
/// registered under; logs a failure and carries on, since the child runs
/// whether or not it joined. Does nothing for an agent without a parent.
pub(crate) fn join_or_warn(hub: &AgentHub, child: &RegisteredAgent) {
    let Some(parent) = child.parent.as_deref().map(derive_agent_id) else {
        return;
    };
    if let Err(error) = join_parent_channel(hub, parent, child) {
        warn!(
            parent = %parent.to_hex(),
            agent = %child.agent_id().to_hex(),
            error = %format!("{error:#}"),
            "Could not add a spawned agent to its parent's channel"
        );
    }
}

/// The `<channel>` block for the turns of `agent_id` when it coordinates
/// a channel: its slug, title, description, linked projects and members.
/// `None` for any other agent, or when the channel cannot be read (logged).
///
/// Reads the channel store synchronously: a handful of point reads and
/// short prefix scans of one channel, cheap enough for the turn setup it
/// runs in, which already reads definition files.
pub(crate) fn channel_prompt(hub: &AgentHub, agent_id: AgentId) -> Option<String> {
    let store = hub.channels.as_ref()?;
    let read = || -> Result<_, aura_store_db::ChannelError> {
        let Some(channel_id) = store.channel_of(&agent_id)? else {
            return Ok(None);
        };
        let Some(channel) = store.get(&channel_id)? else {
            return Ok(None);
        };
        if channel.coordinator != agent_id {
            return Ok(None);
        }
        let projects = store.projects(&channel.id)?;
        let members = store.members(&channel.id)?;
        Ok(Some((channel, projects, members)))
    };
    match read() {
        Ok(found) => {
            let (channel, projects, members) = found?;
            let registry = lock(&hub.registry);
            Some(render(&registry, &channel, &projects, &members))
        }
        Err(error) => {
            warn!(
                agent = %agent_id.to_hex(),
                error = %error,
                "Could not read the channel for the coordinator's prompt"
            );
            None
        }
    }
}

/// Whether `definition` is the channel coordinator's.
pub(crate) fn is_coordinator(definition: Option<&str>) -> bool {
    definition == Some(COORDINATOR_DEFINITION)
}

fn render(
    registry: &AgentRegistry,
    channel: &ChannelRecord,
    projects: &[ProjectLink],
    members: &[MemberRecord],
) -> String {
    let mut out = String::from("<channel>\n");
    let _ = writeln!(
        out,
        "You coordinate the channel #{}: {}",
        channel.slug, channel.title
    );
    if !channel.description.is_empty() {
        let _ = writeln!(out, "Description: {}", channel.description);
    }
    out.push_str("Linked projects:");
    if projects.is_empty() {
        out.push_str(" none\n");
    } else {
        out.push('\n');
        for link in projects {
            let folder = registry.project(&link.project_id).map_or_else(
                || "(unknown folder)".to_string(),
                |p| p.folder.display().to_string(),
            );
            let _ = writeln!(out, "- {}: {folder}", link.project_id);
        }
    }
    out.push_str("Members:\n");
    for member in members {
        let agent = registry.find(member.agent_id);
        let code = agent
            .and_then(|a| a.code.as_deref())
            .unwrap_or("?")
            .to_string();
        let name = agent.map_or_else(
            || member.agent_id.to_hex(),
            |a| a.display_name().into_owned(),
        );
        let role = match member.role {
            MemberRole::Coordinator => "coordinator",
            MemberRole::Agent => "agent",
            MemberRole::Subagent => "subagent",
        };
        let _ = write!(out, "- {code} {name} ({role})");
        if let Some(parent) = member.parent {
            let parent_code = registry
                .find(parent)
                .and_then(|a| a.code.as_deref())
                .unwrap_or("?");
            let _ = write!(out, ", brought in by {parent_code}");
        }
        out.push('\n');
    }
    out.push_str("</channel>");
    out
}

/// A channel that is not archived, as read from the store.
struct ChannelRead {
    channel: ChannelRecord,
    projects: Vec<ProjectLink>,
    members: Vec<MemberRecord>,
}

/// Every channel that is not archived, with its projects and members.
async fn read_channels(store: &ChannelStore) -> anyhow::Result<Vec<ChannelRead>> {
    blocking(store, |store| {
        let mut out = Vec::new();
        for channel in store.list()? {
            if channel.archived {
                continue;
            }
            let projects = store.projects(&channel.id)?;
            let members = store.members(&channel.id)?;
            out.push(ChannelRead {
                channel,
                projects,
                members,
            });
        }
        Ok(out)
    })
    .await
    .context("reading the channels")
}

/// The channels list: every channel that is not archived, in the order
/// they were made, each with its linked projects and its members. Member
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
    let ChannelRead {
        channel,
        projects,
        members,
    } = read;
    let projects = projects
        .into_iter()
        .map(|link| match registry.project(&link.project_id) {
            Some(project) => ChannelProjectView {
                name: project.name(),
                folder: project.folder.display().to_string(),
                id: link.project_id,
            },
            // Still listed, so the user can see it and unlink it.
            None => ChannelProjectView {
                name: link.project_id.clone(),
                folder: String::new(),
                id: link.project_id,
            },
        })
        .collect();
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
        projects,
        archived: channel.archived,
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

/// The channel store, or why channels are unavailable.
fn store_of(hub: &AgentHub) -> anyhow::Result<&ChannelStore> {
    hub.channels
        .as_ref()
        .context("channels are not available without the agent database")
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

/// Give channel `channel_id` the title `title` (kept when blank) and the
/// description `description`.
///
/// # Errors
/// Returns an error if the channel is unknown or cannot be written.
pub(crate) async fn update_channel(
    hub: &AgentHub,
    channel_id: &str,
    title: &str,
    description: &str,
) -> anyhow::Result<()> {
    let store = store_of(hub)?;
    let title = title.trim();
    let update = ChannelUpdate {
        title: (!title.is_empty()).then(|| title.to_string()),
        description: Some(description.trim().to_string()),
    };
    let id = channel_id.to_string();
    blocking(store, move |store| {
        store.update(&id, update, chrono::Utc::now())
    })
    .await
    .with_context(|| format!("updating channel {channel_id}"))?;
    Ok(())
}

/// Link the project `project_id`, which must be in the registry, to
/// channel `channel_id`.
///
/// # Errors
/// Returns an error if the project is unknown, or the channel is unknown
/// or cannot be written.
pub(crate) async fn link_project(
    hub: &AgentHub,
    channel_id: &str,
    project_id: &str,
) -> anyhow::Result<()> {
    let store = store_of(hub)?;
    if lock(&hub.registry).project(project_id).is_none() {
        bail!("there is no project {project_id}");
    }
    let (channel, project) = (channel_id.to_string(), project_id.to_string());
    blocking(store, move |store| {
        store.link_project(&channel, &project, chrono::Utc::now())
    })
    .await
    .with_context(|| format!("linking project {project_id} to channel {channel_id}"))?;
    Ok(())
}

/// Unlink the project `project_id` from channel `channel_id`.
///
/// # Errors
/// Returns an error if the link cannot be removed.
pub(crate) async fn unlink_project(
    hub: &AgentHub,
    channel_id: &str,
    project_id: &str,
) -> anyhow::Result<()> {
    let store = store_of(hub)?;
    let (channel, project) = (channel_id.to_string(), project_id.to_string());
    blocking(store, move |store| store.unlink_project(&channel, &project))
        .await
        .with_context(|| format!("unlinking project {project_id} from channel {channel_id}"))
}

/// Move channel `channel_id` to the Archive.
///
/// # Errors
/// Returns an error if the channel is unknown or cannot be written.
pub(crate) async fn archive_channel(hub: &AgentHub, channel_id: &str) -> anyhow::Result<()> {
    let store = store_of(hub)?;
    let id = channel_id.to_string();
    let channel = blocking(store, move |store| store.archive(&id, chrono::Utc::now()))
        .await
        .with_context(|| format!("archiving channel {channel_id}"))?;
    info!(channel_id = %channel.id, slug = %channel.slug, "Archived channel");
    Ok(())
}

/// `UiEvent::CreateChannel`: make the channel, list it and open it.
pub(super) async fn handle_create(state: &mut LoopState<'_>, new: NewChannel) {
    match create_channel(&state.hub, new).await {
        Ok(channel) => {
            send_channels(&state.hub).await;
            handle_open(state, &channel.id).await;
        }
        Err(error) => show_error(state, format!("Could not create the channel: {error:#}")).await,
    }
}

/// `UiEvent::OpenChannel`: make the channel's coordinator the active
/// agent, as selecting it would, and show the channel's tasks.
pub(super) async fn handle_open(state: &mut LoopState<'_>, channel_id: &str) {
    let channel = match open_channel_record(&state.hub, channel_id).await {
        Ok(channel) => channel,
        Err(error) => {
            show_error(state, format!("Could not open the channel: {error:#}")).await;
            return;
        }
    };
    super::agents::handle_select_agent(state, &channel.coordinator.to_hex()).await;
    state.open_channel = Some(channel.id.clone());
    if let Err(error) = super::tasks::send_channel(&state.hub, &channel.id).await {
        warn!(
            channel_id = %channel.id,
            error = %format!("{error:#}"),
            "Could not show the channel's tasks"
        );
    }
}

/// `UiEvent::CloseChannel`: no channel is open any more.
pub(super) fn handle_close(state: &mut LoopState<'_>) {
    state.open_channel = None;
    state.hub.factory.tasks.close_channel();
}

/// `UiEvent::ArchiveChannel`: archive it, closing it when it is open.
pub(super) async fn handle_archive(state: &mut LoopState<'_>, channel_id: &str) {
    let archived = archive_channel(&state.hub, channel_id).await;
    if archived.is_ok() && state.open_channel.as_deref() == Some(channel_id) {
        handle_close(state);
    }
    after_change(state, "archive the channel", archived).await;
}

/// After a channel action: list the channels again when it worked, else
/// tell the user it could not `what`.
pub(super) async fn after_change(state: &LoopState<'_>, what: &str, result: anyhow::Result<()>) {
    match result {
        Ok(()) => send_channels(&state.hub).await,
        Err(error) => show_error(state, format!("Could not {what}: {error:#}")).await,
    }
}

async fn show_error(state: &LoopState<'_>, message: String) {
    let _ = state.commands.send(UiCommand::ShowError(message)).await;
}

#[cfg(test)]
mod tests;

mod chat;
pub(crate) use chat::{
    drop_unanswered, post_replies, route_user_message, send_posts_of, take_digest, ChannelChat,
};
