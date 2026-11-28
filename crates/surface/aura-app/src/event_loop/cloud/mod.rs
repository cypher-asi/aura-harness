//! The event loop's side of cloud agents: the footer switch, `/cloud`,
//! the HOST tab and the turns of agents that run on boxd machines.
//!
//! The [`CloudHub`] is built on first use, so a boxd key added later
//! works without a restart. Everything slow (a build, a hand-off, a turn)
//! runs on a task of its own; the loop keeps who moves and who works.
//!
//! ## Invariants
//!
//! - **One writer per agent.** A moving agent starts no work: what it is
//!   sent waits in its queue until the loop hears the move ended
//!   ([`Ended::Move`]), then goes where the agent runs now. A cloud agent's
//!   prompts and `/new` go to its node, never to the local kernel
//!   ([`CloudState::node_writes`] keeps `/resume`, `/rewind` and auth
//!   notes off its local record), and a recalled agent's local session is
//!   reopened from the record (which now holds what the node wrote) before
//!   its next local turn.
//! - **Only the loop frees an agent it was told about.** A task that ends
//!   a turn or a move says so on [`Shared::ended`] and leaves the agent
//!   busy, so no work the loop starts before it hears overlaps; only with
//!   no loop listening does the task free the agent itself.
//! - **A cloud turn settles what it was asked as a turn here does.** When
//!   the loop hears it ended ([`TurnOutcome`]), the channel questions and
//!   inbox requests it saw are answered from the conversation the hub
//!   copied back, or their senders told none is coming.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use aura_core_protocol::cloud::MeshProcess;
use aura_core_types::{AgentId, Placement};
use aura_engine_cloud::{BootEvent, BootReporter, BootStage, StageState};
use aura_store_db::Store;
use aura_ui::events::{
    BootLogLine, BootPlanStep, BootStageState, BootUpdate, MessageData, MessageRole, ProcessKind,
    ProcessStatus, ProcessSummary, ToolData,
};
use aura_ui::host::{HostAction, HostRow, HostView, PlacementState, PlacementView};
use aura_ui::sandbox::RemoteEntry;
use aura_ui::UiCommand;
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

mod boot;
mod ended;
#[cfg(test)]
mod ended_tests;
pub(in crate::event_loop) mod follow;
#[cfg(test)]
mod follow_tests;
pub(in crate::event_loop) mod forward;
mod machine;
mod member_start;
mod moving;
mod offline;
mod placement;
mod publish;
mod spawns;
#[cfg(test)]
mod spawns_tests;
mod submit;
#[cfg(test)]
mod tests;
mod view;

use boot::BootConsoleUi;
pub(super) use ended::handle_ended;
pub(in crate::event_loop) use machine::node_offers_ship;
use machine::MachineRows;
pub(super) use machine::{list_sandbox, read_sandbox_file, refresh_machine_plans, MachinePlans};
pub(super) use member_start::start_in_cloud;
use member_start::{within, Start, START_TIMEOUT};
use moving::MoveMark;
pub(super) use placement::{
    handle_host_action, handle_set_placement, handle_setup, on_archive, refresh_reach,
};
pub(super) use publish::start;
use publish::start_publisher;
#[cfg(test)]
pub(super) use submit::refused;
pub(super) use submit::{new_session, start_queued, submit};
use view::{build_view, placement_state};

/// What a cloud task tells the loop it is done with.
#[derive(Debug)]
pub(super) enum Ended {
    /// The agent's cloud turn, and how it went; `/new` has no outcome
    Turn(AgentId, Option<TurnOutcome>),
    /// The agent's move between machines, whichever way it went
    Move(AgentId),
}

/// How a cloud turn went, for what its agent was asked: the channel
/// questions and inbox requests the turn saw are answered from its
/// conversation, or dropped, as a turn here settles them.
#[derive(Debug)]
pub(super) struct TurnOutcome {
    /// Why it stopped before finishing; `None` when it finished.
    pub(super) failure: Option<String>,
    /// Its conversation as its machine left it, copied here; `None`
    /// when the copy failed.
    pub(super) messages: Option<Vec<aura_model::Message>>,
}

/// Shared between the loop and its tasks.
#[derive(Default)]
struct Shared {
    hub: Mutex<Option<Arc<CloudHub>>>,
    /// Agents being moved, each held by a [`MoveMark`].
    moving: Mutex<HashSet<AgentId>>,
    /// New members whose cloud start failed, and why, until the loop hears
    /// their move ended
    failed_starts: Mutex<std::collections::HashMap<AgentId, String>>,
    /// Wakes the HOST publisher before its next refresh is due.
    changed: tokio::sync::Notify,
    /// Cloud agents with a turn running, and how to stop it.
    running: Mutex<std::collections::HashMap<AgentId, CancellationToken>>,
    /// Agents whose local session is older than their record.
    stale: Mutex<HashSet<AgentId>>,
    /// Where an ended turn or move is said, once the loop listens
    /// ([`CloudState::take_ended`]).
    ended: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Ended>>>,
    /// The plans last read from each cloud agent's machine (path and
    /// text), shown while it sleeps.
    plans: Mutex<std::collections::HashMap<AgentId, Vec<(String, String)>>>,
}

impl Shared {
    /// Refresh the footer switches and the HOST tab now, so the switch a
    /// click turned shows where its agent really is without waiting for
    /// the next refresh. Wakes that come while one is under way make one
    /// more.
    fn publish_now(&self) {
        self.changed.notify_one();
    }

    /// `agent`'s cloud turn (or `/new`) is over, as `outcome` says. The
    /// loop frees it, settles what the turn was asked and starts what it
    /// queued meanwhile, so nothing sent while the turn ended overtakes
    /// that; with no loop listening it is freed here.
    fn end_turn(&self, agent: AgentId, outcome: Option<TurnOutcome>) {
        if !self.tell_loop(Ended::Turn(agent, outcome)) {
            self.running.lock().remove(&agent);
        }
    }

    /// `agent`'s move is over, done or not. The loop lets it take work
    /// again and starts what it queued meanwhile, where it runs now; with
    /// no loop listening it is let go here.
    fn end_move(&self, agent: AgentId) {
        if !self.tell_loop(Ended::Move(agent)) {
            self.moving.lock().remove(&agent);
        }
    }

    /// Say `ended` to the loop. Returns whether one listens.
    fn tell_loop(&self, ended: Ended) -> bool {
        self.ended
            .lock()
            .as_ref()
            .is_some_and(|loop_tx| loop_tx.send(ended).is_ok())
    }
}

/// The loop's cloud state.
#[derive(Default)]
pub(crate) struct CloudState {
    store: Option<Arc<dyn Store>>,
    shared: Arc<Shared>,
    publisher: Option<tokio::task::JoinHandle<()>>,
    /// The hub's folder, whose saved placements name the cloud agents
    /// while the hub is down ([`offline`])
    saved: Option<PathBuf>,
}

impl CloudState {
    /// Cloud state over the main `store`.
    pub(super) fn new(store: Option<Arc<dyn Store>>) -> Self {
        Self {
            store,
            shared: Arc::default(),
            publisher: None,
            saved: Some(cloud_home()),
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
        self.is_moving(agent) || self.machine_of(agent).is_some()
    }

    /// The cloud agent on whose machine `agent` was spawned, by it or by
    /// an agent it spawned there: that machine runs `agent`.
    pub(super) fn machine_root(&self, agent: AgentId) -> Option<AgentId> {
        self.hub()?.machine_root(agent)
    }

    /// The cloud agent whose machine runs `agent`: `agent` itself, or the
    /// one on whose machine it was spawned ([`Self::machine_root`]).
    pub(super) fn machine_of(&self, agent: AgentId) -> Option<AgentId> {
        let hub = self.hub()?;
        if hub.is_placed(agent) {
            Some(agent)
        } else {
            hub.machine_root(agent)
        }
    }

    /// Forget `agent`'s local session if it is older than its record.
    pub(super) fn take_stale(&self, agent: AgentId) -> bool {
        self.shared.stale.lock().remove(&agent)
    }

    /// The agents with a cloud turn (or `/new`) under way.
    pub(in crate::event_loop) fn cloud_turn_agents(&self) -> Vec<AgentId> {
        self.shared.running.lock().keys().copied().collect()
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

    /// The cloud turns (and `/new`) and moves that ended, each to be
    /// freed with [`Self::free`].
    pub(super) fn take_ended(&self) -> tokio::sync::mpsc::UnboundedReceiver<Ended> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        *self.shared.ended.lock() = Some(tx);
        rx
    }

    /// Let the agent whose turn or move `ended` take work again. Returns
    /// the agent, and how its turn went if it was one.
    pub(super) fn free(&self, ended: Ended) -> (AgentId, Option<TurnOutcome>) {
        match ended {
            Ended::Turn(agent, outcome) => {
                self.shared.running.lock().remove(&agent);
                (agent, outcome)
            }
            Ended::Move(agent) => {
                self.shared.moving.lock().remove(&agent);
                self.shared.publish_now();
                (agent, None)
            }
        }
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
    attach_app(state, &hub);
    let keep: HashSet<String> = lock(&state.hub.registry)
        .agents()
        .iter()
        .filter(|agent| !agent.archived && !agent.removed)
        .map(|agent| agent.agent_id().to_hex())
        .collect();
    hub.start_reported(&keep, boot).await;
    // Reaches live on the hub only: what members mail goes through the
    // inbox from the start.
    crate::event_loop::inbox::refresh_reach(&state.hub, &hub).await;
    *state.cloud.shared.hub.lock() = Some(Arc::clone(&hub));
    state
        .processes
        .set_remote(Arc::new(MachineRows(Arc::downgrade(&hub))));
    follow::follow_unfinished(state, &hub).await;
    start_publisher(state);
    Ok(hub)
}

/// Give a new `hub` what it shares with the app: the agent host, the
/// inbox members' requests from their machines are filed in, the session
/// checkpoints, the record observer, the trace store, the current owner
/// and the users' plan credentials.
fn attach_app(state: &LoopState<'_>, hub: &Arc<CloudHub>) {
    if let Some(agents) = &state.hub.agents {
        hub.attach_host(agents.host());
    }
    hub.attach_desk(Arc::new(crate::event_loop::inbox::MachineMail(
        Arc::downgrade(&state.hub),
    )));
    if let Some(checkpoints) = &state.hub.factory.checkpoints {
        hub.attach_sessions(checkpoints.clone());
    }
    if let Some(observer) = &state.hub.factory.record_observer {
        hub.attach_observer(Arc::clone(observer));
    }
    if let Some(traces) = state.hub.traces.store() {
        hub.attach_traces(traces);
    }
    hub.set_current_owner(state.auth.owner());
    hub.set_plan_credentials(Arc::new(crate::cloud::plans::VaultPlans(Arc::clone(
        &state.auth,
    ))));
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

/// Why `agent` takes no work, `/new` or move from here: it was spawned
/// on cloud agent `root`'s machine, whose node writes its record.
fn spawned_on_machine(state: &LoopState<'_>, agent: AgentId, root: AgentId) -> String {
    let registry = lock(&state.hub.registry);
    let short = |id: AgentId| {
        registry.find(id).map_or_else(
            || id.to_hex()[..8].to_string(),
            |found| found.code.clone().unwrap_or_else(|| found.name.clone()),
        )
    };
    let (name, root) = (short(agent), short(root));
    format!(
        "{name} runs on {root}'s cloud machine: ask {root} to continue it, or bring {root} back (/local) to continue it here."
    )
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
