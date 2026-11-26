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

mod claim;
mod provision;
mod teardown;
mod usage;
mod wake;

pub use self::wake::{IdleMachine, WorkGuard};

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aura_config::{BoxdToken, CloudConfig, MachineSize};
use aura_core_protocol::cloud::{GitClaim, PlanTemplate};
use aura_core_types::{VmEvent, VmPayload, VmResources};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::names::LABEL_OWNER;
use crate::provider::{MachineInfo, MachineOrigin, UsageReading, VmProvider};

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

    /// Whether the machine is up and its node answers.
    #[must_use]
    pub const fn is_awake(&self) -> bool {
        matches!(self, Self::Ready | Self::Busy)
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

#[cfg(test)]
mod tests;
