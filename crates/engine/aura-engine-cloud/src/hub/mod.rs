//! The cloud hub: this app's side of every cloud agent.
//!
//! The hub owns the machine fleet, the snapshot builder, the record
//! mirror and the mesh sockets. It moves an agent's single writer to a
//! cloud machine ([`CloudHub::place`]) and back ([`CloudHub::recall`]),
//! drives a cloud agent's turns over the node's wire API
//! ([`CloudHub::run_turn`]) and carries mail between the local
//! [`AgentHost`] and the nodes.
//!
//! ## Invariants
//!
//! - **One writer per agent.** An agent is placed only while the event
//!   loop holds it idle; the local kernel does not write for a placed
//!   agent, and a recalled agent is sealed on its node before the local
//!   kernel writes again. The [`aura_core_types::SystemKind::Placement`]
//!   row marks each move.
//! - **The local store is the main copy.** Every page a node commits is
//!   checked ([`crate::replica::verify_page`]) and appended
//!   verbatim; a bad page stops the mirror for that agent and is never
//!   written. Agents it spawned on its machine are copied too, but only
//!   ones named by a `spawn_agent` row of a record already verified.
//! - **Nothing that matters stays only on a machine.** The session
//!   checkpoint, the trace runs and, before each sleep, a patch of the
//!   machine's changes are copied here too; [`CloudHub::sync_status`]
//!   says how current each copy is.
//! - **The boxd key never leaves the hub.** Machines get the env
//!   allowlist of [`crate::machine_env`] and their tokens over
//!   boxd's authenticated `Exec`.
//!
//! ## Failure modes
//!
//! - A node that can't be reached keeps its agent placed; turns fail with
//!   the node's error and the HOST tab shows it. Recall retries on the
//!   next attempt.
//! - A patch that doesn't apply is saved under
//!   `AURA_HOME/cloud/<agent>/` and reported, never dropped.
//! - A machine deleted outside aura: Bring back applies the patch saved
//!   at its last sleep; work done since that sleep is lost.
//!
//! ## Submodules
//!
//! - [`store`]: the machine ledger writer and the record mirror's sink
//!   over the main store.
//! - [`plans`]: users' plan credentials from the vault, for their
//!   machines' boxd plan secrets.
//! - [`node_client`]: HTTP calls to one cloud node.
//! - [`setup`]: `aura cloud setup`, which puts a boxd API key in
//!   `AURA_HOME/.env`.

mod busy;
mod lifecycle;
mod links;
mod machine_files;
mod machine_work;
mod machines;
mod mirror;
pub mod node_client;
mod outcomes;
mod persist;
mod place;
pub mod plans;
mod pull;
mod reach;
mod recall;
mod request;
mod seed;
mod session;
pub mod setup;
mod state;
pub mod store;
#[cfg(feature = "test-support")]
pub mod test_support;
mod turn;

pub use self::outcomes::{CommitsOutcome, PatchOutcome, PullOutcome};
pub use self::persist::SyncStatus;
pub use self::reach::{MailDesk, Reach};
pub use self::request::{cloud_policy, TurnAsk};
pub use self::turn::{map_outbound, RemoteEvent};

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use crate::cost::{CostModel, CostSample};
use crate::fleet::VmFleet;
use crate::github::{BotIdentity, RepoRef};
use crate::mesh::{Directory, MailBook};
use crate::plans::PlanSync;
use crate::repos::RepoSync;
use crate::snapshots::{AuraBinary, SnapshotBuilder};
use crate::CloudError;
use aura_agent::checkpoint::Checkpoints;
use aura_agent_kernel::RecordObserver;
use aura_config::CloudConfig;
pub use aura_core_auth::Owner;
use aura_core_protocol::cloud::{HostStats, MeshFrame};
use aura_core_types::{AgentId, Placement};
use aura_engine::AgentHost;
use aura_store_db::Store;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use self::store::{agent_id_of, StoreLedger};

/// How often idle machines are checked for sleep.
const TICK_EVERY: Duration = Duration::from_secs(5);
/// How often every machine is re-read from boxd.
const POLL_EVERY: Duration = Duration::from_secs(30);
/// How often meters are read.
const USAGE_EVERY: Duration = Duration::from_secs(300);
/// How often a running agent's record is mirrored while a turn runs.
const MIRROR_EVERY: Duration = Duration::from_secs(5);
/// How often every awake machine's record is copied, for the hints a
/// dropped socket lost.
const SWEEP_EVERY: Duration = Duration::from_secs(60);
/// Limit on the copies made before a machine sleeps (records, session,
/// traces and the patch of its changes).
const COPY_BEFORE_SLEEP: Duration = Duration::from_secs(90);
/// Cost samples kept per machine.
const MAX_SAMPLES: usize = 720;
/// Longest a turn may run on a node before the hub gives up on it.
const TURN_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
/// Limit on applying the project's changes on a new machine.
const APPLY_TIMEOUT: Duration = Duration::from_secs(120);
/// Limit on the short git scripts run on a machine (branching, marking a
/// pull, bundling its branch).
const GIT_SCRIPT_TIMEOUT: Duration = Duration::from_secs(120);
/// Limit on git work in the project folder here (applying a patch,
/// fetching an agent's branch).
const LOCAL_GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// `commit` as shown: its first 12 characters.
fn short_commit(commit: &str) -> String {
    commit.chars().take(12).collect()
}

/// What the hub needs to move an agent to a cloud machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceRequest {
    /// The agent.
    pub agent: AgentId,
    /// Its ZNS id.
    pub agent_zns: String,
    /// Its display name.
    pub name: String,
    /// Its code (`RE36`), for the machine name.
    pub code: Option<String>,
    /// Project id.
    pub project_id: String,
    /// Project folder on this machine.
    pub project_dir: PathBuf,
    /// Build command for the project snapshot; detected when `None`.
    pub build_command: Option<String>,
    /// A cloud parent whose machine to fork instead of restoring the
    /// project snapshot. Only forked when the parent has the same owner.
    pub fork_from: Option<AgentId>,
    /// The user placing it, who owns it from now on: its machine spends
    /// this user's plans, and only this user can manage it.
    pub owner: Owner,
    /// That user's display name, for "placed by" notices here.
    pub owner_name: Option<String>,
}

/// What a new machine gets of its project's GitHub repository.
#[derive(Default)]
struct RepoShare {
    /// The repository, when origin is on GitHub.
    repo: Option<RepoRef>,
    /// Its secret's name, once boxd holds it.
    secret: Option<String>,
    /// The App's bot, who the agent commits as.
    identity: Option<BotIdentity>,
}

/// The project here, packed for a new machine.
struct PackedProject {
    /// Its HEAD.
    head: String,
    /// The commits made since the snapshot, as a git bundle.
    bundle: Option<Vec<u8>>,
    /// How many files differ from HEAD: they stay on the hub.
    changed: usize,
}

/// Where a placed agent lives, kept in `AURA_HOME/cloud/placements.json`
/// so a restarted hub takes its machines back.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlacedAgent {
    /// Agent ZNS id.
    pub zns: String,
    /// Display name.
    pub name: String,
    /// Project id.
    pub project: String,
    /// Project folder on this machine.
    pub project_dir: PathBuf,
    /// Machine name.
    pub machine: String,
    /// The user who placed it (kept on this machine only; boxd sees the
    /// owner tag). `None` in files from before owners: the local user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<Owner>,
    /// That user's display name, for notices here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_name: Option<String>,
    /// The agent's branch on its machine, and here once its commits come
    /// back. `None` in files from before agent branches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// The project's GitHub repository, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<RepoRef>,
    /// The branch's tip last fetched here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fetched_tip: Option<String>,
    /// Agents it spawned on its machine (hex ids), named in its record
    /// or a spawned agent's; their records are copied with its own.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
}

impl PlacedAgent {
    /// The user who placed it.
    #[must_use]
    pub fn owner(&self) -> Owner {
        self.owner.clone().unwrap_or(Owner::Local)
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct PlacementFile {
    /// Placed agents by hex id.
    #[serde(default)]
    agents: BTreeMap<String, PlacedAgent>,
    /// The last placement chosen in each project, for new agents.
    #[serde(default)]
    projects: BTreeMap<String, Placement>,
    /// This install's id, the label every machine carries.
    #[serde(default)]
    install: String,
}

/// The latest host sample of a machine and the samples cost is
/// estimated from.
#[derive(Debug, Clone, Default)]
struct HostTrack {
    latest: Option<HostStats>,
    samples: Vec<CostSample>,
}

/// One open mesh socket.
struct Link {
    frames: mpsc::UnboundedSender<MeshFrame>,
    stop: CancellationToken,
}

/// This app's side of every cloud agent.
pub struct CloudHub {
    config: CloudConfig,
    store: Arc<dyn Store>,
    ledger: Arc<StoreLedger>,
    fleet: Arc<VmFleet>,
    snapshots: SnapshotBuilder,
    costs: CostModel,
    directory: Directory,
    mail: MailBook,
    home: PathBuf,
    placements: Mutex<PlacementFile>,
    host: Mutex<Option<AgentHost>>,
    /// Where requests to the agents placed agents reach are filed.
    desk: Mutex<Option<Arc<dyn MailDesk>>>,
    /// The agents each placed agent (hex) reaches through the hub.
    reach: Mutex<HashMap<String, Vec<Reach>>>,
    links: Mutex<HashMap<String, Link>>,
    tracks: Mutex<HashMap<String, HostTrack>>,
    /// One copy per agent at a time: its record, session or runs.
    mirrors: Mutex<HashMap<AgentId, Arc<tokio::sync::Mutex<()>>>>,
    /// Agents with a background copy running, and whether another hint
    /// came in while it ran ([`Self::copy_soon`]).
    copies: Mutex<HashMap<AgentId, bool>>,
    /// Sees every entry copied here, as it sees what local kernels write.
    observer: Mutex<Option<Arc<dyn RecordObserver>>>,
    /// Set when a copy brought entries, until the app looks
    /// ([`Self::take_copied`]).
    copied: AtomicBool,
    /// The main trace store, where cloud agents' runs are copied.
    trace_store: Mutex<Option<Arc<aura_store_db::TraceStore>>>,
    /// What each copied run held when stored, by run id.
    run_marks: Mutex<HashMap<String, persist::RunMark>>,
    /// The agents whose trace runs were copied since the app last looked
    /// ([`Self::take_traces_copied`]).
    traces_copied: Mutex<HashSet<AgentId>>,
    /// How current each placed agent's copy is.
    sync: Mutex<HashMap<AgentId, SyncStatus>>,
    /// Users' plan tokens as boxd secrets; `None` until the app hands
    /// the hub its credentials.
    plans: Mutex<Option<Arc<PlanSync>>>,
    /// Projects' GitHub tokens as boxd secrets.
    repos: Mutex<Arc<RepoSync>>,
    /// The app's session checkpoints: a placed agent's node continues
    /// the conversation its local session had.
    sessions: Mutex<Option<Checkpoints>>,
    /// The user signed in here: local agents act as this user.
    current_owner: Mutex<Owner>,
    /// What each link's node has under way for its agent, by agent hex.
    work: Mutex<HashMap<String, machine_work::MachineWork>>,
    /// The chat run each agent's turn is streamed from, by agent hex.
    streaming: Mutex<HashMap<String, String>>,
    stop: CancellationToken,
}

impl std::fmt::Debug for CloudHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudHub")
            .field("home", &self.home)
            .finish_non_exhaustive()
    }
}

fn io(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> CloudError {
    let context = context.into();
    move |err| CloudError::io(context, err)
}

/// The aura binary the base snapshot gets: `AURA_CLOUD_BINARY`, else
/// `<home>/bin/aura` when it exists, else the running executable.
fn choose_binary(env: Option<&Path>, home: &Path, current_exe: Option<PathBuf>) -> AuraBinary {
    if let Some(path) = env {
        tracing::info!(path = %path.display(), "cloud aura binary from AURA_CLOUD_BINARY");
        return AuraBinary::Upload(path.to_path_buf());
    }
    let built = crate::snapshots::cloud_binary_path(home);
    if built.is_file() {
        tracing::info!(path = %built.display(), "cloud aura binary from the cloud home");
        return AuraBinary::Upload(built);
    }
    match current_exe {
        Some(path) => {
            tracing::info!(path = %path.display(), "cloud aura binary is the running executable");
            AuraBinary::Upload(path)
        }
        None => AuraBinary::Installed,
    }
}

fn load_placements(path: &Path) -> PlacementFile {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

/// The agent id a ZNS id has; re-exported for callers of the hub.
#[must_use]
pub fn zns_agent(zns: &str) -> AgentId {
    agent_id_of(zns)
}

#[cfg(test)]
mod tests;
