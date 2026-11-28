//! This node as a cloud machine's node, run for a hub.
//!
//! A node started with `AURA_CLOUD_NODE=1` comes up **unclaimed**: every
//! protected route answers `503` until the hub claims it. The hub does
//! that over boxd's authenticated `Exec`, running `aura node claim
//! --stdin` in the machine, which passes a [`NodeClaim`] to this process
//! over a Unix socket only the machine's user can open
//! ([`claim_socket_path`]). Nothing secret travels over the public
//! domain, and a machine restored from a snapshot (whose memory every
//! clone shares) gets tokens of its own.
//!
//! ## Invariants
//!
//! - Once claimed, only the claim's node token (the hub) is accepted,
//!   plus the project's mesh token (peer nodes) on `/v1/mesh`; the
//!   node's own startup token is never accepted on a cloud node.
//! - A claim replaces the previous one (the hub re-claims after a
//!   restart). Agents listed in `seal` are sealed: this node never runs
//!   or appends for them again.
//! - Tokens are compared in constant time and never logged.
//! - The claim's GitHub token reaches git and `gh` only as its boxd
//!   placeholder, in files only the machine's user can read
//!   ([`write_git_files`]).
//! - The agent's session checkpoints stay in the node's data directory
//!   ([`CloudNode::sessions`]), so its conversation survives standby,
//!   hibernation, a stopped machine and node restarts.
//! - The hub is told whenever a hosted agent's record grows
//!   ([`CloudNode::hosted`]); the hint carries no entries, the hub reads
//!   and checks them itself.
//! - Each agent's conversation is traced as one run across the node runs
//!   the hub starts for it ([`CloudNode::trace`]); the hub copies the
//!   runs back ([`CloudNode::runs_of`]).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use aura_agent::checkpoint::Checkpoints;
use aura_agent_trace::db::RunCopy;
use aura_agent_trace::{RunMeta, RunRecorder};
use aura_core_protocol::cloud::{GitClaim, HostStats, NodeClaim, NodeClaimed, NODE_PROTOCOL};
use aura_core_types::AgentId;
use aura_engine_cloud::host::HostSampler;
use axum::http::{HeaderMap, StatusCode};
use zeroize::Zeroizing;

use crate::config::NodeConfig;

mod git_files;
mod plans;
#[cfg(test)]
mod tests;

pub use git_files::{write_git_files, GitFiles};
pub use plans::{fill_placeholders, PlanResolver};
use plans::{is_placeholder, url_encoded, yaml_quoted};

/// Where the snapshot a machine came from is described.
pub const SNAPSHOT_MANIFEST: &str = "/etc/aura/snapshot.json";

/// Largest claim accepted on the socket.
const MAX_CLAIM_BYTES: usize = 64 * 1024;

/// Who called a cloud node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caller {
    /// The hub, with the node token.
    Hub,
    /// A peer node of the project, with the mesh token.
    Peer,
}

struct Claim {
    node_id: String,
    node_token: Zeroizing<String>,
    mesh_token: Zeroizing<String>,
    public_url: Option<String>,
}

/// Cloud-node state shared by the router.
pub struct CloudNode {
    enabled: bool,
    /// Where this node's model client gets its owner's plan credentials;
    /// filled at each claim.
    plans: RwLock<Option<Arc<PlanResolver>>>,
    claim: RwLock<Option<Claim>>,
    /// The claim's git settings, without the placeholder.
    git: RwLock<Option<GitClaim>>,
    sealed: RwLock<HashSet<AgentId>>,
    /// Agents this node took a record or a run for since it started.
    hosted: RwLock<HashSet<AgentId>>,
    workspace: Option<PathBuf>,
    /// Where git and `gh` look for their settings; `None` off the cloud.
    home: Option<PathBuf>,
    sampler: Arc<Mutex<HostSampler>>,
    sessions: Checkpoints,
    /// Where run bundles go: the data directory, kept with the sessions.
    trace_home: PathBuf,
    /// Each agent's open run.
    traces: Mutex<HashMap<AgentId, Arc<RunRecorder>>>,
}

impl std::fmt::Debug for CloudNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CloudNode")
            .field("enabled", &self.enabled)
            .field("node_id", &self.node_id())
            .finish_non_exhaustive()
    }
}

/// The claim socket in `data_dir`.
#[must_use]
pub fn claim_socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("node.sock")
}

impl CloudNode {
    /// The cloud-node state for `config`; inert unless `cloud_node`.
    #[must_use]
    pub fn new(config: &NodeConfig) -> Arc<Self> {
        let workspace = config.workspace.clone();
        let disk_path = workspace.clone().unwrap_or_else(|| config.data_dir.clone());
        let manifest = config.cloud_node.then(|| PathBuf::from(SNAPSHOT_MANIFEST));
        Arc::new(Self {
            enabled: config.cloud_node,
            plans: RwLock::new(None),
            claim: RwLock::new(None),
            git: RwLock::new(None),
            sealed: RwLock::new(HashSet::new()),
            hosted: RwLock::new(HashSet::new()),
            workspace,
            home: config.cloud_node.then(dirs::home_dir).flatten(),
            sampler: Arc::new(Mutex::new(HostSampler::new(disk_path, manifest))),
            sessions: Checkpoints::new(&config.data_dir),
            trace_home: config.data_dir.clone(),
            traces: Mutex::new(HashMap::new()),
        })
    }

    /// The branch the hub placed this machine's agent on: the only one
    /// its git tools push.
    #[must_use]
    pub fn git_branch(&self) -> Option<String> {
        self.git
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|git| git.branch.clone())
    }

    /// Where a cloud node keeps its agents' session checkpoints; `None`
    /// on other nodes, whose callers bring the conversation.
    #[must_use]
    pub fn sessions(&self) -> Option<&Checkpoints> {
        self.enabled.then_some(&self.sessions)
    }

    /// The run tracing `agent`'s conversation, started with `meta` on
    /// first use. `None` on other nodes, or when tracing is off.
    #[must_use]
    pub fn trace(&self, agent: AgentId, mut meta: RunMeta) -> Option<Arc<RunRecorder>> {
        if !self.enabled {
            return None;
        }
        let mut traces = self.traces.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(recorder) = traces.get(&agent) {
            return Some(Arc::clone(recorder));
        }
        meta.agent_id = Some(agent.to_hex());
        let recorder = match RunRecorder::start(&self.trace_home, meta, aura_config::trace().level)
        {
            Ok(recorder) => recorder?,
            Err(e) => {
                tracing::warn!(error = %e, "cloud run trace disabled: could not create bundle");
                return None;
            }
        };
        traces.insert(agent, Arc::clone(&recorder));
        Some(recorder)
    }

    /// Finish `agent`'s run, so its next conversation is a run of its own.
    pub fn end_trace(&self, agent: AgentId) {
        let recorder = self
            .traces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&agent);
        if let Some(recorder) = recorder {
            recorder.finish();
        }
    }

    /// `agent`'s newest `limit` runs here, whole and newest first. Its
    /// open run is flushed first, so the copy has every step so far.
    /// Blocks on file reads.
    #[must_use]
    pub fn runs_of(&self, agent: AgentId, limit: usize) -> Vec<RunCopy> {
        let open = self
            .traces
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&agent)
            .cloned();
        if let Some(recorder) = open {
            recorder.flush();
        }
        aura_agent_trace::bundle::runs_of(&self.trace_home, &agent.to_hex(), limit)
            .iter()
            .filter_map(|bundle| RunCopy::read(bundle).ok())
            .collect()
    }

    /// Fill `plans` from every claim from now on (the node's model client
    /// reads its credentials there).
    pub fn attach_plans(&self, plans: Arc<PlanResolver>) {
        *self.plans.write().unwrap_or_else(PoisonError::into_inner) = Some(plans);
    }

    /// Whether this node runs on a cloud machine.
    #[must_use]
    pub const fn enabled(&self) -> bool {
        self.enabled
    }

    /// The node id the hub gave it.
    #[must_use]
    pub fn node_id(&self) -> Option<String> {
        self.claim
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|claim| claim.node_id.clone())
    }

    /// The project's mesh token, once claimed, for direct sends to peers.
    #[must_use]
    pub fn mesh_token(&self) -> Option<Zeroizing<String>> {
        self.claim
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map(|claim| claim.mesh_token.clone())
    }

    /// The machine's public base URL, from a hub that sends it.
    #[must_use]
    pub fn public_url(&self) -> Option<String> {
        self.claim
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|claim| claim.public_url.clone())
    }

    /// The project directory, when configured.
    #[must_use]
    pub fn workspace(&self) -> Option<&Path> {
        self.workspace.as_deref()
    }

    /// Take `claim`, sealing the agents it lists.
    pub fn apply_claim(&self, claim: NodeClaim, port: u16) -> NodeClaimed {
        let mut sealed = 0;
        {
            let mut set = self.sealed.write().unwrap_or_else(PoisonError::into_inner);
            for hex in &claim.seal {
                match AgentId::from_hex(hex) {
                    Ok(agent) => {
                        if set.insert(agent) {
                            sealed += 1;
                        }
                    }
                    Err(err) => {
                        tracing::warn!(agent = %hex, error = %err, "claim names a bad agent id");
                    }
                }
            }
        }
        let node_id = claim.node_id.clone();
        if let Some(plans) = self
            .plans
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
        {
            plans.set(claim.owner.clone(), &claim.plans);
        }
        *self.git.write().unwrap_or_else(PoisonError::into_inner) =
            claim.git.clone().map(|git| GitClaim {
                placeholder: None,
                ..git
            });
        *self.claim.write().unwrap_or_else(PoisonError::into_inner) = Some(Claim {
            node_id: claim.node_id,
            node_token: Zeroizing::new(claim.node_token),
            mesh_token: Zeroizing::new(claim.mesh_token),
            public_url: claim.public_url,
        });
        tracing::info!(node = %node_id, sealed, "cloud node claimed");
        NodeClaimed {
            node_id,
            port,
            sealed,
            protocol: NODE_PROTOCOL,
        }
    }

    /// Check a request's bearer token. Only meaningful when
    /// [`Self::enabled`].
    ///
    /// # Errors
    ///
    /// `503` while unclaimed, `401` for a missing or wrong token.
    pub fn authorize(&self, headers: &HeaderMap, path: &str) -> Result<Caller, StatusCode> {
        let guard = self.claim.read().unwrap_or_else(PoisonError::into_inner);
        let Some(claim) = guard.as_ref() else {
            return Err(StatusCode::SERVICE_UNAVAILABLE);
        };
        if aura_auth_kit::bearer::check_bearer(headers, &claim.node_token).is_ok() {
            return Ok(Caller::Hub);
        }
        if path.starts_with("/v1/mesh")
            && aura_auth_kit::bearer::check_bearer(headers, &claim.mesh_token).is_ok()
        {
            return Ok(Caller::Peer);
        }
        Err(StatusCode::UNAUTHORIZED)
    }

    /// Whether `agent` was handed off and must not run here.
    #[must_use]
    pub fn is_sealed(&self, agent: &AgentId) -> bool {
        self.sealed
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(agent)
    }

    /// Seal `agent`: this node never runs or appends for it again.
    pub fn seal(&self, agent: AgentId) {
        self.sealed
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(agent);
    }

    /// Note that this node holds `agent`'s record: the hub hears when it
    /// grows. A no-op off a cloud machine.
    pub fn host(&self, agent: AgentId) {
        if self.enabled {
            self.hosted
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(agent);
        }
    }

    /// The agents whose records this node writes: those it took a record
    /// or a run for, less the sealed ones. Forgotten at a restart until
    /// the hub's next import or run; the hub copies on reconnecting
    /// anyway.
    #[must_use]
    pub fn hosted(&self) -> Vec<AgentId> {
        let sealed = self.sealed.read().unwrap_or_else(PoisonError::into_inner);
        self.hosted
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|agent| !sealed.contains(agent))
            .copied()
            .collect()
    }

    /// Sample this machine.
    pub async fn host_stats(&self) -> HostStats {
        let sampler = Arc::clone(&self.sampler);
        tokio::task::spawn_blocking(move || {
            sampler
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .sample()
        })
        .await
        .unwrap_or_default()
    }
}

/// Serve claims on `path` until the process ends. One JSON
/// [`NodeClaim`] per connection, answered with one [`NodeClaimed`].
///
/// # Errors
///
/// When the socket can't be bound.
#[cfg(unix)]
pub async fn serve_claims(node: Arc<CloudNode>, path: PathBuf, port: u16) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    // A stale socket from an earlier run (or a snapshot) blocks the bind.
    let _ = tokio::fs::remove_file(&path).await;
    let listener = tokio::net::UnixListener::bind(&path)?;
    tokio::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).await?;
    tracing::info!(socket = %path.display(), "cloud node waiting for its claim");
    loop {
        let (mut stream, _) = listener.accept().await?;
        let node = Arc::clone(&node);
        tokio::spawn(async move {
            let mut buf = Vec::new();
            let read = tokio::time::timeout(
                std::time::Duration::from_secs(10),
                (&mut stream)
                    .take(MAX_CLAIM_BYTES as u64)
                    .read_to_end(&mut buf),
            )
            .await;
            let answer = match read {
                Ok(Ok(_)) => match serde_json::from_slice::<NodeClaim>(&buf) {
                    Ok(claim) => {
                        if let (Some(git), Some(home)) = (claim.git.clone(), node.home.clone()) {
                            let written =
                                tokio::task::spawn_blocking(move || write_git_files(&home, &git))
                                    .await
                                    .map_err(std::io::Error::other)
                                    .and_then(|written| written);
                            if let Err(err) = written {
                                tracing::warn!(error = %err, "could not write the machine's git settings; pushes will fail");
                            }
                        }
                        serde_json::to_vec(&node.apply_claim(claim, port)).unwrap_or_default()
                    }
                    Err(err) => format!("{{\"error\":\"bad claim: {err}\"}}").into_bytes(),
                },
                Ok(Err(err)) => format!("{{\"error\":\"read failed: {err}\"}}").into_bytes(),
                Err(_) => b"{\"error\":\"timed out\"}".to_vec(),
            };
            let _ = stream.write_all(&answer).await;
            let _ = stream.shutdown().await;
        });
    }
}

/// Send `claim` (JSON) to the node listening on `path` and return its
/// answer. Used by `aura node claim --stdin`.
///
/// # Errors
///
/// When the node is not listening or answers with an error.
#[cfg(unix)]
pub async fn send_claim(path: &Path, claim: &[u8]) -> anyhow::Result<Vec<u8>> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        tokio::net::UnixStream::connect(path),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timed out connecting to {}", path.display()))??;
    stream.write_all(claim).await?;
    stream.shutdown().await?;
    let mut answer = Vec::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        stream.read_to_end(&mut answer),
    )
    .await
    .map_err(|_| anyhow::anyhow!("timed out waiting for the node's answer"))??;
    if answer.starts_with(b"{\"error\"") {
        anyhow::bail!("{}", String::from_utf8_lossy(&answer));
    }
    Ok(answer)
}

/// Claims need a Unix socket; cloud machines are Linux.
#[cfg(not(unix))]
pub async fn send_claim(_path: &Path, _claim: &[u8]) -> anyhow::Result<Vec<u8>> {
    anyhow::bail!("aura node claim needs a Unix socket")
}
