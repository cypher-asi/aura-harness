//! Channels on the event loop: making a channel together with its
//! coordinator, the project its agents work in, the `<channel>` block a
//! coordinator's turns get, the channels list the UI shows, and the UI's
//! channel actions.
//!
//! # Invariants
//!
//! * A channel and its coordinator's agent row land in one batch
//!   ([`ChannelStore::create`]); the coordinator joins the in-memory
//!   registry only after that batch landed, so a failed create leaves the
//!   registry as it was.
//! * A channel with a project has its coordinator at the top of that
//!   project (no folder of its own), so the coordinator and every agent
//!   under it are listed in the Projects tree. A channel without one has
//!   its coordinator in the launch project working in
//!   `<data_dir>/channels/<slug>/`, and its members are left out of the
//!   Projects tree ([`AgentHub::hidden_members`]). A channel naming a
//!   project the registry doesn't list (unknown or removed) is treated as
//!   having none.
//! * Every member is in its coordinator's project. Every member but the
//!   coordinator has a folder of its own, pinned when it joins (or when
//!   the channel's project changes), so moving the coordinator never
//!   moves the work of agents spawned before.
//! * A project change writes the channel record and every moved agent's
//!   row in one batch, under the registry lock, and the registry changes
//!   in memory only once it landed ([`AgentRegistry::place_agents`]).
//!   Joins and project changes read the channel under the registry lock,
//!   so they serialize.
//! * The registry lock is never held across an `await`; the channel
//!   store is read and written off the async runtime, but for the short
//!   synchronous joins and prompt reads documented on their functions.

use super::subagents::{lock, AgentHub};
use super::LoopState;
use crate::agent_definitions::COORDINATOR_DEFINITION;
use crate::agent_registry::{AgentPlacement, AgentRegistry, CoordinatorHome, RegisteredAgent};
use anyhow::{bail, Context};
use aura_core_types::AgentId;
use aura_store_db::agents::derive_agent_id;
use aura_store_db::{
    valid_slug, ChannelRecord, ChannelStore, ChannelUpdate, MemberRecord, MemberRole, NewChannel,
    RegistryDiff, WriteBatch,
};
use aura_ui::events::{nest_members, AgentSummary, ChannelProjectView, ChannelSummary};
use aura_ui::UiCommand;
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

/// Directory under the data directory holding each channel's folder.
const CHANNELS_DIR: &str = "channels";

/// The folder a channel without a project works in.
fn channel_folder(data_dir: &Path, slug: &str) -> PathBuf {
    data_dir.join(CHANNELS_DIR).join(slug)
}

/// Whether `project_id` names a listed (registered, not removed) project
/// of `registry`.
pub(crate) fn is_known_project(registry: &AgentRegistry, project_id: &str) -> bool {
    registry.project(project_id).is_some_and(|p| !p.removed)
}

/// Whether `channel` has a project its agents can work in: one it names
/// that `registry` lists. A channel naming an unknown or removed project
/// is treated as having none: its members are hidden.
fn has_known_project(registry: &AgentRegistry, channel: &ChannelRecord) -> bool {
    channel
        .project_id
        .as_deref()
        .is_some_and(|project_id| is_known_project(registry, project_id))
}

/// Fail unless `project_id` names a listed project of `registry`.
fn check_project(registry: &AgentRegistry, project_id: &str) -> anyhow::Result<()> {
    match registry.project(project_id) {
        Some(project) if !project.removed => Ok(()),
        Some(project) => bail!("project {} was removed", project.name()),
        None => bail!("there is no project {project_id}"),
    }
}

/// Make the channel `new` with a fresh coordinator, and register the
/// coordinator. With a project, the coordinator works in it; without,
/// in `<data_dir>/channels/<slug>/`, made here.
///
/// # Errors
/// Returns an error if the slug is invalid or taken, the project is
/// unknown or removed, the hub has no channel store, the folder cannot be
/// made, or the channel or registry cannot be written. Nothing is added
/// to the registry on error.
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

    let home = match &new.project_id {
        Some(project_id) => CoordinatorHome::Project(project_id.clone()),
        None => {
            let folder = channel_folder(lock(&hub.registry).data_dir(), &slug);
            tokio::fs::create_dir_all(&folder)
                .await
                .with_context(|| format!("creating channel folder {}", folder.display()))?;
            CoordinatorHome::Folder(folder)
        }
    };
    let made_folder = match &home {
        CoordinatorHome::Folder(folder) => Some(folder.clone()),
        CoordinatorHome::Project(_) => None,
    };
    let (draft, row) = {
        let mut registry = lock(&hub.registry);
        if let Some(project_id) = &new.project_id {
            check_project(&registry, project_id)
                .with_context(|| format!("creating channel #{slug}"))?;
        }
        let draft = registry.coordinator_draft(&slug, home);
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
            if let Some(folder) = made_folder {
                remove_empty_folder(folder).await;
            }
            return Err(error);
        }
    };
    let coordinator = {
        let mut registry = lock(&hub.registry);
        let coordinator = registry.adopt(draft);
        registry.mark_channel_members([channel.coordinator]);
        coordinator
    };
    lock(&hub.channel_members).insert(channel.coordinator);
    if channel.project_id.is_none() {
        lock(&hub.hidden_members).insert(channel.coordinator);
    }
    info!(
        channel_id = %channel.id,
        slug = %channel.slug,
        project_id = channel.project_id.as_deref().unwrap_or(""),
        coordinator = %coordinator.agent_id().to_hex(),
        "Created channel"
    );
    Ok(channel)
}

/// `project_id` trimmed, `None` when blank.
fn trimmed_project(project_id: Option<String>) -> Option<String> {
    project_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

/// `new` with its fields trimmed, the title defaulting to the slug and a
/// blank project meaning none.
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
        project_id: trimmed_project(new.project_id),
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

/// What the hub keeps of the channels in memory, see
/// [`AgentHub::channel_members`] and [`AgentHub::hidden_members`].
#[derive(Debug, Default)]
pub(crate) struct ChannelSets {
    /// Every agent in a channel
    pub(crate) members: HashSet<AgentId>,
    /// The members of channels without a project
    pub(crate) hidden: HashSet<AgentId>,
}

/// Bring the channels in `store` up to date with `registry` and read what
/// the hub keeps of them: fold the legacy project links into the
/// channels' records ([`ChannelStore::migrate_links`], taking only
/// projects `registry` lists), place the agents of channels with a
/// project in it ([`reconcile_projects`]), then read the member sets. A
/// channel naming a project `registry` doesn't list (unknown or removed)
/// is treated as having none: its members are hidden and its coordinator
/// is left where it is (logged). Failures are logged and the start
/// carries on with what could be read.
///
/// Reads and writes synchronously: it runs once, while the hub is built
/// at start-up.
pub(crate) fn load_channels(
    store: Option<&ChannelStore>,
    registry: &mut AgentRegistry,
) -> ChannelSets {
    let Some(store) = store else {
        return ChannelSets::default();
    };
    let known = |project_id: &str| is_known_project(registry, project_id);
    match store.migrate_links(known) {
        Ok(0) => {}
        Ok(migrated) => info!(migrated, "Moved legacy channel project links"),
        Err(error) => warn!(%error, "Could not move the legacy channel project links"),
    }
    reconcile_projects(store, registry);
    let members = load_members(Some(store));
    let hidden = load_hidden(store, registry).unwrap_or_else(|error| {
        warn!(%error, "Could not read which channels have a project; hiding every member");
        members.clone()
    });
    ChannelSets { members, hidden }
}

/// Every agent in a channel in `store`, for [`AgentHub::channel_members`].
/// Empty without a store, or when the store cannot be read (logged): the
/// set marks chat membership and keeps members of a removed project
/// listed, so a failed read loses those rather than failing the start.
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

/// The members of every channel in `store` without a project `registry`
/// lists, archived or not, for [`AgentHub::hidden_members`].
fn load_hidden(
    store: &ChannelStore,
    registry: &AgentRegistry,
) -> Result<HashSet<AgentId>, aura_store_db::ChannelError> {
    let mut hidden = HashSet::new();
    for channel in store.list()? {
        if !has_known_project(registry, &channel) {
            hidden.extend(store.members(&channel.id)?.into_iter().map(|m| m.agent_id));
        }
    }
    Ok(hidden)
}

/// Place the agents of every channel with a project in it, as
/// [`update_channel`] would, for channels whose record got its project
/// some other way (the legacy link migration). Idempotent: a channel
/// whose agents are placed already is left alone. A channel whose
/// project is unknown or removed is left as it is (logged), as is one
/// that cannot be moved.
fn reconcile_projects(store: &ChannelStore, registry: &mut AgentRegistry) {
    let channels = match store.list() {
        Ok(channels) => channels,
        Err(error) => {
            warn!(%error, "Could not read the channels to place their coordinators");
            return;
        }
    };
    for channel in channels.into_iter().filter(|c| !c.archived) {
        let Some(project_id) = channel.project_id.clone() else {
            continue;
        };
        if !is_known_project(registry, &project_id) {
            warn!(
                channel_id = %channel.id,
                project_id = %project_id,
                "The channel's project is unknown or removed; treating it as having none"
            );
            continue;
        }
        let home = CoordinatorHome::Project(project_id.clone());
        let moved = store
            .members(&channel.id)
            .with_context(|| format!("reading the members of channel {}", channel.id))
            .and_then(|members| {
                let placements = move_placements(registry, channel.coordinator, &members, home)?;
                if placements.is_empty() {
                    return Ok(false);
                }
                registry.place_agents(&placements, |agents, rows| {
                    let mut batch = WriteBatch::default();
                    stage_rows(agents, rows, &mut batch)?;
                    store.write(batch).context("writing the moved agents")
                })?;
                Ok(true)
            });
        match moved {
            Ok(false) => {}
            Ok(true) => info!(
                channel_id = %channel.id,
                project_id = %project_id,
                "Placed the channel's agents in its project"
            ),
            Err(error) => warn!(
                channel_id = %channel.id,
                project_id = %project_id,
                error = %format!("{error:#}"),
                "Could not place the channel's agents in its project"
            ),
        }
    }
}

/// Stage the agent `rows` into `batch`.
fn stage_rows(
    agents: &aura_store_db::AgentStore,
    rows: Vec<aura_store_db::AgentRecord>,
    batch: &mut WriteBatch,
) -> anyhow::Result<()> {
    let diff = RegistryDiff {
        agents_put: rows,
        ..RegistryDiff::default()
    };
    agents
        .stage(&diff, batch)
        .context("staging the moved agents' rows")
}

/// The placements that move the channel coordinator `coordinator` into
/// `home` and bring the other members of `members` along: each gets the
/// coordinator's new project, so the Projects tree lists it there and
/// what it spawns from now on is in that project too, and keeps the
/// folder it works in now as its own, so its work stays put (its history
/// and copies are there). With a [`CoordinatorHome::Folder`] the new
/// project is the launch project. Only placements that change an agent
/// are returned; members the registry no longer has are skipped.
///
/// # Errors
/// Returns an error if the coordinator is not registered.
fn move_placements(
    registry: &mut AgentRegistry,
    coordinator: AgentId,
    members: &[MemberRecord],
    home: CoordinatorHome,
) -> anyhow::Result<Vec<AgentPlacement>> {
    if registry.find(coordinator).is_none() {
        bail!("the coordinator {} is not registered", coordinator.to_hex());
    }
    let moved = registry.coordinator_placement(coordinator, home);
    let mut placements: Vec<AgentPlacement> = members
        .iter()
        .filter(|member| member.agent_id != coordinator)
        .filter_map(|member| pinned(registry, member.agent_id, moved.project_id.as_deref()))
        .collect();
    if changes(registry, &moved) {
        placements.push(moved);
    }
    Ok(placements)
}

/// Whether `placement` moves its agent anywhere.
fn changes(registry: &AgentRegistry, placement: &AgentPlacement) -> bool {
    registry.find(placement.agent_id).is_some_and(|agent| {
        agent.project_id != placement.project_id || agent.folder != placement.folder
    })
}

/// The placement putting `agent_id` in `project_id`, with the folder it
/// works in now as its own; `None` when that changes nothing or the
/// agent is unknown.
fn pinned(
    registry: &AgentRegistry,
    agent_id: AgentId,
    project_id: Option<&str>,
) -> Option<AgentPlacement> {
    let agent = registry.find(agent_id)?;
    let placement = AgentPlacement {
        agent_id,
        project_id: project_id.map(str::to_string),
        folder: agent
            .folder
            .clone()
            .or_else(|| registry.project_folder(agent_id)),
    };
    changes(registry, &placement).then_some(placement)
}

/// Make `child`, just registered under `parent`, a member of the channel
/// `parent` is in, if any: an agent of it when `parent` coordinates the
/// channel, else a subagent. Returns the channel's id.
///
/// The child's agent row and its member row land in one batch, so a
/// member never lacks its agent row. A child without a folder of its own
/// gets the one it works in now ([`AgentRegistry::project_folder`]) in
/// that row, so it keeps working there when the channel's project, and
/// so its coordinator, moves. The registry lock is held from reading the
/// channel to updating [`AgentHub::hidden_members`], so a join and a
/// project change ([`update_channel`]) never interleave: the child is
/// hidden or listed as the channel's project is when it joins, and the
/// row written is the registry's current one with no registry save
/// landing between. Like the registry's own saves, the reads and the
/// write are synchronous: a few point reads and one small batch.
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
    let mut registry = lock(&hub.registry);
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
    let agent = registry
        .find(child_id)
        .with_context(|| format!("{} is not registered", child_id.to_hex()))?;
    // Always one placement, so the batch carries the child's row even
    // when its folder is its own already.
    let placement = AgentPlacement {
        agent_id: child_id,
        project_id: agent.project_id.clone(),
        folder: agent
            .folder
            .clone()
            .or_else(|| registry.project_folder(child_id)),
    };
    registry.place_agents(&[placement], |agents, rows| {
        let mut batch = WriteBatch::default();
        stage_rows(agents, rows, &mut batch)
            .with_context(|| format!("staging the agent row of {}", child_id.to_hex()))?;
        store
            .stage_member(&channel_id, &member, &mut batch)
            .with_context(|| format!("staging {} in channel {channel_id}", child_id.to_hex()))?;
        store
            .write(batch)
            .with_context(|| format!("adding {} to channel {channel_id}", child_id.to_hex()))
    })?;
    registry.mark_channel_members([child_id]);
    let workspace = registry.project_folder(child_id);
    lock(&hub.channel_members).insert(child_id);
    if !has_known_project(&registry, &channel) {
        lock(&hub.hidden_members).insert(child_id);
    }
    drop(registry);
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
/// a channel: its slug, title, description, project and members.
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
        let members = store.members(&channel.id)?;
        Ok(Some((channel, members)))
    };
    match read() {
        Ok(found) => {
            let (channel, members) = found?;
            let registry = lock(&hub.registry);
            Some(render(&registry, &channel, &members))
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

fn render(registry: &AgentRegistry, channel: &ChannelRecord, members: &[MemberRecord]) -> String {
    let mut out = String::from("<channel>\n");
    let _ = writeln!(
        out,
        "You coordinate the channel #{}: {}",
        channel.slug, channel.title
    );
    if !channel.description.is_empty() {
        let _ = writeln!(out, "Description: {}", channel.description);
    }
    match &channel.project_id {
        Some(project_id) if is_known_project(registry, project_id) => {
            let view = project_view(registry, project_id);
            let _ = writeln!(
                out,
                "Project: {} ({}). Agents you spawn work there.",
                view.name, view.folder
            );
        }
        Some(_) => out.push_str(
            "Project: missing (unknown or removed). Agents you spawn work where you do; \
             ask the user to pick another project.\n",
        ),
        None => out.push_str("Project: none. Agents you spawn work in the channel's folder.\n"),
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
    }
}

/// What INFO calls a channel's project that the registry doesn't list.
const MISSING_PROJECT: &str = "(missing project)";

/// How the channel project `project_id` shows. A project the registry
/// doesn't list (unknown or removed) still shows, as
/// [`MISSING_PROJECT`] with its id and no folder, so the user sees the
/// channel has lost its project and can change it.
fn project_view(registry: &AgentRegistry, project_id: &str) -> ChannelProjectView {
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

/// What [`update_channel`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChannelUpdated {
    /// The channel's coordinator
    pub(crate) coordinator: AgentId,
    /// Whether the channel's project changed, moving its agents
    pub(crate) moved: bool,
}

/// Why [`update_channel`] refused a project change: the coordinator is
/// running a turn, whose session works in the folder it had before.
#[derive(Debug, thiserror::Error)]
#[error("#{slug}'s coordinator is busy; change the project when it is idle")]
pub(crate) struct CoordinatorBusy {
    slug: String,
}

/// Give channel `channel_id` the title `title` (kept when blank), the
/// description `description` and the project `project_id` (`None`, or
/// blank: no project).
///
/// When the project changes, the coordinator moves: into the new project
/// (working in its folder), or, without one, back to the launch project
/// working in `<data_dir>/channels/<slug>/`. Every other member goes to
/// the coordinator's new project and keeps working in its own folder
/// (see [`move_placements`]), and the Projects tree shows the members
/// under the project, or leaves them out without one. The channel record
/// and every moved agent's row are written in one batch.
///
/// The reads and writes run off the async runtime. From reading the
/// channel and its members to updating [`AgentHub::hidden_members`] they
/// hold the registry lock, so a project change and an agent joining
/// ([`join_parent_channel`]) never interleave.
///
/// # Errors
/// Returns an error if the channel is unknown or archived, the project
/// would change while the coordinator is running a turn
/// ([`CoordinatorBusy`]), the new project is unknown or removed, the
/// channel's folder cannot be made, or the channel or the agents cannot
/// be written. Nothing changes on error.
pub(crate) async fn update_channel(
    hub: &Arc<AgentHub>,
    channel_id: &str,
    title: &str,
    description: &str,
    project_id: Option<String>,
) -> anyhow::Result<ChannelUpdated> {
    store_of(hub)?;
    let title = title.trim();
    let update = ChannelUpdate {
        title: (!title.is_empty()).then(|| title.to_string()),
        description: Some(description.trim().to_string()),
        project_id: None,
    };
    let project_id = trimmed_project(project_id);
    let hub = Arc::clone(hub);
    let id = channel_id.to_string();
    tokio::task::spawn_blocking(move || apply_update(&hub, &id, update, project_id.as_deref()))
        .await
        .context("channel update task failed")?
}

/// The channel `channel_id`, unless it is missing or archived; read
/// synchronously.
fn open_record(store: &ChannelStore, channel_id: &str) -> anyhow::Result<ChannelRecord> {
    let channel = store
        .get(channel_id)
        .with_context(|| format!("reading channel {channel_id}"))?;
    match channel {
        Some(channel) if !channel.archived => Ok(channel),
        Some(channel) => bail!("#{} is archived", channel.slug),
        None => bail!("there is no channel {channel_id}"),
    }
}

/// The blocking part of [`update_channel`]: `update` with `project_id`.
fn apply_update(
    hub: &AgentHub,
    channel_id: &str,
    mut update: ChannelUpdate,
    project_id: Option<&str>,
) -> anyhow::Result<ChannelUpdated> {
    let store = store_of(hub)?;
    // A channel's slug and coordinator never change, so the busy check
    // and the folder a cleared project needs can come before the lock.
    // Turns start from the event loop, which waits for this update.
    let before = open_record(store, channel_id)?;
    if before.project_id.as_deref() != project_id && hub.is_running(before.coordinator) {
        return Err(CoordinatorBusy { slug: before.slug }.into());
    }
    let folder = match project_id {
        Some(_) => None,
        None => {
            let folder = channel_folder(lock(&hub.registry).data_dir(), &before.slug);
            std::fs::create_dir_all(&folder)
                .with_context(|| format!("creating channel folder {}", folder.display()))?;
            Some(folder)
        }
    };

    let mut registry = lock(&hub.registry);
    let channel = open_record(store, channel_id)?;
    let unchanged = ChannelUpdated {
        coordinator: channel.coordinator,
        moved: false,
    };
    if channel.project_id.as_deref() == project_id {
        store
            .update(channel_id, update, chrono::Utc::now())
            .with_context(|| format!("updating channel {channel_id}"))?;
        return Ok(unchanged);
    }
    let home = match (project_id, folder) {
        (Some(project_id), _) => {
            check_project(&registry, project_id)?;
            CoordinatorHome::Project(project_id.to_string())
        }
        (None, Some(folder)) => CoordinatorHome::Folder(folder),
        // Unreachable: the folder is made whenever there is no project.
        (None, None) => bail!("channel {channel_id} has no folder to work in"),
    };
    let members = store
        .members(&channel.id)
        .with_context(|| format!("reading the members of channel {channel_id}"))?;
    let placements = move_placements(&mut registry, channel.coordinator, &members, home)?;
    update.project_id = Some(project_id.map(str::to_string));
    registry.place_agents(&placements, |agents, rows| {
        let mut batch = WriteBatch::default();
        store
            .stage_update(&channel.id, update, chrono::Utc::now(), &mut batch)
            .with_context(|| format!("staging the update of channel {channel_id}"))?;
        stage_rows(agents, rows, &mut batch)?;
        store
            .write(batch)
            .with_context(|| format!("moving channel {channel_id} to its new project"))
    })?;
    {
        let mut hidden = lock(&hub.hidden_members);
        for member in &members {
            if project_id.is_some() {
                hidden.remove(&member.agent_id);
            } else {
                hidden.insert(member.agent_id);
            }
        }
    }
    drop(registry);
    info!(
        channel_id = %channel.id,
        slug = %channel.slug,
        from = channel.project_id.as_deref().unwrap_or(""),
        project_id = project_id.unwrap_or(""),
        members = members.len(),
        moved = placements.len(),
        "Changed the channel's project"
    );
    Ok(ChannelUpdated {
        moved: true,
        ..unchanged
    })
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

/// `UiEvent::CreateChannel`: make the channel, list it and open it. The
/// agent list goes out too, as a coordinator in a project is listed
/// under it.
pub(super) async fn handle_create(state: &mut LoopState<'_>, new: NewChannel) {
    match create_channel(&state.hub, new).await {
        Ok(channel) => {
            state.hub.send_agent_list().await;
            handle_open(state, &channel.id).await;
        }
        Err(error) => show_error(state, format!("Could not create the channel: {error:#}")).await,
    }
}

/// `UiEvent::UpdateChannel`: change the channel, then send the agent list
/// (and the channels with it), as a project change moves its members in
/// or out of the Projects tree. A project change is refused while the
/// coordinator runs a turn; once it is made, the coordinator's session,
/// which works in the old folder, is dropped so its next turn opens in
/// the new one, and the open channel's tasks are read again from where
/// its members work now.
pub(super) async fn handle_update(
    state: &mut LoopState<'_>,
    channel_id: &str,
    title: &str,
    description: &str,
    project_id: Option<String>,
) {
    let updated = match update_channel(&state.hub, channel_id, title, description, project_id).await
    {
        Ok(updated) => updated,
        Err(error) => {
            let message = match error.downcast_ref::<CoordinatorBusy>() {
                Some(busy) => busy.to_string(),
                None => format!("Could not update the channel: {error:#}"),
            };
            show_error(state, message).await;
            return;
        }
    };
    if updated.moved {
        reopen_coordinator(state, updated.coordinator).await;
        if state.open_channel.as_deref() == Some(channel_id) {
            if let Err(error) = super::tasks::send_channel(&state.hub, channel_id).await {
                warn!(
                    channel_id,
                    error = %format!("{error:#}"),
                    "Could not show the channel's tasks after its project changed"
                );
            }
        }
    }
    state.hub.send_agent_list().await;
}

/// Drop the idle session of `coordinator`, opened in the folder it had
/// before its channel's project changed, so its next turn opens a session
/// in the new one. When it is the active agent its new session opens now
/// and the Sandbox panel follows it.
async fn reopen_coordinator(state: &mut LoopState<'_>, coordinator: AgentId) {
    if state.idle.remove(&coordinator).is_none() || state.active != coordinator {
        return;
    }
    // On failure `open_session` logged why; the next turn opens it lazily.
    if let Ok(session) = super::agents::open_session(state, coordinator) {
        state.idle.insert(coordinator, session);
        super::agents::send_sandbox_root(state).await;
    }
}

/// `UiEvent::OpenChannel`: make the channel's coordinator the active
/// agent, as selecting it would, and show the channel's tasks. A UI that
/// put the coordinator on screen itself asked for it just before
/// (`SelectAgent` or `FocusAgent`), so it is active already and its chat
/// is not loaded a second time.
pub(super) async fn handle_open(state: &mut LoopState<'_>, channel_id: &str) {
    let channel = match open_channel_record(&state.hub, channel_id).await {
        Ok(channel) => channel,
        Err(error) => {
            show_error(state, format!("Could not open the channel: {error:#}")).await;
            return;
        }
    };
    if state.active != channel.coordinator {
        super::agents::handle_select_agent(state, &channel.coordinator.to_hex()).await;
    }
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
