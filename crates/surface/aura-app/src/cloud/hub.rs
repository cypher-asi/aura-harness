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
//!   checked ([`aura_engine_cloud::replica::verify_page`]) and appended
//!   verbatim; a bad page stops the mirror for that agent and is never
//!   written.
//! - **The boxd key never leaves the hub.** Machines get the env
//!   allowlist of [`aura_engine_cloud::machine_env`] and their tokens over
//!   boxd's authenticated `Exec`.
//!
//! ## Failure modes
//!
//! - A node that can't be reached keeps its agent placed; turns fail with
//!   the node's error and the HOST tab shows it. Recall retries on the
//!   next attempt.
//! - A patch that doesn't apply is saved under
//!   `AURA_HOME/cloud/<agent>/` and reported, never dropped.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};
use std::time::Duration;

use aura_config::{BoxdToken, CloudConfig};
use aura_core_protocol::cloud::{
    HostStats, MeshFrame, MeshMail, MeshMailKind, SyncImport, MESH_PROTOCOL,
};
use aura_core_protocol::{
    AgentCapabilities, AgentIdentity, AgentPermissionsWire, InboundMessage, ModelSelection,
    OutboundMessage, RuntimeRequest, RuntimeRequestType, UserMessage, WorkspaceLocation,
};
use aura_core_types::{AgentId, Placement, PlacementPayload, VmPayload};
use aura_engine::{AgentHost, MessageKind, RemoteMail, RemoteRouter, ThreadId};
use aura_engine_cloud::cost::{latest_reading, CostEstimate, CostModel, CostSample};
use aura_engine_cloud::fleet::{MachineView, ProvisionRequest, VmFleet};
use aura_engine_cloud::machine_env::WORKSPACE_DIR;
use aura_engine_cloud::mesh::{global_thread, Directory, Holder, MailBook, HUB_NODE};
use aura_engine_cloud::snapshots::{AuraBinary, SnapshotBuilder};
use aura_engine_cloud::{
    delta, names, replica, CloudError, CloudResult, MachineOrigin, VmProvider,
};
use aura_store_db::Store;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::node_client::NodeClient;
use super::store::{agent_id_of, record_placement, StoreLedger, StoreSink};

/// How often idle machines are checked for sleep.
const TICK_EVERY: Duration = Duration::from_secs(5);
/// How often every machine is re-read from boxd.
const POLL_EVERY: Duration = Duration::from_secs(30);
/// How often meters are read.
const USAGE_EVERY: Duration = Duration::from_secs(300);
/// How often a running agent's record is mirrored while a turn runs.
const MIRROR_EVERY: Duration = Duration::from_secs(5);
/// Cost samples kept per machine.
const MAX_SAMPLES: usize = 720;
/// Longest a turn may run on a node before the hub gives up on it.
const TURN_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
/// Where an uploaded delta goes (see [`delta::APPLY_DELTA`]).
const DELTA_PATH: &str = "/tmp/aura-delta.tgz";

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
    /// project snapshot.
    pub fork_from: Option<AgentId>,
}

/// How pulling an agent's work back went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchOutcome {
    /// The machine changed nothing.
    Empty,
    /// The patch applied to the project folder.
    Applied {
        /// Files it touched.
        files: Vec<String>,
    },
    /// The patch did not apply; it is saved here.
    Saved {
        /// The saved patch.
        path: PathBuf,
        /// Why it did not apply.
        reason: String,
    },
}

impl PatchOutcome {
    /// A one-line report for the chat.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::Empty => "no changes to pull".to_string(),
            Self::Applied { files } => format!("pulled {} changed file(s)", files.len()),
            Self::Saved { path, reason } => {
                format!(
                    "changes did not apply ({reason}); saved to {}",
                    path.display()
                )
            }
        }
    }
}

/// What a cloud agent's turn shows, as it streams.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteEvent {
    /// The machine is waking or the run is starting.
    Status(String),
    /// Reply text.
    Text(String),
    /// Thinking text.
    Thinking(String),
    /// A tool call started.
    ToolStart {
        /// Tool use id.
        id: String,
        /// Tool name.
        name: String,
    },
    /// A tool call's input so far.
    ToolInput {
        /// Tool use id.
        id: String,
        /// Input JSON.
        input: String,
    },
    /// A tool call finished.
    ToolResult {
        /// Tool use id.
        id: String,
        /// Result text.
        result: String,
        /// Whether it failed.
        is_error: bool,
    },
    /// The turn ended.
    Done {
        /// Model used.
        model: String,
        /// Input tokens of the turn.
        input_tokens: u64,
        /// Output tokens of the turn.
        output_tokens: u64,
    },
    /// The turn failed.
    Failed(String),
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
    links: Mutex<HashMap<String, Link>>,
    tracks: Mutex<HashMap<String, HostTrack>>,
    mirrors: Mutex<HashMap<AgentId, Arc<tokio::sync::Mutex<()>>>>,
    stop: CancellationToken,
}

impl std::fmt::Debug for CloudHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudHub")
            .field("home", &self.home)
            .finish_non_exhaustive()
    }
}

fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

fn io(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> CloudError {
    let context = context.into();
    move |err| CloudError::io(context, err)
}

fn load_placements(path: &Path) -> PlacementFile {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

impl CloudHub {
    /// A hub over `provider`, keeping its files in `home`
    /// (`AURA_HOME/cloud`).
    ///
    /// # Errors
    ///
    /// When `home` can't be created.
    pub fn new(
        config: CloudConfig,
        store: Arc<dyn Store>,
        home: PathBuf,
        provider: Arc<dyn VmProvider>,
        binary: AuraBinary,
    ) -> CloudResult<Arc<Self>> {
        std::fs::create_dir_all(&home).map_err(io(format!("create {}", home.display())))?;
        let placements_path = home.join("placements.json");
        let mut placements = load_placements(&placements_path);
        if placements.install.is_empty() {
            placements.install = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        }
        let ledger = Arc::new(StoreLedger::new(Arc::clone(&store)));
        let settings = aura_engine_cloud::fleet::FleetSettings::from_config(
            &config,
            placements.install.clone(),
        );
        let fleet = Arc::new(VmFleet::new(
            Arc::clone(&provider),
            Arc::clone(&ledger) as Arc<dyn aura_engine_cloud::fleet::VmLedger>,
            settings,
        ));
        let snapshots = SnapshotBuilder::new(
            provider,
            Arc::clone(&ledger) as Arc<dyn aura_engine_cloud::fleet::VmLedger>,
            placements.install.clone(),
            config.size,
            config.isolated,
            env!("CARGO_PKG_VERSION"),
            binary,
            home.join("snapshots.json"),
        );
        let hub = Arc::new(Self {
            costs: CostModel::from_config(&config),
            config,
            store,
            ledger,
            fleet,
            snapshots,
            directory: Directory::new(),
            mail: MailBook::new(),
            home,
            placements: Mutex::new(placements),
            host: Mutex::new(None),
            links: Mutex::new(HashMap::new()),
            tracks: Mutex::new(HashMap::new()),
            mirrors: Mutex::new(HashMap::new()),
            stop: CancellationToken::new(),
        });
        hub.save_placements();
        Ok(hub)
    }

    /// A hub over boxd itself, from the cloud settings.
    ///
    /// # Errors
    ///
    /// [`CloudError::NotConfigured`] without a key; otherwise as
    /// [`Self::new`].
    pub fn from_config(
        config: CloudConfig,
        store: Arc<dyn Store>,
        home: PathBuf,
    ) -> CloudResult<Arc<Self>> {
        let provider = Arc::new(aura_engine_cloud::BoxdProvider::from_config(&config)?);
        let binary = std::env::current_exe()
            .ok()
            .filter(|_| cfg!(target_os = "linux"))
            .map_or(AuraBinary::Installed, AuraBinary::Upload);
        Self::new(config, store, home, provider, binary)
    }

    /// The settings the hub runs with.
    #[must_use]
    pub const fn config(&self) -> &CloudConfig {
        &self.config
    }

    /// The fleet, for the HOST tab.
    #[must_use]
    pub fn fleet(&self) -> &Arc<VmFleet> {
        &self.fleet
    }

    fn save_placements(&self) {
        let path = self.home.join("placements.json");
        let bytes = serde_json::to_vec_pretty(&*self.placements.lock()).unwrap_or_default();
        let tmp = path.with_extension("json.tmp");
        if let Err(err) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, &path)) {
            tracing::warn!(path = %path.display(), error = %err, "could not save cloud placements");
        }
    }

    /// Hand mail from nodes to `host`'s agents, and send mail for cloud
    /// agents out through this hub.
    pub fn attach_host(self: &Arc<Self>, host: &AgentHost) {
        *self.host.lock() = Some(host.clone());
        host.set_remote_router(Arc::new(HubRouter(Arc::downgrade(self))));
    }

    /// Note that `agent` (hex) of `project`, named `name`, runs here, so
    /// cloud agents can mail it.
    pub fn note_local(&self, agent: AgentId, name: &str, project: &str) {
        self.directory.place(
            &agent.to_hex(),
            Holder {
                node: HUB_NODE.to_string(),
                url: None,
                project: project.to_string(),
                name: name.to_string(),
            },
        );
        self.send_directories(project);
    }

    /// Whether `agent` runs on a cloud machine.
    #[must_use]
    pub fn is_placed(&self, agent: AgentId) -> bool {
        self.placements.lock().agents.contains_key(&agent.to_hex())
    }

    /// Where `agent` runs.
    #[must_use]
    pub fn placement_of(&self, agent: AgentId) -> Placement {
        if self.is_placed(agent) {
            Placement::Cloud
        } else {
            Placement::Local
        }
    }

    /// The placement new agents of `project` get.
    #[must_use]
    pub fn project_default(&self, project: &str) -> Placement {
        self.placements
            .lock()
            .projects
            .get(project)
            .copied()
            .unwrap_or(Placement::Local)
    }

    /// Remember `placement` as `project`'s choice for new agents.
    pub fn set_project_default(&self, project: &str, placement: Placement) {
        self.placements
            .lock()
            .projects
            .insert(project.to_string(), placement);
        self.save_placements();
    }

    /// Every placed agent.
    #[must_use]
    pub fn placed(&self) -> Vec<(String, PlacedAgent)> {
        self.placements
            .lock()
            .agents
            .iter()
            .map(|(hex, placed)| (hex.clone(), placed.clone()))
            .collect()
    }

    /// Take back the machines placed agents had before this hub started,
    /// destroy machines no kept agent owns, and start the fleet's
    /// timers. `keep` are the agents that still exist (hex).
    pub async fn start(self: &Arc<Self>, keep: &HashSet<String>) {
        let placed = self.placed();
        for (hex, agent) in &placed {
            if !keep.contains(hex) {
                continue;
            }
            match self
                .fleet
                .adopt(hex, &agent.zns, &agent.project, &agent.machine)
                .await
            {
                Ok(view) => self.place_in_directory(hex, agent, &view),
                Err(err) => {
                    tracing::warn!(agent = %hex, machine = %agent.machine, error = %err, "cloud machine is gone; the agent runs here again");
                    self.placements.lock().agents.remove(hex);
                }
            }
        }
        self.save_placements();
        let owned: HashSet<String> = placed
            .iter()
            .filter(|(hex, _)| keep.contains(hex))
            .map(|(hex, _)| hex.clone())
            .collect();
        match self.fleet.sweep_orphans(&owned).await {
            Ok(swept) if !swept.is_empty() => {
                tracing::info!(machines = ?swept, "destroyed cloud machines no agent owns");
            }
            Ok(_) => {}
            Err(err) => tracing::warn!(error = %err, "could not list cloud machines"),
        }
        for (hex, _) in self.placed() {
            self.open_link(&hex);
        }
        let hub = Arc::downgrade(self);
        let stop = self.stop.clone();
        tokio::spawn(async move { maintain(hub, stop).await });
    }

    /// Stop the timers and close every socket. Machines keep running and
    /// sleep on boxd's own idle timer.
    pub fn shutdown(&self) {
        self.stop.cancel();
        for (_, link) in self.links.lock().drain() {
            link.stop.cancel();
        }
    }

    fn place_in_directory(&self, hex: &str, agent: &PlacedAgent, view: &MachineView) {
        self.directory.place(
            hex,
            Holder {
                node: view.machine.name.clone(),
                url: Some(view.machine.url()),
                project: agent.project.clone(),
                name: agent.name.clone(),
            },
        );
        self.send_directories(&agent.project);
    }

    fn node(&self, agent: &str) -> CloudResult<NodeClient> {
        let view = self.fleet.view(agent).ok_or_else(|| CloudError::NotFound {
            what: "machine",
            name: agent.to_string(),
        })?;
        let token = self
            .fleet
            .node_token(agent)
            .ok_or_else(|| CloudError::BadState {
                machine: view.machine.name.clone(),
                state: view.state.label().to_string(),
                op: "call its node before it is claimed",
            })?;
        Ok(NodeClient::new(
            view.machine.name.clone(),
            view.machine.url(),
            token,
        ))
    }

    async fn awake_node(&self, agent: &str) -> CloudResult<NodeClient> {
        self.fleet.ensure_awake(agent).await?;
        self.node(agent)
    }

    /// Move `request.agent`'s single writer to a cloud machine. The
    /// caller must hold the agent idle until this returns.
    ///
    /// # Errors
    ///
    /// When the snapshot, the machine or the record import fails; the
    /// agent then stays here and any machine made for it is destroyed.
    pub async fn place(self: &Arc<Self>, request: PlaceRequest) -> CloudResult<MachineView> {
        let hex = request.agent.to_hex();
        if self.is_placed(request.agent) {
            return self.fleet.view(&hex).ok_or_else(|| CloudError::NotFound {
                what: "machine",
                name: hex.clone(),
            });
        }
        let parent = request
            .fork_from
            .map(|parent| parent.to_hex())
            .filter(|parent| self.config.fork_on_spawn && self.fleet.has(parent));
        let (origin, base_commit, seal) = match parent.as_deref().and_then(|p| self.fleet.view(p)) {
            Some(parent_view) => (
                MachineOrigin::Fork(parent_view.machine.name.clone()),
                None,
                vec![parent_view.agent.clone()],
            ),
            None => {
                let mut record = self
                    .snapshots
                    .ensure_project(
                        &request.project_id,
                        &request.project_dir,
                        request.build_command.as_deref(),
                        false,
                    )
                    .await?;
                // The delta is taken against the snapshot's commit; when
                // this repository no longer has it, rebuild from HEAD.
                let missing = match record.commit.clone() {
                    Some(commit) => {
                        let dir = request.project_dir.clone();
                        !tokio::task::spawn_blocking(move || delta::has_commit(&dir, &commit))
                            .await
                            .unwrap_or(false)
                    }
                    None => false,
                };
                if missing {
                    tracing::info!(project = %request.project_id, "the project snapshot's commit is gone here; rebuilding it");
                    record = self
                        .snapshots
                        .ensure_project(
                            &request.project_id,
                            &request.project_dir,
                            request.build_command.as_deref(),
                            true,
                        )
                        .await?;
                }
                (
                    MachineOrigin::Snapshot(record.name),
                    record.commit,
                    Vec::new(),
                )
            }
        };
        let view = self
            .fleet
            .provision(ProvisionRequest {
                agent: hex.clone(),
                agent_zns: request.agent_zns.clone(),
                code: request.code.clone(),
                project: request.project_id.clone(),
                name: names::machine_name(request.code.as_deref(), &request.agent),
                origin,
                seal,
            })
            .await?;
        if let Err(err) = self.seed(&request, &view, base_commit.as_deref()).await {
            tracing::warn!(agent = %hex, error = %err, "could not move the agent to its machine; destroying it");
            let _ = self.fleet.destroy(&hex, "placement failed").await;
            return Err(err);
        }
        let placed = PlacedAgent {
            zns: request.agent_zns.clone(),
            name: request.name.clone(),
            project: request.project_id.clone(),
            project_dir: request.project_dir.clone(),
            machine: view.machine.name.clone(),
        };
        record_placement(
            &self.ledger,
            &names::machine_ledger(&request.agent_zns),
            &PlacementPayload::new(
                Placement::Local,
                Placement::Cloud,
                Some(view.machine.name.clone()),
            ),
        )?;
        self.placements
            .lock()
            .agents
            .insert(hex.clone(), placed.clone());
        self.save_placements();
        self.place_in_directory(&hex, &placed, &view);
        self.open_link(&hex);
        Ok(self.fleet.view(&hex).unwrap_or(view))
    }

    /// Bring the project files up to date on the machine and give its
    /// node the agent's record.
    async fn seed(
        &self,
        request: &PlaceRequest,
        view: &MachineView,
        base_commit: Option<&str>,
    ) -> CloudResult<()> {
        let provider = self.fleet.provider();
        if let Some(base) = base_commit {
            let dir = request.project_dir.clone();
            let base = base.to_string();
            let packed = tokio::task::spawn_blocking(move || -> CloudResult<Option<Vec<u8>>> {
                let changes = delta::collect(&dir, &base)?;
                if changes.is_empty() {
                    return Ok(None);
                }
                delta::pack(&dir, &changes).map(Some)
            })
            .await
            .map_err(|err| {
                CloudError::io("pack changes", std::io::Error::other(err.to_string()))
            })??;
            if let Some(bytes) = packed {
                provider
                    .upload(&view.machine.name, DELTA_PATH, bytes)
                    .await?;
                let out = provider
                    .exec(
                        &view.machine.name,
                        delta::APPLY_DELTA,
                        None,
                        Duration::from_secs(120),
                    )
                    .await?;
                if !out.success() {
                    return Err(CloudError::ExecFailed {
                        machine: view.machine.name.clone(),
                        command: "apply changes".into(),
                        exit_code: out.exit_code,
                        stderr: out.stderr_tail(400),
                    });
                }
            }
        }
        let node = self.node(&view.agent)?;
        let head = self
            .store
            .get_head_seq(request.agent)
            .map_err(|e| CloudError::io("read record", std::io::Error::other(e.to_string())))?;
        let remote = node.head(request.agent).await.unwrap_or(0);
        let mut from = remote + 1;
        loop {
            let entries = if from > head {
                Vec::new()
            } else {
                self.store
                    .scan_record(request.agent, from, super::node_client::PAGE)
                    .map_err(|e| {
                        CloudError::io("read record", std::io::Error::other(e.to_string()))
                    })?
            };
            let count = entries.len() as u64;
            let done = from + count > head;
            node.import(&SyncImport {
                agent_id: request.agent.to_hex(),
                name: request.name.clone(),
                entries,
                done,
            })
            .await?;
            if done {
                return Ok(());
            }
            from += count;
        }
    }

    /// Copy what `agent`'s node committed since the local head. Returns
    /// the entries copied.
    ///
    /// # Errors
    ///
    /// When the node can't be read or a page fails its checks.
    pub async fn mirror(&self, agent: AgentId) -> CloudResult<usize> {
        let node = self.node(&agent.to_hex())?;
        self.mirror_from(&node, agent).await
    }

    async fn mirror_from(&self, node: &NodeClient, agent: AgentId) -> CloudResult<usize> {
        // One mirror per agent at a time: two racing copies of the same
        // page would have the loser refused by the store.
        let gate = Arc::clone(self.mirrors.lock().entry(agent).or_default());
        let _one = gate.lock().await;
        let sink = StoreSink(Arc::clone(&self.store));
        let mut copied = 0;
        loop {
            let head = replica::ReplicaSink::head(&sink, agent)?;
            let page = node.record(agent, head + 1).await?;
            if page.is_empty() {
                return Ok(copied);
            }
            let applied = replica::apply_page(&sink, agent, &page)?;
            copied += applied;
            if applied == 0 {
                return Ok(copied);
            }
        }
    }

    /// Bring `agent`'s single writer back here: seal it on its node,
    /// copy the rest of its record, pull its work into the project and
    /// destroy the machine.
    ///
    /// # Errors
    ///
    /// When the node can't be sealed or its record can't be copied; the
    /// agent then stays placed.
    pub async fn recall(&self, agent: AgentId, reason: &str) -> CloudResult<PatchOutcome> {
        let hex = agent.to_hex();
        let Some(placed) = self.placements.lock().agents.get(&hex).cloned() else {
            return Ok(PatchOutcome::Empty);
        };
        if self.fleet.view(&hex).is_none() {
            // The machine is gone (deleted outside aura): the record the
            // mirror already holds is all there is.
            self.forget_placement(&hex, &placed.project);
            return Ok(PatchOutcome::Saved {
                path: self.home.join(&hex),
                reason: format!(
                    "machine {} no longer exists; its unsynced work is lost",
                    placed.machine
                ),
            });
        }
        let node = self.awake_node(&hex).await?;
        let sealed = node.seal(agent).await?;
        self.mirror_from(&node, agent).await?;
        let local = self
            .store
            .get_head_seq(agent)
            .map_err(|e| CloudError::io("read record", std::io::Error::other(e.to_string())))?;
        if local < sealed.head_seq {
            return Err(CloudError::Node {
                node: node.node().to_string(),
                op: "recall",
                message: format!("copied up to {local} of {} entries", sealed.head_seq),
            });
        }
        let outcome = match node.patch().await {
            Ok(patch) => {
                self.apply_patch(&hex, &placed.project_dir, &patch.patch, patch.files)
                    .await
            }
            Err(err) => PatchOutcome::Saved {
                path: self.home.join(&hex),
                reason: format!("could not read the machine's changes: {err}"),
            },
        };
        record_placement(
            &self.ledger,
            &names::machine_ledger(&placed.zns),
            &PlacementPayload::new(
                Placement::Cloud,
                Placement::Local,
                Some(placed.machine.clone()),
            ),
        )?;
        self.forget_placement(&hex, &placed.project);
        match self.fleet.destroy(&hex, reason).await {
            Ok(Some(id)) => {
                let fleet = Arc::clone(&self.fleet);
                let ledger = names::machine_ledger(&placed.zns);
                let machine = placed.machine.clone();
                tokio::spawn(async move {
                    fleet
                        .settle_usage(&id, &machine, &ledger, 6, Duration::from_secs(300))
                        .await;
                });
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(agent = %hex, error = %err, "could not destroy the machine; the orphan sweep will");
            }
        }
        Ok(outcome)
    }

    fn forget_placement(&self, hex: &str, project: &str) {
        self.placements.lock().agents.remove(hex);
        self.save_placements();
        self.directory.remove(hex);
        self.send_directories(project);
        if let Some(link) = self.links.lock().remove(hex) {
            link.stop.cancel();
        }
        self.tracks.lock().remove(hex);
    }

    /// Pull `agent`'s work into the project without moving the agent.
    ///
    /// # Errors
    ///
    /// When the node can't be reached.
    pub async fn pull(&self, agent: AgentId) -> CloudResult<PatchOutcome> {
        let hex = agent.to_hex();
        let Some(placed) = self.placements.lock().agents.get(&hex).cloned() else {
            return Ok(PatchOutcome::Empty);
        };
        let node = self.awake_node(&hex).await?;
        let patch = node.patch().await?;
        let outcome = self
            .apply_patch(&hex, &placed.project_dir, &patch.patch, patch.files)
            .await;
        if matches!(outcome, PatchOutcome::Applied { .. }) {
            // The machine keeps working; the next pull starts from here.
            if let Some(view) = self.fleet.view(&hex) {
                let marked = self
                    .fleet
                    .provider()
                    .exec(
                        &view.machine.name,
                        delta::MARK_PULLED,
                        None,
                        Duration::from_secs(60),
                    )
                    .await;
                if let Err(err) = marked {
                    tracing::warn!(agent = %hex, error = %err, "could not mark the pulled work on the machine; the next pull repeats it");
                }
            }
        }
        Ok(outcome)
    }

    async fn apply_patch(
        &self,
        hex: &str,
        project: &Path,
        patch: &str,
        files: Vec<String>,
    ) -> PatchOutcome {
        if patch.trim().is_empty() {
            return PatchOutcome::Empty;
        }
        let dir = self.home.join(hex);
        let path = dir.join(format!("changes-{}.patch", now_ms()));
        if let Err(err) = tokio::fs::create_dir_all(&dir).await {
            return PatchOutcome::Saved {
                path,
                reason: format!("could not save it: {err}"),
            };
        }
        if let Err(err) = tokio::fs::write(&path, patch).await {
            return PatchOutcome::Saved {
                path,
                reason: format!("could not save it: {err}"),
            };
        }
        let applied = tokio::process::Command::new("git")
            .arg("-C")
            .arg(project)
            .args(["apply", "--3way", "--whitespace=nowarn"])
            .arg(&path)
            .output()
            .await;
        match applied {
            Ok(out) if out.status.success() => {
                let _ = tokio::fs::remove_file(&path).await;
                PatchOutcome::Applied { files }
            }
            Ok(out) => PatchOutcome::Saved {
                path,
                reason: String::from_utf8_lossy(&out.stderr)
                    .trim()
                    .chars()
                    .take(300)
                    .collect(),
            },
            Err(err) => PatchOutcome::Saved {
                path,
                reason: format!("git apply: {err}"),
            },
        }
    }

    /// Run one prompt on `agent`'s node and stream what it shows. The
    /// node writes the turn; the hub mirrors it as it runs and when it
    /// ends.
    #[must_use]
    pub fn run_turn(
        self: &Arc<Self>,
        agent: AgentId,
        prompt: String,
        model: Option<String>,
        auth_jwt: Option<String>,
        cancel: CancellationToken,
    ) -> mpsc::Receiver<RemoteEvent> {
        let (tx, rx) = mpsc::channel(256);
        let hub = Arc::clone(self);
        tokio::spawn(async move {
            let hex = agent.to_hex();
            let _ = tx
                .send(RemoteEvent::Status("Waking cloud machine".into()))
                .await;
            let node = match hub.awake_node(&hex).await {
                Ok(node) => node,
                Err(err) => {
                    let _ = tx.send(RemoteEvent::Failed(err.to_string())).await;
                    return;
                }
            };
            let guard = hub.fleet.begin_work(&hex);
            let turn = tokio::time::timeout(
                TURN_TIMEOUT,
                stream_turn(&hub, &node, agent, prompt, model, auth_jwt, &tx, cancel),
            )
            .await
            .unwrap_or(Err(CloudError::Timeout {
                op: "turn",
                after: TURN_TIMEOUT,
            }));
            if let Err(err) = hub.mirror_from(&node, agent).await {
                tracing::warn!(agent = %hex, error = %err, "could not copy the turn's record");
            }
            drop(guard);
            if let Err(err) = turn {
                let _ = tx.send(RemoteEvent::Failed(err.to_string())).await;
            }
        });
        rx
    }

    /// Put `agent`'s machine to sleep now (hibernate: disk-only billing).
    ///
    /// # Errors
    ///
    /// When it has no machine or boxd refuses.
    pub async fn sleep(&self, agent: AgentId) -> CloudResult<()> {
        let hex = agent.to_hex();
        let view = self.fleet.view(&hex).ok_or_else(|| CloudError::NotFound {
            what: "machine",
            name: hex.clone(),
        })?;
        if let Some(link) = self.links.lock().remove(&hex) {
            link.stop.cancel();
        }
        self.fleet.provider().hibernate(&view.machine.name).await?;
        self.fleet.poll().await;
        Ok(())
    }

    /// Wake `agent`'s machine and reopen its mesh socket.
    ///
    /// # Errors
    ///
    /// When it has no machine or waking fails.
    pub async fn wake(self: &Arc<Self>, agent: AgentId) -> CloudResult<()> {
        let hex = agent.to_hex();
        self.fleet.ensure_awake(&hex).await?;
        self.open_link(&hex);
        Ok(())
    }

    /// Run a shell command in `agent`'s project folder on its machine.
    ///
    /// # Errors
    ///
    /// When the machine can't be woken or the call fails.
    pub async fn exec(
        &self,
        agent: AgentId,
        command: &str,
    ) -> CloudResult<aura_engine_cloud::ExecOutput> {
        let hex = agent.to_hex();
        let view = self.fleet.ensure_awake(&hex).await?;
        let _guard = self.fleet.begin_work(&hex);
        self.fleet
            .provider()
            .exec(
                &view.machine.name,
                &format!("cd \"$AURA_WORKSPACE\" && {command}"),
                None,
                Duration::from_secs(600),
            )
            .await
    }

    /// The latest host sample of `agent`'s machine.
    #[must_use]
    pub fn host_stats(&self, agent: AgentId) -> Option<HostStats> {
        self.tracks
            .lock()
            .get(&agent.to_hex())
            .and_then(|t| t.latest.clone())
    }

    /// `agent`'s machine cost: boxd's meter plus an estimate since.
    #[must_use]
    pub fn cost(&self, agent: AgentId) -> Option<CostEstimate> {
        let hex = agent.to_hex();
        let view = self.fleet.view(&hex)?;
        let samples = self
            .tracks
            .lock()
            .get(&hex)
            .map(|t| t.samples.clone())
            .unwrap_or_default();
        let metered = view
            .usage
            .clone()
            .or_else(|| latest_reading(&self.ledger.rows(&view.ledger, 200)));
        Some(self.costs.estimate(metered.as_ref(), &samples))
    }

    /// The last `limit` rows of `agent`'s machine ledger.
    #[must_use]
    pub fn ledger_rows(&self, agent: AgentId, limit: usize) -> Vec<VmPayload> {
        self.fleet
            .view(&agent.to_hex())
            .map(|view| self.ledger.rows(&view.ledger, limit))
            .unwrap_or_default()
    }

    /// Every agent of `project` and the node holding it, for list
    /// markers.
    fn send_directories(&self, project: &str) {
        let links = self.links.lock();
        for node in self.directory.nodes_of(project) {
            let frame = self.directory.frame_for(&node, project);
            for (hex, link) in links.iter() {
                if self.directory.holder(hex).is_some_and(|h| h.node == node) {
                    let _ = link.frames.send(frame.clone());
                }
            }
        }
    }

    fn open_link(self: &Arc<Self>, hex: &str) {
        let stop = self.stop.child_token();
        let (frames, rx) = mpsc::unbounded_channel();
        if let Some(old) = self.links.lock().insert(
            hex.to_string(),
            Link {
                frames,
                stop: stop.clone(),
            },
        ) {
            old.stop.cancel();
        }
        let hub = Arc::downgrade(self);
        let hex = hex.to_string();
        tokio::spawn(async move { run_link(hub, hex, rx, stop).await });
    }

    /// Mail from a node, for an agent here or on another node.
    fn take_mail(&self, mail: MeshMail) -> MeshFrame {
        let ack = |delivered: bool, error: Option<String>| MeshFrame::MailAck {
            from_node: mail.from_node.clone(),
            id: mail.id,
            delivered,
            error,
        };
        if !self.mail.first_sight(&mail) {
            return ack(true, None);
        }
        match self.directory.holder(&mail.to) {
            Some(holder) if holder.node != HUB_NODE => {
                // For another node: forward it; that node drops it if
                // its sender already reached it directly.
                if holder.node != mail.from_node {
                    self.forward(&mail.to, MeshFrame::Mail(mail.clone()));
                }
                ack(true, None)
            }
            _ => {
                let host = self.host.lock().clone();
                let delivered = match (host, remote_mail(&mail)) {
                    (Some(host), Ok(remote)) => host.deliver_remote(remote),
                    (None, _) => Err("the hub has no agents".to_string()),
                    (_, Err(err)) => Err(err),
                };
                match delivered {
                    Ok(()) => ack(true, None),
                    Err(err) => {
                        // A resend may succeed (the agent restarted).
                        self.mail.forget(&mail);
                        ack(false, Some(err))
                    }
                }
            }
        }
    }

    /// Send `frame` toward the node holding `agent`: its socket, or a
    /// direct call when the socket is down.
    fn forward(&self, agent: &str, frame: MeshFrame) {
        let sent = self
            .links
            .lock()
            .get(agent)
            .is_some_and(|link| link.frames.send(frame.clone()).is_ok());
        if sent {
            return;
        }
        let MeshFrame::Mail(mail) = frame else {
            return;
        };
        self.send_direct(agent, mail);
    }

    /// Wake the machine of `agent` and hand it `mail` over HTTPS.
    fn send_direct(&self, agent: &str, mail: MeshMail) {
        let fleet = Arc::clone(&self.fleet);
        let agent = agent.to_string();
        tokio::spawn(async move {
            if let Err(err) = fleet.ensure_awake(&agent).await {
                tracing::warn!(agent = %agent, error = %err, "could not wake a machine for mail");
                return;
            }
            let (Some(view), Some(token)) = (fleet.view(&agent), fleet.node_token(&agent)) else {
                return;
            };
            let node = NodeClient::new(view.machine.name.clone(), view.machine.url(), token);
            if let Err(err) = node.mail(&mail).await {
                tracing::warn!(agent = %agent, error = %err, "could not deliver mail to a cloud agent");
            }
        });
    }

    fn track(&self, hex: &str, stats: HostStats) {
        let mut tracks = self.tracks.lock();
        let track = tracks.entry(hex.to_string()).or_default();
        track.samples.push(CostSample {
            at_ms: stats.sampled_ms,
            cpu_busy_secs: stats.cpu_busy_secs,
            mem_used_bytes: stats.mem_used_bytes,
            disk_used_bytes: stats.disk_used_bytes,
        });
        if track.samples.len() > MAX_SAMPLES {
            let excess = track.samples.len() - MAX_SAMPLES;
            track.samples.drain(..excess);
        }
        track.latest = Some(stats);
    }
}

fn remote_mail(mail: &MeshMail) -> Result<RemoteMail, String> {
    Ok(RemoteMail {
        from: AgentId::from_hex(&mail.from).map_err(|e| format!("bad sender: {e}"))?,
        from_name: mail.from_name.clone(),
        to: AgentId::from_hex(&mail.to).map_err(|e| format!("bad recipient: {e}"))?,
        kind: match mail.kind {
            MeshMailKind::Request => MessageKind::Request,
            MeshMailKind::Reply => MessageKind::Reply,
        },
        thread: ThreadId(mail.thread.clone()),
        subject: mail.subject.clone(),
        text: mail.text.clone(),
    })
}

/// The chat run request for a cloud agent's turn.
fn chat_request(agent: AgentId, model: Option<String>, auth_jwt: Option<String>) -> RuntimeRequest {
    RuntimeRequest {
        r#type: RuntimeRequestType::Chat {
            conversation_messages: Vec::new(),
        },
        agent_identity: AgentIdentity {
            partition_id: Some(agent.to_hex()),
            ..AgentIdentity::default()
        },
        model: ModelSelection {
            id: model,
            ..ModelSelection::default()
        },
        workspace: WorkspaceLocation {
            project_path: Some(WORKSPACE_DIR.to_string()),
            ..WorkspaceLocation::default()
        },
        project: None,
        agent_permissions: AgentPermissionsWire::default(),
        tool_permissions: None,
        agent_capabilities: AgentCapabilities::default(),
        auth_jwt,
        user_id: "hub".to_string(),
        mode: None,
    }
}

/// `https://x` → `wss://x`.
fn ws_url(base: &str, path: &str) -> String {
    let base = base
        .strip_prefix("https://")
        .map(|rest| format!("wss://{rest}"))
        .or_else(|| {
            base.strip_prefix("http://")
                .map(|rest| format!("ws://{rest}"))
        })
        .unwrap_or_else(|| base.to_string());
    format!("{base}{path}")
}

fn ws_request(
    url: &str,
    token: &BoxdToken,
) -> CloudResult<tokio_tungstenite::tungstenite::handshake::client::Request> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = url
        .into_client_request()
        .map_err(|err| CloudError::Transport {
            endpoint: url.to_string(),
            message: err.to_string(),
        })?;
    let value = format!("Bearer {}", token.expose())
        .parse()
        .map_err(|_| CloudError::Auth("node token is not a valid header".into()))?;
    request.headers_mut().insert("authorization", value);
    Ok(request)
}

/// One turn: start the chat run, send the prompt, map the stream.
#[allow(clippy::too_many_arguments)]
async fn stream_turn(
    hub: &CloudHub,
    node: &NodeClient,
    agent: AgentId,
    prompt: String,
    model: Option<String>,
    auth_jwt: Option<String>,
    tx: &mpsc::Sender<RemoteEvent>,
    cancel: CancellationToken,
) -> CloudResult<()> {
    use tokio_tungstenite::tungstenite::Message;
    let started = node
        .start_run(&chat_request(agent, model, auth_jwt))
        .await?;
    let path = started
        .get("event_stream_url")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            started
                .get("run_id")
                .and_then(serde_json::Value::as_str)
                .map(|id| format!("/stream/{id}"))
        })
        .ok_or_else(|| CloudError::InvalidResponse {
            op: "run",
            message: "no run id".into(),
        })?;
    let url = ws_url(node.base(), &path);
    let (socket, _) = tokio_tungstenite::connect_async(ws_request(&url, node.token())?)
        .await
        .map_err(|err| CloudError::Transport {
            endpoint: url.clone(),
            message: err.to_string(),
        })?;
    let (mut sink, mut stream) = socket.split();
    let message = InboundMessage::UserMessage(UserMessage {
        content: prompt,
        tool_hints: None,
        attachments: None,
    });
    let text = serde_json::to_string(&message).unwrap_or_default();
    sink.send(Message::Text(text.into()))
        .await
        .map_err(|err| CloudError::Transport {
            endpoint: url.clone(),
            message: err.to_string(),
        })?;
    let _ = tx
        .send(RemoteEvent::Status("Working in the cloud".into()))
        .await;
    let mut mirror = tokio::time::interval(MIRROR_EVERY);
    mirror.tick().await;
    loop {
        tokio::select! {
            () = cancel.cancelled() => {
                let cancel = serde_json::to_string(&InboundMessage::Cancel).unwrap_or_default();
                let _ = sink.send(Message::Text(cancel.into())).await;
                return Err(CloudError::BadState { machine: node.node().into(), state: "cancelled".into(), op: "finish the turn" });
            }
            _ = mirror.tick() => {
                if let Err(err) = hub.mirror_from(node, agent).await {
                    tracing::debug!(error = %err, "mirror during a turn failed");
                }
            }
            frame = stream.next() => {
                let Some(frame) = frame else {
                    return Err(CloudError::Node { node: node.node().into(), op: "stream", message: "closed before the turn ended".into() });
                };
                let frame = frame.map_err(|err| CloudError::Transport { endpoint: url.clone(), message: err.to_string() })?;
                let Message::Text(text) = frame else { continue };
                let Ok(message) = serde_json::from_str::<OutboundMessage>(&text) else { continue };
                match map_outbound(message) {
                    Some(RemoteEvent::Failed(err)) => return Err(CloudError::Node { node: node.node().into(), op: "turn", message: err }),
                    Some(event @ RemoteEvent::Done { .. }) => {
                        let _ = tx.send(event).await;
                        return Ok(());
                    }
                    Some(event) => {
                        if tx.send(event).await.is_err() {
                            return Ok(());
                        }
                    }
                    None => {}
                }
            }
        }
    }
}

/// What a node's stream frame shows, if anything.
#[must_use]
pub fn map_outbound(message: OutboundMessage) -> Option<RemoteEvent> {
    Some(match message {
        OutboundMessage::TextDelta(delta) => RemoteEvent::Text(delta.text),
        OutboundMessage::ThinkingDelta(delta) => RemoteEvent::Thinking(delta.thinking),
        OutboundMessage::ToolUseStart(start) => RemoteEvent::ToolStart {
            id: start.id,
            name: start.name,
        },
        OutboundMessage::ToolCallSnapshot(snapshot) => RemoteEvent::ToolInput {
            id: snapshot.id,
            input: snapshot.input.to_string(),
        },
        OutboundMessage::ToolResult(result) => RemoteEvent::ToolResult {
            id: result.tool_use_id.unwrap_or_default(),
            result: result.result,
            is_error: result.is_error,
        },
        OutboundMessage::AssistantMessageEnd(end) => RemoteEvent::Done {
            model: end.usage.model.clone(),
            input_tokens: end.usage.input_tokens,
            output_tokens: end.usage.output_tokens,
        },
        OutboundMessage::Error(err) if !err.recoverable => RemoteEvent::Failed(err.message),
        _ => return None,
    })
}

/// The fleet's timers.
async fn maintain(hub: Weak<CloudHub>, stop: CancellationToken) {
    let mut tick = tokio::time::interval(TICK_EVERY);
    let mut poll = tokio::time::interval(POLL_EVERY);
    let mut usage = tokio::time::interval(USAGE_EVERY);
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = tick.tick() => {
                let Some(hub) = hub.upgrade() else { return };
                let slept = hub.fleet.tick().await;
                for agent in slept {
                    if let Some(link) = hub.links.lock().get(&agent) {
                        link.stop.cancel();
                    }
                }
            }
            _ = poll.tick() => {
                let Some(hub) = hub.upgrade() else { return };
                for agent in hub.fleet.poll().await {
                    hub.open_link(&agent);
                }
            }
            _ = usage.tick() => {
                let Some(hub) = hub.upgrade() else { return };
                hub.fleet.record_usage().await;
            }
        }
    }
}

/// Keep one mesh socket open to `hex`'s node while the machine is awake.
async fn run_link(
    hub: Weak<CloudHub>,
    hex: String,
    mut outbox: mpsc::UnboundedReceiver<MeshFrame>,
    stop: CancellationToken,
) {
    run_link_socket(&hub, &hex, &mut outbox, &stop).await;
    // Mail queued for a socket that is gone still goes out, directly.
    outbox.close();
    if let Some(hub) = hub.upgrade() {
        while let Ok(frame) = outbox.try_recv() {
            if let MeshFrame::Mail(mail) = frame {
                hub.send_direct(&hex, mail);
            }
        }
    }
}

async fn run_link_socket(
    hub: &Weak<CloudHub>,
    hex: &str,
    outbox: &mut mpsc::UnboundedReceiver<MeshFrame>,
    stop: &CancellationToken,
) {
    use tokio_tungstenite::tungstenite::Message;
    let mut backoff = Duration::from_secs(1);
    loop {
        if stop.is_cancelled() {
            return;
        }
        let Some(strong) = hub.upgrade() else { return };
        let asleep = strong.fleet.view(hex).is_none_or(|v| v.state.is_asleep());
        let node = strong.node(hex);
        let project = strong.directory.holder(hex).map(|h| h.project);
        drop(strong);
        let node = match node {
            Ok(node) if !asleep => node,
            // Asleep or not claimed yet: a wake reopens the link.
            _ => return,
        };
        let url = ws_url(node.base(), "/v1/mesh");
        let connected = match ws_request(&url, node.token()) {
            Ok(request) => tokio_tungstenite::connect_async(request).await.ok(),
            Err(_) => None,
        };
        let Some((socket, _)) = connected else {
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(Duration::from_secs(60));
            continue;
        };
        backoff = Duration::from_secs(1);
        let (mut sink, mut stream) = socket.split();
        let hello = MeshFrame::Hello {
            node_id: HUB_NODE.into(),
            protocol: MESH_PROTOCOL,
        };
        let _ = sink.send(Message::Text(hello.to_text().into())).await;
        if let (Some(hub), Some(project)) = (hub.upgrade(), project.as_deref()) {
            let frame = hub.directory.frame_for(node.node(), project);
            let _ = sink.send(Message::Text(frame.to_text().into())).await;
        }
        loop {
            tokio::select! {
                () = stop.cancelled() => return,
                frame = outbox.recv() => {
                    let Some(frame) = frame else { return };
                    if sink.send(Message::Text(frame.to_text().into())).await.is_err() {
                        break;
                    }
                }
                message = stream.next() => {
                    let Some(Ok(message)) = message else { break };
                    let Message::Text(text) = message else { continue };
                    let Ok(frame) = MeshFrame::from_text(&text) else { continue };
                    let Some(hub) = hub.upgrade() else { return };
                    match frame {
                        MeshFrame::Mail(mail) => {
                            hub.fleet.touch(hex);
                            let ack = hub.take_mail(mail);
                            let _ = sink.send(Message::Text(ack.to_text().into())).await;
                        }
                        MeshFrame::HostStats(stats) => hub.track(hex, stats),
                        MeshFrame::RecordCommitted { .. } => {
                            if let Ok(agent) = AgentId::from_hex(hex) {
                                let _ = hub.mirror(agent).await;
                            }
                        }
                        MeshFrame::Ping { ms } => {
                            let _ = sink.send(Message::Text(MeshFrame::Pong { ms }.to_text().into())).await;
                        }
                        _ => {}
                    }
                }
            }
        }
    }
}

/// [`RemoteRouter`] for this app's [`AgentHost`]: mail for a cloud agent
/// goes to its node.
struct HubRouter(Weak<CloudHub>);

impl RemoteRouter for HubRouter {
    fn name_of(&self, agent: AgentId) -> Option<String> {
        let hub = self.0.upgrade()?;
        hub.directory
            .holder(&agent.to_hex())
            .filter(|holder| holder.node != HUB_NODE)
            .map(|holder| holder.name)
    }

    fn route(&self, mail: RemoteMail) -> Result<(), String> {
        let hub = self.0.upgrade().ok_or("the cloud hub stopped")?;
        let to = mail.to.to_hex();
        if !hub.directory.is_remote(&to) {
            return Err(format!("unknown agent {}", mail.to));
        }
        let frame = MeshMail {
            id: hub.mail.next_id(),
            from_node: HUB_NODE.into(),
            thread: global_thread(HUB_NODE, &mail.thread.0),
            subject: mail.subject,
            from: mail.from.to_hex(),
            from_name: mail.from_name,
            to: to.clone(),
            kind: match mail.kind {
                MessageKind::Request => MeshMailKind::Request,
                MessageKind::Reply => MeshMailKind::Reply,
            },
            text: mail.text,
            sent_ms: now_ms(),
        };
        hub.fleet.touch(&to);
        hub.forward(&to, MeshFrame::Mail(frame));
        Ok(())
    }

    fn peers(&self, _root: AgentId) -> Vec<serde_json::Value> {
        let Some(hub) = self.0.upgrade() else {
            return Vec::new();
        };
        hub.placed()
            .into_iter()
            .map(|(hex, placed)| {
                serde_json::json!({
                    "agent_id": hex,
                    "name": placed.name,
                    "node": placed.machine,
                    "placement": "cloud",
                })
            })
            .collect()
    }
}

/// The agent id a ZNS id has; re-exported for callers of the hub.
#[must_use]
pub fn zns_agent(zns: &str) -> AgentId {
    agent_id_of(zns)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aura_core_protocol::{ErrorMsg, TextDelta, ToolUseStart};
    use aura_engine_cloud::FakeProvider;
    use aura_store_db::RocksStore;

    fn hub() -> (Arc<CloudHub>, FakeProvider, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store: Arc<dyn Store> =
            Arc::new(RocksStore::open(dir.path().join("db"), false).expect("open"));
        let fake = FakeProvider::new();
        let hub = CloudHub::new(
            CloudConfig::defaults(),
            store,
            dir.path().join("cloud"),
            Arc::new(fake.clone()),
            AuraBinary::Installed,
        )
        .expect("hub");
        (hub, fake, dir)
    }

    #[test]
    fn stream_frames_map_to_events() {
        assert_eq!(
            map_outbound(OutboundMessage::TextDelta(TextDelta { text: "hi".into() })),
            Some(RemoteEvent::Text("hi".into()))
        );
        assert_eq!(
            map_outbound(OutboundMessage::ToolUseStart(ToolUseStart {
                id: "1".into(),
                name: "read_file".into()
            })),
            Some(RemoteEvent::ToolStart {
                id: "1".into(),
                name: "read_file".into()
            })
        );
        let fatal = OutboundMessage::Error(ErrorMsg {
            code: "x".into(),
            message: "boom".into(),
            recoverable: false,
            support_id: None,
        });
        assert_eq!(
            map_outbound(fatal),
            Some(RemoteEvent::Failed("boom".into()))
        );
        let soft = OutboundMessage::Error(ErrorMsg {
            code: "x".into(),
            message: "retry".into(),
            recoverable: true,
            support_id: None,
        });
        assert_eq!(map_outbound(soft), None);
    }

    #[test]
    fn urls_and_requests() {
        assert_eq!(
            ws_url("https://m.boxd.sh", "/v1/mesh"),
            "wss://m.boxd.sh/v1/mesh"
        );
        assert_eq!(
            ws_url("http://127.0.0.1:8080", "/stream/1"),
            "ws://127.0.0.1:8080/stream/1"
        );
        let request = chat_request(AgentId::new([1; 32]), Some("gate".into()), None);
        assert_eq!(
            request.agent_identity.partition_id,
            Some(AgentId::new([1; 32]).to_hex())
        );
        assert_eq!(
            request.workspace.project_path.as_deref(),
            Some(WORKSPACE_DIR)
        );
        let json = serde_json::to_string(&request).expect("json");
        assert!(!json.contains("bxd_"), "{json}");
    }

    #[test]
    fn project_defaults_persist() {
        let (hub, _fake, dir) = hub();
        assert_eq!(hub.project_default("p1"), Placement::Local);
        hub.set_project_default("p1", Placement::Cloud);
        drop(hub);
        let store: Arc<dyn Store> =
            Arc::new(RocksStore::open(dir.path().join("db2"), false).expect("open"));
        let again = CloudHub::new(
            CloudConfig::defaults(),
            store,
            dir.path().join("cloud"),
            Arc::new(FakeProvider::new()),
            AuraBinary::Installed,
        )
        .expect("hub");
        assert_eq!(again.project_default("p1"), Placement::Cloud);
    }

    #[test]
    fn mail_for_a_local_agent_without_a_host_is_refused_and_resends_are_retried() {
        let (hub, _fake, _dir) = hub();
        let mail = MeshMail {
            id: 1,
            from_node: "aura-x".into(),
            thread: "aura-x:t1".into(),
            subject: "s".into(),
            from: AgentId::new([2; 32]).to_hex(),
            from_name: "scout".into(),
            to: AgentId::new([3; 32]).to_hex(),
            kind: MeshMailKind::Request,
            text: "hi".into(),
            sent_ms: 0,
        };
        let MeshFrame::MailAck { delivered, .. } = hub.take_mail(mail.clone()) else {
            panic!("not an ack");
        };
        assert!(!delivered);
        let MeshFrame::MailAck { delivered, .. } = hub.take_mail(mail) else {
            panic!("not an ack");
        };
        assert!(
            !delivered,
            "a failed delivery is forgotten, so the resend is tried again"
        );
    }

    #[tokio::test]
    async fn an_agent_that_was_never_placed_recalls_to_nothing() {
        let (hub, _fake, _dir) = hub();
        let agent = AgentId::new([5; 32]);
        assert!(!hub.is_placed(agent));
        assert_eq!(
            hub.recall(agent, "test").await.expect("recall"),
            PatchOutcome::Empty
        );
        assert_eq!(hub.placement_of(agent), Placement::Local);
    }

    #[test]
    fn patch_outcomes_describe_themselves() {
        assert_eq!(PatchOutcome::Empty.describe(), "no changes to pull");
        assert_eq!(
            PatchOutcome::Applied {
                files: vec!["a".into()]
            }
            .describe(),
            "pulled 1 changed file(s)"
        );
    }
}
