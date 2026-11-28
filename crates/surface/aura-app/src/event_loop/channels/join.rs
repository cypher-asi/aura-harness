use super::*;

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
    let Some((channel_id, channel)) = parent_channel(store, parent)? else {
        return Ok(None);
    };
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
    write_member(store, &mut registry, &channel_id, &member)?;
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

/// The id and record of the channel `parent` is in, if any.
fn parent_channel(
    store: &ChannelStore,
    parent: AgentId,
) -> anyhow::Result<Option<(String, ChannelRecord)>> {
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
    Ok(Some((channel_id, channel)))
}

/// Write the agent row of `member` and its member row of `channel_id` in
/// one batch.
fn write_member(
    store: &ChannelStore,
    registry: &mut AgentRegistry,
    channel_id: &str,
    member: &MemberRecord,
) -> anyhow::Result<()> {
    let child_id = member.agent_id;
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
            .stage_member(channel_id, member, &mut batch)
            .with_context(|| format!("staging {} in channel {channel_id}", child_id.to_hex()))?;
        store
            .write(batch)
            .with_context(|| format!("adding {} to channel {channel_id}", child_id.to_hex()))
    })?;
    Ok(())
}

/// Start a new member of the channel the agent `coordinator` runs: an
/// instance of `definition` at the top of the channel's project, working
/// in the folder the coordinator works in ([`AgentRegistry::member_draft`]).
///
/// Its agent row and its member row land in one batch, and it joins the
/// registry only once they did. As in [`join_parent_channel`], the
/// registry lock is held from reading the channel to updating
/// [`AgentHub::hidden_members`], and the reads and the write are
/// synchronous: callers on the async runtime run it on a blocking thread.
/// In a channel whose agents run in the cloud it goes on
/// [`AgentHub::cloud_starts`], so the loop moves it to a machine before
/// the work it is started for.
///
/// # Errors
/// Returns an error if there is no channel store, `coordinator` runs no
/// channel or an archived one, or the store cannot be read or written.
/// Nothing is registered on error.
pub(crate) fn add_agent(
    hub: &AgentHub,
    coordinator: AgentId,
    definition: &str,
) -> anyhow::Result<RegisteredAgent> {
    let Some(store) = hub.channels.as_ref() else {
        bail!("channels are not available without the agent database");
    };
    let mut registry = lock(&hub.registry);
    let (channel_id, channel) = open_channel_run_by(store, coordinator)?;
    let draft = registry.member_draft(definition, coordinator);
    let member = MemberRecord {
        agent_id: draft.agent_id(),
        role: MemberRole::Agent,
        parent: Some(coordinator),
        joined_at: chrono::Utc::now(),
    };
    let (agent, ()) = registry.add_with(draft, |agents, row| {
        let mut batch = WriteBatch::default();
        stage_rows(agents, vec![row], &mut batch)?;
        store
            .stage_member(&channel_id, &member, &mut batch)
            .with_context(|| format!("staging a new member of channel {channel_id}"))?;
        store
            .write(batch)
            .with_context(|| format!("adding a member to channel {channel_id}"))
    })?;
    let agent_id = agent.agent_id();
    registry.mark_channel_members([agent_id]);
    let workspace = registry.project_folder(agent_id);
    lock(&hub.channel_members).insert(agent_id);
    if !has_known_project(&registry, &channel) {
        lock(&hub.hidden_members).insert(agent_id);
    }
    if channel.placement == ChannelPlacement::Cloud {
        lock(&hub.cloud_starts).insert(agent_id);
        lock(&hub.cloud_members).insert(agent_id);
    }
    drop(registry);
    if let Some(workspace) = workspace {
        hub.factory
            .tasks
            .join_channel(&channel_id, agent_id, workspace);
    }
    info!(
        channel_id = %channel_id,
        agent = %agent_id.to_hex(),
        %definition,
        "Started a new channel member"
    );
    Ok(agent)
}

/// The id and row of the channel `coordinator` runs, when it isn't
/// archived.
fn open_channel_run_by(
    store: &ChannelStore,
    coordinator: AgentId,
) -> anyhow::Result<(String, ChannelRecord)> {
    let channel_id = store
        .channel_of(&coordinator)
        .with_context(|| format!("reading the channel of {}", coordinator.to_hex()))?
        .with_context(|| format!("{} is in no channel", coordinator.to_hex()))?;
    let channel = store
        .get(&channel_id)
        .with_context(|| format!("reading channel {channel_id}"))?
        .with_context(|| format!("channel {channel_id} is missing"))?;
    if channel.coordinator != coordinator {
        bail!("only the coordinator of #{} adds members", channel.slug);
    }
    if channel.archived {
        bail!("#{} is archived", channel.slug);
    }
    Ok((channel_id, channel))
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
