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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aura_core_types::{AgentId, Placement};
use aura_engine_cloud::{BootEvent, BootReporter, StageState};
use aura_store_db::Store;
use aura_ui_terminal::events::{
    BootLogLine, BootPlanStep, BootStageState, BootUpdate, MessageData, MessageRole, ToolData,
};
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

/// Numbers boot consoles, so each move or wake gets one of its own.
static NEXT_BOOT: AtomicU64 = AtomicU64::new(1);

/// A boot console in an agent's chat: what a move or a wake shows while
/// it happens. It appears on [`Self::begin`] or with the first event.
#[derive(Clone)]
struct BootConsoleUi {
    ui: AgentUi,
    id: String,
    title: String,
    begun: Arc<AtomicBool>,
}

impl BootConsoleUi {
    fn new(ui: AgentUi, title: &str) -> Self {
        Self {
            ui,
            id: format!("boot-{}", NEXT_BOOT.fetch_add(1, Ordering::Relaxed)),
            title: title.to_string(),
            begun: Arc::default(),
        }
    }

    async fn send(&self, update: BootUpdate) {
        self.ui
            .send(UiCommand::Boot {
                id: self.id.clone(),
                update,
            })
            .await;
    }

    async fn stage(&self, key: &str, label: &str, state: BootStageState) {
        self.begin().await;
        self.send(BootUpdate::Stage {
            key: key.to_string(),
            label: label.to_string(),
            state,
        })
        .await;
    }

    /// Show the console now.
    async fn begin(&self) {
        if !self.begun.swap(true, Ordering::Relaxed) {
            self.send(BootUpdate::Begin {
                title: self.title.clone(),
            })
            .await;
        }
    }

    async fn show(&self, events: Vec<BootEvent>) {
        self.begin().await;
        for update in boot_updates(events) {
            self.send(update).await;
        }
    }

    /// Show everything sent on `events` until its reporter is dropped.
    fn pump(
        &self,
        mut events: tokio::sync::mpsc::UnboundedReceiver<BootEvent>,
    ) -> tokio::task::JoinHandle<()> {
        let console = self.clone();
        tokio::spawn(async move {
            let mut batch = Vec::new();
            while events.recv_many(&mut batch, 256).await > 0 {
                console.show(std::mem::take(&mut batch)).await;
            }
        })
    }

    /// End the console with `summary`. Returns `false` when it never
    /// showed, so the caller says how it went another way.
    async fn end(&self, ok: bool, summary: String) -> bool {
        if !self.begun.load(Ordering::Relaxed) {
            return false;
        }
        self.send(BootUpdate::End { ok, summary }).await;
        true
    }
}

/// `events` as console updates, runs of log lines sent as one.
fn boot_updates(events: Vec<BootEvent>) -> Vec<BootUpdate> {
    let mut updates = Vec::new();
    for event in events {
        let update = match event {
            BootEvent::Plan(stages) => BootUpdate::Plan(
                stages
                    .into_iter()
                    .map(|stage| BootPlanStep {
                        key: stage.key().to_string(),
                        label: stage.label().to_string(),
                    })
                    .collect(),
            ),
            BootEvent::Stage { stage, state } => BootUpdate::Stage {
                key: stage.key().to_string(),
                label: stage.label().to_string(),
                state: match state {
                    StageState::Started => BootStageState::Running,
                    StageState::Done { ms } => BootStageState::Done { ms },
                    StageState::Skipped(reason) => BootStageState::Skipped(reason),
                    StageState::Failed(reason) => BootStageState::Failed(reason),
                },
            },
            BootEvent::Fact { key, value } => BootUpdate::Fact { key, value },
            BootEvent::Log { stderr, line, .. } => {
                let line = BootLogLine { stderr, text: line };
                if let Some(BootUpdate::Log(lines)) = updates.last_mut() {
                    lines.push(line);
                    continue;
                }
                BootUpdate::Log(vec![line])
            }
            BootEvent::Progress { stage, done, total } => BootUpdate::Progress {
                key: stage.key().to_string(),
                done,
                total,
            },
        };
        updates.push(update);
    }
    updates
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
    hub.set_current_owner(state.auth.owner());
    hub.set_plan_credentials(Arc::new(crate::cloud::plans::VaultPlans(Arc::clone(
        &state.auth,
    ))));
    hub.start(&keep).await;
    *state.cloud.shared.hub.lock() = Some(Arc::clone(&hub));
    start_publisher(state);
    Ok(hub)
}

/// The hub, started if need be. With a `console`, it shows at once, the
/// start as its first stage.
async fn connect(
    state: &mut LoopState<'_>,
    console: Option<&BootConsoleUi>,
) -> Result<Arc<CloudHub>, String> {
    let Some(console) = console else {
        return ensure_hub(state).await;
    };
    console.begin().await;
    if let Some(hub) = state.cloud.hub() {
        return Ok(hub);
    }
    let (key, label) = ("connect", "Connecting to boxd");
    console.stage(key, label, BootStageState::Running).await;
    let started = std::time::Instant::now();
    let hub = ensure_hub(state).await;
    let done = match &hub {
        Ok(_) => BootStageState::Done {
            ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        },
        Err(err) => BootStageState::Failed(err.clone()),
    };
    console.stage(key, label, done).await;
    hub
}

/// Say why a move or a return can't go ahead: on its console, if shown.
async fn refuse(
    state: &LoopState<'_>,
    console: Option<&BootConsoleUi>,
    agent: AgentId,
    note: String,
) {
    match console {
        Some(console) => {
            console.end(false, note).await;
        }
        None => system(state, agent, note).await,
    }
}

/// Why the user signed in here may not manage cloud agent `agent`: only
/// the user who placed it can move, pull, sleep, wake, archive or talk
/// to it.
fn foreign_owner(state: &LoopState<'_>, hub: &CloudHub, agent: AgentId) -> Option<String> {
    let owner = hub.owner_of(agent)?;
    refusal_for(&owner, hub.owner_name_of(agent), &state.auth.owner())
}

/// The notice when `current` is not `owner` (named `owner_name`).
fn refusal_for(
    owner: &aura_core_auth::Owner,
    owner_name: Option<String>,
    current: &aura_core_auth::Owner,
) -> Option<String> {
    if owner == current {
        return None;
    }
    let name = owner_name.unwrap_or_else(|| match owner {
        aura_core_auth::Owner::Local => "this machine's local profile".to_string(),
        aura_core_auth::Owner::Zos { .. } => "another user".to_string(),
    });
    Some(format!("Placed by {name}; sign in as {name} to manage it."))
}

/// Why `agents` may not be archived by the user signed in here: one of
/// them is another user's cloud agent.
pub(super) fn archive_refusal(state: &LoopState<'_>, agents: &[AgentId]) -> Option<String> {
    let hub = state.cloud.hub()?;
    agents
        .iter()
        .find_map(|agent| foreign_owner(state, &hub, *agent))
}

/// The plan a model id is served by (`claude/...`, `chatgpt/...`,
/// `openrouter/...`), if any.
fn plan_of_model(model: &str) -> Option<aura_core_auth::ProviderKind> {
    let (prefix, _) = model.split_once('/')?;
    prefix
        .parse::<aura_core_auth::ProviderKind>()
        .ok()
        .filter(|kind| aura_engine_cloud::plans::plan_host(*kind).is_some())
}

/// The user signed in here or their plans changed: local agents act as
/// them, and their plan tokens go to their machines now.
pub(super) fn on_auth_change(state: &LoopState<'_>) {
    let Some(hub) = state.cloud.hub() else {
        return;
    };
    let owner = state.auth.owner();
    hub.set_current_owner(owner.clone());
    tokio::spawn(async move { hub.sync_plans(&owner).await });
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
    let auth = Arc::clone(&state.auth);
    state.cloud.publisher = Some(tokio::spawn(async move {
        let mut sampler: Option<aura_engine_cloud::host::HostSampler> = None;
        let mut interval = tokio::time::interval(PUBLISH_EVERY);
        loop {
            interval.tick().await;
            let Some(agent) = hub.record_panel.get() else {
                continue;
            };
            let cloud = shared.hub.lock().clone();
            let current = auth.owner();
            let view =
                build_view(&hub, cloud.as_ref(), &shared, agent, &current, &mut sampler).await;
            // Every placed agent, for the projects list's cloud marks, and
            // the agent on screen, for its footer switch.
            let mut agents: Vec<AgentId> = cloud
                .as_ref()
                .map(|c| {
                    c.placed()
                        .iter()
                        .filter_map(|(hex, _)| AgentId::from_hex(hex).ok())
                        .collect()
                })
                .unwrap_or_default();
            if !agents.contains(&agent) {
                agents.push(agent);
            }
            for id in agents {
                let placement = placement_state(cloud.as_ref(), &shared, id, &current);
                let _ = commands
                    .send(UiCommand::SetPlacement {
                        agent_id: id.to_hex(),
                        state: placement,
                    })
                    .await;
            }
            if let Some(view) = view {
                let _ = commands.send(UiCommand::SetHost(Box::new(view))).await;
            }
        }
    }));
}

/// How the user who placed `agent` is named here.
fn owner_label(hub: &CloudHub, agent: AgentId) -> String {
    hub.owner_name_of(agent)
        .unwrap_or_else(|| match hub.owner_of(agent) {
            Some(aura_core_auth::Owner::Zos { .. }) => "another user".to_string(),
            _ => "local profile".to_string(),
        })
}

/// The owner's name when `agent` was placed by someone other than
/// `current`.
fn foreign_label(
    cloud: Option<&Arc<CloudHub>>,
    agent: AgentId,
    current: &aura_core_auth::Owner,
) -> Option<String> {
    let hub = cloud?;
    let owner = hub.owner_of(agent)?;
    (owner != *current).then(|| owner_label(hub, agent))
}

fn placement_state(
    cloud: Option<&Arc<CloudHub>>,
    shared: &Shared,
    agent: AgentId,
    current: &aura_core_auth::Owner,
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
        owner: foreign_label(cloud, agent, current),
    }
}

async fn build_view(
    hub: &Arc<AgentHub>,
    cloud: Option<&Arc<CloudHub>>,
    shared: &Shared,
    agent: AgentId,
    current: &aura_core_auth::Owner,
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
        Some((cloud, machine)) => {
            let mut view = host_info::cloud_view(
                agent,
                &machine,
                cloud.host_stats(agent).as_ref(),
                cloud.cost(agent).as_ref(),
                &cloud.ledger_rows(agent, 20),
            );
            let owner = cloud
                .owner_of(agent)
                .unwrap_or(aura_core_auth::Owner::Local);
            let now = u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0);
            host_info::add_owner_and_models(
                &mut view,
                &owner_label(cloud, agent),
                &cloud.plan_status(&owner),
                now,
            );
            view
        }
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
                        costs.push((owner_label(cloud, *id), cost.clone()));
                    }
                    host_info::cloud_row(
                        *id,
                        name,
                        &machine,
                        cost.as_ref(),
                        foreign_label(Some(cloud), *id, current),
                    )
                }
                None => host_info::local_row(
                    *id,
                    name,
                    hub.is_running(*id) || shared.running.lock().contains_key(id),
                ),
            },
        )
        .collect::<Vec<HostRow>>();
    view.project_total = host_info::project_total_by_owner(&costs);
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
    // A move shows from the moment it's asked for, connecting included.
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
    let project = lock(&state.hub.registry)
        .project_of(agent)
        .map(|p| p.id.clone());
    if let Some(refusal) = foreign_owner(state, &hub, agent) {
        refuse(state, console.as_ref(), agent, refusal).await;
        return;
    }
    match placement {
        PlacementView::Cloud => {
            let Some(console) = console else { return };
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
            if let Some(project) = &project {
                hub.set_project_default(project, Placement::Cloud);
            }
            state.cloud.shared.moving.lock().insert(agent);
            // The local kernel must not write for it from here on.
            state.idle.remove(&agent);
            let shared = Arc::clone(&state.cloud.shared);
            tokio::spawn(async move {
                let (boot, events) = BootReporter::channel();
                let shown = console.pump(events);
                let result = hub.place(request, &boot).await;
                drop(boot);
                let _ = shown.await;
                shared.moving.lock().remove(&agent);
                let (ok, note) = match result {
                    Ok(view) => (true, format!(
                        "Now running in the cloud on {} ({} vCPU). Its record syncs back here; `/local` brings it back.",
                        view.machine.name, view.machine.vcpu
                    )),
                    Err(err) => (false, format!("Could not move the agent to the cloud: {err}")),
                };
                info!(%agent, %note, "placement");
                console.end(ok, note).await;
            });
        }
        PlacementView::Local => {
            if !hub.is_placed(agent) {
                return;
            }
            if let Some(project) = &project {
                hub.set_project_default(project, Placement::Local);
            }
            recall(state, &hub, agent, "returned to local");
        }
    }
}

fn recall(state: &LoopState<'_>, hub: &Arc<CloudHub>, agent: AgentId, reason: &'static str) {
    let shared = Arc::clone(&state.cloud.shared);
    let hub = Arc::clone(hub);
    let console = BootConsoleUi::new(
        AgentUi::new(state.commands.clone(), agent),
        "Bringing the agent back",
    );
    shared.moving.lock().insert(agent);
    tokio::spawn(async move {
        console.begin().await;
        let (boot, events) = BootReporter::channel();
        let shown = console.pump(events);
        let result = hub.recall(agent, reason, &boot).await;
        drop(boot);
        let _ = shown.await;
        shared.moving.lock().remove(&agent);
        let (ok, note) = match result {
            Ok(outcome) => {
                shared.stale.lock().insert(agent);
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
        owner: state.auth.owner(),
        owner_name: state.auth.identity().display_name().map(str::to_string),
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
    if let Some(refusal) = foreign_owner(state, &hub, agent) {
        system(state, agent, refusal).await;
        return;
    }
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
                    return;
                }
                note
            }
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
    if let Some(refusal) = foreign_owner(state, &hub, agent) {
        system(state, agent, refusal).await;
        return None;
    }
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
            // A plan model runs on the owner's plan, through the boxd
            // secret their machine holds.
            if let Some(kind) = plan_of_model(&current) {
                if !state.auth.linked_plans(&state.auth.owner()).contains(&kind) {
                    system(
                        state,
                        agent,
                        format!(
                            "Sign in to {} (`/login {}`) to use it from cloud agents.",
                            kind.label(),
                            kind.id()
                        ),
                    )
                    .await;
                    return None;
                }
            }
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
    // A sleeping machine's wake, shown until the turn's first event.
    let mut waking: Option<BootConsoleUi> = None;
    while let Some(event) = rx.recv().await {
        if let RemoteEvent::Boot(event) = event {
            waking
                .get_or_insert_with(|| BootConsoleUi::new(ui.clone(), "Waking cloud machine"))
                .show(vec![event])
                .await;
            continue;
        }
        if let Some(console) = waking.take() {
            match &event {
                RemoteEvent::Failed(err) => {
                    console
                        .end(false, format!("Could not wake the machine: {err}"))
                        .await;
                }
                _ => {
                    console
                        .end(true, "Awake; the turn runs on it.".to_string())
                        .await;
                }
            }
        }
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
            RemoteEvent::Boot(_) => {}
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
                let lower = err.to_ascii_lowercase();
                let hint = if lower.contains("401") || lower.contains("unauthorized") {
                    " The plan refused the machine's token: check the plan is linked here \
                     (`/login <plan>`); the machine picks up a new sign-in within two minutes."
                } else {
                    ""
                };
                ui.send(UiCommand::ShowError(format!(
                    "cloud turn failed: {err}{hint}"
                )))
                .await;
                break;
            }
        }
    }
    if let Some(console) = waking {
        console
            .end(false, "The turn ended while the machine woke.".to_string())
            .await;
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

#[cfg(test)]
mod owner_tests {
    use super::*;
    use aura_core_auth::{Owner, ProviderKind};

    #[test]
    fn only_the_placing_user_manages_a_cloud_agent() {
        let alice = Owner::Zos {
            user_id: "alice".into(),
        };
        let bob = Owner::Zos {
            user_id: "bob".into(),
        };
        assert_eq!(refusal_for(&alice, Some("Alice".into()), &alice), None);
        assert_eq!(
            refusal_for(&alice, Some("Alice".into()), &bob).as_deref(),
            Some("Placed by Alice; sign in as Alice to manage it.")
        );
        assert!(refusal_for(&Owner::Local, None, &bob)
            .is_some_and(|note| note.contains("local profile")));
    }

    #[test]
    fn plan_models_are_recognized() {
        assert_eq!(
            plan_of_model("claude/claude-sonnet-4"),
            Some(ProviderKind::Claude)
        );
        assert_eq!(plan_of_model("chatgpt/gpt-5"), Some(ProviderKind::Chatgpt));
        assert_eq!(
            plan_of_model("openrouter/x/y"),
            Some(ProviderKind::OpenRouter)
        );
        assert_eq!(plan_of_model("aura-claude-sonnet"), None);
        assert_eq!(plan_of_model("local/qwen3:4b"), None);
        assert_eq!(plan_of_model("zos/x"), None);
    }
}

#[cfg(test)]
mod boot_tests {
    use super::*;
    use aura_engine_cloud::BootStage;

    #[tokio::test]
    async fn boot_events_become_console_updates_with_log_runs_joined() {
        let (boot, mut events) = BootReporter::channel();
        boot.fact("machine", "aura-1");
        let _ = boot
            .run(BootStage::Snapshot, async {
                boot.log(BootStage::Snapshot, false, "$ cargo build");
                boot.log(BootStage::Snapshot, true, "Compiling aura");
                Ok::<_, String>(())
            })
            .await;
        drop(boot);
        let mut batch = Vec::new();
        while events.recv_many(&mut batch, 64).await > 0 {}

        let updates = boot_updates(batch);
        assert_eq!(updates.len(), 4, "{updates:?}");
        assert!(matches!(
            &updates[0],
            BootUpdate::Fact { key, value } if key == "machine" && value == "aura-1"
        ));
        assert!(matches!(
            &updates[1],
            BootUpdate::Stage { key, state: BootStageState::Running, .. } if key == "snapshot"
        ));
        let BootUpdate::Log(lines) = &updates[2] else {
            panic!("expected a log run: {updates:?}");
        };
        assert_eq!(
            lines,
            &vec![
                BootLogLine {
                    stderr: false,
                    text: "$ cargo build".into()
                },
                BootLogLine {
                    stderr: true,
                    text: "Compiling aura".into()
                },
            ]
        );
        assert!(matches!(
            &updates[3],
            BootUpdate::Stage {
                state: BootStageState::Done { .. },
                ..
            }
        ));
    }
}
