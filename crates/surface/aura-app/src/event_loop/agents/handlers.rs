//! UI commands on agents, projects and permanent agent definitions.

use super::*;

pub(crate) async fn handle_select_agent(state: &mut LoopState<'_>, agent_hex: &str) {
    match AgentId::from_hex(agent_hex) {
        Ok(agent_id) => activate_agent(state, agent_id, Chat::Reload).await,
        Err(e) => {
            warn!(agent = agent_hex, error = %e, "Invalid agent id from UI");
            show_error(state, format!("Invalid agent id: {e}")).await;
        }
    }
}

/// Make the agent with hex id `agent_hex` the active one for a view that
/// already shows its chat.
pub(crate) async fn handle_focus_agent(state: &mut LoopState<'_>, agent_hex: &str) {
    match AgentId::from_hex(agent_hex) {
        Ok(agent_id) => activate_agent(state, agent_id, Chat::Keep).await,
        Err(e) => warn!(agent = agent_hex, error = %e, "Invalid agent id from UI"),
    }
}

/// What the UI needs of an agent's chat when the agent becomes active.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Chat {
    /// Rebuild it from the record, unless the agent is at work and the
    /// chat is live.
    Reload,
    /// The view asking already has it; rebuilding would lose its scroll
    /// position and what it has expanded.
    Keep,
}

/// Open `folder` as a project: a new one, the one it already is, or the
/// one it was before it was removed. With `switch`, the screen moves to
/// its agent; otherwise the lists refresh and the screen stays where it
/// is, so the channel form can select the project.
pub(crate) async fn handle_create_project(state: &mut LoopState<'_>, folder: &Path, switch: bool) {
    let opened = {
        let mut registry = lock(&state.hub.registry);
        registry.open_project(folder).map(|(agent, opened)| {
            let project = registry.project_of(agent.agent_id()).cloned();
            (agent, opened, project)
        })
    };
    let (agent, opened, project) = match opened {
        Ok((agent, opened, Some(project))) => (agent, opened, project),
        Ok((_, _, None)) => {
            show_error(state, "Could not open the project".to_string()).await;
            return;
        }
        Err(e) => {
            show_error(state, format!("Could not open project: {e}")).await;
            return;
        }
    };
    let name = project.name();
    let path = project.folder.display().to_string();
    info!(zns_id = %agent.zns_id, project = %name, folder = %path, ?opened, "Opened project");
    if switch {
        activate_agent(state, agent.agent_id(), Chat::Reload).await;
    } else {
        state.hub.refresh_agent_list().await;
    }
    let notice = match opened {
        Opened::Created => UiCommand::ShowSuccess(format!("Created project {name} in {path}")),
        Opened::Existing => UiCommand::ShowWarning(format!("{path} is already project {name}")),
        Opened::Revived => UiCommand::ShowSuccess(format!("Reopened project {name}")),
    };
    let _ = state.commands.send(notice).await;
}

/// Save the order projects were dragged into.
pub(crate) async fn handle_reorder_projects(state: &mut LoopState<'_>, order: &[String]) {
    let saved = lock(&state.hub.registry).reorder_projects(order);
    if let Err(e) = saved {
        show_error(state, format!("Could not save the order of projects: {e}")).await;
    }
    state.hub.send_agent_list().await;
}

/// Save the order a project's agents were dragged into.
pub(crate) async fn handle_reorder_agents(
    state: &mut LoopState<'_>,
    project_id: &str,
    order: &[String],
) {
    let saved = lock(&state.hub.registry).reorder_agents(project_id, order);
    if let Err(e) = saved {
        show_error(state, format!("Could not save the order of agents: {e}")).await;
    }
    state.hub.send_agent_list().await;
}

pub(crate) async fn handle_create_agent(state: &mut LoopState<'_>, name: Option<String>) {
    let created = lock(&state.hub.registry).create(name);
    match created {
        Ok(agent) => {
            info!(zns_id = %agent.zns_id, name = %agent.name, "Created terminal agent");
            activate_agent(state, agent.agent_id(), Chat::Reload).await;
            show_success(state, format!("Created agent {}", agent.name)).await;
        }
        Err(e) => show_error(state, format!("Could not create agent: {e}")).await,
    }
}

/// Add a new agent to project `project_id`, and switch to it.
pub(crate) async fn handle_add_agent_to_project(state: &mut LoopState<'_>, project_id: &str) {
    let created = lock(&state.hub.registry).create_in_project(project_id);
    match created {
        Ok(agent) => {
            info!(zns_id = %agent.zns_id, name = %agent.name, "Added agent to project");
            activate_agent(state, agent.agent_id(), Chat::Reload).await;
            show_success(state, format!("Created agent {}", agent.name)).await;
        }
        Err(e) => show_error(state, format!("Could not create agent: {e}")).await,
    }
}

/// Parse a hex agent id from the UI, reporting a bad one.
pub(super) async fn parse_agent(state: &LoopState<'_>, agent_hex: &str) -> Option<AgentId> {
    match AgentId::from_hex(agent_hex) {
        Ok(agent_id) => Some(agent_id),
        Err(e) => {
            warn!(agent = agent_hex, error = %e, "Invalid agent id from UI");
            show_error(state, format!("Invalid agent id: {e}")).await;
            None
        }
    }
}

/// Name of the first agent in `agents` with work in flight.
pub(super) fn busy_agent(state: &LoopState<'_>, agents: &[AgentId]) -> Option<String> {
    let busy = agents.iter().find(|&&id| state.hub.is_running(id))?;
    Some(
        lock(&state.hub.registry)
            .find(*busy)
            .map_or_else(|| "an agent".to_string(), |a| a.display_name().into_owned()),
    )
}

/// Stop the work of every agent in `agents` that still has work in
/// flight. Returns the display name of each one stopped, in order.
///
/// A turn waiting on an approval is denied first so it sees the
/// cancellation rather than hanging until its prompt times out.
pub(super) async fn stop_busy(state: &mut LoopState<'_>, agents: &[AgentId]) -> Vec<String> {
    let mut stopped = Vec::new();
    for &id in agents {
        if !state.hub.is_running(id) {
            continue;
        }
        state.approvals.cancel_agent(id).await;
        if state.hub.cancel(id) {
            let name = lock(&state.hub.registry)
                .find(id)
                .map_or_else(|| "an agent".to_string(), |a| a.display_name().into_owned());
            stopped.push(name);
        }
    }
    stopped
}

/// After agents left the list, show the one the registry opens now if the
/// active agent was among them; otherwise just refresh the list.
pub(super) async fn after_list_shrank(state: &mut LoopState<'_>, gone: &[AgentId]) {
    let next = gone.contains(&state.active).then(|| {
        // The lists mark the stored active agent even when it is archived,
        // so the next one is stored first for them to mark it.
        let mut registry = lock(&state.hub.registry);
        let next = registry.active().agent_id();
        if let Err(e) = registry.set_active(next) {
            warn!(agent_id = %next, error = %e, "Could not save the active agent");
        }
        next
    });
    // The agents leave the list at once; opening the next one can take a
    // while when it has a long record.
    state.hub.send_agent_list().await;
    if let Some(next) = next {
        activate_agent(state, next, Chat::Reload).await;
    }
}

/// Move an agent, and the agents under it, to the Archive.
pub(crate) async fn handle_archive_agent(state: &mut LoopState<'_>, agent_hex: &str) {
    let Some(agent_id) = parse_agent(state, agent_hex).await else {
        return;
    };
    let (subtree, name) = {
        let registry = lock(&state.hub.registry);
        (
            registry.subtree(agent_id),
            registry
                .find(agent_id)
                .map(|a| a.display_name().into_owned()),
        )
    };
    let Some(name) = name else {
        show_error(state, "Unknown agent".to_string()).await;
        return;
    };
    // Archiving is reversible, so it does not wait for work in flight to
    // finish on its own: it stops it and carries on.
    if let Some(refusal) = super::super::cloud::archive_refusal(state, &subtree) {
        show_error(state, refusal).await;
        return;
    }
    let stopped = stop_busy(state, &subtree).await;
    super::super::cloud::on_archive(state, &subtree);
    let archived = lock(&state.hub.registry).archive(agent_id);
    match archived {
        Ok(()) => {
            info!(%agent_id, %name, stopped = stopped.len(), "Archived agent");
            after_list_shrank(state, &subtree).await;
            let message = if stopped.is_empty() {
                format!("Archived {name}")
            } else {
                format!("Archived {name}; stopped {}", stopped.join(", "))
            };
            show_success(state, message).await;
        }
        Err(e) => show_error(state, format!("Could not archive {name}: {e}")).await,
    }
}

/// Bring an agent back from the Archive and switch to it.
pub(crate) async fn handle_restore_agent(state: &mut LoopState<'_>, agent_hex: &str) {
    let Some(agent_id) = parse_agent(state, agent_hex).await else {
        return;
    };
    let restored = lock(&state.hub.registry).restore(agent_id);
    match restored {
        Ok(()) => {
            let name = lock(&state.hub.registry)
                .find(agent_id)
                .map(|a| a.display_name().into_owned())
                .unwrap_or_default();
            info!(%agent_id, %name, "Restored agent");
            activate_agent(state, agent_id, Chat::Reload).await;
            show_success(state, format!("Restored {name}")).await;
        }
        Err(e) => show_error(state, format!("Could not restore agent: {e}")).await,
    }
}

/// Why the project `project_id`, named `project`, with the agents
/// `agents`, cannot be removed, if it cannot.
async fn removal_refusal(
    state: &LoopState<'_>,
    project_id: &str,
    project: &str,
    agents: &[AgentId],
) -> Option<String> {
    match super::super::channels::channel_in_project(&state.hub, project_id).await {
        Ok(None) => {}
        Ok(Some(channel)) => {
            return Some(format!(
                "Cannot remove {project}: #{} works in this project; edit the channel to change its project first",
                channel.slug
            ));
        }
        Err(e) => {
            return Some(format!("Cannot remove {project}: {e:#}"));
        }
    }
    busy_agent(state, agents)
        .map(|busy| format!("Cannot remove {project}: {busy} is still working"))
}

/// Remove project `project_id` from the list. Its records and folder stay
/// on disk.
pub(crate) async fn handle_remove_project(state: &mut LoopState<'_>, project_id: &str) {
    let found = {
        let registry = lock(&state.hub.registry);
        registry.project(project_id).map(|project| {
            let agents: Vec<AgentId> = registry
                .agents()
                .iter()
                .filter(|a| a.project_id.as_deref() == Some(project_id))
                .map(RegisteredAgent::agent_id)
                .collect();
            (agents, project.name())
        })
    };
    let Some((agents, project)) = found else {
        show_error(state, "Unknown project".to_string()).await;
        return;
    };
    if let Some(refusal) = removal_refusal(state, project_id, &project, &agents).await {
        show_error(state, refusal).await;
        return;
    }
    let removed = lock(&state.hub.registry).remove_project(project_id);
    match removed {
        Ok(removed) => {
            for id in &removed {
                state.idle.remove(id);
            }
            info!(%project, agents = removed.len(), "Removed project");
            after_list_shrank(state, &removed).await;
            show_success(state, format!("Removed project {project}")).await;
        }
        Err(e) => show_error(state, format!("Could not remove {project}: {e}")).await,
    }
}

/// Start an instance of a permanent agent and switch to it.
pub(crate) async fn handle_instantiate_definition(state: &mut LoopState<'_>, name: &str) {
    if let Some(agent) = start_instance(state, name, Placement::Alone).await {
        show_success(state, format!("Started {}", agent.name)).await;
    }
}

/// Where a new permanent-agent instance goes in the agents list.
pub(super) enum Placement {
    /// The launch directory's project.
    Alone,
    /// At the top of the project with this id.
    InProject(String),
}

/// Start an instance of a permanent agent inside the project that contains
/// the active agent, and switch to it.
pub(crate) async fn handle_add_definition_to_project(state: &mut LoopState<'_>, name: &str) {
    let project = lock(&state.hub.registry).project_of(state.active).cloned();
    let Some(project) = project else {
        show_error(state, "Unknown agent".to_string()).await;
        return;
    };
    if let Some(agent) = start_instance(state, name, Placement::InProject(project.id.clone())).await
    {
        show_success(state, format!("Added {} to {}", agent.name, project.name())).await;
    }
}

/// Why a coordinator can't be started from AGENTS.
pub(super) const COORDINATOR_START_HINT: &str =
    "Coordinators run channels: make a channel to start one";

/// Register an instance of the permanent agent `name` at `placement` and
/// switch to it. Reports failures itself and returns `None`.
pub(super) async fn start_instance(
    state: &mut LoopState<'_>,
    name: &str,
    placement: Placement,
) -> Option<RegisteredAgent> {
    let definition = lock(&state.hub.definitions)
        .get(name)
        .map(|d| (d.name.clone(), d.is_coordinator()));
    let definition = match definition {
        Some((definition, false)) => definition,
        Some((_, true)) => {
            show_error(state, COORDINATOR_START_HINT.to_string()).await;
            return None;
        }
        None => {
            show_error(state, format!("No permanent agent named {name}")).await;
            return None;
        }
    };
    let created = {
        let mut registry = lock(&state.hub.registry);
        match placement {
            Placement::Alone => registry.create_instance(&definition, None),
            Placement::InProject(project) => {
                registry.add_instance_to_project(&definition, &project)
            }
        }
    };
    match created {
        Ok(agent) => {
            info!(name = %agent.name, %definition, "Started permanent agent instance");
            activate_agent(state, agent.agent_id(), Chat::Reload).await;
            Some(agent)
        }
        Err(e) => {
            show_error(state, format!("Could not start {definition}: {e}")).await;
            None
        }
    }
}

/// Open a permanent agent's file in the editor, creating a new one when
/// `name` is `None`.
pub(crate) async fn handle_edit_definition(state: &LoopState<'_>, name: Option<String>) {
    let path = {
        let mut definitions = lock(&state.hub.definitions);
        match name {
            Some(name) => definitions
                .get(&name)
                .map(|d| d.path.clone())
                .ok_or_else(|| format!("No permanent agent named {name}")),
            None => definitions
                .create(None)
                .map_err(anyhow::Error::from)
                .map_err(|e| format!("Could not create a permanent agent: {e:#}")),
        }
    };
    match path {
        Ok(path) => {
            state.hub.send_definitions().await;
            let _ = state.commands.send(UiCommand::OpenEditor(path)).await;
        }
        Err(problem) => show_error(state, problem).await,
    }
}

pub(crate) async fn handle_delete_definition(state: &LoopState<'_>, name: &str) {
    let deleted = lock(&state.hub.definitions).delete(name);
    match deleted {
        Ok(definition) => {
            info!(name = %definition.name, "Deleted permanent agent");
            state.hub.send_definitions().await;
            show_success(
                state,
                format!("Deleted permanent agent {}", definition.name),
            )
            .await;
        }
        Err(e) => show_error(state, format!("{:#}", anyhow::Error::from(e))).await,
    }
}

/// Reload the permanent agents after the editor closed on `path`.
pub(crate) async fn handle_definition_edited(state: &LoopState<'_>, path: &Path) {
    let problems = state.hub.reload_definitions();
    state.hub.send_definitions().await;
    let file = path.display().to_string();
    if let Some(problem) = problems.iter().find(|p| p.starts_with(&file)) {
        let _ = state
            .commands
            .send(UiCommand::ShowWarning(format!("Not loaded: {problem}")))
            .await;
        return;
    }
    let name = lock(&state.hub.definitions)
        .all()
        .iter()
        .find(|d| d.path == path)
        .map(|d| d.name.clone());
    if let Some(name) = name {
        show_success(state, format!("Saved permanent agent {name}")).await;
    }
}
