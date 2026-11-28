use super::*;

/// What the hub keeps of the channels in memory, see
/// [`AgentHub::channel_members`] and [`AgentHub::hidden_members`].
#[derive(Debug, Default)]
pub(crate) struct ChannelSets {
    /// Every agent in a channel
    pub(crate) members: HashSet<AgentId>,
    /// The members of channels without a project
    pub(crate) hidden: HashSet<AgentId>,
    /// The members of channels whose agents run in the cloud, for
    /// [`AgentHub::cloud_members`]
    pub(crate) cloud: HashSet<AgentId>,
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
    promote_members(store, registry);
    let members = load_members(Some(store));
    let hidden = load_hidden(store, registry).unwrap_or_else(|error| {
        warn!(%error, "Could not read which channels have a project; hiding every member");
        members.clone()
    });
    let cloud = load_cloud(store, registry).unwrap_or_else(|error| {
        warn!(%error, "Could not read which channels run in the cloud");
        HashSet::new()
    });
    ChannelSets {
        members,
        hidden,
        cloud,
    }
}

/// The members of every channel in `store` whose agents run in the cloud,
/// for [`AgentHub::cloud_members`]: those brought in as members, but for
/// planners, which plan on this computer.
fn load_cloud(
    store: &ChannelStore,
    registry: &AgentRegistry,
) -> Result<HashSet<AgentId>, aura_store_db::ChannelError> {
    let mut cloud = HashSet::new();
    for channel in store.list()? {
        if channel.placement != ChannelPlacement::Cloud {
            continue;
        }
        let members = store.members(&channel.id)?;
        cloud.extend(
            members
                .into_iter()
                .filter(|member| member.role == MemberRole::Agent)
                .map(|member| member.agent_id)
                .filter(|agent| !is_planner(registry, *agent)),
        );
    }
    Ok(cloud)
}

/// Whether `agent` is an instance of the planner, which plans here.
fn is_planner(registry: &AgentRegistry, agent: AgentId) -> bool {
    registry
        .find(agent)
        .and_then(|found| found.definition.as_deref())
        .is_some_and(|definition| definition == super::plan::PLANNER_DEFINITION)
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

/// Make the agents coordinators spawned (members with the agent role
/// whose parent is still the coordinator) top-level members of their
/// channels, keeping the folders they work in: a coordinator hands work
/// to agents of the channel's own, never to agents under it. Idempotent:
/// a channel without such members is left alone. One that cannot be read
/// or written is left as it is (logged).
fn promote_members(store: &ChannelStore, registry: &mut AgentRegistry) {
    let channels = match store.list() {
        Ok(channels) => channels,
        Err(error) => {
            warn!(%error, "Could not read the channels to promote their agents");
            return;
        }
    };
    for channel in channels {
        match promote_channel_members(store, registry, &channel) {
            Ok(0) => {}
            Ok(agents) => info!(
                channel_id = %channel.id,
                agents,
                "Made the agents a coordinator spawned members of its channel"
            ),
            Err(error) => warn!(
                channel_id = %channel.id,
                error = %format!("{error:#}"),
                "Could not make the agents a coordinator spawned members of its channel"
            ),
        }
    }
}

/// [`promote_members`] for one channel; returns how many agents became
/// its members.
fn promote_channel_members(
    store: &ChannelStore,
    registry: &mut AgentRegistry,
    channel: &ChannelRecord,
) -> anyhow::Result<usize> {
    let members = store
        .members(&channel.id)
        .with_context(|| format!("reading the members of channel {}", channel.id))?;
    let Some(coordinator) = registry.find(channel.coordinator).map(|c| c.zns_id.clone()) else {
        return Ok(0);
    };
    let agents: Vec<AgentId> = members
        .iter()
        .filter(|member| member.role == MemberRole::Agent)
        .filter(|member| {
            registry
                .find(member.agent_id)
                .is_some_and(|a| a.parent.as_deref() == Some(coordinator.as_str()))
        })
        .map(|member| member.agent_id)
        .collect();
    if agents.is_empty() {
        return Ok(0);
    }
    registry.promote_agents(&agents, |agent_store, rows| {
        let mut batch = WriteBatch::default();
        stage_rows(agent_store, rows, &mut batch)?;
        store.write(batch).context("writing the promoted agents")
    })?;
    Ok(agents.len())
}

/// Stage the agent `rows` into `batch`.
pub(super) fn stage_rows(
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
pub(super) fn move_placements(
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
