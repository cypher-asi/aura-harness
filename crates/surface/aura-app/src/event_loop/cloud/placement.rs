use super::*;

/// The footer switch, `/cloud` and `/local`.
pub(in crate::event_loop) async fn handle_set_placement(
    state: &mut LoopState<'_>,
    agent_hex: &str,
    placement: PlacementView,
) {
    let agent = state.agent_or_active(agent_hex);
    if let Some(root) = state.cloud.machine_root(agent) {
        // The switch showed a move when it was clicked; put it right.
        state.cloud.shared.publish_now();
        let note = spawned_on_machine(state, agent, root);
        system(state, agent, note).await;
        return;
    }
    if turned_down(state, agent).await {
        return;
    }
    // A move shows from the moment it's asked for, connecting included.
    let mark = MoveMark::take(&state.cloud.shared, agent);
    let placed = state.cloud.hub().is_some_and(|hub| hub.is_placed(agent));
    let console = (matches!(placement, PlacementView::Cloud) && !placed).then(|| {
        BootConsoleUi::new(
            AgentUi::new(state.commands.clone(), agent),
            "Moving to the cloud",
        )
    });
    let hub = match connect(state, console.as_ref()).await {
        Ok(hub) => hub,
        Err(err) => {
            refuse(state, console.as_ref(), agent, err).await;
            return;
        }
    };
    if let Some(refusal) = foreign_owner(state, &hub, agent) {
        refuse(state, console.as_ref(), agent, refusal).await;
        return;
    }
    match placement {
        PlacementView::Cloud => {
            let Some(console) = console else { return };
            move_to_cloud(state, hub, agent, console, mark).await;
        }
        PlacementView::Local => {
            if !hub.is_placed(agent) {
                return;
            }
            set_project_default(state, &hub, agent, Placement::Local);
            recall(state, &hub, agent, "returned to local", mark);
        }
    }
}

/// Turn down a move of `agent` while it works or is already moving.
/// Returns whether it was turned down.
async fn turned_down(state: &LoopState<'_>, agent: AgentId) -> bool {
    let busy = state.hub.is_running(agent) || state.cloud.is_running(agent);
    if !busy && !state.cloud.is_moving(agent) {
        return false;
    }
    // The switch showed a move when it was clicked; put it right.
    state.cloud.shared.publish_now();
    if busy {
        system(
            state,
            agent,
            "This agent is working; stop it or wait before moving it.",
        )
        .await;
    }
    true
}

/// Make `placement` the default of `agent`'s project, if it has one.
fn set_project_default(
    state: &LoopState<'_>,
    hub: &CloudHub,
    agent: AgentId,
    placement: Placement,
) {
    let project = lock(&state.hub.registry)
        .project_of(agent)
        .map(|p| p.id.clone());
    if let Some(project) = &project {
        hub.set_project_default(project, placement);
    }
}

/// Place `agent` on a cloud machine, showing the move on `console` and
/// holding its `mark` until the move ends.
async fn move_to_cloud(
    state: &mut LoopState<'_>,
    hub: Arc<CloudHub>,
    agent: AgentId,
    console: BootConsoleUi,
    mark: MoveMark,
) {
    if hub.is_placed(agent) {
        console
            .end(true, "This agent already runs in the cloud.".to_string())
            .await;
        return;
    }
    let Some(request) = place_request(state, &hub, agent) else {
        console
            .end(
                false,
                "This agent has no project folder to run in the cloud.".to_string(),
            )
            .await;
        return;
    };
    set_project_default(state, &hub, agent, Placement::Cloud);
    place(state, hub, request, console, (mark, Start::Move));
}

/// Place the agent `request` asks for on a cloud machine, showing the
/// move on `console` and holding its `mark` until the move ends. Work
/// sent meanwhile waits in its queue; the loop starts it once it hears
/// the move ended, in the cloud or, when the move failed, here.
pub(super) fn place(
    state: &mut LoopState<'_>,
    hub: Arc<CloudHub>,
    request: PlaceRequest,
    console: BootConsoleUi,
    (mark, start): (MoveMark, Start),
) {
    let agent = request.agent;
    let shared = Arc::clone(&state.cloud.shared);
    // The local kernel must not write for it from here on, and
    // its machine traces the conversation as a run of its own.
    state.idle.remove(&agent);
    state.hub.traces.close(agent);
    let ui = AgentUi::new(state.commands.clone(), agent);
    tokio::spawn(async move {
        let (boot, events) = BootReporter::channel();
        let shown = console.pump(events);
        let result = within(START_TIMEOUT, hub.place(request, &boot)).await;
        drop(boot);
        let _ = shown.await;
        if result.is_ok() {
            let root = PathBuf::from(aura_engine_cloud::machine_env::WORKSPACE_DIR);
            ui.send(UiCommand::SetSandboxRemote(root)).await;
        }
        let (ok, note) = match result {
            Ok(view) => (true, format!(
                "Now running in the cloud on {} ({} vCPU). Its record syncs back here; `/local` brings it back.",
                view.machine.name, view.machine.vcpu
            )),
            Err(err) => {
                if start == Start::Member {
                    shared.failed_starts.lock().insert(agent, err.clone());
                }
                (false, format!("Could not move the agent to the cloud: {err}"))
            }
        };
        info!(%agent, %note, "placement");
        console.end(ok, note).await;
        // After the console's last line, so queued work shows below it.
        drop(mark);
    });
}

/// Bring `agent` back to this machine, holding its `mark` until it is.
fn recall(
    state: &LoopState<'_>,
    hub: &Arc<CloudHub>,
    agent: AgentId,
    reason: &'static str,
    mark: MoveMark,
) {
    let shared = Arc::clone(&state.cloud.shared);
    let hub = Arc::clone(hub);
    let ui = AgentUi::new(state.commands.clone(), agent);
    let console = BootConsoleUi::new(ui.clone(), "Bringing the agent back");
    let workspace = lock(&state.hub.registry).project_folder(agent);
    tokio::spawn(async move {
        console.begin().await;
        let (boot, events) = BootReporter::channel();
        let shown = console.pump(events);
        let result = hub.recall(agent, reason, &boot).await;
        drop(boot);
        let _ = shown.await;
        let (ok, note) = match result {
            Ok(outcome) => {
                shared.stale.lock().insert(agent);
                shared.plans.lock().remove(&agent);
                if let Some(workspace) = workspace {
                    ui.send(UiCommand::SetSandboxRoot(workspace)).await;
                }
                (
                    true,
                    format!("Back on this machine: {}.", outcome.describe()),
                )
            }
            Err(err) => (
                false,
                format!("Could not bring the agent back (it keeps running in the cloud): {err}"),
            ),
        };
        console.end(ok, note).await;
        drop(mark);
    });
}

pub(super) fn place_request(
    state: &LoopState<'_>,
    hub: &CloudHub,
    agent: AgentId,
) -> Option<PlaceRequest> {
    let registry = lock(&state.hub.registry);
    let entry = registry.find(agent)?;
    let project = registry.project_of(agent)?;
    let folder = registry.project_folder(agent)?;
    let fork_from = entry
        .parent
        .as_deref()
        .map(zns_agent)
        .filter(|parent| hub.is_placed(*parent));
    Some(PlaceRequest {
        agent,
        agent_zns: entry.zns_id.clone(),
        name: entry.display_name().into_owned(),
        code: entry.code.clone(),
        project_id: project.id.clone(),
        project_dir: folder,
        build_command: None,
        fork_from,
        owner: state.auth.owner(),
        owner_name: state.auth.identity().display_name().map(str::to_string),
    })
}

/// Archiving a cloud agent pulls its work and destroys its machine.
pub(in crate::event_loop) fn on_archive(state: &LoopState<'_>, agents: &[AgentId]) {
    let Some(hub) = state.cloud.hub() else {
        return;
    };
    for agent in agents.iter().copied().filter(|agent| hub.is_placed(*agent)) {
        let mark = MoveMark::take(&state.cloud.shared, agent);
        recall(state, &hub, agent, "agent archived", mark);
    }
}

/// A HOST button or `/cloud pull|sleep|wake`.
pub(in crate::event_loop) async fn handle_host_action(
    state: &mut LoopState<'_>,
    agent_hex: &str,
    action: HostAction,
) {
    let agent = state.agent_or_active(agent_hex);
    let (Some(hub), Some(placed)) = (state.cloud.hub(), state.cloud.machine_of(agent)) else {
        system(
            state,
            agent,
            "This agent runs on this machine; `/cloud` moves it to a cloud machine.",
        )
        .await;
        return;
    };
    if let Some(refusal) = foreign_owner(state, &hub, placed) {
        system(state, agent, refusal).await;
        return;
    }
    let ui = AgentUi::new(state.commands.clone(), agent);
    tokio::spawn(async move {
        let Some(note) = host_action_note(&hub, placed, action, &ui).await else {
            return;
        };
        ui.send(UiCommand::ShowMessage(MessageData::new(
            MessageRole::System,
            note,
        )))
        .await;
    });
}

/// Run `action` on `agent`'s machine. Returns the note to show, or
/// `None` when there is none (a wake's console already says how it went).
async fn host_action_note(
    hub: &Arc<CloudHub>,
    agent: AgentId,
    action: HostAction,
    ui: &AgentUi,
) -> Option<String> {
    let note = match action {
        HostAction::Pull => match hub.pull(agent).await {
            Ok(outcome) => format!("Cloud changes: {}.", outcome.describe()),
            Err(err) => format!("Could not pull the cloud changes: {err}"),
        },
        HostAction::Sleep => match hub.sleep(agent).await {
            Ok(()) => "The cloud machine is asleep; it wakes on the next message.".to_string(),
            Err(err) => format!("Could not put the machine to sleep: {err}"),
        },
        HostAction::Wake => {
            let console = BootConsoleUi::new(ui.clone(), "Waking cloud machine");
            let (boot, events) = BootReporter::channel();
            let shown = console.pump(events);
            let result = hub.wake(agent, &boot).await;
            drop(boot);
            let _ = shown.await;
            let (ok, note) = match result {
                Ok(()) => (true, "The cloud machine is awake.".to_string()),
                Err(err) => (false, format!("Could not wake the machine: {err}")),
            };
            if console.end(ok, note.clone()).await {
                return None;
            }
            note
        }
        HostAction::Flip => return None,
    };
    Some(note)
}

/// `/cloud setup`: sign in to boxd and store a key in `AURA_HOME/.env`.
pub(in crate::event_loop) async fn handle_setup(state: &mut LoopState<'_>) {
    let agent = state.active;
    system(
        state,
        agent,
        format!(
            "Setting up cloud agents: this signs in to boxd in your browser and stores an API key in {}.",
            env_file().display()
        ),
    )
    .await;
    let ui = AgentUi::new(state.commands.clone(), agent);
    tokio::spawn(async move {
        let home = dirs::home_dir();
        let note = match crate::cloud::setup::run(home.as_deref(), &env_file()).await {
            Ok(key) => {
                format!("boxd key {key} saved. `/cloud` moves this agent to its own machine.")
            }
            Err(err) => format!("Cloud setup did not finish: {err}"),
        };
        ui.send(UiCommand::ShowMessage(MessageData::new(
            MessageRole::System,
            note,
        )))
        .await;
    });
}
