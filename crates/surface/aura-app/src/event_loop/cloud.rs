//! The event loop's side of cloud agents: the footer switch, `/cloud`,
//! the HOST tab and the turns of agents that run on boxd machines.
//!
//! The [`CloudHub`] is built on first use, so a boxd key added to `.env`
//! (or by `/cloud setup`) works without a restart. Everything slow (a
//! snapshot build, a hand-off, a turn) runs on a task of its own; the
//! loop only keeps who is moving and who is working.
//!
//! ## Invariants
//!
//! - **One writer per agent.** A moving agent takes no work: its prompts
//!   are refused until the move ends. A cloud agent's prompts go to its
//!   node, never to the local kernel, and a recalled agent's local
//!   session is reopened from the record (which now holds what the node
//!   wrote) before its next local turn.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use aura_core_types::{AgentId, Placement};
use aura_store_db::Store;
use aura_ui_terminal::events::{MessageData, MessageRole, ToolData};
use aura_ui_terminal::host::{HostAction, HostRow, HostView, PlacementState, PlacementView};
use aura_ui_terminal::UiCommand;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::agent_events::AgentUi;
use super::subagents::{lock, AgentHub};
use super::turns::Work;
use super::LoopState;
use crate::cloud::host_info;
use crate::cloud::hub::zns_agent;
use crate::cloud::{CloudHub, PlaceRequest, RemoteEvent};

/// How often the HOST tab and the footer switch are refreshed.
const PUBLISH_EVERY: Duration = Duration::from_secs(3);

/// Shared between the loop and its tasks.
#[derive(Default)]
struct Shared {
    hub: Mutex<Option<Arc<CloudHub>>>,
    /// Agents being moved.
    moving: Mutex<HashSet<AgentId>>,
    /// Cloud agents with a turn running, and how to stop it.
    running: Mutex<std::collections::HashMap<AgentId, CancellationToken>>,
    /// Agents whose local session is older than their record.
    stale: Mutex<HashSet<AgentId>>,
}

/// The loop's cloud state.
#[derive(Default)]
pub(crate) struct CloudState {
    store: Option<Arc<dyn Store>>,
    shared: Arc<Shared>,
    publisher: Option<tokio::task::JoinHandle<()>>,
}

impl CloudState {
    /// Cloud state over the main `store`.
    pub(super) fn new(store: Option<Arc<dyn Store>>) -> Self {
        Self {
            store,
            shared: Arc::default(),
            publisher: None,
        }
    }

    /// Whether `agent` runs on a cloud machine.
    pub(super) fn is_cloud(&self, agent: AgentId) -> bool {
        self.shared
            .hub
            .lock()
            .as_ref()
            .is_some_and(|hub| hub.is_placed(agent))
    }

    /// Whether `agent` is being moved.
    pub(super) fn is_moving(&self, agent: AgentId) -> bool {
        self.shared.moving.lock().contains(&agent)
    }

    /// Forget `agent`'s local session if it is older than its record.
    pub(super) fn take_stale(&self, agent: AgentId) -> bool {
        self.shared.stale.lock().remove(&agent)
    }

    /// Stop `agent`'s cloud turn. Returns whether one was running.
    pub(super) fn cancel(&self, agent: AgentId) -> bool {
        self.shared
            .running
            .lock()
            .get(&agent)
            .map(CancellationToken::cancel)
            .is_some()
    }

    fn hub(&self) -> Option<Arc<CloudHub>> {
        self.shared.hub.lock().clone()
    }
}

fn cloud_home() -> PathBuf {
    aura_config::AuraHome::resolve()
        .map(|home| home.path)
        .unwrap_or_else(|_| PathBuf::from("aura_data"))
        .join("cloud")
}

fn env_file() -> PathBuf {
    aura_config::AuraHome::resolve()
        .map(|home| home.path)
        .unwrap_or_else(|_| PathBuf::from("aura_data"))
        .join(".env")
}

async fn system(state: &LoopState<'_>, agent: AgentId, text: impl Into<String>) {
    AgentUi::new(state.commands.clone(), agent)
        .send(UiCommand::ShowMessage(MessageData::new(
            MessageRole::System,
            text,
        )))
        .await;
}

/// Build the hub if a boxd key is set now, attach it to the agent host
/// and start the HOST publisher.
async fn ensure_hub(state: &mut LoopState<'_>) -> Result<Arc<CloudHub>, String> {
    if let Some(hub) = state.cloud.hub() {
        return Ok(hub);
    }
    let config = tokio::task::spawn_blocking(aura_config::CloudConfig::reload)
        .await
        .map_err(|err| err.to_string())?
        .map_err(|err| err.to_string())?;
    if !config.is_configured() {
        return Err(aura_config::MISSING_CREDENTIAL_NOTICE.to_string());
    }
    let store = state
        .cloud
        .store
        .clone()
        .ok_or_else(|| "cloud agents need the main store".to_string())?;
    let hub = CloudHub::from_config(config, store, cloud_home()).map_err(|err| err.to_string())?;
    if let Some(agents) = &state.hub.agents {
        hub.attach_host(agents.host());
    }
    let keep: HashSet<String> = lock(&state.hub.registry)
        .agents()
        .iter()
        .filter(|agent| !agent.archived && !agent.removed)
        .map(|agent| agent.agent_id().to_hex())
        .collect();
    hub.start(&keep).await;
    *state.cloud.shared.hub.lock() = Some(Arc::clone(&hub));
    start_publisher(state);
    Ok(hub)
}

/// Bring the hub up at start when a key is set, so placed agents keep
/// running in the cloud; otherwise just publish local host facts.
pub(super) async fn start(state: &mut LoopState<'_>) {
    let configured = aura_config::cloud().is_configured();
    if configured {
        if let Err(err) = ensure_hub(state).await {
            warn!(error = %err, "cloud agents unavailable");
        }
    }
    if state.cloud.publisher.is_none() {
        start_publisher(state);
    }
}

fn start_publisher(state: &mut LoopState<'_>) {
    if let Some(old) = state.cloud.publisher.take() {
        old.abort();
    }
    let shared = Arc::clone(&state.cloud.shared);
    let hub = Arc::clone(&state.hub);
    let commands = state.commands.clone();
    state.cloud.publisher = Some(tokio::spawn(async move {
        let mut sampler: Option<aura_engine_cloud::host::HostSampler> = None;
        let mut interval = tokio::time::interval(PUBLISH_EVERY);
        loop {
            interval.tick().await;
            let Some(agent) = hub.record_panel.get() else {
                continue;
            };
            let cloud = shared.hub.lock().clone();
            let view = build_view(&hub, cloud.as_ref(), &shared, agent, &mut sampler).await;
            let placement = placement_state(cloud.as_ref(), &shared, agent);
            let _ = commands
                .send(UiCommand::SetPlacement {
                    agent_id: agent.to_hex(),
                    state: placement,
                })
                .await;
            if let Some(view) = view {
                let _ = commands.send(UiCommand::SetHost(Box::new(view))).await;
            }
        }
    }));
}

fn placement_state(
    cloud: Option<&Arc<CloudHub>>,
    shared: &Shared,
    agent: AgentId,
) -> PlacementState {
    let moving = shared.moving.lock().contains(&agent);
    let placement = cloud.map_or(Placement::Local, |hub| hub.placement_of(agent));
    PlacementState {
        placement: match placement {
            Placement::Local => PlacementView::Local,
            Placement::Cloud => PlacementView::Cloud,
        },
        moving: moving.then(|| match placement {
            Placement::Local => "moving to cloud".to_string(),
            Placement::Cloud => "bringing back".to_string(),
        }),
        machine_state: cloud
            .and_then(|hub| hub.fleet().view(&agent.to_hex()))
            .map(|view| view.state.label().to_string()),
        cloud_ready: cloud.is_some() || aura_config::cloud().is_configured(),
    }
}

async fn build_view(
    hub: &Arc<AgentHub>,
    cloud: Option<&Arc<CloudHub>>,
    shared: &Shared,
    agent: AgentId,
    sampler: &mut Option<aura_engine_cloud::host::HostSampler>,
) -> Option<HostView> {
    let (project_agents, folder) = {
        let registry = lock(&hub.registry);
        let project = registry.project_of(agent).map(|p| p.id.clone());
        let folder = registry.project_folder(agent);
        let agents: Vec<(AgentId, String)> = registry
            .agents()
            .iter()
            .filter(|a| !a.archived && !a.removed && a.project_id == project)
            .map(|a| (a.agent_id(), a.display_name().into_owned()))
            .collect();
        (agents, folder)
    };
    let mut view = match cloud.and_then(|c| c.fleet().view(&agent.to_hex()).map(|v| (c, v))) {
        Some((cloud, machine)) => host_info::cloud_view(
            agent,
            &machine,
            cloud.host_stats(agent).as_ref(),
            cloud.cost(agent).as_ref(),
            &cloud.ledger_rows(agent, 20),
        ),
        None => {
            let disk = folder.unwrap_or_else(|| PathBuf::from("."));
            let mut taken = sampler
                .take()
                .unwrap_or_else(|| aura_engine_cloud::host::HostSampler::new(disk, None));
            let (back, stats) = tokio::task::spawn_blocking(move || {
                let stats = taken.sample();
                (taken, stats)
            })
            .await
            .ok()?;
            *sampler = Some(back);
            let ready = cloud.is_some() || aura_config::cloud().is_configured();
            let key_file = aura_config::env_file::report()
                .and_then(|r| r.loaded_files().last().map(|p| p.display().to_string()));
            host_info::local_view(agent, &stats, ready, key_file.as_deref())
        }
    };
    let mut costs = Vec::new();
    view.project = project_agents
        .iter()
        .map(
            |(id, name)| match cloud.and_then(|c| c.fleet().view(&id.to_hex()).map(|v| (c, v))) {
                Some((cloud, machine)) => {
                    let cost = cloud.cost(*id);
                    if let Some(cost) = &cost {
                        costs.push(cost.clone());
                    }
                    host_info::cloud_row(*id, name, &machine, cost.as_ref())
                }
                None => host_info::local_row(
                    *id,
                    name,
                    hub.is_running(*id) || shared.running.lock().contains_key(id),
                ),
            },
        )
        .collect::<Vec<HostRow>>();
    view.project_total = host_info::project_total(&costs);
    Some(view)
}

/// The footer switch, `/cloud` and `/local`.
pub(super) async fn handle_set_placement(
    state: &mut LoopState<'_>,
    agent_hex: &str,
    placement: PlacementView,
) {
    let agent = state.agent_or_active(agent_hex);
    if state.hub.is_running(agent) || state.cloud.shared.running.lock().contains_key(&agent) {
        system(
            state,
            agent,
            "This agent is working; stop it or wait before moving it.",
        )
        .await;
        return;
    }
    if state.cloud.is_moving(agent) {
        return;
    }
    let hub = match ensure_hub(state).await {
        Ok(hub) => hub,
        Err(err) => {
            system(state, agent, err).await;
            return;
        }
    };
    let project = lock(&state.hub.registry)
        .project_of(agent)
        .map(|p| p.id.clone());
    match placement {
        PlacementView::Cloud => {
            if hub.is_placed(agent) {
                return;
            }
            let Some(request) = place_request(state, &hub, agent) else {
                system(
                    state,
                    agent,
                    "This agent has no project folder to run in the cloud.",
                )
                .await;
                return;
            };
            if let Some(project) = &project {
                hub.set_project_default(project, Placement::Cloud);
            }
            state.cloud.shared.moving.lock().insert(agent);
            // The local kernel must not write for it from here on.
            state.idle.remove(&agent);
            system(state, agent, "Moving this agent to its own cloud machine. The first move of a project builds its snapshot, which can take a while.").await;
            let shared = Arc::clone(&state.cloud.shared);
            let ui = AgentUi::new(state.commands.clone(), agent);
            tokio::spawn(async move {
                let result = hub.place(request).await;
                shared.moving.lock().remove(&agent);
                let note = match result {
                    Ok(view) => format!(
                        "Now running in the cloud on {} ({} vCPU). Its record syncs back here; `/local` brings it back.",
                        view.machine.name, view.machine.vcpu
                    ),
                    Err(err) => format!("Could not move the agent to the cloud: {err}"),
                };
                info!(%agent, %note, "placement");
                ui.send(UiCommand::ShowMessage(MessageData::new(
                    MessageRole::System,
                    note,
                )))
                .await;
            });
        }
        PlacementView::Local => {
            if !hub.is_placed(agent) {
                return;
            }
            if let Some(project) = &project {
                hub.set_project_default(project, Placement::Local);
            }
            state.cloud.shared.moving.lock().insert(agent);
            system(
                state,
                agent,
                "Bringing this agent back: copying its record and pulling its changes.",
            )
            .await;
            recall(state, &hub, agent, "returned to local");
        }
    }
}

fn recall(state: &LoopState<'_>, hub: &Arc<CloudHub>, agent: AgentId, reason: &'static str) {
    let shared = Arc::clone(&state.cloud.shared);
    let hub = Arc::clone(hub);
    let ui = AgentUi::new(state.commands.clone(), agent);
    shared.moving.lock().insert(agent);
    tokio::spawn(async move {
        let result = hub.recall(agent, reason).await;
        shared.moving.lock().remove(&agent);
        let note = match result {
            Ok(outcome) => {
                shared.stale.lock().insert(agent);
                format!("Back on this machine: {}.", outcome.describe())
            }
            Err(err) => {
                format!("Could not bring the agent back (it keeps running in the cloud): {err}")
            }
        };
        ui.send(UiCommand::ShowMessage(MessageData::new(
            MessageRole::System,
            note,
        )))
        .await;
    });
}

fn place_request(state: &LoopState<'_>, hub: &CloudHub, agent: AgentId) -> Option<PlaceRequest> {
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
    })
}

/// Archiving a cloud agent pulls its work and destroys its machine.
pub(super) fn on_archive(state: &LoopState<'_>, agents: &[AgentId]) {
    let Some(hub) = state.cloud.hub() else {
        return;
    };
    for agent in agents.iter().copied().filter(|agent| hub.is_placed(*agent)) {
        recall(state, &hub, agent, "agent archived");
    }
}

/// A HOST button or `/cloud pull|sleep|wake`.
pub(super) async fn handle_host_action(
    state: &mut LoopState<'_>,
    agent_hex: &str,
    action: HostAction,
) {
    let agent = state.agent_or_active(agent_hex);
    let Some(hub) = state.cloud.hub().filter(|hub| hub.is_placed(agent)) else {
        system(
            state,
            agent,
            "This agent runs on this machine; `/cloud` moves it to a cloud machine.",
        )
        .await;
        return;
    };
    let ui = AgentUi::new(state.commands.clone(), agent);
    tokio::spawn(async move {
        let note = match action {
            HostAction::Pull => match hub.pull(agent).await {
                Ok(outcome) => format!("Cloud changes: {}.", outcome.describe()),
                Err(err) => format!("Could not pull the cloud changes: {err}"),
            },
            HostAction::Sleep => match hub.sleep(agent).await {
                Ok(()) => "The cloud machine is asleep; it wakes on the next message.".to_string(),
                Err(err) => format!("Could not put the machine to sleep: {err}"),
            },
            HostAction::Wake => match hub.wake(agent).await {
                Ok(()) => "The cloud machine is awake.".to_string(),
                Err(err) => format!("Could not wake the machine: {err}"),
            },
            HostAction::Flip => return,
        };
        ui.send(UiCommand::ShowMessage(MessageData::new(
            MessageRole::System,
            note,
        )))
        .await;
    });
}

/// `/cloud setup`: sign in to boxd and store a key in `AURA_HOME/.env`.
pub(super) async fn handle_setup(state: &mut LoopState<'_>) {
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

/// Prompts and commands for a cloud agent, or for one being moved.
/// Returns `false` when `work` is for the local kernel after all.
pub(super) async fn submit(state: &mut LoopState<'_>, agent: AgentId, work: Work) -> Option<Work> {
    if state.cloud.is_moving(agent) {
        system(
            state,
            agent,
            "This agent is moving between machines; send it again when the move is done.",
        )
        .await;
        return None;
    }
    if state.cloud.take_stale(agent) {
        state.idle.remove(&agent);
    }
    if !state.cloud.is_cloud(agent) {
        return Some(work);
    }
    let hub = state.cloud.hub()?;
    match work {
        Work::Prompt { prompt, shown } => {
            if state.cloud.shared.running.lock().contains_key(&agent) {
                system(
                    state,
                    agent,
                    "The cloud agent is still working; wait or stop it first.",
                )
                .await;
                return None;
            }
            let ui = AgentUi::new(state.commands.clone(), agent);
            if !shown {
                ui.send(UiCommand::ShowMessage(MessageData::new(
                    MessageRole::User,
                    prompt.text.clone(),
                )))
                .await;
            }
            if !prompt.images.is_empty() {
                system(
                    state,
                    agent,
                    "Images stay on this machine; the cloud agent gets the text.",
                )
                .await;
            }
            let current = state.models.current().to_string();
            let model =
                (!current.is_empty() && current != "gate" && !current.starts_with("local/"))
                    .then_some(current);
            let jwt = match state.auth.identity() {
                aura_auth::Identity::Zos(session) => Some(session.access_token),
                aura_auth::Identity::Local => None,
            };
            let cancel = CancellationToken::new();
            state
                .cloud
                .shared
                .running
                .lock()
                .insert(agent, cancel.clone());
            let rx = hub.run_turn(agent, prompt.text, model, jwt, cancel);
            let shared = Arc::clone(&state.cloud.shared);
            tokio::spawn(async move {
                forward(ui, rx).await;
                shared.running.lock().remove(&agent);
            });
        }
        Work::Command { command, .. } => {
            let ui = AgentUi::new(state.commands.clone(), agent);
            tokio::spawn(async move {
                let note = match hub.exec(agent, &command).await {
                    Ok(out) => format!(
                        "$ {command}  (cloud, exit {})\n{}{}",
                        out.exit_code,
                        out.stdout_text(),
                        out.stderr_tail(4000)
                    ),
                    Err(err) => format!("$ {command}: {err}"),
                };
                ui.send(UiCommand::ShowMessage(MessageData::new(
                    MessageRole::System,
                    note,
                )))
                .await;
            });
        }
        Work::Compact { .. } | Work::Build { .. } => {
            system(state, agent, "Cloud agents summarize and build plans on their own machine; send it as a message instead.").await;
        }
    }
    None
}

/// Show a cloud turn's events in its agent's chat.
async fn forward(ui: AgentUi, mut rx: tokio::sync::mpsc::Receiver<RemoteEvent>) {
    let mut streaming = false;
    let mut thinking = false;
    while let Some(event) = rx.recv().await {
        if thinking && !matches!(event, RemoteEvent::Thinking(_)) {
            ui.send(UiCommand::FinishThinking).await;
            thinking = false;
        }
        if streaming && !matches!(event, RemoteEvent::Text(_)) {
            ui.send(UiCommand::FinishStreaming).await;
            streaming = false;
        }
        match event {
            RemoteEvent::Status(status) => ui.send(UiCommand::SetStatus(status)).await,
            RemoteEvent::Text(text) => {
                if !streaming {
                    ui.send(UiCommand::StartStreaming).await;
                    streaming = true;
                }
                ui.send(UiCommand::AppendText(text)).await;
            }
            RemoteEvent::Thinking(text) => {
                if !thinking {
                    ui.send(UiCommand::StartThinking).await;
                    thinking = true;
                }
                ui.send(UiCommand::AppendThinking(text)).await;
            }
            RemoteEvent::ToolStart { id, name } => {
                ui.send(UiCommand::ShowTool(ToolData {
                    id,
                    name,
                    args: String::new(),
                }))
                .await;
            }
            RemoteEvent::ToolInput { id, input } => {
                ui.send(UiCommand::UpdateToolArgs { id, args: input }).await;
            }
            RemoteEvent::ToolResult {
                id,
                result,
                is_error,
            } => {
                ui.send(UiCommand::CompleteTool {
                    id,
                    result,
                    success: !is_error,
                })
                .await;
            }
            RemoteEvent::Done { .. } => break,
            RemoteEvent::Failed(err) => {
                ui.send(UiCommand::ShowError(format!("cloud turn failed: {err}")))
                    .await;
                break;
            }
        }
    }
    if thinking {
        ui.send(UiCommand::FinishThinking).await;
    }
    if streaming {
        ui.send(UiCommand::FinishStreaming).await;
    }
    ui.send(UiCommand::SetStatus("Ready".to_string())).await;
}

impl Drop for CloudState {
    fn drop(&mut self) {
        if let Some(publisher) = self.publisher.take() {
            publisher.abort();
        }
        if let Some(hub) = self.shared.hub.lock().take() {
            hub.shutdown();
        }
    }
}
