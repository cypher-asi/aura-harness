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
pub use aura_core_auth::Owner;
use aura_core_protocol::cloud::{
    GitClaim, HostStats, MeshFrame, MeshMail, MeshMailKind, SyncImport, MESH_PROTOCOL,
};
use aura_core_protocol::{
    AgentCapabilities, AgentIdentity, AgentPermissionsWire, InboundMessage, ModelSelection,
    OutboundMessage, RuntimeRequest, RuntimeRequestType, UserMessage, WorkspaceLocation,
};
use aura_core_types::time::now_ms;
use aura_core_types::{AgentId, Placement, PlacementPayload, VmPayload};
use aura_engine::{AgentHost, MessageKind, RemoteMail, RemoteRouter, ThreadId};
use aura_engine_cloud::boot::human_bytes;
use aura_engine_cloud::cost::{latest_reading, CostEstimate, CostModel, CostSample};
use aura_engine_cloud::delta::BranchFetch;
use aura_engine_cloud::fleet::{MachineView, ProvisionRequest, VmFleet};
use aura_engine_cloud::github::{BotIdentity, GithubApp, RepoRef, RepoTokens};
use aura_engine_cloud::machine_env::{MachineEnv, WORKSPACE_DIR};
use aura_engine_cloud::mesh::{global_thread, Directory, Holder, MailBook, HUB_NODE};
use aura_engine_cloud::plans::{PlanCredentials, PlanStatus, PlanSync, SYNC_EVERY};
use aura_engine_cloud::repos::{RepoStatus, RepoSync};
use aura_engine_cloud::snapshots::{self, AuraBinary, SnapshotBuilder, SnapshotRecord};
use aura_engine_cloud::workspace::{self, BranchStart, Bundled};
use aura_engine_cloud::{
    boxd, delta, names, replica, BootEvent, BootReporter, BootStage, CloudError, CloudResult,
    MachineOrigin, VmProvider,
};
use aura_store_db::Store;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::node_client::{NodeClient, NODE_TIMEOUT};
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

/// How bringing an agent's commits back went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitsOutcome {
    /// It made no commits the project lacks (or its machine predates
    /// agent branches).
    None,
    /// They are on the project's branch of the same name.
    Fetched {
        /// The branch.
        branch: String,
        /// How many came.
        commits: u64,
    },
    /// They stayed on the machine.
    Kept {
        /// The branch.
        branch: String,
        /// Why.
        reason: String,
    },
}

impl CommitsOutcome {
    /// A short report for the chat; `None` when there is nothing to say.
    #[must_use]
    pub fn describe(&self) -> Option<String> {
        match self {
            Self::None => None,
            Self::Fetched { branch, commits } => {
                Some(format!("{commits} commit(s) on branch {branch}"))
            }
            Self::Kept { branch, reason } => {
                Some(format!("commits on {branch} not fetched: {reason}"))
            }
        }
    }
}

/// What pulling an agent's work back brought.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullOutcome {
    /// Its working tree's changes, applied to the project folder.
    pub patch: PatchOutcome,
    /// Its commits, as a branch here.
    pub commits: CommitsOutcome,
}

impl PullOutcome {
    /// Only a patch outcome.
    #[must_use]
    pub const fn patch_only(patch: PatchOutcome) -> Self {
        Self {
            patch,
            commits: CommitsOutcome::None,
        }
    }

    /// A one-line report for the chat.
    #[must_use]
    pub fn describe(&self) -> String {
        match self.commits.describe() {
            Some(commits) => format!("{}; {commits}", self.patch.describe()),
            None => self.patch.describe(),
        }
    }
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
    /// How many files differ from HEAD.
    changed: usize,
    /// Those files, as a gzipped tar.
    archive: Option<Vec<u8>>,
}

/// What a cloud agent's turn shows, as it streams.
#[derive(Debug, Clone, PartialEq)]
pub enum RemoteEvent {
    /// The machine is waking or the run is starting.
    Status(String),
    /// A step of waking a sleeping machine.
    Boot(BootEvent),
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
    links: Mutex<HashMap<String, Link>>,
    tracks: Mutex<HashMap<String, HostTrack>>,
    mirrors: Mutex<HashMap<AgentId, Arc<tokio::sync::Mutex<()>>>>,
    /// Users' plan tokens as boxd secrets; `None` until the app hands
    /// the hub its credentials.
    plans: Mutex<Option<Arc<PlanSync>>>,
    /// Projects' GitHub tokens as boxd secrets.
    repos: Mutex<Arc<RepoSync>>,
    /// The user signed in here: local agents act as this user.
    current_owner: Mutex<Owner>,
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
    let built = aura_engine_cloud::snapshots::cloud_binary_path(home);
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
        let tokens = config.github_app.clone().and_then(|app| match GithubApp::new(app) {
            Ok(app) => Some(Arc::new(app) as Arc<dyn RepoTokens>),
            Err(err) => {
                tracing::warn!(error = %err, "the GitHub App can't be used; cloud agents get no GitHub access");
                None
            }
        });
        let repos = Arc::new(RepoSync::new(
            Arc::clone(&provider),
            tokens,
            placements.install.clone(),
        ));
        let snapshots = SnapshotBuilder::new(
            provider,
            Arc::clone(&ledger) as Arc<dyn aura_engine_cloud::fleet::VmLedger>,
            placements.install.clone(),
            config.size,
            config.isolated,
            env!("CARGO_PKG_VERSION"),
            binary,
            home.clone(),
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
            plans: Mutex::new(None),
            repos: Mutex::new(repos),
            current_owner: Mutex::new(Owner::Local),
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
        let binary = if cfg!(target_os = "linux") {
            choose_binary(
                config.binary.as_deref(),
                &home,
                std::env::current_exe().ok(),
            )
        } else {
            AuraBinary::Installed
        };
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
                // Local agents act as the user signed in here.
                owner: names::owner_tag(&self.current_owner()),
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

    /// Read users' plan credentials from `credentials`: from now on
    /// placed machines can use their owner's Claude, ChatGPT and
    /// OpenRouter plans.
    pub fn set_plan_credentials(&self, credentials: Arc<dyn PlanCredentials>) {
        let install = self.placements.lock().install.clone();
        *self.plans.lock() = Some(Arc::new(PlanSync::new(
            self.fleet.provider(),
            credentials,
            install,
        )));
    }

    fn plan_sync(&self) -> Option<Arc<PlanSync>> {
        self.plans.lock().clone()
    }

    /// Mint GitHub tokens with `tokens` from now on (`None`: no App).
    pub fn set_repo_tokens(&self, tokens: Option<Arc<dyn RepoTokens>>) {
        let install = self.placements.lock().install.clone();
        *self.repos.lock() = Arc::new(RepoSync::new(self.fleet.provider(), tokens, install));
    }

    fn repo_sync(&self) -> Arc<RepoSync> {
        Arc::clone(&self.repos.lock())
    }

    /// The GitHub token state of `repo`'s machines, once it was synced.
    #[must_use]
    pub fn repo_status(&self, repo: &RepoRef) -> Option<RepoStatus> {
        self.repo_sync().status(repo)
    }

    /// The repositories placed agents work in.
    fn placed_repos(&self) -> Vec<RepoRef> {
        let mut repos: Vec<RepoRef> = Vec::new();
        for placed in self.placements.lock().agents.values() {
            if let Some(repo) = &placed.repo {
                if !repos.contains(repo) {
                    repos.push(repo.clone());
                }
            }
        }
        repos
    }

    /// The user signed in here changed; local agents act as `owner`.
    pub fn set_current_owner(&self, owner: Owner) {
        *self.current_owner.lock() = owner;
    }

    /// The user signed in here.
    #[must_use]
    pub fn current_owner(&self) -> Owner {
        self.current_owner.lock().clone()
    }

    /// The user who placed `agent`, while it is placed.
    #[must_use]
    pub fn owner_of(&self, agent: AgentId) -> Option<Owner> {
        self.placements
            .lock()
            .agents
            .get(&agent.to_hex())
            .map(PlacedAgent::owner)
    }

    /// Where `agent` stands, while it is placed.
    #[must_use]
    pub fn placed_agent(&self, agent: AgentId) -> Option<PlacedAgent> {
        self.placements.lock().agents.get(&agent.to_hex()).cloned()
    }

    /// The display name of the user who placed `agent`.
    #[must_use]
    pub fn owner_name_of(&self, agent: AgentId) -> Option<String> {
        self.placements
            .lock()
            .agents
            .get(&agent.to_hex())
            .and_then(|placed| placed.owner_name.clone())
    }

    /// Every user with a placed agent.
    #[must_use]
    pub fn owners(&self) -> Vec<Owner> {
        let mut owners: Vec<Owner> = Vec::new();
        for placed in self.placements.lock().agents.values() {
            let owner = placed.owner();
            if !owners.contains(&owner) {
                owners.push(owner);
            }
        }
        owners
    }

    /// `owner`'s plans as their cloud machines see them.
    #[must_use]
    pub fn plan_status(&self, owner: &Owner) -> Vec<PlanStatus> {
        self.plan_sync()
            .map(|plans| plans.status(owner))
            .unwrap_or_default()
    }

    /// Push `owner`'s current plan tokens to boxd now (they linked or
    /// re-linked a plan).
    pub async fn sync_plans(&self, owner: &Owner) {
        if let Some(plans) = self.plan_sync() {
            plans.tick(std::slice::from_ref(owner)).await;
        }
    }

    /// Stop `owner`'s machines using their `kind` plan (they signed out
    /// of it).
    ///
    /// # Errors
    ///
    /// When boxd refuses the update.
    pub async fn revoke_plan(
        &self,
        owner: &Owner,
        kind: aura_core_auth::ProviderKind,
    ) -> CloudResult<()> {
        match self.plan_sync() {
            Some(plans) => plans.revoke(owner, kind).await,
            None => Ok(()),
        }
    }

    /// Take back the machines placed agents had before this hub started,
    /// destroy machines no kept agent owns, and start the fleet's
    /// timers. `keep` are the agents that still exist (hex).
    pub async fn start(self: &Arc<Self>, keep: &HashSet<String>) {
        self.start_reported(keep, &BootReporter::none()).await;
    }

    /// [`Self::start`], showing on `boot` each machine it takes back and
    /// the sweep, as the [`BootStage::Connect`] stage's waits.
    pub async fn start_reported(self: &Arc<Self>, keep: &HashSet<String>, boot: &BootReporter) {
        let placed = self.placed();
        let kept = placed.iter().filter(|(hex, _)| keep.contains(hex)).count();
        if kept > 0 {
            boot.log(
                BootStage::Connect,
                false,
                format!("taking back {kept} cloud machine(s)"),
            );
        }
        let repos = self.repo_sync();
        let identity = if placed.iter().any(|(_, agent)| agent.repo.is_some()) {
            repos.identity().await
        } else {
            None
        };
        for (hex, agent) in &placed {
            if !keep.contains(hex) {
                continue;
            }
            let templates = self
                .plan_sync()
                .map(|plans| plans.templates(&agent.owner()))
                .unwrap_or_default();
            let git = Self::git_claim(
                agent.branch.as_deref(),
                agent.repo.as_ref(),
                agent.repo.as_ref().map(|repo| repos.secret_name(repo)),
                identity.as_ref(),
            );
            let adopted = boot
                .wait(
                    BootStage::Connect,
                    format!("boxd is looking up {} ({})", agent.machine, agent.name),
                    Some(boxd::CALL_TIMEOUT),
                    self.fleet.adopt(
                        hex,
                        &agent.zns,
                        &agent.project,
                        &agent.machine,
                        templates,
                        git,
                    ),
                )
                .await;
            match adopted {
                Ok(view) => {
                    boot.log(
                        BootStage::Connect,
                        false,
                        format!("{} is {}", agent.machine, view.state.label()),
                    );
                    self.place_in_directory(hex, agent, &view);
                }
                Err(err) => {
                    tracing::warn!(agent = %hex, machine = %agent.machine, error = %err, "cloud machine is gone; the agent runs here again");
                    boot.log(
                        BootStage::Connect,
                        true,
                        format!("{} is gone; {} runs here again", agent.machine, agent.name),
                    );
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
        let swept = boot
            .wait(
                BootStage::Connect,
                "boxd is listing this install's machines, to destroy those no agent owns",
                Some(boxd::CALL_TIMEOUT),
                self.fleet.sweep_orphans(&owned),
            )
            .await;
        match swept {
            Ok(swept) if !swept.is_empty() => {
                tracing::info!(machines = ?swept, "destroyed cloud machines no agent owns");
                boot.log(
                    BootStage::Connect,
                    false,
                    format!("destroyed {} machine(s) no agent owns", swept.len()),
                );
            }
            Ok(_) => {}
            Err(err) => {
                tracing::warn!(error = %err, "could not list cloud machines");
                boot.log(
                    BootStage::Connect,
                    true,
                    format!("could not list cloud machines: {err}"),
                );
            }
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
                owner: names::owner_tag(&agent.owner()),
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

    async fn awake_node(&self, agent: &str, boot: &BootReporter) -> CloudResult<NodeClient> {
        self.fleet.ensure_awake_reported(agent, boot).await?;
        self.node(agent)
    }

    /// Move `request.agent`'s single writer to a cloud machine, showing
    /// each step on `boot`. The caller must hold the agent idle until
    /// this returns.
    ///
    /// # Errors
    ///
    /// When the snapshot, the machine or the record import fails; the
    /// agent then stays here and any machine made for it is destroyed.
    pub async fn place(
        self: &Arc<Self>,
        request: PlaceRequest,
        boot: &BootReporter,
    ) -> CloudResult<MachineView> {
        let hex = request.agent.to_hex();
        if self.is_placed(request.agent) {
            return self.fleet.view(&hex).ok_or_else(|| CloudError::NotFound {
                what: "machine",
                name: hex.clone(),
            });
        }
        // A fork inherits its parent's plan secrets: only within one user.
        let parent = request
            .fork_from
            .filter(|parent| self.owner_of(*parent).as_ref() == Some(&request.owner))
            .map(|parent| parent.to_hex())
            .filter(|parent| self.config.fork_on_spawn && self.fleet.has(parent));
        let owner_tag = names::owner_tag(&request.owner);
        boot.fact("agent", request.name.clone());
        boot.plan(&[
            BootStage::Plans,
            BootStage::Repo,
            BootStage::Snapshot,
            BootStage::Quota,
            BootStage::Create,
            BootStage::Configure,
            BootStage::Claim,
            BootStage::SyncFiles,
            BootStage::ImportRecord,
        ]);
        let (secrets, plans) = match self.plan_sync() {
            Some(sync) => {
                boot.run(BootStage::Plans, async {
                    let ensured = boot
                        .wait(
                            BootStage::Plans,
                            "boxd is storing the plan sign-ins as machine secrets",
                            None,
                            sync.ensure_owner(&request.owner),
                        )
                        .await;
                    Ok::<_, CloudError>(match ensured {
                        Ok(secrets) => {
                            let plans = sync.templates(&request.owner);
                            boot.fact("plans", format!("{} plan sign-in(s) shared", plans.len()));
                            (secrets, plans)
                        }
                        Err(err) => {
                            tracing::warn!(owner = %owner_tag, error = %err, "could not set the plan secrets; the machine gets no plans");
                            boot.log(BootStage::Plans, true, format!("no plans for this machine: {err}"));
                            (Vec::new(), Vec::new())
                        }
                    })
                })
                .await?
            }
            None => {
                boot.skip(BootStage::Plans, "no plan sign-ins here");
                (Vec::new(), Vec::new())
            }
        };
        let branch = workspace::agent_branch(&request.name, request.code.as_deref(), &hex);
        boot.fact("branch", branch.clone());
        let github = self.share_repo(&request, boot).await?;
        let git = Self::git_claim(
            Some(&branch),
            github.repo.as_ref(),
            github.secret.clone(),
            github.identity.as_ref(),
        );
        let (origin, base_commit, seal) = match parent.as_deref().and_then(|p| self.fleet.view(p)) {
            Some(parent_view) => {
                boot.skip(
                    BootStage::Snapshot,
                    format!("forking the parent's machine {}", parent_view.machine.name),
                );
                (
                    MachineOrigin::Fork(parent_view.machine.name.clone()),
                    None,
                    vec![parent_view.agent.clone()],
                )
            }
            None => {
                let record = boot
                    .run(BootStage::Snapshot, self.project_snapshot(&request, boot))
                    .await?;
                (
                    MachineOrigin::Snapshot(record.name),
                    record.commit,
                    Vec::new(),
                )
            }
        };
        let view = self
            .fleet
            .provision_reported(
                ProvisionRequest {
                    agent: hex.clone(),
                    agent_zns: request.agent_zns.clone(),
                    code: request.code.clone(),
                    project: request.project_id.clone(),
                    name: names::machine_name(request.code.as_deref(), &request.agent),
                    origin,
                    seal,
                    owner: owner_tag.clone(),
                    secrets,
                    plans,
                    repo_secret: github.secret.clone(),
                    git,
                },
                boot,
            )
            .await?;
        let env = MachineEnv::for_agent(view.machine.vcpu).into_map();
        boot.fact("workspace", WORKSPACE_DIR);
        boot.fact(
            "env",
            format!(
                "{} (allowlist)",
                env.keys().map(String::as_str).collect::<Vec<_>>().join(" ")
            ),
        );
        let remote_url = github.repo.as_ref().map(RepoRef::https_url);
        if let Err(err) = self
            .seed(
                &request,
                &view,
                base_commit.as_deref(),
                &branch,
                remote_url.as_deref(),
                boot,
            )
            .await
        {
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
            owner: Some(request.owner.clone()),
            owner_name: request.owner_name.clone(),
            branch: Some(branch),
            repo: github.repo,
            fetched_tip: None,
        };
        record_placement(
            &self.ledger,
            &names::machine_ledger(&request.agent_zns),
            &PlacementPayload::new(
                Placement::Local,
                Placement::Cloud,
                Some(view.machine.name.clone()),
            )
            .with_owner(Some(owner_tag)),
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

    /// The project snapshot a new machine starts from, rebuilt from HEAD
    /// when this repository no longer has the snapshot's commit (the
    /// delta is taken against it).
    async fn project_snapshot(
        &self,
        request: &PlaceRequest,
        boot: &BootReporter,
    ) -> CloudResult<SnapshotRecord> {
        let ensure = |force| {
            self.snapshots.ensure_project_reported(
                &request.project_id,
                &request.project_dir,
                request.build_command.as_deref(),
                force,
                boot,
            )
        };
        let record = ensure(false).await?;
        let missing = match record.commit.clone() {
            Some(commit) => {
                let dir = request.project_dir.clone();
                !tokio::task::spawn_blocking(move || delta::has_commit(&dir, &commit))
                    .await
                    .unwrap_or(false)
            }
            None => false,
        };
        if !missing {
            return Ok(record);
        }
        tracing::info!(project = %request.project_id, "the project snapshot's commit is gone here; rebuilding it");
        boot.log(
            BootStage::Snapshot,
            false,
            "the snapshot's commit is gone here; rebuilding it from HEAD",
        );
        ensure(true).await
    }

    /// Hand the project's GitHub token to boxd as the repository's
    /// secret. Never fails placement: without a token the agent still
    /// commits on its branch, and pulls bring the commits back.
    async fn share_repo(
        &self,
        request: &PlaceRequest,
        boot: &BootReporter,
    ) -> CloudResult<RepoShare> {
        let dir = request.project_dir.clone();
        let url = tokio::task::spawn_blocking(move || delta::origin_url(&dir))
            .await
            .ok()
            .flatten();
        let Some(repo) = url.as_deref().and_then(RepoRef::from_remote) else {
            let why = if url.is_some() {
                "origin is not on GitHub"
            } else {
                "the project has no origin"
            };
            boot.skip(
                BootStage::Repo,
                format!("{why}: the agent's commits come back with pulls"),
            );
            return Ok(RepoShare::default());
        };
        boot.run(BootStage::Repo, async {
            boot.fact("repository", repo.full_name());
            let sync = self.repo_sync();
            let ensured = boot
                .wait(
                    BootStage::Repo,
                    format!("boxd is storing the GitHub token for {repo} as a machine secret"),
                    None,
                    sync.ensure(&repo),
                )
                .await;
            let status = match ensured {
                Ok(status) => status,
                Err(err) => {
                    tracing::warn!(repo = %repo, error = %err, "could not set the repository secret; the machine gets no GitHub access");
                    boot.log(
                        BootStage::Repo,
                        true,
                        format!("no GitHub access for this machine: {err}"),
                    );
                    return Ok(RepoShare {
                        repo: Some(repo.clone()),
                        ..RepoShare::default()
                    });
                }
            };
            if status.linked {
                boot.fact("github", "token shared; the App's bot pushes and opens pull requests");
            } else {
                let notice = match (&status.install_url, &status.error) {
                    (Some(url), _) => format!("GitHub App not installed on {repo}: {url}"),
                    (None, Some(error)) => format!("no GitHub token for {repo}: {error}"),
                    (None, None) => format!("no GitHub token for {repo}"),
                };
                boot.log(BootStage::Repo, true, notice);
                boot.fact("github", "no token; the agent's commits come back with pulls");
            }
            // Only ask GitHub who the bot is when it answered just now.
            let identity = if status.linked || status.install_url.is_some() {
                sync.identity().await
            } else {
                None
            };
            Ok::<_, CloudError>(RepoShare {
                repo: Some(repo.clone()),
                secret: Some(status.secret),
                identity,
            })
        })
        .await
    }

    /// What a node is told about git: the agent's branch, and the
    /// repository it pushes to as the App's bot.
    fn git_claim(
        branch: Option<&str>,
        repo: Option<&RepoRef>,
        secret: Option<String>,
        identity: Option<&BotIdentity>,
    ) -> Option<GitClaim> {
        Some(GitClaim {
            branch: branch?.to_string(),
            remote_url: repo.map(RepoRef::https_url),
            env: secret,
            placeholder: None,
            author_name: identity.map(BotIdentity::login),
            author_email: identity.map(BotIdentity::email),
        })
    }

    /// Bring the project files up to date on the machine and give its
    /// node the agent's record.
    async fn seed(
        &self,
        request: &PlaceRequest,
        view: &MachineView,
        snapshot_commit: Option<&str>,
        branch: &str,
        remote_url: Option<&str>,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        match snapshot_commit {
            Some(commit) => {
                boot.run(
                    BootStage::SyncFiles,
                    self.sync_files(request, view, commit, branch, remote_url, boot),
                )
                .await?;
            }
            None => {
                boot.run(BootStage::SyncFiles, async {
                    boot.fact("files", "the parent's, on a branch of its own");
                    let script = workspace::fork_branch(branch)?;
                    self.run_script(view, BootStage::SyncFiles, "branch the fork", &script, boot)
                        .await
                })
                .await?;
            }
        }
        boot.run(
            BootStage::ImportRecord,
            self.import_record(request, view, boot),
        )
        .await
    }

    /// Bring the machine's workspace to the project here: the commits
    /// made since the snapshot's `snapshot_commit`, the agent's branch at
    /// HEAD, and the uncommitted changes on top.
    async fn sync_files(
        &self,
        request: &PlaceRequest,
        view: &MachineView,
        snapshot_commit: &str,
        branch: &str,
        remote_url: Option<&str>,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        boot.fact("snapshot commit", short_commit(snapshot_commit));
        let dir = request.project_dir.clone();
        let base = snapshot_commit.to_string();
        let packed = boot
            .wait(
                BootStage::SyncFiles,
                format!(
                    "bundling the project's commits since {} and packing its changes",
                    short_commit(snapshot_commit)
                ),
                None,
                tokio::task::spawn_blocking(move || -> CloudResult<PackedProject> {
                    let head = delta::head_commit(&dir)?;
                    let bundle = if head == base {
                        None
                    } else {
                        delta::bundle_since(&dir, &base)?
                    };
                    let changes = delta::collect(&dir, &head)?;
                    let changed = changes.files.len() + changes.deleted.len();
                    let archive = if changes.is_empty() {
                        None
                    } else {
                        Some(delta::pack(&dir, &changes)?)
                    };
                    Ok(PackedProject {
                        head,
                        bundle,
                        changed,
                        archive,
                    })
                }),
            )
            .await
            .map_err(|err| {
                CloudError::io("pack changes", std::io::Error::other(err.to_string()))
            })??;
        boot.fact("head", short_commit(&packed.head));
        let has_bundle = packed.bundle.is_some();
        match packed.bundle {
            Some(bytes) => {
                boot.fact(
                    "commits",
                    format!(
                        "made since the snapshot, {} bundled",
                        human_bytes(bytes.len() as u64)
                    ),
                );
                self.upload_file(view, workspace::HEAD_BUNDLE, bytes, boot)
                    .await?;
            }
            None => boot.fact("commits", "none since the snapshot"),
        }
        let has_delta = packed.archive.is_some();
        match packed.archive {
            Some(bytes) => {
                boot.fact(
                    "changes",
                    format!(
                        "{} uncommitted file(s), {} packed",
                        packed.changed,
                        human_bytes(bytes.len() as u64)
                    ),
                );
                self.upload_file(view, workspace::DELTA_ARCHIVE, bytes, boot)
                    .await?;
            }
            None => boot.fact("changes", "none uncommitted"),
        }
        let script = workspace::start_branch(&BranchStart {
            branch,
            head: &packed.head,
            remote_url,
            bundle: has_bundle,
            delta: has_delta,
        })?;
        self.run_script(
            view,
            BootStage::SyncFiles,
            "start the agent's branch",
            &script,
            boot,
        )
        .await
    }

    /// Upload `bytes` to `path` on the machine, showing progress.
    async fn upload_file(
        &self,
        view: &MachineView,
        path: &str,
        bytes: Vec<u8>,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        let provider = self.fleet.provider();
        let machine = &view.machine.name;
        let size = human_bytes(bytes.len() as u64);
        boot.log(
            BootStage::SyncFiles,
            false,
            format!("upload {path} ({size})"),
        );
        boot.wait(
            BootStage::SyncFiles,
            format!("uploading {path} ({size}) to {machine}"),
            Some(boxd::CREATE_TIMEOUT),
            provider.upload_reported(machine, path, bytes, &mut |sent, total| {
                boot.waiting(
                    BootStage::SyncFiles,
                    snapshots::uploading(path, sent, total, machine),
                    Some(boxd::CREATE_TIMEOUT),
                );
            }),
        )
        .await?;
        Ok(())
    }

    /// Run `script` on the machine, streaming its output to `stage`.
    async fn run_script(
        &self,
        view: &MachineView,
        stage: BootStage,
        what: &str,
        script: &str,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        let provider = self.fleet.provider();
        let machine = &view.machine.name;
        boot.log(stage, false, format!("$ {what}"));
        let mut lines = boot.output(stage);
        let out = boot
            .wait(
                stage,
                format!("{what} on {machine}"),
                Some(APPLY_TIMEOUT),
                provider.exec_streaming(
                    machine,
                    script,
                    None,
                    APPLY_TIMEOUT,
                    &mut |stderr, chunk| lines.chunk(stderr, chunk),
                ),
            )
            .await?;
        lines.finish();
        if out.success() {
            Ok(())
        } else {
            Err(CloudError::ExecFailed {
                machine: machine.clone(),
                command: what.to_string(),
                exit_code: out.exit_code,
                stderr: out.stderr_tail(400),
            })
        }
    }

    /// Give the machine's node the agent's record, page by page.
    async fn import_record(
        &self,
        request: &PlaceRequest,
        view: &MachineView,
        boot: &BootReporter,
    ) -> CloudResult<()> {
        let node = self.node(&view.agent)?;
        let head = self
            .store
            .get_head_seq(request.agent)
            .map_err(|e| CloudError::io("read record", std::io::Error::other(e.to_string())))?;
        let remote = boot
            .wait(
                BootStage::ImportRecord,
                format!("asking {} how much of the record it has", view.machine.name),
                Some(NODE_TIMEOUT),
                node.head(request.agent),
            )
            .await
            .unwrap_or(0);
        boot.fact("record", format!("{head} entries"));
        let mut from = remote + 1;
        loop {
            boot.progress(
                BootStage::ImportRecord,
                from.saturating_sub(1).min(head),
                head,
            );
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
            let last = (from + count).saturating_sub(1);
            boot.wait(
                BootStage::ImportRecord,
                format!(
                    "{} is importing entries {from}..={last} of {head}",
                    view.machine.name
                ),
                Some(NODE_TIMEOUT),
                node.import(&SyncImport {
                    agent_id: request.agent.to_hex(),
                    name: request.name.clone(),
                    entries,
                    done,
                }),
            )
            .await?;
            if done {
                boot.progress(BootStage::ImportRecord, head, head);
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
    /// destroy the machine, showing each step on `boot`.
    ///
    /// # Errors
    ///
    /// When the node can't be sealed or its record can't be copied; the
    /// agent then stays placed.
    pub async fn recall(
        &self,
        agent: AgentId,
        reason: &str,
        boot: &BootReporter,
    ) -> CloudResult<PullOutcome> {
        let hex = agent.to_hex();
        let Some(placed) = self.placements.lock().agents.get(&hex).cloned() else {
            return Ok(PullOutcome::patch_only(PatchOutcome::Empty));
        };
        boot.fact("machine", placed.machine.clone());
        boot.fact("project folder", placed.project_dir.display().to_string());
        if self.fleet.view(&hex).is_none() {
            // The machine is gone (deleted outside aura): the record the
            // mirror already holds is all there is.
            self.forget_placement(&hex, &placed.project);
            return Ok(PullOutcome::patch_only(PatchOutcome::Saved {
                path: self.home.join(&hex),
                reason: format!(
                    "machine {} no longer exists; its unsynced work is lost",
                    placed.machine
                ),
            }));
        }
        boot.plan(&[
            BootStage::Seal,
            BootStage::CopyRecord,
            BootStage::PullChanges,
            BootStage::Destroy,
        ]);
        let node = self.awake_node(&hex, boot).await?;
        let sealed = boot.run(BootStage::Seal, node.seal(agent)).await?;
        boot.fact("sealed at", format!("entry {}", sealed.head_seq));
        boot.run(BootStage::CopyRecord, async {
            let copied = self.mirror_from(&node, agent).await?;
            boot.fact("copied", format!("{copied} new entries"));
            let local = self
                .store
                .get_head_seq(agent)
                .map_err(|e| CloudError::io("read record", std::io::Error::other(e.to_string())))?;
            boot.progress(
                BootStage::CopyRecord,
                local.min(sealed.head_seq),
                sealed.head_seq,
            );
            if local < sealed.head_seq {
                return Err(CloudError::Node {
                    node: node.node().to_string(),
                    op: "recall",
                    message: format!("copied up to {local} of {} entries", sealed.head_seq),
                });
            }
            Ok(())
        })
        .await?;
        let outcome = boot
            .run(BootStage::PullChanges, async {
                let patch = match node.patch().await {
                    Ok(patch) => {
                        self.apply_patch(&hex, &placed.project_dir, &patch.patch, patch.files)
                            .await
                    }
                    Err(err) => PatchOutcome::Saved {
                        path: self.home.join(&hex),
                        reason: format!("could not read the machine's changes: {err}"),
                    },
                };
                if let PatchOutcome::Applied { files } = &patch {
                    for file in files {
                        boot.log(BootStage::PullChanges, false, format!("M {file}"));
                    }
                }
                boot.fact("changes", patch.describe());
                let commits = self.fetch_commits(&hex, &placed).await;
                if let Some(report) = commits.describe() {
                    let kept = matches!(commits, CommitsOutcome::Kept { .. });
                    boot.log(BootStage::PullChanges, kept, report.clone());
                    boot.fact("commits", report);
                }
                Ok::<_, CloudError>(PullOutcome { patch, commits })
            })
            .await?;
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
        let destroyed = boot
            .run(BootStage::Destroy, async {
                Ok::<_, CloudError>(self.fleet.destroy(&hex, reason).await)
            })
            .await?;
        match destroyed {
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
                boot.log(
                    BootStage::Destroy,
                    true,
                    format!("could not destroy it ({err}); the orphan sweep will"),
                );
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

    /// Pull `agent`'s work into the project without moving the agent:
    /// its working tree's changes into the project folder, its commits
    /// onto the project's branch of the same name.
    ///
    /// # Errors
    ///
    /// When the node can't be reached.
    pub async fn pull(&self, agent: AgentId) -> CloudResult<PullOutcome> {
        let hex = agent.to_hex();
        let Some(placed) = self.placements.lock().agents.get(&hex).cloned() else {
            return Ok(PullOutcome::patch_only(PatchOutcome::Empty));
        };
        let node = self.awake_node(&hex, &BootReporter::none()).await?;
        let patch = node.patch().await?;
        let tree = patch.tree.clone();
        let outcome = self
            .apply_patch(&hex, &placed.project_dir, &patch.patch, patch.files)
            .await;
        if matches!(outcome, PatchOutcome::Applied { .. }) {
            // The machine keeps working; the next pull starts from what
            // this one took.
            self.mark_pulled(&hex, tree.as_deref()).await;
        }
        let commits = self.fetch_commits(&hex, &placed).await;
        Ok(PullOutcome {
            patch: outcome,
            commits,
        })
    }

    /// Tell `hex`'s machine its patch up to `tree` is in the project.
    /// Machines from before the patch named its tree mark all they have.
    async fn mark_pulled(&self, hex: &str, tree: Option<&str>) {
        let Some(view) = self.fleet.view(hex) else {
            return;
        };
        let script = match tree {
            Some(tree) => match workspace::mark_pulled(tree) {
                Ok(script) => script,
                Err(err) => {
                    tracing::warn!(agent = %hex, error = %err, "the machine named a bad tree; the next pull repeats this one");
                    return;
                }
            },
            None => delta::MARK_PULLED_LEGACY.to_string(),
        };
        let marked = self
            .fleet
            .provider()
            .exec(&view.machine.name, &script, None, GIT_SCRIPT_TIMEOUT)
            .await;
        match marked {
            Ok(out) if out.success() => {}
            Ok(out) => {
                tracing::warn!(agent = %hex, stderr = %out.stderr_tail(300), "could not mark the pulled work on the machine; the next pull repeats it");
            }
            Err(err) => {
                tracing::warn!(agent = %hex, error = %err, "could not mark the pulled work on the machine; the next pull repeats it");
            }
        }
    }

    /// Fetch the commits on `placed`'s branch that the project here
    /// lacks into the project's branch of the same name.
    async fn fetch_commits(&self, hex: &str, placed: &PlacedAgent) -> CommitsOutcome {
        let Some(branch) = placed.branch.clone() else {
            return CommitsOutcome::None;
        };
        let Some(view) = self.fleet.view(hex) else {
            return CommitsOutcome::None;
        };
        match self.fetch_branch(hex, placed, &view, &branch).await {
            Ok(outcome) => outcome,
            Err(err) => {
                tracing::warn!(agent = %hex, error = %err, "could not fetch the agent's commits");
                CommitsOutcome::Kept {
                    branch,
                    reason: err.to_string(),
                }
            }
        }
    }

    async fn fetch_branch(
        &self,
        hex: &str,
        placed: &PlacedAgent,
        view: &MachineView,
        branch: &str,
    ) -> CloudResult<CommitsOutcome> {
        let provider = self.fleet.provider();
        let machine = &view.machine.name;
        // The bundle leaves out what the hub last took; only while it
        // still has it.
        let fetched = match placed.fetched_tip.clone() {
            Some(tip) => {
                let project = placed.project_dir.clone();
                let check = tip.clone();
                tokio::task::spawn_blocking(move || delta::has_commit(&project, &check))
                    .await
                    .unwrap_or(false)
                    .then_some(tip)
            }
            None => None,
        };
        let script = workspace::bundle_branch(branch, fetched.as_deref())?;
        let out = provider
            .exec(machine, &script, None, GIT_SCRIPT_TIMEOUT)
            .await?;
        if !out.success() {
            return Err(CloudError::ExecFailed {
                machine: machine.clone(),
                command: "bundle the agent's branch".into(),
                exit_code: out.exit_code,
                stderr: out.stderr_tail(400),
            });
        }
        let commits = match workspace::parse_bundled(&out.stdout_text()) {
            Some(Bundled::Nothing) => return Ok(CommitsOutcome::None),
            Some(Bundled::Commits { commits, .. }) => commits,
            None => {
                return Err(CloudError::ExecFailed {
                    machine: machine.clone(),
                    command: "bundle the agent's branch".into(),
                    exit_code: out.exit_code,
                    stderr: "it did not say what it bundled".into(),
                })
            }
        };
        let bytes = provider
            .download(machine, workspace::OUT_BUNDLE, delta::MAX_DELTA_BYTES)
            .await?;
        let dir = self.home.join(hex);
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|err| CloudError::io("save the agent's commits", err))?;
        let path = dir.join(format!("branch-{}.bundle", now_ms()));
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|err| CloudError::io("save the agent's commits", err))?;
        let project = placed.project_dir.clone();
        let bundle = path.clone();
        let name = branch.to_string();
        let fetched = tokio::time::timeout(
            LOCAL_GIT_TIMEOUT,
            tokio::task::spawn_blocking(move || delta::fetch_branch(&project, &bundle, &name)),
        )
        .await;
        let _ = tokio::fs::remove_file(&path).await;
        let fetched = match fetched {
            Ok(Ok(fetched)) => fetched?,
            Ok(Err(err)) => {
                return Err(CloudError::io(
                    "fetch the agent's commits",
                    std::io::Error::other(err.to_string()),
                ))
            }
            Err(_) => {
                return Err(CloudError::io(
                    "fetch the agent's commits",
                    std::io::Error::new(std::io::ErrorKind::TimedOut, "git fetch took too long"),
                ))
            }
        };
        let branch = branch.to_string();
        Ok(match fetched {
            BranchFetch::Updated { tip } => {
                if let Some(placed) = self.placements.lock().agents.get_mut(hex) {
                    placed.fetched_tip = Some(tip);
                }
                self.save_placements();
                CommitsOutcome::Fetched { branch, commits }
            }
            BranchFetch::CheckedOut => CommitsOutcome::Kept {
                reason: format!(
                    "{branch} is checked out here; switch to another branch and pull again"
                ),
                branch,
            },
            BranchFetch::Diverged => CommitsOutcome::Kept {
                reason: format!(
                    "{branch} here has commits the agent's branch lacks; rename it and pull again"
                ),
                branch,
            },
        })
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
        let applied = tokio::time::timeout(
            LOCAL_GIT_TIMEOUT,
            tokio::process::Command::new("git")
                .arg("-C")
                .arg(project)
                .args(["apply", "--3way", "--whitespace=nowarn"])
                .arg(&path)
                .kill_on_drop(true)
                .output(),
        )
        .await;
        match applied {
            Ok(Ok(out)) if out.status.success() => {
                let _ = tokio::fs::remove_file(&path).await;
                PatchOutcome::Applied { files }
            }
            Ok(Ok(out)) => PatchOutcome::Saved {
                path,
                reason: String::from_utf8_lossy(&out.stderr)
                    .trim()
                    .chars()
                    .take(300)
                    .collect(),
            },
            Ok(Err(err)) => PatchOutcome::Saved {
                path,
                reason: format!("git apply: {err}"),
            },
            Err(_) => PatchOutcome::Saved {
                path,
                reason: "git apply took too long".into(),
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
            // A machine that sleeps shows itself waking; the events all
            // reach the turn's stream before anything else does.
            let (boot, mut events) = BootReporter::channel();
            let shown = tx.clone();
            let forward = tokio::spawn(async move {
                while let Some(event) = events.recv().await {
                    if shown.send(RemoteEvent::Boot(event)).await.is_err() {
                        break;
                    }
                }
            });
            let awake = hub.awake_node(&hex, &boot).await;
            drop(boot);
            let _ = forward.await;
            let node = match awake {
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

    /// Wake `agent`'s machine and reopen its mesh socket, showing the
    /// wake on `boot`.
    ///
    /// # Errors
    ///
    /// When it has no machine or waking fails.
    pub async fn wake(self: &Arc<Self>, agent: AgentId, boot: &BootReporter) -> CloudResult<()> {
        let hex = agent.to_hex();
        self.fleet.ensure_awake_reported(&hex, boot).await?;
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
    fn take_mail(&self, mail: &MeshMail) -> MeshFrame {
        let ack = |delivered: bool, error: Option<String>| MeshFrame::MailAck {
            from_node: mail.from_node.clone(),
            id: mail.id,
            delivered,
            error,
        };
        if !self.mail.first_sight(mail) {
            return ack(true, None);
        }
        // Mail never crosses users: the sender's machine owner must own
        // the recipient (a cloud agent) or be the user signed in here (a
        // local agent).
        let sender = self.directory.owner_of_node(&mail.from_node);
        let recipient = self
            .directory
            .owner_of(&mail.to)
            .unwrap_or_else(|| names::owner_tag(&self.current_owner()));
        if sender.as_deref() != Some(recipient.as_str()) {
            self.mail.forget(mail);
            return ack(
                false,
                Some(format!("agent {} belongs to another user", mail.to)),
            );
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
                let delivered = match (host, remote_mail(mail)) {
                    (Some(host), Ok(remote)) => host.deliver_remote(remote),
                    (None, _) => Err("the hub has no agents".to_string()),
                    (_, Err(err)) => Err(err),
                };
                match delivered {
                    Ok(()) => ack(true, None),
                    Err(err) => {
                        // A resend may succeed (the agent restarted).
                        self.mail.forget(mail);
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
        agent_permissions: cloud_agent_permissions(),
        tool_permissions: None,
        agent_capabilities: AgentCapabilities::default(),
        auth_jwt,
        user_id: "hub".to_string(),
        mode: None,
    }
}

/// What a cloud agent may do on its machine. Sent explicitly: an empty
/// bundle means full access on a node, which includes computer use, and
/// a cloud machine has no screen (and OpenRouter refuses the computer
/// tool's server-tool entry).
fn cloud_agent_permissions() -> AgentPermissionsWire {
    use aura_core_protocol::CapabilityWire;
    AgentPermissionsWire {
        capabilities: vec![
            CapabilityWire::SpawnAgent,
            CapabilityWire::ControlAgent,
            CapabilityWire::ReadAgent,
            CapabilityWire::ListAgents,
            CapabilityWire::InvokeProcess,
            CapabilityWire::ReadAllProjects,
            CapabilityWire::WriteAllProjects,
        ],
        ..AgentPermissionsWire::default()
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
    let mut plans = tokio::time::interval(SYNC_EVERY);
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
            _ = plans.tick() => {
                let Some(hub) = hub.upgrade() else { return };
                if let Some(sync) = hub.plan_sync() {
                    sync.tick(&hub.owners()).await;
                }
                let repos = hub.placed_repos();
                if !repos.is_empty() {
                    hub.repo_sync().tick(&repos).await;
                }
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
                            let ack = hub.take_mail(&mail);
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
        let me = names::owner_tag(&hub.current_owner());
        hub.directory
            .holder(&agent.to_hex())
            .filter(|holder| holder.node != HUB_NODE && holder.owner == me)
            .map(|holder| holder.name)
    }

    fn route(&self, mail: RemoteMail) -> Result<(), String> {
        let hub = self.0.upgrade().ok_or("the cloud hub stopped")?;
        let to = mail.to.to_hex();
        if !hub.directory.is_remote(&to) {
            return Err(format!("unknown agent {}", mail.to));
        }
        let me = names::owner_tag(&hub.current_owner());
        if hub.directory.owner_of(&to).as_deref() != Some(me.as_str()) {
            return Err(format!("agent {} belongs to another user", mail.to));
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
        let me = hub.current_owner();
        hub.placed()
            .into_iter()
            .filter(|(_, placed)| placed.owner() == me)
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

    #[test]
    fn choose_binary_prefers_env_then_cloud_home_then_running_exe() {
        let home = tempfile::tempdir().expect("tempdir");
        let exe = PathBuf::from("/usr/bin/aura-running");
        let env = PathBuf::from("/opt/aura-linux");

        // Nothing built in the cloud home: the running executable.
        assert_eq!(
            choose_binary(None, home.path(), Some(exe.clone())),
            AuraBinary::Upload(exe.clone())
        );
        assert_eq!(
            choose_binary(None, home.path(), None),
            AuraBinary::Installed
        );

        // A build in the cloud home wins over the running executable.
        let built = aura_engine_cloud::snapshots::cloud_binary_path(home.path());
        std::fs::create_dir_all(built.parent().expect("parent")).expect("mkdir");
        std::fs::write(&built, b"elf").expect("write");
        assert_eq!(
            choose_binary(None, home.path(), Some(exe.clone())),
            AuraBinary::Upload(built)
        );

        // AURA_CLOUD_BINARY wins over both.
        assert_eq!(
            choose_binary(Some(&env), home.path(), Some(exe)),
            AuraBinary::Upload(env)
        );
    }

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
        place_cloud(&hub, AgentId::new([2; 32]), "aura-x", &Owner::Local);
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
        let MeshFrame::MailAck { delivered, .. } = hub.take_mail(&mail) else {
            panic!("not an ack");
        };
        assert!(!delivered);
        let MeshFrame::MailAck { delivered, .. } = hub.take_mail(&mail) else {
            panic!("not an ack");
        };
        assert!(
            !delivered,
            "a failed delivery is forgotten, so the resend is tried again"
        );
    }

    fn place_cloud(hub: &CloudHub, agent: AgentId, node: &str, owner: &Owner) {
        hub.directory.place(
            &agent.to_hex(),
            Holder {
                node: node.into(),
                url: Some(format!("https://{node}.boxd.sh")),
                project: "p1".into(),
                name: format!("agent on {node}"),
                owner: names::owner_tag(owner),
            },
        );
    }

    fn test_git(dir: &Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn test_commit(dir: &Path, file: &str, message: &str) -> String {
        std::fs::write(dir.join(file), message).expect("write");
        test_git(dir, &["add", "-A"]);
        test_git(
            dir,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                message,
            ],
        );
        test_git(dir, &["rev-parse", "HEAD"])
    }

    /// A project with one commit and `origin`, if any.
    fn project_repo(dir: &Path, origin: Option<&str>) {
        std::fs::create_dir_all(dir).expect("mkdir");
        test_git(dir, &["init", "-q", "-b", "main"]);
        if let Some(url) = origin {
            test_git(dir, &["remote", "add", "origin", url]);
        }
        test_commit(dir, "a.txt", "first");
    }

    fn request_in(project: &Path) -> PlaceRequest {
        PlaceRequest {
            agent: AgentId::new([1; 32]),
            agent_zns: "0://scout".into(),
            name: "scout".into(),
            code: Some("SC01".into()),
            project_id: "p1".into(),
            project_dir: project.to_path_buf(),
            build_command: None,
            fork_from: None,
            owner: Owner::Local,
            owner_name: None,
        }
    }

    #[tokio::test]
    async fn a_project_on_github_gives_its_machine_a_repository_secret_and_the_bot() {
        let (hub, fake, dir) = hub();
        let tokens = Arc::new(aura_engine_cloud::github::FakeRepoTokens::new(
            "aura-agents",
        ));
        let repo = RepoRef {
            owner: "acme".into(),
            name: "proj".into(),
        };
        tokens.install(&repo, "ghs_secret", now_ms() + 3_600_000);
        hub.set_repo_tokens(Some(tokens));

        let project = dir.path().join("project");
        project_repo(&project, Some("git@github.com:acme/proj.git"));
        let share = hub
            .share_repo(&request_in(&project), &BootReporter::none())
            .await
            .expect("share");
        assert_eq!(share.repo, Some(repo.clone()));
        let secret = share.secret.clone().expect("secret");
        assert!(names::is_repo_secret(&secret), "{secret}");
        assert_eq!(fake.secret_value(&secret).as_deref(), Some("ghs_secret"));
        let claim = CloudHub::git_claim(
            Some("aura/scout-sc01"),
            share.repo.as_ref(),
            share.secret.clone(),
            share.identity.as_ref(),
        )
        .expect("claim");
        assert_eq!(claim.branch, "aura/scout-sc01");
        assert_eq!(
            claim.remote_url.as_deref(),
            Some("https://github.com/acme/proj.git")
        );
        assert_eq!(claim.env.as_deref(), Some(secret.as_str()));
        assert_eq!(claim.placeholder, None);
        assert_eq!(claim.author_name.as_deref(), Some("aura-agents[bot]"));
        assert_eq!(
            claim.author_email.as_deref(),
            Some("41000000+aura-agents[bot]@users.noreply.github.com")
        );
        assert!(hub.repo_status(&repo).is_some_and(|status| status.linked));

        // Not installed: the secret holds no token, and placing goes on.
        let other = dir.path().join("other");
        project_repo(&other, Some("https://github.com/acme/other.git"));
        let share = hub
            .share_repo(&request_in(&other), &BootReporter::none())
            .await
            .expect("share");
        let secret = share.secret.expect("secret");
        assert_eq!(
            fake.secret_value(&secret).as_deref(),
            Some(aura_engine_cloud::repos::UNLINKED)
        );
        let status = hub
            .repo_status(share.repo.as_ref().expect("repo"))
            .expect("status");
        assert!(!status.linked);
        assert_eq!(
            status.install_url.as_deref(),
            Some("https://github.com/apps/aura-agents/installations/new")
        );

        // No origin, or one off GitHub: no secret, and still a branch.
        for (name, origin) in [
            ("local", None),
            ("gitlab", Some("https://gitlab.com/acme/proj.git")),
        ] {
            let project = dir.path().join(name);
            project_repo(&project, origin);
            let share = hub
                .share_repo(&request_in(&project), &BootReporter::none())
                .await
                .expect("share");
            assert!(share.repo.is_none() && share.secret.is_none(), "{name}");
            let claim =
                CloudHub::git_claim(Some("aura/scout-sc01"), None, None, None).expect("claim");
            assert_eq!((claim.remote_url, claim.env), (None, None));
        }
        assert_eq!(CloudHub::git_claim(None, Some(&repo), None, None), None);
    }

    /// Answer the fake's execs by running them against `workspace`, with
    /// `/tmp/` under `root`, and hand back the bundle they leave.
    fn run_scripts_in(fake: &FakeProvider, root: &Path, workspace_dir: &Path) {
        let files = fake.clone();
        let root = root.to_path_buf();
        let workspace_dir = workspace_dir.to_path_buf();
        fake.on_exec(Arc::new(move |machine, command, _stdin| {
            let out = std::process::Command::new("sh")
                .arg("-c")
                .arg(command.replace("/tmp/", &format!("{}/", root.display())))
                .env("AURA_WORKSPACE", &workspace_dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .expect("sh");
            if let Ok(bytes) = std::fs::read(root.join("aura-out.bundle")) {
                files.put_file(machine, workspace::OUT_BUNDLE, bytes);
            }
            aura_engine_cloud::ExecOutput {
                exit_code: out.status.code().unwrap_or(-1),
                stdout: out.stdout,
                stderr: out.stderr,
            }
        }));
    }

    /// Place `agent` on a fake machine as a restart would find it.
    async fn adopt_machine(
        hub: &CloudHub,
        fake: &FakeProvider,
        agent: AgentId,
        project: &Path,
        branch: Option<&str>,
    ) -> PlacedAgent {
        use aura_engine_cloud::{MachineInfo, MachineStatus};
        let name = "aura-sc01-0101";
        fake.insert_machine(MachineInfo {
            id: "vm-1".into(),
            name: name.into(),
            status: MachineStatus::Running,
            region: String::new(),
            vcpu: 4,
            memory_bytes: 0,
            disk_bytes: 0,
            domain: format!("{name}.boxd.sh"),
            public_ip: String::new(),
            ssh_port: 0,
            isolated: true,
            labels: std::collections::BTreeMap::new(),
            source: None,
            created_ms: 0,
            boot_ms: 0,
        });
        hub.fleet
            .adopt(&agent.to_hex(), "0://scout", "p1", name, Vec::new(), None)
            .await
            .expect("adopt");
        let placed = PlacedAgent {
            zns: "0://scout".into(),
            name: "scout".into(),
            project: "p1".into(),
            project_dir: project.to_path_buf(),
            machine: name.into(),
            owner: None,
            owner_name: None,
            branch: branch.map(str::to_string),
            repo: None,
            fetched_tip: None,
        };
        hub.placements
            .lock()
            .agents
            .insert(agent.to_hex(), placed.clone());
        placed
    }

    #[tokio::test]
    async fn the_agents_commits_come_back_on_its_branch_once_each() {
        let (hub, fake, dir) = hub();
        let project = dir.path().join("project");
        project_repo(&project, None);
        let root = dir.path().join("vm");
        std::fs::create_dir_all(&root).expect("mkdir");
        let machine = root.join("workspace");
        test_git(
            dir.path(),
            &[
                "clone",
                "-q",
                &project.display().to_string(),
                &machine.display().to_string(),
            ],
        );
        test_git(&machine, &["update-ref", workspace::HUB_HEAD_REF, "HEAD"]);
        let branch = "aura/scout-sc01";
        test_git(&machine, &["checkout", "-q", "-b", branch]);
        test_commit(&machine, "b.txt", "agent work");
        let tip = test_commit(&machine, "c.txt", "more agent work");
        run_scripts_in(&fake, &root, &machine);
        let agent = AgentId::new([1; 32]);
        let hex = agent.to_hex();
        let placed = adopt_machine(&hub, &fake, agent, &project, Some(branch)).await;

        assert_eq!(
            hub.fetch_commits(&hex, &placed).await,
            CommitsOutcome::Fetched {
                branch: branch.into(),
                commits: 2
            }
        );
        assert_eq!(
            test_git(&project, &["rev-parse", &format!("refs/heads/{branch}")]),
            tip
        );
        // The project's own branch and files are left as they were.
        assert_eq!(test_git(&project, &["branch", "--show-current"]), "main");
        assert!(!project.join("b.txt").exists());

        // What came is remembered: nothing new comes again.
        let placed = hub.placed_agent(agent).expect("placed");
        assert_eq!(placed.fetched_tip.as_deref(), Some(tip.as_str()));
        assert_eq!(hub.fetch_commits(&hex, &placed).await, CommitsOutcome::None);
        test_commit(&machine, "d.txt", "last");
        assert_eq!(
            hub.fetch_commits(&hex, &placed).await,
            CommitsOutcome::Fetched {
                branch: branch.into(),
                commits: 1
            }
        );

        // A branch checked out here is not moved under the user.
        test_git(&project, &["checkout", "-q", branch]);
        test_commit(&machine, "e.txt", "after the checkout");
        let placed = hub.placed_agent(agent).expect("placed");
        let CommitsOutcome::Kept { reason, .. } = hub.fetch_commits(&hex, &placed).await else {
            panic!("fetched into the checked-out branch");
        };
        assert!(reason.contains("checked out here"), "{reason}");

        // A placement from before agent branches has none to fetch.
        let legacy = PlacedAgent {
            branch: None,
            ..placed
        };
        assert_eq!(hub.fetch_commits(&hex, &legacy).await, CommitsOutcome::None);
    }

    fn alice() -> Owner {
        Owner::Zos {
            user_id: "alice".into(),
        }
    }

    fn bob() -> Owner {
        Owner::Zos {
            user_id: "bob".into(),
        }
    }

    fn mail_to(from: AgentId, from_node: &str, to: AgentId, id: u64) -> MeshMail {
        MeshMail {
            id,
            from_node: from_node.into(),
            thread: format!("{from_node}:t1"),
            subject: "s".into(),
            from: from.to_hex(),
            from_name: "scout".into(),
            to: to.to_hex(),
            kind: MeshMailKind::Request,
            text: "hi".into(),
            sent_ms: 0,
        }
    }

    fn ack_error(frame: MeshFrame) -> Option<String> {
        let MeshFrame::MailAck {
            delivered, error, ..
        } = frame
        else {
            panic!("not an ack");
        };
        (!delivered).then(|| error.unwrap_or_default())
    }

    #[test]
    fn mail_never_crosses_users() {
        let (hub, _fake, _dir) = hub();
        hub.set_current_owner(alice());
        let local = AgentId::new([1; 32]);
        hub.note_local(local, "local", "p1");
        let alice_cloud = AgentId::new([2; 32]);
        let bob_cloud = AgentId::new([3; 32]);
        place_cloud(&hub, alice_cloud, "aura-alice", &alice());
        place_cloud(&hub, bob_cloud, "aura-bob", &bob());

        // Bob's machine can't mail Alice's local agent or her cloud agent.
        let err = ack_error(hub.take_mail(&mail_to(bob_cloud, "aura-bob", local, 1)));
        assert!(err.is_some_and(|e| e.contains("another user")));
        let err = ack_error(hub.take_mail(&mail_to(bob_cloud, "aura-bob", alice_cloud, 2)));
        assert!(err.is_some_and(|e| e.contains("another user")));
        // Alice's machine reaches her local agent (no host here: refused
        // for that reason, not for ownership).
        let err = ack_error(hub.take_mail(&mail_to(alice_cloud, "aura-alice", local, 3)));
        assert!(err.is_some_and(|e| !e.contains("another user")));

        // Local agents act as Alice: Bob's cloud agent is unknown to them.
        let router = HubRouter(Arc::downgrade(&hub));
        assert_eq!(router.name_of(bob_cloud), None);
        assert!(router.name_of(alice_cloud).is_some());
        let err = router
            .route(RemoteMail {
                from: local,
                from_name: "local".into(),
                to: bob_cloud,
                kind: MessageKind::Request,
                thread: ThreadId("t1".into()),
                subject: "s".into(),
                text: "hi".into(),
            })
            .expect_err("cross-user");
        assert!(err.contains("another user"), "{err}");

        // Each node's directory holds only its owner's agents.
        let MeshFrame::Directory { peers } = hub.directory.frame_for("aura-bob", "p1") else {
            panic!("not a directory");
        };
        assert!(peers.is_empty(), "{peers:?}");
    }

    #[tokio::test]
    async fn an_agent_that_was_never_placed_recalls_to_nothing() {
        let (hub, _fake, _dir) = hub();
        let agent = AgentId::new([5; 32]);
        assert!(!hub.is_placed(agent));
        assert_eq!(
            hub.recall(agent, "test", &BootReporter::none())
                .await
                .expect("recall"),
            PullOutcome::patch_only(PatchOutcome::Empty)
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
        assert_eq!(
            PullOutcome {
                patch: PatchOutcome::Empty,
                commits: CommitsOutcome::Fetched {
                    branch: "aura/scout-a1b2".into(),
                    commits: 2,
                },
            }
            .describe(),
            "no changes to pull; 2 commit(s) on branch aura/scout-a1b2"
        );
        assert_eq!(
            PullOutcome::patch_only(PatchOutcome::Empty).describe(),
            "no changes to pull"
        );
    }

    #[test]
    fn cloud_turns_never_offer_computer_use() {
        let request = chat_request(AgentId::new([1; 32]), None, None);
        let json = serde_json::to_value(&request.agent_permissions).expect("json");
        let text = json.to_string();
        assert!(!text.contains("computer"), "{text}");
        assert!(
            text.contains("spawnAgent") || text.contains("spawn_agent"),
            "{text}"
        );
    }
}
