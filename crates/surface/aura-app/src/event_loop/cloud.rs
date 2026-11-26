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
//!   are refused until the move ends. A cloud agent's prompts and `/new`
//!   go to its node, never to the local kernel ([`CloudState::node_writes`]
//!   keeps `/resume`, `/rewind` and auth notes off its local record), and
//!   a recalled agent's local session is reopened from the record (which
//!   now holds what the node wrote) before its next local turn.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aura_core_protocol::cloud::MeshProcess;
use aura_core_types::{AgentId, Placement};
use aura_engine_cloud::{BootEvent, BootReporter, BootStage, StageState};
use aura_store_db::Store;
use aura_ui_terminal::events::{
    BootLogLine, BootPlanStep, BootStageState, BootUpdate, MessageData, MessageRole, ProcessKind,
    ProcessStatus, ProcessSummary, ToolData,
};
use aura_ui_terminal::host::{HostAction, HostRow, HostView, PlacementState, PlacementView};
use aura_ui_terminal::UiCommand;
use futures_util::future::BoxFuture;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::agent_events::AgentUi;
use super::processes::RemoteRows;
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
    /// Where an ended turn is said, once the loop listens
    /// ([`CloudState::take_ended`]).
    ended: Mutex<Option<tokio::sync::mpsc::UnboundedSender<AgentId>>>,
}

impl Shared {
    /// `agent`'s cloud turn (or `/new`) is over. The loop frees it and
    /// starts what it queued meanwhile, so nothing sent while the turn
    /// ended overtakes that; with no loop listening it is freed here.
    fn end_turn(&self, agent: AgentId) {
        let told = self
            .ended
            .lock()
            .as_ref()
            .is_some_and(|ended| ended.send(agent).is_ok());
        if !told {
            self.running.lock().remove(&agent);
        }
    }
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

    /// Whether the local kernel must leave `agent`'s record alone: it
    /// runs on a cloud machine, whose node writes it, or is being moved.
    pub(super) fn node_writes(&self, agent: AgentId) -> bool {
        self.is_cloud(agent) || self.is_moving(agent)
    }

    /// Forget `agent`'s local session if it is older than its record.
    pub(super) fn take_stale(&self, agent: AgentId) -> bool {
        self.shared.stale.lock().remove(&agent)
    }

    /// Whether `agent` has a cloud turn (or `/new`) under way.
    pub(super) fn is_running(&self, agent: AgentId) -> bool {
        self.shared.running.lock().contains_key(&agent)
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

    /// The agents whose cloud turn (or `/new`) ended, each to be freed
    /// with [`Self::free`].
    pub(super) fn take_ended(&self) -> tokio::sync::mpsc::UnboundedReceiver<AgentId> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        *self.shared.ended.lock() = Some(tx);
        rx
    }

    /// Let `agent`, whose cloud turn ended, take work again.
    pub(super) fn free(&self, agent: AgentId) {
        self.shared.running.lock().remove(&agent);
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

/// Numbers boot consoles, so each move or wake gets one of its own. Ids
/// also carry when they were made: kept consoles come back after a
/// restart, and must not take a new console's updates.
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
            id: format!(
                "boot-{}-{}",
                crate::session_checkpoint::now_ms(),
                NEXT_BOOT.fetch_add(1, Ordering::Relaxed)
            ),
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
            BootEvent::Waiting { stage, what, limit } => BootUpdate::Waiting {
                key: stage.key().to_string(),
                what,
                limit_ms: limit.map(|limit| u64::try_from(limit.as_millis()).unwrap_or(u64::MAX)),
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
/// and start the HOST publisher, showing on `boot` what it waits on.
async fn ensure_hub(
    state: &mut LoopState<'_>,
    boot: &BootReporter,
) -> Result<Arc<CloudHub>, String> {
    if let Some(hub) = state.cloud.hub() {
        return Ok(hub);
    }
    let config = boot
        .wait(
            BootStage::Connect,
            "reading the cloud settings",
            None,
            tokio::task::spawn_blocking(aura_config::CloudConfig::reload),
        )
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
    if let Some(checkpoints) = &state.hub.factory.checkpoints {
        hub.attach_sessions(checkpoints.clone());
    }
    if let Some(observer) = &state.hub.factory.record_observer {
        hub.attach_observer(Arc::clone(observer));
    }
    if let Some(traces) = state.hub.traces.store() {
        hub.attach_traces(traces);
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
    hub.start_reported(&keep, boot).await;
    *state.cloud.shared.hub.lock() = Some(Arc::clone(&hub));
    state
        .processes
        .set_remote(Arc::new(MachineRows(Arc::downgrade(&hub))));
    start_publisher(state);
    Ok(hub)
}

/// What cloud agents' machines have under way, as PROCESSES rows.
struct MachineRows(std::sync::Weak<CloudHub>);

impl RemoteRows for MachineRows {
    fn rows(&self) -> Vec<ProcessSummary> {
        self.0.upgrade().map_or_else(Vec::new, |hub| {
            hub.machine_processes()
                .iter()
                .flat_map(|(agent, rows)| machine_rows(agent, rows))
                .collect()
        })
    }
}

/// `rows`, what placed `agent`'s machine has under way, as PROCESSES
/// rows. Spawned agents nest under the agent that spawned them, with the
/// ids local ones have, so MEMBERS lists them too.
fn machine_rows(agent: &str, rows: &[MeshProcess]) -> Vec<ProcessSummary> {
    let parents: std::collections::HashMap<&str, &str> = rows
        .iter()
        .filter(|row| row.is_agent())
        .filter_map(|row| Some((row.agent_id.as_str(), row.parent.as_deref()?)))
        .collect();
    let names: std::collections::HashMap<&str, &str> = rows
        .iter()
        .filter_map(|row| Some((row.agent_id.as_str(), row.name.as_deref()?)))
        .collect();
    rows.iter()
        .map(|row| {
            let status = match row.status.as_str() {
                "idle" => ProcessStatus::Idle,
                "paused" => ProcessStatus::Paused,
                "done" => ProcessStatus::Exited,
                "failed" => ProcessStatus::Failed,
                _ => ProcessStatus::Running,
            };
            let summary = ProcessSummary {
                label: row.label.clone(),
                status,
                started_ms: row.started_ms,
                jump_agent_id: agent.to_string(),
                ..ProcessSummary::default()
            };
            if !row.is_agent() {
                return ProcessSummary {
                    id: format!("machine:{}", row.id),
                    kind: ProcessKind::Turn,
                    detail: row.label.clone(),
                    owner_agent_id: row.agent_id.clone(),
                    ..summary
                };
            }
            let owner = row.parent.clone().unwrap_or_else(|| agent.to_string());
            let mut chain = Vec::new();
            let mut above = parents.get(owner.as_str()).copied();
            while let Some(parent) = above.filter(|_| chain.len() < rows.len()) {
                chain.push(parent.to_string());
                above = parents.get(parent).copied();
            }
            ProcessSummary {
                id: format!("agent:{}", row.agent_id),
                kind: ProcessKind::Agent,
                owner_name: names
                    .get(owner.as_str())
                    .map(|name| (*name).to_string())
                    .unwrap_or_default(),
                owner_agent_id: owner,
                parent_chain: chain,
                ..summary
            }
        })
        .collect()
}

/// The hub, started if need be. With a `console`, it shows at once, the
/// start as its first stage with each machine taken back.
async fn connect(
    state: &mut LoopState<'_>,
    console: Option<&BootConsoleUi>,
) -> Result<Arc<CloudHub>, String> {
    let Some(console) = console else {
        return ensure_hub(state, &BootReporter::none()).await;
    };
    console.begin().await;
    if let Some(hub) = state.cloud.hub() {
        return Ok(hub);
    }
    let (boot, events) = BootReporter::channel();
    let shown = console.pump(events);
    let hub = boot.run(BootStage::Connect, ensure_hub(state, &boot)).await;
    drop(boot);
    let _ = shown.await;
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
        if let Err(err) = ensure_hub(state, &BootReporter::none()).await {
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
            let cloud = shared.hub.lock().clone();
            if cloud.as_ref().is_some_and(|cloud| cloud.take_copied()) {
                hub.refresh_agent_list().await;
            }
            let Some(agent) = hub.record_panel.get() else {
                continue;
            };
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
            if let Some(sync) = cloud.sync_status(agent) {
                host_info::add_sync(&mut view, &sync, now);
            }
            if let Some(placed) = cloud.placed_agent(agent) {
                if let Some(branch) = placed.branch.as_deref() {
                    let token = placed
                        .repo
                        .as_ref()
                        .and_then(|repo| cloud.repo_status(repo));
                    host_info::add_git(
                        &mut view,
                        &host_info::GitFacts {
                            branch,
                            repo: placed.repo.as_ref(),
                            token: token.as_ref(),
                            fetched: placed.fetched_tip.as_deref(),
                        },
                        now,
                    );
                }
            }
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
            // The local kernel must not write for it from here on, and
            // its machine traces the conversation as a run of its own.
            state.idle.remove(&agent);
            state.hub.traces.close(agent);
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
/// Returns the work when it is for the local kernel after all. Work for
/// a cloud agent with a turn running waits in its queue.
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
    if state.cloud.is_running(agent) {
        info!(%agent, "Cloud agent is busy; queueing work");
        state.queued.push_back(agent, work);
        super::hud::send_queued(state, agent).await;
        return None;
    }
    run(state, hub, agent, work).await;
    None
}

/// Start the work queued for cloud agent `agent`, an item at a time while
/// it has no turn running: one that starts none (a command, a refusal)
/// leaves the next free to.
pub(super) async fn start_queued(state: &mut LoopState<'_>, agent: AgentId) {
    while !state.cloud.is_running(agent) {
        let Some(work) = state.queued.pop_front(agent) else {
            return;
        };
        super::hud::send_queued(state, agent).await;
        if let Some(work) = submit(state, agent, work).await {
            super::start_work(state, agent, work).await;
            return;
        }
    }
}

/// Run `work` on cloud agent `agent`, which has no turn running.
async fn run(state: &mut LoopState<'_>, hub: Arc<CloudHub>, agent: AgentId, work: Work) {
    match work {
        Work::Prompt { prompt, shown } => {
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
            let linked = state.auth.linked_plans(&state.auth.owner());
            // A plan model runs on the owner's plan, through the boxd
            // secret their machine holds.
            if let Some(kind) = plan_of_model(&current) {
                if !linked.contains(&kind) {
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
                    return;
                }
            }
            let model =
                (!current.is_empty() && current != "gate" && !current.starts_with("local/"))
                    .then_some(current);
            let jwt = match state.auth.identity() {
                aura_auth::Identity::Zos(session) => Some(session.access_token),
                aura_auth::Identity::Local => None,
            };
            // Titled like a local agent: on a typed prompt, while it still
            // has a placeholder name. Its machine makes the call, since only
            // the machine writes the agent's record while it is placed.
            let wants_title = state.hub.titles_turn(agent, prompt.mentions);
            let title_model = wants_title
                .then(|| super::titles::title_model(state.models.classifier_model()))
                .filter(|model| runs_on_machine(model, &linked));
            let mut titles = wants_title.then(|| {
                Titles::new(
                    title_sink(Arc::clone(&state.hub), Arc::clone(&hub), agent),
                    &prompt.text,
                )
            });
            if title_model.is_none() {
                if let Some(titles) = titles.as_mut() {
                    titles.finish().await;
                }
            }
            let mut extras = TurnExtras {
                titles,
                context_limit: model.as_deref().map(aura_engine::context_window_for_model),
            };
            // Stopped from the chat or its PROCESSES row alike.
            let cancel = CancellationToken::new();
            state
                .cloud
                .shared
                .running
                .lock()
                .insert(agent, cancel.clone());
            let mut row = super::handlers::turn_summary(
                agent,
                &prompt.text,
                model.as_deref().unwrap_or_default(),
            );
            row.model.clone_from(&model);
            state.processes.start(row, Some(cancel.clone()));
            let mut rx = hub.run_turn(agent, prompt.text, model, title_model, jwt, cancel.clone());
            let shared = Arc::clone(&state.cloud.shared);
            let processes = Arc::clone(&state.processes);
            let sessions = state.hub.factory.checkpoints.clone();
            tokio::spawn(async move {
                let end = forward(&ui, &mut rx, &mut extras, &cancel).await;
                processes.finish(
                    &super::processes::turn_id(agent),
                    end.status(cancel.is_cancelled()),
                );
                // The hub copies the turn's session here before it lets go
                // of the channel; the totals come from that copy.
                while rx.recv().await.is_some() {}
                if let Some(sessions) = sessions {
                    let copied =
                        tokio::task::spawn_blocking(move || sessions.load_current(agent)).await;
                    if let Ok(Some(checkpoint)) = copied {
                        ui.send(UiCommand::SetSessionTotals(super::hud::session_totals(
                            &checkpoint.usage,
                        )))
                        .await;
                    }
                }
                shared.end_turn(agent);
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
}

/// `/new` for a cloud agent: its node writes the `SessionStart`, waking
/// the machine if it sleeps, and the hub copies it here. Returns whether
/// the node was asked; a failure is said in the agent's chat.
pub(super) async fn new_session(state: &mut LoopState<'_>, agent: AgentId) -> bool {
    if state.cloud.is_moving(agent) {
        system(
            state,
            agent,
            "This agent is moving between machines; start a new session when the move is done.",
        )
        .await;
        return false;
    }
    let Some(hub) = state.cloud.hub() else {
        return false;
    };
    if let Some(refusal) = foreign_owner(state, &hub, agent) {
        system(state, agent, refusal).await;
        return false;
    }
    if state.cloud.shared.running.lock().contains_key(&agent) {
        system(
            state,
            agent,
            "The cloud agent is still working; wait or stop it first.",
        )
        .await;
        return false;
    }
    // Prompts wait while the node starts the session, so none runs in the
    // old one.
    state
        .cloud
        .shared
        .running
        .lock()
        .insert(agent, CancellationToken::new());
    let shared = Arc::clone(&state.cloud.shared);
    let ui = AgentUi::new(state.commands.clone(), agent);
    tokio::spawn(async move {
        let result = hub.new_session(agent).await;
        shared.end_turn(agent);
        if let Err(err) = result {
            warn!(%agent, error = %err, "the cloud node did not start a new session");
            ui.send(UiCommand::ShowMessage(MessageData::new(
                MessageRole::System,
                format!("Could not start a new session on the cloud machine; the agent keeps its conversation: {err}"),
            )))
            .await;
        }
    });
    true
}

/// Whether `model` can run on a cloud machine: not Gate or a model on
/// this computer, and a plan model only when its plan is `linked`.
fn runs_on_machine(model: &str, linked: &[aura_core_auth::ProviderKind]) -> bool {
    !model.is_empty()
        && model != "gate"
        && !model.starts_with("local/")
        && plan_of_model(model).is_none_or(|kind| linked.contains(&kind))
}

/// Applies a cloud chat's title.
type TitleSink = Box<dyn Fn(String) -> BoxFuture<'static, ()> + Send + Sync>;

/// Titles `agent` in the agents list and, when that took, on its placement,
/// which names it in mail.
fn title_sink(agents: Arc<AgentHub>, hub: Arc<CloudHub>, agent: AgentId) -> TitleSink {
    Box::new(move |title| {
        let agents = Arc::clone(&agents);
        let hub = Arc::clone(&hub);
        Box::pin(async move {
            if agents.apply_title(agent, &title).await {
                hub.rename(agent, &title);
            }
        })
    })
}

/// Where a cloud chat's title goes, with the title made from its prompt
/// for when the machine sends none.
struct Titles {
    sink: TitleSink,
    fallback: Option<String>,
}

impl Titles {
    fn new(sink: TitleSink, prompt: &str) -> Self {
        Self {
            sink,
            fallback: Some(aura_agent::titles::fallback_title(prompt)),
        }
    }

    async fn apply(&mut self, title: String) {
        self.fallback = None;
        (self.sink)(title).await;
    }

    /// Apply the title made from the prompt, unless one was applied.
    async fn finish(&mut self) {
        if let Some(title) = self.fallback.take() {
            (self.sink)(title).await;
        }
    }
}

/// What a cloud turn shows outside its chat's stream.
#[derive(Default)]
struct TurnExtras {
    /// Where the chat's title goes; `None` when it needs none.
    titles: Option<Titles>,
    /// The context window of the model the turn asked for.
    context_limit: Option<u64>,
}

/// How a cloud turn's stream ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TurnEnd {
    /// The reply ended.
    Done,
    /// The turn failed, or its stream closed first.
    Failed,
}

impl TurnEnd {
    /// The PROCESSES status of a turn that ended so, when the user
    /// `stopped` it or not.
    fn status(self, stopped: bool) -> ProcessStatus {
        match (self, stopped) {
            (_, true) => ProcessStatus::Killed,
            (Self::Done, false) => ProcessStatus::Exited,
            (Self::Failed, false) => ProcessStatus::Failed,
        }
    }
}

/// Show a cloud turn's events in its agent's chat, until the turn ends.
/// A turn ended by `cancel` says so, without an error.
async fn forward(
    ui: &AgentUi,
    rx: &mut tokio::sync::mpsc::Receiver<RemoteEvent>,
    extras: &mut TurnExtras,
    cancel: &CancellationToken,
) -> TurnEnd {
    let mut end = TurnEnd::Failed;
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
        // Arrives mid-reply; the reply streams on around it.
        if let RemoteEvent::Title(title) = event {
            if let Some(titles) = extras.titles.as_mut() {
                titles.apply(title).await;
            }
            continue;
        }
        if let Some(console) = waking.take() {
            match &event {
                RemoteEvent::Failed(_) if cancel.is_cancelled() => {
                    console
                        .end(false, "The turn was stopped.".to_string())
                        .await;
                }
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
            RemoteEvent::Boot(_) | RemoteEvent::Title(_) => {}
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
            RemoteEvent::Done(usage) => {
                let limit = extras.context_limit.or_else(|| {
                    (!usage.model.is_empty())
                        .then(|| aura_engine::context_window_for_model(&usage.model))
                });
                ui.send(UiCommand::SetContext(super::hud::remote_context_view(
                    &usage, limit,
                )))
                .await;
                end = TurnEnd::Done;
                break;
            }
            RemoteEvent::Failed(err) if cancel.is_cancelled() => {
                info!(error = %err, "Cancelled cloud turn ended with an error");
                ui.send(UiCommand::ShowWarning("Turn cancelled".to_string()))
                    .await;
                break;
            }
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
    if let Some(titles) = extras.titles.as_mut() {
        titles.finish().await;
    }
    if thinking {
        ui.send(UiCommand::FinishThinking).await;
    }
    if streaming {
        ui.send(UiCommand::FinishStreaming).await;
    }
    // The chat entered Processing when the prompt was sent. Status text
    // alone leaves it there, so the working line keeps saying Thinking
    // after the turn is over.
    ui.send(UiCommand::Complete).await;
    end
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
mod forward_tests {
    use super::*;
    use tokio::sync::mpsc;

    fn chat() -> (AgentUi, mpsc::Receiver<UiCommand>) {
        let (tx, rx) = mpsc::channel(32);
        (AgentUi::new(tx, AgentId::new([7; 32])), rx)
    }

    fn commands(rx: &mut mpsc::Receiver<UiCommand>) -> Vec<UiCommand> {
        let mut out = Vec::new();
        while let Ok(command) = rx.try_recv() {
            match command {
                UiCommand::ForAgent { command, .. } => out.push(*command),
                other => out.push(other),
            }
        }
        out
    }

    /// The working line clears only when the chat leaves Processing, which
    /// `Complete` does. A status of Ready does not.
    fn ends_the_turn(commands: &[UiCommand]) {
        assert!(
            matches!(commands.last(), Some(UiCommand::Complete)),
            "{commands:?}"
        );
        assert!(
            !commands.iter().any(
                |command| matches!(command, UiCommand::SetStatus(status) if status == "Ready")
            ),
            "{commands:?}"
        );
    }

    #[tokio::test]
    async fn a_finished_cloud_turn_completes_the_chat() {
        let (ui, mut rx) = chat();
        let (tx, mut events) = mpsc::channel(8);
        tx.send(RemoteEvent::Text("hello".into())).await.unwrap();
        tx.send(RemoteEvent::ToolStart {
            id: "t1".into(),
            name: "run_command".into(),
        })
        .await
        .unwrap();
        tx.send(RemoteEvent::ToolResult {
            id: "t1".into(),
            result: "ok".into(),
            is_error: false,
        })
        .await
        .unwrap();
        tx.send(RemoteEvent::Text("done".into())).await.unwrap();
        tx.send(done(100)).await.unwrap();
        drop(tx);

        let mut extras = TurnExtras {
            context_limit: Some(1_000),
            ..TurnExtras::default()
        };
        let end = forward(&ui, &mut events, &mut extras, &CancellationToken::new()).await;

        assert_eq!(end, TurnEnd::Done);
        let sent = commands(&mut rx);
        ends_the_turn(&sent);
        assert!(sent
            .iter()
            .any(|command| matches!(command, UiCommand::FinishStreaming)));
        assert!(sent
            .iter()
            .any(|command| matches!(command, UiCommand::CompleteTool { success: true, .. })));
        assert!(
            sent.iter().any(|command| matches!(
                command,
                UiCommand::SetContext(view)
                    if view.total_tokens == 100 && view.limit_tokens == Some(1_000)
            )),
            "the status bar shows the turn's context: {sent:?}"
        );
    }

    #[tokio::test]
    async fn a_failed_cloud_turn_completes_the_chat() {
        let (ui, mut rx) = chat();
        let (tx, mut events) = mpsc::channel(8);
        tx.send(RemoteEvent::Failed("boom".into())).await.unwrap();
        drop(tx);

        let end = forward(
            &ui,
            &mut events,
            &mut TurnExtras::default(),
            &CancellationToken::new(),
        )
        .await;

        assert_eq!(end, TurnEnd::Failed);
        let sent = commands(&mut rx);
        ends_the_turn(&sent);
        assert!(sent.iter().any(|command| matches!(
            command,
            UiCommand::ShowError(message) if message.contains("boom")
        )));
    }

    #[tokio::test]
    async fn a_stopped_cloud_turn_says_so_without_an_error() {
        let (ui, mut rx) = chat();
        let (tx, mut events) = mpsc::channel(8);
        tx.send(RemoteEvent::Text("half a reply".into()))
            .await
            .unwrap();
        tx.send(RemoteEvent::Failed(
            "machine aura-1 is cancelled; cannot finish the turn".into(),
        ))
        .await
        .unwrap();
        drop(tx);
        let cancel = CancellationToken::new();
        cancel.cancel();

        let end = forward(&ui, &mut events, &mut TurnExtras::default(), &cancel).await;

        assert_eq!(end.status(cancel.is_cancelled()), ProcessStatus::Killed);
        let sent = commands(&mut rx);
        ends_the_turn(&sent);
        assert!(
            sent.iter().any(
                |command| matches!(command, UiCommand::ShowWarning(message) if message == "Turn cancelled")
            ),
            "{sent:?}"
        );
        assert!(
            !sent
                .iter()
                .any(|command| matches!(command, UiCommand::ShowError(_))),
            "{sent:?}"
        );
    }

    #[test]
    fn a_cloud_turn_s_row_ends_as_the_turn_did() {
        assert_eq!(TurnEnd::Done.status(false), ProcessStatus::Exited);
        assert_eq!(TurnEnd::Failed.status(false), ProcessStatus::Failed);
        assert_eq!(TurnEnd::Done.status(true), ProcessStatus::Killed);
    }

    #[tokio::test]
    async fn a_dropped_cloud_turn_completes_the_chat() {
        let (ui, mut rx) = chat();
        let (tx, mut events) = mpsc::channel(8);
        tx.send(RemoteEvent::Thinking("hmm".into())).await.unwrap();
        drop(tx);

        let end = forward(
            &ui,
            &mut events,
            &mut TurnExtras::default(),
            &CancellationToken::new(),
        )
        .await;

        assert_eq!(end, TurnEnd::Failed, "closed before the reply ended");
        let sent = commands(&mut rx);
        ends_the_turn(&sent);
        assert!(sent
            .iter()
            .any(|command| matches!(command, UiCommand::FinishThinking)));
    }

    fn done(context_tokens: u64) -> RemoteEvent {
        RemoteEvent::Done(Box::new(aura_core_protocol::SessionUsage {
            model: "m".into(),
            input_tokens: 1,
            output_tokens: 2,
            estimated_context_tokens: context_tokens,
            ..Default::default()
        }))
    }

    /// Titles that remember what they were given.
    fn titles(prompt: &str) -> (Titles, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&seen);
        let sink: TitleSink = Box::new(move |title| {
            let kept = Arc::clone(&kept);
            Box::pin(async move { kept.lock().push(title) })
        });
        (Titles::new(sink, prompt), seen)
    }

    #[tokio::test]
    async fn the_machine_s_title_names_the_chat_without_breaking_the_reply() {
        let (ui, mut rx) = chat();
        let (tx, mut events) = mpsc::channel(8);
        tx.send(RemoteEvent::Text("hel".into())).await.unwrap();
        tx.send(RemoteEvent::Title("Build fix".into()))
            .await
            .unwrap();
        tx.send(RemoteEvent::Text("lo".into())).await.unwrap();
        tx.send(done(10)).await.unwrap();
        drop(tx);
        let (titles, seen) = titles("fix the build please");
        let mut extras = TurnExtras {
            titles: Some(titles),
            ..TurnExtras::default()
        };

        forward(&ui, &mut events, &mut extras, &CancellationToken::new()).await;

        assert_eq!(*seen.lock(), ["Build fix"], "and no title from the prompt");
        let sent = commands(&mut rx);
        let starts = sent
            .iter()
            .filter(|command| matches!(command, UiCommand::StartStreaming))
            .count();
        assert_eq!(starts, 1, "one reply: {sent:?}");
    }

    #[tokio::test]
    async fn without_a_title_from_the_machine_the_prompt_names_the_chat() {
        for last in [done(10), RemoteEvent::Failed("boom".into())] {
            let (ui, _rx) = chat();
            let (tx, mut events) = mpsc::channel(8);
            tx.send(RemoteEvent::Text("ok".into())).await.unwrap();
            tx.send(last).await.unwrap();
            drop(tx);
            let (titles, seen) = titles("\n  fix   the build\nthen run tests");
            let mut extras = TurnExtras {
                titles: Some(titles),
                ..TurnExtras::default()
            };

            forward(&ui, &mut events, &mut extras, &CancellationToken::new()).await;

            assert_eq!(*seen.lock(), ["fix the build"]);
        }
    }

    #[test]
    fn titles_are_written_on_the_machine_only_by_models_it_can_run() {
        use aura_core_auth::ProviderKind;
        let linked = [ProviderKind::Claude];
        assert!(runs_on_machine("aura-claude-haiku-4-5", &linked));
        assert!(runs_on_machine("claude/claude-haiku-4-5", &linked));
        assert!(
            !runs_on_machine("chatgpt/gpt-5-mini", &linked),
            "plan not linked"
        );
        assert!(!runs_on_machine("local/qwen3:4b", &linked));
        assert!(!runs_on_machine("gate", &linked));
        assert!(!runs_on_machine("", &linked));
    }
}

#[cfg(test)]
mod machine_tests {
    use super::*;
    use aura_core_protocol::cloud::MESH_PROCESS_AGENT;

    fn row(id: &str, agent: &str, status: &str, parent: Option<&str>) -> MeshProcess {
        MeshProcess {
            id: id.into(),
            agent_id: agent.into(),
            label: id.into(),
            status: status.into(),
            started_ms: 5,
            kind: parent.map(|_| MESH_PROCESS_AGENT.into()),
            run_id: None,
            parent: parent.map(str::to_string),
            name: parent.map(|_| format!("name of {agent}")),
        }
    }

    #[test]
    fn a_machine_s_work_becomes_rows_members_can_list() {
        let rows = machine_rows(
            "aa",
            &[
                row("turn:r1:5", "aa", "running", None),
                row("agent:bb", "bb", "running", Some("aa")),
                row("agent:cc", "cc", "idle", Some("bb")),
            ],
        );

        assert_eq!(rows[0].id, "machine:turn:r1:5");
        assert_eq!(rows[0].kind, ProcessKind::Turn);
        assert_eq!(rows[0].owner_agent_id, "aa");

        assert_eq!(rows[1].id, "agent:bb");
        assert_eq!(rows[1].kind, ProcessKind::Agent);
        assert_eq!(rows[1].owner_agent_id, "aa");
        assert!(rows[1].parent_chain.is_empty());
        assert_eq!(rows[1].status, ProcessStatus::Running);

        assert_eq!(rows[2].owner_agent_id, "bb");
        assert_eq!(rows[2].owner_name, "name of bb");
        assert_eq!(rows[2].parent_chain, ["aa"]);
        assert_eq!(rows[2].status, ProcessStatus::Idle);

        assert!(rows.iter().all(|row| row.jump_agent_id == "aa"));
        assert!(rows.iter().all(|row| !row.killable));
    }

    #[test]
    fn an_ended_cloud_turn_frees_its_agent_through_the_loop() {
        let cloud = CloudState::default();
        let agent = AgentId::generate();
        cloud
            .shared
            .running
            .lock()
            .insert(agent, CancellationToken::new());
        cloud.shared.end_turn(agent);
        assert!(!cloud.is_running(agent), "no loop listens: freed at once");

        let mut ended = cloud.take_ended();
        cloud
            .shared
            .running
            .lock()
            .insert(agent, CancellationToken::new());
        cloud.shared.end_turn(agent);
        assert!(
            cloud.is_running(agent),
            "what is sent until the loop frees it still queues"
        );
        assert_eq!(ended.try_recv().ok(), Some(agent));
        cloud.free(agent);
        assert!(!cloud.is_running(agent));
    }
}

#[cfg(test)]
mod boot_tests {
    use super::*;

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

    #[tokio::test]
    async fn a_wait_becomes_a_waiting_line_then_its_clear() {
        let (boot, mut events) = BootReporter::channel();
        boot.wait(
            BootStage::Create,
            "boxd is creating aura-1",
            Some(Duration::from_secs(180)),
            async {},
        )
        .await;
        drop(boot);
        let mut batch = Vec::new();
        while events.recv_many(&mut batch, 64).await > 0 {}
        assert_eq!(
            boot_updates(batch),
            vec![
                BootUpdate::Waiting {
                    key: "create".into(),
                    what: Some("boxd is creating aura-1".into()),
                    limit_ms: Some(180_000),
                },
                BootUpdate::Waiting {
                    key: "create".into(),
                    what: None,
                    limit_ms: None,
                },
            ]
        );
    }
}
