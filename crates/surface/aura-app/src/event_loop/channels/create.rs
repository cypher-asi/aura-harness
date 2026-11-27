use super::*;

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
    ensure_free_slug(&store, &slug).await?;
    let home = coordinator_home(hub, new.project_id.as_deref(), &slug).await?;
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
        let draft = registry.coordinator_draft(home);
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
    let coordinator = register_coordinator(hub, draft, &channel);
    info!(
        channel_id = %channel.id,
        slug = %channel.slug,
        project_id = channel.project_id.as_deref().unwrap_or(""),
        coordinator = %coordinator.agent_id().to_hex(),
        "Created channel"
    );
    Ok(channel)
}

/// Fail if a channel named `slug` exists already.
async fn ensure_free_slug(store: &ChannelStore, slug: &str) -> anyhow::Result<()> {
    if blocking(store, {
        let slug = slug.to_string();
        move |store| store.by_slug(&slug)
    })
    .await
    .with_context(|| format!("checking whether #{slug} exists"))?
    .is_some()
    {
        bail!("a channel named #{slug} already exists");
    }
    Ok(())
}

/// Where the coordinator of a new channel `slug` works: in `project_id`,
/// or, without one, in the channel's folder, made here.
async fn coordinator_home(
    hub: &AgentHub,
    project_id: Option<&str>,
    slug: &str,
) -> anyhow::Result<CoordinatorHome> {
    Ok(match project_id {
        Some(project_id) => CoordinatorHome::Project(project_id.to_string()),
        None => {
            let folder = channel_folder(lock(&hub.registry).data_dir(), slug);
            tokio::fs::create_dir_all(&folder)
                .await
                .with_context(|| format!("creating channel folder {}", folder.display()))?;
            CoordinatorHome::Folder(folder)
        }
    })
}

/// Register `draft`, the coordinator of the just created `channel`, and
/// add it to the hub's member sets.
fn register_coordinator(
    hub: &AgentHub,
    draft: RegisteredAgent,
    channel: &ChannelRecord,
) -> RegisteredAgent {
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
    coordinator
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
