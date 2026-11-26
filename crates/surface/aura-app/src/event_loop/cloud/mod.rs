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
use aura_engine_cloud::{BootEvent, BootReporter, BootStage, StageState};
use aura_store_db::Store;
use aura_ui::events::{
    BootLogLine, BootPlanStep, BootStageState, BootUpdate, MessageData, MessageRole, ToolData,
};
use aura_ui::host::{HostAction, HostRow, HostView, PlacementState, PlacementView};
use aura_ui::UiCommand;
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

mod boot;
mod placement;
mod submit;
#[cfg(test)]
mod tests;
mod view;

use boot::BootConsoleUi;
pub(super) use placement::{handle_host_action, handle_set_placement, handle_setup, on_archive};
pub(super) use submit::submit;
use view::{build_view, placement_state};

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
        .map_or_else(|_| PathBuf::from("aura_data"), |home| home.path)
        .join("cloud")
}

fn env_file() -> PathBuf {
    aura_config::AuraHome::resolve()
        .map_or_else(|_| PathBuf::from("aura_data"), |home| home.path)
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
    start_publisher(state);
    Ok(hub)
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
