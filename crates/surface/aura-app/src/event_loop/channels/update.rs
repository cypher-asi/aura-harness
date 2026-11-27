use super::*;

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

/// A channel's title, description, project and placement, as the edit
/// form sends them: every one, each time.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChannelEdit {
    /// New title; a blank one keeps the old title
    pub(crate) title: String,
    /// New description
    pub(crate) description: String,
    /// Project its agents work in (`None`, or blank: no project)
    pub(crate) project_id: Option<String>,
    /// Where the members it starts from now on run; members it has stay
    /// where they are
    pub(crate) placement: ChannelPlacement,
}

/// Give channel `channel_id` the title, description, project and
/// placement of `edit`.
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
    edit: ChannelEdit,
) -> anyhow::Result<ChannelUpdated> {
    store_of(hub)?;
    let title = edit.title.trim();
    let update = ChannelUpdate {
        title: (!title.is_empty()).then(|| title.to_string()),
        description: Some(edit.description.trim().to_string()),
        project_id: None,
        placement: Some(edit.placement),
    };
    let project_id = trimmed_project(edit.project_id);
    let hub = Arc::clone(hub);
    let id = channel_id.to_string();
    tokio::task::spawn_blocking(move || apply_update(&hub, &id, update, project_id.as_deref()))
        .await
        .context("channel update task failed")?
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
        None => Some(make_folder(hub, &before.slug)?),
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
    let home = new_home(&registry, channel_id, project_id, folder)?;
    let members = store
        .members(&channel.id)
        .with_context(|| format!("reading the members of channel {channel_id}"))?;
    let placements = move_placements(&mut registry, channel.coordinator, &members, home)?;
    update.project_id = Some(project_id.map(str::to_string));
    write_moved(store, &mut registry, &channel.id, update, &placements)?;
    update_hidden(hub, &members, project_id.is_some());
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

/// Write `update` of channel `channel_id` and the rows of the agents
/// `placements` move in one batch.
fn write_moved(
    store: &ChannelStore,
    registry: &mut AgentRegistry,
    channel_id: &str,
    update: ChannelUpdate,
    placements: &[AgentPlacement],
) -> anyhow::Result<()> {
    registry.place_agents(placements, |agents, rows| {
        let mut batch = WriteBatch::default();
        store
            .stage_update(channel_id, update, chrono::Utc::now(), &mut batch)
            .with_context(|| format!("staging the update of channel {channel_id}"))?;
        stage_rows(agents, rows, &mut batch)?;
        store
            .write(batch)
            .with_context(|| format!("moving channel {channel_id} to its new project"))
    })?;
    Ok(())
}

/// Make the folder of channel `slug`, for a channel without a project.
fn make_folder(hub: &AgentHub, slug: &str) -> anyhow::Result<PathBuf> {
    let folder = channel_folder(lock(&hub.registry).data_dir(), slug);
    std::fs::create_dir_all(&folder)
        .with_context(|| format!("creating channel folder {}", folder.display()))?;
    Ok(folder)
}

/// Where the coordinator of `channel_id` works with `project_id`: in it,
/// or without one in `folder`.
fn new_home(
    registry: &AgentRegistry,
    channel_id: &str,
    project_id: Option<&str>,
    folder: Option<PathBuf>,
) -> anyhow::Result<CoordinatorHome> {
    Ok(match (project_id, folder) {
        (Some(project_id), _) => {
            check_project(registry, project_id)?;
            CoordinatorHome::Project(project_id.to_string())
        }
        (None, Some(folder)) => CoordinatorHome::Folder(folder),
        // Unreachable: the folder is made whenever there is no project.
        (None, None) => bail!("channel {channel_id} has no folder to work in"),
    })
}

/// List the `members` in the Projects tree when the channel has a
/// project, else hide them.
fn update_hidden(hub: &AgentHub, members: &[MemberRecord], has_project: bool) {
    let mut hidden = lock(&hub.hidden_members);
    for member in members {
        if has_project {
            hidden.remove(&member.agent_id);
        } else {
            hidden.insert(member.agent_id);
        }
    }
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
