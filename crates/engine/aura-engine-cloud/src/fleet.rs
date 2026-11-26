//! The machine of every cloud agent and its lifecycle.
//!
//! ```text
//! Provisioning -> Claiming -> Ready <-> Busy
//!                               |  idle
//!                               v
//!                  (Standby) -> Hibernated -> Waking -> Ready
//! any -> Destroying (archive) ; any -> Error
//! ```
//!
//! ## Invariants
//!
//! - One machine per cloud agent; the fleet is the only code that
//!   creates, sleeps, wakes or destroys agent machines, and the only
//!   writer of their ledger rows ([`VmLedger`]).
//! - A machine with work in flight ([`VmFleet::begin_work`]) is never
//!   put to sleep.
//! - Node tokens are minted here, held in memory only, and sent to the
//!   node over boxd's authenticated `Exec` (`aura node claim --stdin`).
//!   After a hub restart the machine is claimed again with fresh tokens.
//!
//! ## Failure modes
//!
//! A failed claim destroys the new machine (it would be unreachable) and
//! records an `error` row. A machine that disappears is dropped with an
//! `error` row. Usage calls refused for a non-admin key are remembered,
//! and costs fall back to the estimate.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use aura_config::{BoxdToken, CloudConfig, MachineSize};
use aura_core_protocol::cloud::{GitClaim, NodeClaim, NodeClaimed, PlanTemplate};
use aura_core_types::{VmEvent, VmPayload, VmResources, VmSource};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::boot::{human_bytes, BootReporter, BootStage};
use crate::boxd::{CALL_TIMEOUT, CREATE_TIMEOUT};
use crate::error::{CloudError, CloudResult};
use crate::machine_env::MachineEnv;
use crate::names::{
    self, LABEL_AGENT, LABEL_INSTALL, LABEL_OWNER, LABEL_PROJECT, LABEL_ROLE, ROLE_AGENT,
};
use crate::provider::{
    MachineInfo, MachineOrigin, MachineStatus, NewMachine, UsageReading, VmProvider,
};

/// Command a node is claimed with; reads a [`NodeClaim`] on stdin and
/// prints a [`NodeClaimed`].
pub const CLAIM_COMMAND: &str = "aura node claim --stdin";
/// Limit on a claim.
pub const CLAIM_TIMEOUT: Duration = Duration::from_secs(60);
/// boxd's own hibernation backstop for agent machines, seconds.
pub const AUTO_HIBERNATE_BACKSTOP_SECS: u32 = 1800;

/// Writes machine rows to ledger agents. Implemented by the hub over its
/// store; a failure to write is logged by the implementation, never
/// allowed to stop the fleet.
pub trait VmLedger: Send + Sync {
    /// Append `payload` to the ledger agent `ledger_zns`.
    fn record(&self, ledger_zns: &str, payload: &VmPayload);
}

/// A ledger that keeps rows in memory, for tests and previews.
#[derive(Debug, Default)]
pub struct MemoryLedger(Mutex<Vec<(String, VmPayload)>>);

impl MemoryLedger {
    /// Rows so far.
    #[must_use]
    pub fn rows(&self) -> Vec<(String, VmPayload)> {
        self.0.lock().clone()
    }

    /// Events written to `ledger`, in order.
    #[must_use]
    pub fn events(&self, ledger: &str) -> Vec<VmEvent> {
        self.0
            .lock()
            .iter()
            .filter(|(zns, _)| zns == ledger)
            .map(|(_, row)| row.event)
            .collect()
    }
}

impl VmLedger for MemoryLedger {
    fn record(&self, ledger_zns: &str, payload: &VmPayload) {
        self.0
            .lock()
            .push((ledger_zns.to_string(), payload.clone()));
    }
}

/// Where a cloud agent's machine is in its lifecycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VmState {
    /// Being created.
    Provisioning,
    /// Created; its node is taking the hub's tokens.
    Claiming,
    /// Running, nothing in flight.
    Ready,
    /// Running a turn, a command or a sync.
    Busy,
    /// On standby (memory in RAM).
    Standby,
    /// Hibernated (memory on disk).
    Hibernated,
    /// Coming back from sleep.
    Waking,
    /// Being destroyed.
    Destroying,
    /// Something failed.
    Error(String),
}

impl VmState {
    /// Short label for the footer and HOST.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Provisioning => "creating",
            Self::Claiming => "claiming",
            Self::Ready => "ready",
            Self::Busy => "busy",
            Self::Standby => "standby",
            Self::Hibernated => "asleep",
            Self::Waking => "waking",
            Self::Destroying => "destroying",
            Self::Error(_) => "error",
        }
    }

    /// Whether the machine is asleep.
    #[must_use]
    pub const fn is_asleep(&self) -> bool {
        matches!(self, Self::Standby | Self::Hibernated)
    }
}

/// A cloud agent's machine as the UI and the hub see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineView {
    /// Agent, hex.
    pub agent: String,
    /// Owner tag of the user it belongs to.
    pub owner: String,
    /// Project id.
    pub project: String,
    /// Ledger agent ZNS id.
    pub ledger: String,
    /// The machine.
    pub machine: MachineInfo,
    /// Lifecycle state.
    pub state: VmState,
    /// Work in flight.
    pub busy: u32,
    /// Seconds since the last work ended (0 while busy).
    pub idle_secs: u64,
    /// Seconds until it hibernates, while idle.
    pub sleeps_in_secs: Option<u64>,
    /// The last meter reading.
    pub usage: Option<UsageReading>,
    /// Whether boxd refused usage calls (not an org admin key).
    pub usage_denied: bool,
    /// When the fleet last saw it, Unix milliseconds.
    pub seen_ms: u64,
}

/// What to provision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProvisionRequest {
    /// Agent id, hex.
    pub agent: String,
    /// Agent ZNS id, for the ledger name.
    pub agent_zns: String,
    /// The agent's code (`RE36`), for the machine name.
    pub code: Option<String>,
    /// Project id.
    pub project: String,
    /// Machine name to use; derived from the code and id when `None`.
    pub name: String,
    /// What to make it from.
    pub origin: MachineOrigin,
    /// Agents on the copy to seal (forks).
    pub seal: Vec<String>,
    /// Owner tag of the user placing the agent ([`names::owner_tag`]).
    pub owner: String,
    /// The owner's plan secrets the machine lists (only theirs; forks
    /// inherit their parent's, which must have the same owner).
    pub secrets: Vec<String>,
    /// How the machine's node calls the owner's plans (sent in its claim).
    pub plans: Vec<PlanTemplate>,
    /// The project's repository secret the machine lists too
    /// ([`names::repo_secret`]), when the project is on GitHub.
    pub repo_secret: Option<String>,
    /// The agent's branch and how its git reaches GitHub (sent in its
    /// claim).
    pub git: Option<GitClaim>,
}

struct Entry {
    owner: String,
    plans: Vec<PlanTemplate>,
    git: Option<GitClaim>,
    project: String,
    ledger: String,
    machine: MachineInfo,
    state: VmState,
    busy: u32,
    idle_since: Instant,
    node_token: Option<BoxdToken>,
    usage: Option<UsageReading>,
    seen_ms: u64,
}

/// Key of a mesh token: one per owner and project.
fn mesh_key(owner: &str, project: &str) -> String {
    format!("{owner}/{project}")
}

/// Settings of the fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetSettings {
    /// This install's id, on every machine's [`LABEL_INSTALL`].
    pub install: String,
    /// Size of fresh machines.
    pub size: MachineSize,
    /// Create machines isolated.
    pub isolated: bool,
    /// Hibernate after this long idle; zero never.
    pub idle_hibernate: Duration,
    /// Go to standby after this long idle first; zero skips standby.
    pub idle_standby: Duration,
}

impl FleetSettings {
    /// Settings from `config` for `install`.
    #[must_use]
    pub fn from_config(config: &CloudConfig, install: impl Into<String>) -> Self {
        Self {
            install: install.into(),
            size: config.size,
            isolated: config.isolated,
            idle_hibernate: Duration::from_secs(config.idle_hibernate_secs),
            idle_standby: Duration::from_secs(config.idle_standby_secs),
        }
    }
}

/// Every cloud agent's machine.
pub struct VmFleet {
    provider: Arc<dyn VmProvider>,
    ledger: Arc<dyn VmLedger>,
    settings: FleetSettings,
    machines: Mutex<HashMap<String, Entry>>,
    mesh_tokens: Mutex<HashMap<String, BoxdToken>>,
    usage_denied: Mutex<bool>,
}

fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

/// What boxd says about a machine it just made, as boot facts.
fn machine_facts(boot: &BootReporter, machine: &MachineInfo, networks: &[String]) {
    boot.fact("machine", machine.name.clone());
    boot.fact(
        "region",
        if machine.region.is_empty() {
            "home region".to_string()
        } else {
            machine.region.clone()
        },
    );
    boot.fact(
        "size",
        format!(
            "{} vCPU · {} memory · {} disk",
            machine.vcpu,
            human_bytes(machine.memory_bytes),
            human_bytes(machine.disk_bytes)
        ),
    );
    if let Some(source) = &machine.source {
        boot.fact(
            "source",
            match source {
                VmSource::Snapshot { name, version } => match version {
                    Some(version) => format!("snapshot {name} v{version}"),
                    None => format!("snapshot {name}"),
                },
                VmSource::Fork { parent } => format!("fork of {parent}"),
                VmSource::Image { image } if image.is_empty() => "boxd default image".into(),
                VmSource::Image { image } => format!("image {image}"),
            },
        );
    }
    boot.fact("boot time", format!("{} ms", machine.boot_ms));
    boot.fact(
        "network",
        if machine.isolated {
            "isolated".to_string()
        } else if networks.is_empty() {
            "default".to_string()
        } else {
            networks.join(", ")
        },
    );
    if !machine.domain.is_empty() {
        boot.fact("domain", format!("https://{}", machine.domain));
    }
    if !machine.public_ip.is_empty() {
        boot.fact("address", machine.public_ip.clone());
    }
}

/// What a node answered its claim with, as boot facts.
fn claim_facts(boot: &BootReporter, claimed: &NodeClaimed) {
    boot.fact(
        "node",
        format!("{} on port {}", claimed.node_id, claimed.port),
    );
    if claimed.sealed > 0 {
        boot.fact("sealed", format!("{} inherited agents", claimed.sealed));
    }
}

/// What a claim waits on, as shown: the claim's own output carries
/// tokens and is never streamed, so this is all a boot shows of it.
fn claim_waits_on(machine: &str) -> String {
    format!("`{CLAIM_COMMAND}` on {machine} (the node starts and takes its tokens)")
}

fn random_token() -> BoxdToken {
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    bytes[16..].copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    BoxdToken::new(hex::encode(bytes))
}

impl VmFleet {
    /// A fleet over `provider`, writing rows to `ledger`.
    #[must_use]
    pub fn new(
        provider: Arc<dyn VmProvider>,
        ledger: Arc<dyn VmLedger>,
        settings: FleetSettings,
    ) -> Self {
        Self {
            provider,
            ledger,
            settings,
            machines: Mutex::new(HashMap::new()),
            mesh_tokens: Mutex::new(HashMap::new()),
            usage_denied: Mutex::new(false),
        }
    }

    /// The provider, for snapshot builds and the hub's own calls.
    #[must_use]
    pub fn provider(&self) -> Arc<dyn VmProvider> {
        Arc::clone(&self.provider)
    }

    /// The settings.
    #[must_use]
    pub const fn settings(&self) -> &FleetSettings {
        &self.settings
    }

    fn row(&self, ledger: &str, payload: &VmPayload) {
        self.ledger.record(ledger, payload);
    }

    fn base_row(event: VmEvent, machine: &MachineInfo) -> VmPayload {
        VmPayload {
            region: (!machine.region.is_empty()).then(|| machine.region.clone()),
            resources: Some(VmResources {
                vcpu: machine.vcpu,
                memory_bytes: machine.memory_bytes,
                disk_bytes: machine.disk_bytes,
            }),
            isolated: machine.isolated,
            status: Some(machine.status.as_str().to_string()),
            owner: machine.labels.get(LABEL_OWNER).cloned(),
            ..VmPayload::new(event, machine.id.clone(), machine.name.clone())
        }
    }

    /// The mesh token of `owner`'s agents in `project`, minted on first
    /// use. Owners never share one: their nodes can't reach each other.
    fn mesh_token(&self, owner: &str, project: &str) -> BoxdToken {
        self.mesh_tokens
            .lock()
            .entry(mesh_key(owner, project))
            .or_insert_with(random_token)
            .clone()
    }

    /// The node token the hub calls `agent`'s node with.
    #[must_use]
    pub fn node_token(&self, agent: &str) -> Option<BoxdToken> {
        self.machines
            .lock()
            .get(agent)
            .and_then(|e| e.node_token.clone())
    }

    /// The project's mesh token, if one was minted.
    #[must_use]
    pub fn project_mesh_token(&self, owner: &str, project: &str) -> Option<BoxdToken> {
        self.mesh_tokens
            .lock()
            .get(&mesh_key(owner, project))
            .cloned()
    }

    /// Every machine of `owner`, sorted by agent.
    #[must_use]
    pub fn views_of(&self, owner: &str) -> Vec<MachineView> {
        self.views()
            .into_iter()
            .filter(|view| view.owner == owner)
            .collect()
    }

    /// Replace the plan templates `agent`'s next claims send.
    pub fn set_plans(&self, agent: &str, plans: Vec<PlanTemplate>) {
        if let Some(entry) = self.machines.lock().get_mut(agent) {
            entry.plans = plans;
        }
    }

    /// Create, configure and claim a machine for an agent.
    ///
    /// # Errors
    ///
    /// [`CloudError::MachineLimit`] when the org is full; provider and
    /// claim errors otherwise (the machine is destroyed again then).
    pub async fn provision(&self, request: ProvisionRequest) -> CloudResult<MachineView> {
        self.provision_reported(request, &BootReporter::none())
            .await
    }

    /// [`Self::provision`], showing each stage and what boxd reports
    /// about the machine on `boot`.
    ///
    /// # Errors
    ///
    /// As [`Self::provision`].
    pub async fn provision_reported(
        &self,
        request: ProvisionRequest,
        boot: &BootReporter,
    ) -> CloudResult<MachineView> {
        let limit = boot
            .run(BootStage::Quota, async {
                let limit = boot
                    .wait(
                        BootStage::Quota,
                        "boxd is counting the org's machines",
                        Some(CALL_TIMEOUT),
                        self.provider.machine_limit(),
                    )
                    .await?;
                if limit.has_room() {
                    Ok(limit)
                } else {
                    Err(CloudError::MachineLimit {
                        count: limit.count,
                        max: limit.max,
                    })
                }
            })
            .await?;
        boot.fact(
            "quota",
            format!("{} of {} machines in use", limit.count, limit.max),
        );
        if let MachineOrigin::Fork(parent) = &request.origin {
            // A fork inherits its parent's secrets, so never across owners.
            let parent_owner = self
                .machines
                .lock()
                .values()
                .find(|entry| entry.machine.name == *parent || entry.machine.id == *parent)
                .map(|entry| entry.owner.clone());
            if parent_owner.as_deref() != Some(request.owner.as_str()) {
                return Err(CloudError::BadState {
                    machine: parent.clone(),
                    state: "another user's machine".into(),
                    op: "fork it",
                });
            }
        }
        if let Some(foreign) = request
            .secrets
            .iter()
            .find(|name| !names::is_plan_secret_of(name, &request.owner))
        {
            return Err(CloudError::BadState {
                machine: request.name.clone(),
                state: format!("asked for secret {foreign}"),
                op: "list a secret that is not its owner's plan",
            });
        }
        if let Some(foreign) = request
            .repo_secret
            .as_deref()
            .filter(|name| !names::is_repo_secret(name))
        {
            return Err(CloudError::BadState {
                machine: request.name.clone(),
                state: format!("asked for secret {foreign}"),
                op: "list a repository secret that is not one",
            });
        }
        let ledger = names::machine_ledger(&request.agent_zns);
        let mut labels = BTreeMap::new();
        labels.insert(LABEL_OWNER.to_string(), request.owner.clone());
        labels.insert(LABEL_INSTALL.to_string(), self.settings.install.clone());
        labels.insert(LABEL_AGENT.to_string(), request.agent.clone());
        labels.insert(LABEL_PROJECT.to_string(), request.project.clone());
        labels.insert(LABEL_ROLE.to_string(), ROLE_AGENT.to_string());
        let spec = NewMachine {
            name: request.name.clone(),
            size: self.settings.size,
            isolated: self.settings.isolated,
            labels,
            env: MachineEnv::for_agent(self.settings.size.vcpu()).into_map(),
            packages: Vec::new(),
            networks: if self.settings.isolated {
                Vec::new()
            } else {
                vec![format!("aura-{}", names::dns_fragment(&request.project))]
            },
            delete_after_secs: None,
            secrets: request
                .secrets
                .iter()
                .cloned()
                .chain(request.repo_secret.clone())
                .collect(),
        };
        let creating = match &request.origin {
            MachineOrigin::Snapshot(snapshot) => {
                format!("boxd is creating {} from snapshot {snapshot}", spec.name)
            }
            MachineOrigin::Fork(parent) => format!("boxd is forking {parent} as {}", spec.name),
            MachineOrigin::Fresh => format!("boxd is creating {}", spec.name),
        };
        let machine = boot
            .run(
                BootStage::Create,
                boot.wait(
                    BootStage::Create,
                    creating,
                    Some(CREATE_TIMEOUT),
                    self.provider.create(&spec, &request.origin),
                ),
            )
            .await?;
        machine_facts(boot, &machine, &spec.networks);
        let event = match request.origin {
            MachineOrigin::Fork(_) => VmEvent::Forked,
            _ => VmEvent::Created,
        };
        let source = match &request.origin {
            MachineOrigin::Snapshot(name) => Some(VmSource::Snapshot {
                name: name.clone(),
                version: None,
            }),
            MachineOrigin::Fork(parent) => Some(VmSource::Fork {
                parent: parent.clone(),
            }),
            MachineOrigin::Fresh => machine.source.clone(),
        };
        self.row(
            &ledger,
            &VmPayload {
                source: machine.source.clone().or(source),
                boot_ms: Some(machine.boot_ms),
                ..Self::base_row(event, &machine)
            },
        );
        self.machines.lock().insert(
            request.agent.clone(),
            Entry {
                owner: request.owner.clone(),
                plans: request.plans.clone(),
                git: request.git.clone(),
                project: request.project.clone(),
                ledger: ledger.clone(),
                machine: machine.clone(),
                state: VmState::Claiming,
                busy: 0,
                idle_since: Instant::now(),
                node_token: None,
                usage: None,
                seen_ms: now_ms(),
            },
        );
        boot.run(BootStage::Configure, async {
            // boxd's bot check would answer API calls with a 503.
            let bot = boot
                .wait(
                    BootStage::Configure,
                    "boxd is turning bot protection off",
                    Some(CALL_TIMEOUT),
                    self.provider.set_bot_protection(&machine.name, false),
                )
                .await;
            if let Err(err) = bot {
                tracing::warn!(machine = %machine.name, error = %err, "could not turn bot protection off");
                boot.log(BootStage::Configure, true, format!("bot protection stays on: {err}"));
            } else {
                boot.log(BootStage::Configure, false, "bot protection off for the node API");
            }
            match boot
                .wait(
                    BootStage::Configure,
                    "boxd is setting the hibernation backstop",
                    Some(CALL_TIMEOUT),
                    self.provider
                        .set_auto_hibernate(&machine.name, AUTO_HIBERNATE_BACKSTOP_SECS),
                )
                .await
            {
                Ok(()) => boot.log(
                    BootStage::Configure,
                    false,
                    format!(
                        "boxd hibernates it after {} min idle",
                        AUTO_HIBERNATE_BACKSTOP_SECS / 60
                    ),
                ),
                Err(err) => {
                    tracing::warn!(machine = %machine.name, error = %err, "could not set the hibernation backstop");
                    boot.log(
                        BootStage::Configure,
                        true,
                        format!("no hibernation backstop: {err}"),
                    );
                }
            }
            Ok::<_, CloudError>(())
        })
        .await?;
        let claimed = boot
            .run(
                BootStage::Claim,
                boot.wait(
                    BootStage::Claim,
                    claim_waits_on(&machine.name),
                    Some(CLAIM_TIMEOUT),
                    self.claim(&request.agent, &request.seal),
                ),
            )
            .await;
        if let Ok(claimed) = &claimed {
            claim_facts(boot, claimed);
        }
        if let Err(err) = claimed {
            self.row(
                &ledger,
                &VmPayload {
                    reason: Some(err.to_string()),
                    ..Self::base_row(VmEvent::Error, &machine)
                },
            );
            let _ = self.provider.destroy(&machine.name).await;
            self.machines.lock().remove(&request.agent);
            return Err(err);
        }
        self.view(&request.agent).ok_or(CloudError::NotFound {
            what: "machine",
            name: machine.name,
        })
    }

    /// Give `agent`'s node fresh tokens over `Exec`, sealing `seal`.
    ///
    /// # Errors
    ///
    /// When the command fails or answers something unreadable.
    pub async fn claim(&self, agent: &str, seal: &[String]) -> CloudResult<NodeClaimed> {
        let (machine, project, ledger, owner, plans, git) = {
            let machines = self.machines.lock();
            let entry = machines.get(agent).ok_or_else(|| CloudError::NotFound {
                what: "machine",
                name: agent.to_string(),
            })?;
            (
                entry.machine.clone(),
                entry.project.clone(),
                entry.ledger.clone(),
                entry.owner.clone(),
                entry.plans.clone(),
                entry.git.clone(),
            )
        };
        let node_token = random_token();
        let claim = NodeClaim {
            node_id: machine.name.clone(),
            node_token: node_token.expose().to_string(),
            mesh_token: self.mesh_token(&owner, &project).expose().to_string(),
            seal: seal.to_vec(),
            owner: (!owner.is_empty()).then(|| owner.clone()),
            plans,
            git,
        };
        let stdin = serde_json::to_vec(&claim).map_err(|err| CloudError::InvalidResponse {
            op: "claim",
            message: err.to_string(),
        })?;
        let output = self
            .provider
            .exec(&machine.name, CLAIM_COMMAND, Some(stdin), CLAIM_TIMEOUT)
            .await?;
        if !output.success() {
            return Err(CloudError::ExecFailed {
                machine: machine.name.clone(),
                command: CLAIM_COMMAND.to_string(),
                exit_code: output.exit_code,
                stderr: output.stderr_tail(400),
            });
        }
        let claimed: NodeClaimed = serde_json::from_slice(&output.stdout).unwrap_or(NodeClaimed {
            node_id: machine.name.clone(),
            port: crate::machine_env::NODE_PORT,
            sealed: seal.len(),
        });
        {
            let mut machines = self.machines.lock();
            if let Some(entry) = machines.get_mut(agent) {
                entry.node_token = Some(node_token);
                entry.state = VmState::Ready;
                entry.machine.status = MachineStatus::Running;
                entry.idle_since = Instant::now();
            }
        }
        self.row(
            &ledger,
            &VmPayload {
                reason: (!seal.is_empty()).then(|| format!("sealed {} agents", claimed.sealed)),
                ..Self::base_row(VmEvent::Claimed, &machine)
            },
        );
        Ok(claimed)
    }

    /// Take over a machine that already exists for `agent` (after a hub
    /// restart). It is claimed again with fresh tokens when it next
    /// runs ([`Self::ensure_awake`]).
    ///
    /// # Errors
    ///
    /// [`CloudError::NotFound`] when the machine is gone.
    pub async fn adopt(
        &self,
        agent: &str,
        agent_zns: &str,
        project: &str,
        machine: &str,
        plans: Vec<PlanTemplate>,
        git: Option<GitClaim>,
    ) -> CloudResult<MachineView> {
        let info = self
            .provider
            .get(machine)
            .await?
            .ok_or_else(|| CloudError::NotFound {
                what: "machine",
                name: machine.to_string(),
            })?;
        let state = match info.status {
            MachineStatus::Standby => VmState::Standby,
            MachineStatus::Hibernated | MachineStatus::Stopped => VmState::Hibernated,
            MachineStatus::Error => VmState::Error("boxd reports an error".into()),
            _ => VmState::Claiming,
        };
        let owner = info.labels.get(LABEL_OWNER).cloned().unwrap_or_default();
        self.machines.lock().insert(
            agent.to_string(),
            Entry {
                owner,
                plans,
                git,
                project: project.to_string(),
                ledger: names::machine_ledger(agent_zns),
                machine: info,
                state,
                busy: 0,
                idle_since: Instant::now(),
                node_token: None,
                usage: None,
                seen_ms: now_ms(),
            },
        );
        self.view(agent).ok_or(CloudError::NotFound {
            what: "machine",
            name: machine.to_string(),
        })
    }

    /// Wake `agent`'s machine if it sleeps, and claim it if it has no
    /// token yet, so it can take work.
    ///
    /// # Errors
    ///
    /// When it has no machine, or waking or claiming fails.
    pub async fn ensure_awake(&self, agent: &str) -> CloudResult<MachineView> {
        self.ensure_awake_reported(agent, &BootReporter::none())
            .await
    }

    /// [`Self::ensure_awake`], showing a wake and a claim on `boot`; a
    /// machine that is already up shows nothing.
    ///
    /// # Errors
    ///
    /// As [`Self::ensure_awake`].
    pub async fn ensure_awake_reported(
        &self,
        agent: &str,
        boot: &BootReporter,
    ) -> CloudResult<MachineView> {
        let (machine, state, ledger, claimed) = {
            let machines = self.machines.lock();
            let entry = machines.get(agent).ok_or_else(|| CloudError::NotFound {
                what: "machine",
                name: agent.to_string(),
            })?;
            (
                entry.machine.clone(),
                entry.state.clone(),
                entry.ledger.clone(),
                entry.node_token.is_some(),
            )
        };
        if state == VmState::Destroying {
            return Err(CloudError::BadState {
                machine: machine.name,
                state: state.label().to_string(),
                op: "wake",
            });
        }
        if state.is_asleep() || matches!(state, VmState::Error(_)) {
            boot.fact("machine", machine.name.clone());
            boot.fact("woke from", state.label().to_string());
            self.set_state(agent, VmState::Waking);
            let started = Instant::now();
            boot.run(
                BootStage::Wake,
                boot.wait(
                    BootStage::Wake,
                    format!("boxd is waking {} from {}", machine.name, state.label()),
                    Some(CALL_TIMEOUT),
                    self.provider.wake(&machine.name),
                ),
            )
            .await?;
            let boot_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            boot.fact("wake time", format!("{boot_ms} ms"));
            let mut info = machine.clone();
            info.status = MachineStatus::Running;
            self.row(
                &ledger,
                &VmPayload {
                    boot_ms: Some(boot_ms),
                    ..Self::base_row(VmEvent::Woke, &info)
                },
            );
            {
                let mut machines = self.machines.lock();
                if let Some(entry) = machines.get_mut(agent) {
                    entry.machine.status = MachineStatus::Running;
                    entry.state = VmState::Ready;
                    entry.idle_since = Instant::now();
                }
            }
        }
        if !claimed {
            let claimed = boot
                .run(
                    BootStage::Claim,
                    boot.wait(
                        BootStage::Claim,
                        claim_waits_on(&machine.name),
                        Some(CLAIM_TIMEOUT),
                        self.claim(agent, &[]),
                    ),
                )
                .await?;
            claim_facts(boot, &claimed);
        }
        self.view(agent).ok_or(CloudError::NotFound {
            what: "machine",
            name: machine.name,
        })
    }

    fn set_state(&self, agent: &str, state: VmState) {
        if let Some(entry) = self.machines.lock().get_mut(agent) {
            entry.state = state;
        }
    }

    /// Mark work in flight on `agent`'s machine; it can't sleep until
    /// the returned guard drops.
    #[must_use]
    pub fn begin_work(self: &Arc<Self>, agent: &str) -> WorkGuard {
        if let Some(entry) = self.machines.lock().get_mut(agent) {
            entry.busy += 1;
            entry.state = VmState::Busy;
        }
        WorkGuard {
            fleet: Arc::clone(self),
            agent: agent.to_string(),
        }
    }

    fn end_work(&self, agent: &str) {
        if let Some(entry) = self.machines.lock().get_mut(agent) {
            entry.busy = entry.busy.saturating_sub(1);
            if entry.busy == 0 {
                entry.idle_since = Instant::now();
                if entry.state == VmState::Busy {
                    entry.state = VmState::Ready;
                }
            }
        }
    }

    /// Reset `agent`'s idle clock (mail arrived, the user looked at it).
    pub fn touch(&self, agent: &str) {
        if let Some(entry) = self.machines.lock().get_mut(agent) {
            entry.idle_since = Instant::now();
        }
    }

    /// Put idle machines to sleep. Returns the agents whose machines
    /// went to sleep. Call it every few seconds.
    pub async fn tick(&self) -> Vec<String> {
        let mut slept = Vec::new();
        for due in self.idle_machines() {
            if self.sleep_idle(&due).await {
                slept.push(due.agent);
            }
        }
        slept
    }

    /// How `entry` should sleep now, if it should: `Some(true)` to
    /// hibernate, `Some(false)` for standby.
    fn sleep_due(&self, entry: &Entry, now: Instant) -> Option<bool> {
        if entry.busy > 0 {
            return None;
        }
        let idle = now.saturating_duration_since(entry.idle_since);
        let hibernate = !self.settings.idle_hibernate.is_zero()
            && matches!(entry.state, VmState::Ready | VmState::Standby)
            && idle >= self.settings.idle_hibernate;
        let standby = !self.settings.idle_standby.is_zero()
            && entry.state == VmState::Ready
            && idle >= self.settings.idle_standby
            && !hibernate;
        (hibernate || standby).then_some(hibernate)
    }

    /// The machines idle long enough to sleep now. Nothing changes until
    /// [`Self::sleep_idle`], so the caller can save what they hold first.
    #[must_use]
    pub fn idle_machines(&self) -> Vec<IdleMachine> {
        let now = Instant::now();
        let machines = self.machines.lock();
        let mut due: Vec<IdleMachine> = machines
            .iter()
            .filter_map(|(agent, entry)| {
                self.sleep_due(entry, now).map(|hibernate| IdleMachine {
                    agent: agent.clone(),
                    hibernate,
                    awake: entry.state == VmState::Ready,
                })
            })
            .collect();
        due.sort_by(|a, b| a.agent.cmp(&b.agent));
        due
    }

    /// Put `due` to sleep, unless it is no longer idle (work came in
    /// since [`Self::idle_machines`] listed it). Returns whether it went
    /// to sleep.
    pub async fn sleep_idle(&self, due: &IdleMachine) -> bool {
        let (machine, ledger) = {
            let machines = self.machines.lock();
            let Some(entry) = machines.get(&due.agent) else {
                return false;
            };
            if self.sleep_due(entry, Instant::now()) != Some(due.hibernate) {
                return false;
            }
            (entry.machine.clone(), entry.ledger.clone())
        };
        let (result, state, event, status) = if due.hibernate {
            (
                self.provider.hibernate(&machine.name).await,
                VmState::Hibernated,
                VmEvent::Hibernated,
                MachineStatus::Hibernated,
            )
        } else {
            (
                self.provider.standby(&machine.name).await,
                VmState::Standby,
                VmEvent::Standby,
                MachineStatus::Standby,
            )
        };
        match result {
            Ok(()) => {
                let mut info = machine.clone();
                info.status = status.clone();
                self.row(
                    &ledger,
                    &VmPayload {
                        reason: Some("idle".into()),
                        ..Self::base_row(event, &info)
                    },
                );
                if let Some(entry) = self.machines.lock().get_mut(&due.agent) {
                    // Work may have started while the call ran.
                    if entry.busy == 0 {
                        entry.state = state;
                        entry.machine.status = status;
                    }
                }
                true
            }
            Err(err) => {
                tracing::warn!(machine = %machine.name, error = %err, "could not put an idle machine to sleep");
                false
            }
        }
    }

    /// Re-read every machine from boxd: notice machines woken by a peer
    /// or by boxd's timers, and drop machines that are gone. Returns the
    /// agents whose machine woke up without the fleet waking it (their
    /// sockets must be reopened).
    pub async fn poll(&self) -> Vec<String> {
        let snapshot: Vec<(String, String, VmState, String)> = self
            .machines
            .lock()
            .iter()
            .map(|(agent, e)| {
                (
                    agent.clone(),
                    e.machine.name.clone(),
                    e.state.clone(),
                    e.ledger.clone(),
                )
            })
            .collect();
        let mut woke = Vec::new();
        for (agent, name, state, ledger) in snapshot {
            match self.provider.get(&name).await {
                Ok(Some(info)) => {
                    let mut machines = self.machines.lock();
                    let Some(entry) = machines.get_mut(&agent) else {
                        continue;
                    };
                    entry.seen_ms = now_ms();
                    match (&state, &info.status) {
                        (VmState::Standby | VmState::Hibernated, MachineStatus::Running) => {
                            entry.state = VmState::Ready;
                            entry.idle_since = Instant::now();
                            woke.push(agent.clone());
                        }
                        (VmState::Ready, MachineStatus::Hibernated) => {
                            entry.state = VmState::Hibernated;
                        }
                        (VmState::Ready, MachineStatus::Standby) => entry.state = VmState::Standby,
                        _ => {}
                    }
                    entry.machine = info;
                }
                Ok(None) if state != VmState::Destroying => {
                    if let Some(entry) = self.machines.lock().remove(&agent) {
                        self.row(
                            &ledger,
                            &VmPayload {
                                reason: Some("machine disappeared".into()),
                                ..Self::base_row(VmEvent::Error, &entry.machine)
                            },
                        );
                    }
                }
                Ok(None) => {}
                Err(err) => tracing::debug!(machine = %name, error = %err, "machine poll failed"),
            }
        }
        woke
    }

    /// Read every machine's meter and write a `usage` row for each.
    pub async fn record_usage(&self) {
        if *self.usage_denied.lock() {
            return;
        }
        let targets: Vec<(String, MachineInfo, String)> = self
            .machines
            .lock()
            .iter()
            .map(|(agent, e)| (agent.clone(), e.machine.clone(), e.ledger.clone()))
            .collect();
        for (agent, machine, ledger) in targets {
            match self.provider.machine_usage(&machine.id).await {
                Ok(reading) => {
                    self.row(
                        &ledger,
                        &VmPayload {
                            usage: Some(reading.usage),
                            currency: Some(reading.currency.clone()),
                            ..Self::base_row(VmEvent::Usage, &machine)
                        },
                    );
                    if let Some(entry) = self.machines.lock().get_mut(&agent) {
                        entry.usage = Some(reading);
                    }
                }
                Err(err) if err.is_permission_denied() => {
                    tracing::info!("boxd usage needs an org admin key; costs are estimated");
                    *self.usage_denied.lock() = true;
                    return;
                }
                Err(err) => {
                    tracing::debug!(machine = %machine.name, error = %err, "usage read failed");
                }
            }
        }
    }

    /// Whether boxd refused usage calls.
    #[must_use]
    pub fn usage_denied(&self) -> bool {
        *self.usage_denied.lock()
    }

    /// Destroy `agent`'s machine for `reason` and write its `destroyed`
    /// row. Returns the machine id, for [`Self::settle_usage`].
    ///
    /// # Errors
    ///
    /// When boxd refuses the destroy (the machine is kept and listed).
    pub async fn destroy(&self, agent: &str, reason: &str) -> CloudResult<Option<String>> {
        let Some((machine, ledger, usage)) = self.machines.lock().get_mut(agent).map(|entry| {
            entry.state = VmState::Destroying;
            (
                entry.machine.clone(),
                entry.ledger.clone(),
                entry.usage.clone(),
            )
        }) else {
            return Ok(None);
        };
        if let Err(err) = self.provider.destroy(&machine.name).await {
            self.set_state(agent, VmState::Error(err.to_string()));
            return Err(err);
        }
        self.machines.lock().remove(agent);
        self.row(
            &ledger,
            &VmPayload {
                reason: Some(reason.to_string()),
                usage: usage.as_ref().map(|u| u.usage),
                currency: usage.map(|u| u.currency),
                ..Self::base_row(VmEvent::Destroyed, &machine)
            },
        );
        Ok(Some(machine.id))
    }

    /// After a destroy, read the machine's meter until boxd marks it
    /// complete (or `attempts` run out, `every` apart) and record the
    /// final figure on `ledger`.
    pub async fn settle_usage(
        &self,
        machine_id: &str,
        name: &str,
        ledger: &str,
        attempts: u32,
        every: Duration,
    ) {
        if self.usage_denied() {
            return;
        }
        for attempt in 0..attempts.max(1) {
            if attempt > 0 {
                tokio::time::sleep(every).await;
            }
            match self.provider.machine_usage(machine_id).await {
                Ok(reading) if reading.usage.complete => {
                    self.row(
                        ledger,
                        &VmPayload {
                            usage: Some(reading.usage),
                            currency: Some(reading.currency),
                            reason: Some("final".into()),
                            ..VmPayload::new(VmEvent::Usage, machine_id, name)
                        },
                    );
                    return;
                }
                Ok(_) => {}
                Err(err) if err.is_permission_denied() => return,
                Err(err) => {
                    tracing::debug!(machine = %name, error = %err, "final usage read failed");
                }
            }
        }
    }

    /// Destroy machines of this install whose agent is not in `keep`
    /// (archived, removed or unknown agents). Returns their names.
    ///
    /// # Errors
    ///
    /// When the machines can't be listed.
    pub async fn sweep_orphans(&self, keep: &HashSet<String>) -> CloudResult<Vec<String>> {
        let selector = names::install_selector(&self.settings.install);
        let machines = self.provider.list(&[selector]).await?;
        let mut destroyed = Vec::new();
        for machine in machines {
            if machine.labels.get(LABEL_ROLE).map(String::as_str) != Some(ROLE_AGENT) {
                // Builders delete themselves (delete-after).
                continue;
            }
            let agent = machine.labels.get(LABEL_AGENT).cloned().unwrap_or_default();
            if keep.contains(&agent) {
                continue;
            }
            match self.provider.destroy(&machine.name).await {
                Ok(()) => {
                    tracing::info!(machine = %machine.name, agent = %agent, "destroyed an orphaned cloud machine");
                    destroyed.push(machine.name);
                }
                Err(err) => {
                    tracing::warn!(machine = %machine.name, error = %err, "could not destroy an orphaned machine");
                }
            }
        }
        Ok(destroyed)
    }

    /// `agent`'s machine as it stands.
    #[must_use]
    pub fn view(&self, agent: &str) -> Option<MachineView> {
        let machines = self.machines.lock();
        let entry = machines.get(agent)?;
        let idle = Instant::now().saturating_duration_since(entry.idle_since);
        let sleeps_in = (entry.busy == 0
            && matches!(entry.state, VmState::Ready | VmState::Standby)
            && !self.settings.idle_hibernate.is_zero())
        .then(|| self.settings.idle_hibernate.saturating_sub(idle).as_secs());
        Some(MachineView {
            agent: agent.to_string(),
            owner: entry.owner.clone(),
            project: entry.project.clone(),
            ledger: entry.ledger.clone(),
            machine: entry.machine.clone(),
            state: entry.state.clone(),
            busy: entry.busy,
            idle_secs: if entry.busy > 0 { 0 } else { idle.as_secs() },
            sleeps_in_secs: sleeps_in,
            usage: entry.usage.clone(),
            usage_denied: *self.usage_denied.lock(),
            seen_ms: entry.seen_ms,
        })
    }

    /// Every machine, sorted by agent.
    #[must_use]
    pub fn views(&self) -> Vec<MachineView> {
        let mut agents: Vec<String> = self.machines.lock().keys().cloned().collect();
        agents.sort();
        agents.iter().filter_map(|agent| self.view(agent)).collect()
    }

    /// Whether `agent` has a machine.
    #[must_use]
    pub fn has(&self, agent: &str) -> bool {
        self.machines.lock().contains_key(agent)
    }
}

/// A machine idle long enough to sleep ([`VmFleet::idle_machines`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdleMachine {
    /// Agent, hex.
    pub agent: String,
    /// Hibernate rather than stand by.
    pub hibernate: bool,
    /// Awake now: what it wrote since its last copy is still only on it.
    pub awake: bool,
}

/// Keeps a machine awake while work runs on it.
pub struct WorkGuard {
    fleet: Arc<VmFleet>,
    agent: String,
}

impl Drop for WorkGuard {
    fn drop(&mut self) {
        self.fleet.end_work(&self.agent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boot::{BootEvent, StageState};
    use crate::fake::FakeProvider;
    use aura_core_types::VmUsage;

    fn settings(idle: Duration) -> FleetSettings {
        FleetSettings {
            install: "inst-1".into(),
            size: MachineSize::S4x16,
            isolated: true,
            idle_hibernate: idle,
            idle_standby: Duration::ZERO,
        }
    }

    fn fleet(idle: Duration) -> (Arc<VmFleet>, FakeProvider, Arc<MemoryLedger>) {
        let fake = FakeProvider::new();
        fake.on_exec(Arc::new(|machine, command, stdin| {
            assert_eq!(command, CLAIM_COMMAND);
            let claim: NodeClaim =
                serde_json::from_slice(stdin.unwrap_or_default()).expect("claim json");
            assert_eq!(claim.node_id, machine);
            assert_eq!(claim.node_token.len(), 64);
            let answer = NodeClaimed {
                node_id: claim.node_id,
                port: 8080,
                sealed: claim.seal.len(),
            };
            crate::ExecOutput {
                exit_code: 0,
                stdout: serde_json::to_vec(&answer).expect("json"),
                stderr: Vec::new(),
            }
        }));
        let ledger = Arc::new(MemoryLedger::default());
        let fleet = Arc::new(VmFleet::new(
            Arc::new(fake.clone()),
            ledger.clone(),
            settings(idle),
        ));
        (fleet, fake, ledger)
    }

    fn request(agent: &str, origin: MachineOrigin) -> ProvisionRequest {
        ProvisionRequest {
            agent: agent.into(),
            agent_zns: format!("0://terminal/{agent}"),
            code: Some("RE36".into()),
            project: "p1".into(),
            name: format!("aura-re36-{agent}"),
            origin,
            seal: Vec::new(),
            owner: ALICE.into(),
            secrets: Vec::new(),
            plans: Vec::new(),
            repo_secret: None,
            git: None,
        }
    }

    const ALICE: &str = "a11ce000";
    const BOB: &str = "b0b00000";

    #[tokio::test]
    async fn a_machine_lists_one_repo_secret_and_its_claim_names_the_branch() {
        let (fleet, fake, _) = fleet(Duration::from_secs(300));
        let claims = Arc::new(Mutex::new(Vec::<NodeClaim>::new()));
        let seen = Arc::clone(&claims);
        fake.on_exec(Arc::new(move |_, _, stdin| {
            if let Some(claim) = stdin.and_then(|s| serde_json::from_slice(s).ok()) {
                seen.lock().push(claim);
            }
            crate::provider::ExecOutput::default()
        }));
        let plan = crate::names::plan_secret(ALICE, "inst-1", aura_core_auth::ProviderKind::Claude)
            .expect("name");
        let repo = crate::names::repo_secret("inst-1", "acme/app");
        for name in [&plan, &repo] {
            fake.set_secret(&crate::provider::HostSecret {
                name: name.clone(),
                value: zeroize::Zeroizing::new("unlinked".into()),
                domains: vec!["github.com".into()],
                replace_in: vec!["headers".into()],
            })
            .await
            .expect("secret");
        }
        let mut aa = request("aa", MachineOrigin::Fresh);
        aa.secrets = vec![plan.clone()];
        aa.repo_secret = Some(repo.clone());
        aa.git = Some(GitClaim {
            branch: "aura/reviewer-re36".into(),
            remote_url: Some("https://github.com/acme/app.git".into()),
            env: Some(repo.clone()),
            placeholder: None,
            author_name: Some("aura-agents[bot]".into()),
            author_email: None,
        });
        fleet.provision(aa).await.expect("provision");
        assert_eq!(
            fake.machine_secrets("aura-re36-aa"),
            vec![plan, repo.clone()]
        );
        let git = claims.lock()[0].git.clone().expect("git claim");
        assert_eq!(git.branch, "aura/reviewer-re36");
        assert_eq!(git.env.as_deref(), Some(repo.as_str()));

        // A plan secret passed as the repository secret is refused.
        let mut bb = request("bb", MachineOrigin::Fresh);
        bb.repo_secret = Some(
            crate::names::plan_secret(BOB, "inst-1", aura_core_auth::ProviderKind::Claude)
                .expect("name"),
        );
        let err = fleet.provision(bb).await.expect_err("not a repo secret");
        assert!(err.to_string().contains("repository secret"), "{err}");
    }

    #[tokio::test]
    async fn machines_carry_their_owner_and_only_their_owners_secrets() {
        let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
        let secret = |owner: &str| {
            crate::names::plan_secret(owner, "inst-1", aura_core_auth::ProviderKind::Claude)
                .expect("name")
        };
        for owner in [ALICE, BOB] {
            fake.set_secret(&crate::provider::HostSecret {
                name: secret(owner),
                value: zeroize::Zeroizing::new("unlinked".into()),
                domains: vec!["api.anthropic.com".into()],
                replace_in: Vec::new(),
            })
            .await
            .expect("secret");
        }
        let mut aa = request("aa", MachineOrigin::Fresh);
        aa.secrets = vec![secret(ALICE)];
        let view = fleet.provision(aa).await.expect("aa");
        assert_eq!(view.owner, ALICE);
        assert_eq!(
            view.machine.labels.get(LABEL_OWNER).map(String::as_str),
            Some(ALICE)
        );
        assert_eq!(fake.machine_secrets("aura-re36-aa"), vec![secret(ALICE)]);
        assert!(ledger
            .rows()
            .iter()
            .all(|(_, row)| row.owner.as_deref() == Some(ALICE)));

        // Bob's secret is refused for Alice's machine.
        let mut cc = request("cc", MachineOrigin::Fresh);
        cc.secrets = vec![secret(BOB)];
        let err = fleet.provision(cc).await.expect_err("foreign secret");
        assert!(err.to_string().contains("not its owner's plan"), "{err}");

        // Bob can't fork Alice's machine.
        let mut bb = request("bb", MachineOrigin::Fork("aura-re36-aa".into()));
        bb.owner = BOB.into();
        let err = fleet.provision(bb).await.expect_err("cross-owner fork");
        assert!(err.to_string().contains("another user's machine"), "{err}");
        // Alice can, and the fork keeps her secrets.
        let dd = request("dd", MachineOrigin::Fork("aura-re36-aa".into()));
        fleet.provision(dd).await.expect("same-owner fork");
        assert_eq!(fake.machine_secrets("aura-re36-dd"), vec![secret(ALICE)]);

        assert_eq!(fleet.views_of(ALICE).len(), 2);
        assert!(fleet.views_of(BOB).is_empty());
        assert!(fleet.project_mesh_token(ALICE, "p1").is_some());
        assert!(
            fleet.project_mesh_token(BOB, "p1").is_none(),
            "mesh tokens are per owner"
        );
    }

    #[tokio::test]
    async fn provisioning_labels_claims_and_records() {
        let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
        let view = fleet
            .provision(request("aa", MachineOrigin::Fresh))
            .await
            .expect("provision");
        assert_eq!(view.state, VmState::Ready);
        let machine = fake.machine("aura-re36-aa").expect("machine");
        assert_eq!(
            machine.labels.get(LABEL_AGENT).map(String::as_str),
            Some("aa")
        );
        assert_eq!(
            machine.labels.get(LABEL_INSTALL).map(String::as_str),
            Some("inst-1")
        );
        assert!(machine.isolated);
        assert!(fleet.node_token("aa").is_some());
        assert!(fake
            .calls()
            .iter()
            .any(|c| c == "SetBotProtection(aura-re36-aa, false)"));
        assert_eq!(
            ledger.events("0://cloud/vm/terminal/aa"),
            vec![VmEvent::Created, VmEvent::Claimed]
        );
    }

    fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<BootEvent>) -> Vec<BootEvent> {
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }
        events
    }

    fn stage_trail(events: &[BootEvent]) -> Vec<(BootStage, &'static str)> {
        events
            .iter()
            .filter_map(|event| match event {
                BootEvent::Stage { stage, state } => Some((
                    *stage,
                    match state {
                        StageState::Started => "started",
                        StageState::Done { .. } => "done",
                        StageState::Skipped(_) => "skipped",
                        StageState::Failed(_) => "failed",
                    },
                )),
                _ => None,
            })
            .collect()
    }

    fn fact<'a>(events: &'a [BootEvent], wanted: &str) -> Option<&'a str> {
        events.iter().rev().find_map(|event| match event {
            BootEvent::Fact { key, value } if key == wanted => Some(value.as_str()),
            _ => None,
        })
    }

    #[tokio::test]
    async fn provisioning_shows_each_stage_and_the_machine() {
        let (fleet, _, _) = fleet(Duration::from_secs(300));
        let (boot, mut rx) = BootReporter::channel();
        fleet
            .provision_reported(request("aa", MachineOrigin::Fresh), &boot)
            .await
            .expect("provision");
        let events = drain(&mut rx);
        assert_eq!(
            stage_trail(&events),
            vec![
                (BootStage::Quota, "started"),
                (BootStage::Quota, "done"),
                (BootStage::Create, "started"),
                (BootStage::Create, "done"),
                (BootStage::Configure, "started"),
                (BootStage::Configure, "done"),
                (BootStage::Claim, "started"),
                (BootStage::Claim, "done"),
            ]
        );
        assert_eq!(fact(&events, "machine"), Some("aura-re36-aa"));
        assert_eq!(fact(&events, "network"), Some("isolated"));
        assert_eq!(fact(&events, "node"), Some("aura-re36-aa on port 8080"));
        assert!(fact(&events, "size").is_some_and(|size| size.contains("vCPU")));
        let token = fleet.node_token("aa").expect("token");
        assert!(
            !format!("{events:?}").contains(token.expose()),
            "the node token is never shown"
        );
    }

    #[tokio::test]
    async fn provisioning_says_what_each_slow_call_waits_on() {
        let (fleet, _, _) = fleet(Duration::from_secs(300));
        let (boot, mut rx) = BootReporter::channel();
        fleet
            .provision_reported(request("aa", MachineOrigin::Fresh), &boot)
            .await
            .expect("provision");
        let waits: Vec<(BootStage, Option<String>, Option<Duration>)> = drain(&mut rx)
            .into_iter()
            .filter_map(|event| match event {
                BootEvent::Waiting { stage, what, limit } => Some((stage, what, limit)),
                _ => None,
            })
            .collect();
        let set = |stage: BootStage| {
            waits
                .iter()
                .find(|(s, what, _)| *s == stage && what.is_some())
                .cloned()
        };
        let create = set(BootStage::Create).expect("create waits");
        assert!(create.1.is_some_and(|what| what.contains("aura-re36-aa")));
        assert_eq!(create.2, Some(CREATE_TIMEOUT));
        let claim = set(BootStage::Claim).expect("claim waits");
        assert!(claim.1.is_some_and(|what| what.contains(CLAIM_COMMAND)));
        assert_eq!(claim.2, Some(CLAIM_TIMEOUT));
        assert!(set(BootStage::Quota).is_some());
        assert!(set(BootStage::Configure).is_some());
        // Every wait is cleared: as many clears as sets, and the last
        // word on each stage is a clear.
        let sets = waits.iter().filter(|(_, what, _)| what.is_some()).count();
        let clears = waits.iter().filter(|(_, what, _)| what.is_none()).count();
        assert_eq!(sets, clears);
        for stage in [
            BootStage::Quota,
            BootStage::Create,
            BootStage::Configure,
            BootStage::Claim,
        ] {
            let last = waits.iter().rev().find(|(s, _, _)| *s == stage);
            assert!(last.is_some_and(|(_, what, _)| what.is_none()), "{stage:?}");
        }
    }

    #[tokio::test]
    async fn a_failed_claim_shows_as_a_failed_stage() {
        let (fleet, fake, _) = fleet(Duration::from_secs(300));
        fake.on_exec(Arc::new(|_, _, _| crate::ExecOutput {
            exit_code: 127,
            stdout: Vec::new(),
            stderr: b"aura: command not found".to_vec(),
        }));
        let (boot, mut rx) = BootReporter::channel();
        fleet
            .provision_reported(request("aa", MachineOrigin::Fresh), &boot)
            .await
            .expect_err("claim fails");
        let trail = stage_trail(&drain(&mut rx));
        assert_eq!(trail.last(), Some(&(BootStage::Claim, "failed")));
    }

    #[tokio::test]
    async fn waking_shows_only_when_the_machine_sleeps() {
        let (fleet, _, _) = fleet(Duration::from_secs(300));
        fleet
            .provision(request("aa", MachineOrigin::Fresh))
            .await
            .expect("provision");
        let (boot, mut rx) = BootReporter::channel();
        fleet
            .ensure_awake_reported("aa", &boot)
            .await
            .expect("awake");
        assert!(drain(&mut rx).is_empty(), "an awake machine shows nothing");
        fleet.set_state("aa", VmState::Hibernated);
        fleet
            .ensure_awake_reported("aa", &boot)
            .await
            .expect("wake");
        let events = drain(&mut rx);
        assert_eq!(
            stage_trail(&events),
            vec![(BootStage::Wake, "started"), (BootStage::Wake, "done")]
        );
        assert_eq!(fact(&events, "woke from"), Some("asleep"));
    }

    #[tokio::test]
    async fn a_failed_claim_destroys_the_machine() {
        let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
        fake.on_exec(Arc::new(|_, _, _| crate::ExecOutput {
            exit_code: 127,
            stdout: Vec::new(),
            stderr: b"aura: command not found".to_vec(),
        }));
        let err = fleet
            .provision(request("aa", MachineOrigin::Fresh))
            .await
            .expect_err("claim fails");
        assert!(err.to_string().contains("command not found"), "{err}");
        assert!(fake.machine("aura-re36-aa").is_none());
        assert!(!fleet.has("aa"));
        assert_eq!(
            ledger.events("0://cloud/vm/terminal/aa"),
            vec![VmEvent::Created, VmEvent::Error]
        );
    }

    #[tokio::test]
    async fn the_machine_limit_is_checked_first() {
        let (fleet, fake, _) = fleet(Duration::from_secs(300));
        fake.set_machine_limit(1);
        fleet
            .provision(request("aa", MachineOrigin::Fresh))
            .await
            .expect("first fits");
        let err = fleet
            .provision(request("bb", MachineOrigin::Fresh))
            .await
            .expect_err("full");
        assert!(matches!(err, CloudError::MachineLimit { count: 1, max: 1 }));
    }

    #[tokio::test]
    async fn idle_machines_hibernate_and_busy_ones_do_not() {
        let (fleet, fake, ledger) = fleet(Duration::from_millis(1));
        fleet
            .provision(request("aa", MachineOrigin::Fresh))
            .await
            .expect("aa");
        fleet
            .provision(request("bb", MachineOrigin::Fresh))
            .await
            .expect("bb");
        let guard = fleet.begin_work("bb");
        tokio::time::sleep(Duration::from_millis(5)).await;
        let slept = fleet.tick().await;
        assert_eq!(slept, vec!["aa".to_string()]);
        assert_eq!(
            fake.machine("aura-re36-aa").expect("aa").status,
            MachineStatus::Hibernated
        );
        assert_eq!(
            fake.machine("aura-re36-bb").expect("bb").status,
            MachineStatus::Running
        );
        assert_eq!(fleet.view("aa").expect("aa").state, VmState::Hibernated);
        assert!(ledger
            .events("0://cloud/vm/terminal/aa")
            .contains(&VmEvent::Hibernated));
        drop(guard);
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert_eq!(fleet.tick().await, vec!["bb".to_string()]);
    }

    #[tokio::test]
    async fn a_listed_machine_that_gets_work_stays_awake() {
        let (fleet, fake, _) = fleet(Duration::from_millis(1));
        fleet
            .provision(request("aa", MachineOrigin::Fresh))
            .await
            .expect("aa");
        tokio::time::sleep(Duration::from_millis(5)).await;
        let due = fleet.idle_machines();
        assert_eq!(
            due,
            vec![IdleMachine {
                agent: "aa".into(),
                hibernate: true,
                awake: true,
            }]
        );
        let guard = fleet.begin_work("aa");
        assert!(!fleet.sleep_idle(&due[0]).await, "work came in");
        assert_eq!(
            fake.machine("aura-re36-aa").expect("aa").status,
            MachineStatus::Running
        );
        drop(guard);
        tokio::time::sleep(Duration::from_millis(5)).await;
        assert!(fleet.sleep_idle(&due[0]).await);
        assert_eq!(fleet.view("aa").expect("aa").state, VmState::Hibernated);
        assert!(fleet.idle_machines().is_empty(), "asleep already");
    }

    #[tokio::test]
    async fn work_for_a_sleeping_machine_wakes_it_first() {
        let (fleet, fake, ledger) = fleet(Duration::from_millis(1));
        fleet
            .provision(request("aa", MachineOrigin::Fresh))
            .await
            .expect("aa");
        tokio::time::sleep(Duration::from_millis(5)).await;
        fleet.tick().await;
        let view = fleet.ensure_awake("aa").await.expect("wake");
        assert_eq!(view.state, VmState::Ready);
        assert_eq!(
            fake.machine("aura-re36-aa").expect("aa").status,
            MachineStatus::Running
        );
        assert_eq!(fake.calls_matching("Wake(").len(), 1);
        assert!(ledger
            .events("0://cloud/vm/terminal/aa")
            .ends_with(&[VmEvent::Hibernated, VmEvent::Woke]));
    }

    #[tokio::test]
    async fn a_machine_woken_by_a_peer_is_noticed() {
        let (fleet, fake, _) = fleet(Duration::from_millis(1));
        fleet
            .provision(request("aa", MachineOrigin::Fresh))
            .await
            .expect("aa");
        tokio::time::sleep(Duration::from_millis(5)).await;
        fleet.tick().await;
        fake.set_status("aura-re36-aa", MachineStatus::Running);
        assert_eq!(fleet.poll().await, vec!["aa".to_string()]);
        assert_eq!(fleet.view("aa").expect("aa").state, VmState::Ready);
    }

    #[tokio::test]
    async fn destroying_records_the_last_reading_then_the_final_one() {
        let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
        let view = fleet
            .provision(request("aa", MachineOrigin::Fresh))
            .await
            .expect("aa");
        fake.set_usage(
            "aura-re36-aa",
            VmUsage {
                cost_micro: 410_000,
                ..VmUsage::default()
            },
        );
        fleet.record_usage().await;
        assert_eq!(
            fleet
                .view("aa")
                .and_then(|v| v.usage)
                .map(|u| u.usage.cost_micro),
            Some(410_000)
        );
        let id = fleet
            .destroy("aa", "archived")
            .await
            .expect("destroy")
            .expect("had one");
        assert!(fake.machine("aura-re36-aa").is_none());
        fake.set_usage(
            "aura-re36-aa",
            VmUsage {
                cost_micro: 450_000,
                complete: true,
                ..VmUsage::default()
            },
        );
        fleet
            .settle_usage(
                &id,
                &view.machine.name,
                &view.ledger,
                2,
                Duration::from_millis(1),
            )
            .await;
        let rows = ledger.rows();
        let last = &rows.last().expect("rows").1;
        assert_eq!(last.event, VmEvent::Usage);
        assert_eq!(
            last.usage.map(|u| (u.cost_micro, u.complete)),
            Some((450_000, true))
        );
        assert!(ledger.events(&view.ledger).contains(&VmEvent::Destroyed));
    }

    #[tokio::test]
    async fn usage_refused_for_a_member_key_falls_back_quietly() {
        let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
        fleet
            .provision(request("aa", MachineOrigin::Fresh))
            .await
            .expect("aa");
        fake.deny_usage();
        fleet.record_usage().await;
        assert!(fleet.usage_denied());
        assert!(!ledger
            .events("0://cloud/vm/terminal/aa")
            .contains(&VmEvent::Usage));
    }

    #[tokio::test]
    async fn orphans_are_swept_and_kept_agents_are_not() {
        let (fleet, fake, _) = fleet(Duration::from_secs(300));
        fleet
            .provision(request("aa", MachineOrigin::Fresh))
            .await
            .expect("aa");
        fleet
            .provision(request("bb", MachineOrigin::Fresh))
            .await
            .expect("bb");
        let keep: HashSet<String> = ["aa".to_string()].into_iter().collect();
        let destroyed = fleet.sweep_orphans(&keep).await.expect("sweep");
        assert_eq!(destroyed, vec!["aura-re36-bb".to_string()]);
        assert!(fake.machine("aura-re36-aa").is_some());
    }

    #[tokio::test]
    async fn forks_copy_the_parent_and_seal_its_agents() {
        let (fleet, fake, ledger) = fleet(Duration::from_secs(300));
        fleet
            .provision(request("aa", MachineOrigin::Fresh))
            .await
            .expect("aa");
        let mut child = request("bb", MachineOrigin::Fork("aura-re36-aa".into()));
        child.seal = vec!["aa".into()];
        let view = fleet.provision(child).await.expect("fork");
        assert_eq!(
            view.machine.source,
            Some(VmSource::Fork {
                parent: "aura-re36-aa".into()
            })
        );
        assert_eq!(
            ledger.events("0://cloud/vm/terminal/bb")[0],
            VmEvent::Forked
        );
        assert!(fake
            .calls()
            .iter()
            .any(|c| c.starts_with("Create(aura-re36-bb, Fork(")));
        // Each node gets its own token; the project shares one mesh token.
        assert_ne!(
            fleet.node_token("aa").map(|t| t.expose().to_string()),
            fleet.node_token("bb").map(|t| t.expose().to_string())
        );
        assert!(fleet.project_mesh_token(ALICE, "p1").is_some());
    }

    #[tokio::test]
    async fn an_adopted_machine_is_claimed_again_on_first_use() {
        let (fleet, fake, _) = fleet(Duration::from_secs(300));
        fleet
            .provision(request("aa", MachineOrigin::Fresh))
            .await
            .expect("aa");
        // A new hub process: a new fleet over the same provider.
        let ledger = Arc::new(MemoryLedger::default());
        let restarted = Arc::new(VmFleet::new(
            Arc::new(fake.clone()),
            ledger,
            settings(Duration::from_secs(300)),
        ));
        restarted
            .adopt(
                "aa",
                "0://terminal/aa",
                "p1",
                "aura-re36-aa",
                Vec::new(),
                None,
            )
            .await
            .expect("adopt");
        assert!(restarted.node_token("aa").is_none());
        restarted.ensure_awake("aa").await.expect("claim");
        assert!(restarted.node_token("aa").is_some());
    }
}
