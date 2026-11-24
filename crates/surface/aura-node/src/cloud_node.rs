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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use aura_core_protocol::cloud::{HostStats, NodeClaim, NodeClaimed};
use aura_core_types::AgentId;
use aura_engine_cloud::host::HostSampler;
use axum::http::{HeaderMap, StatusCode};
use zeroize::Zeroizing;

use crate::config::NodeConfig;

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
}

/// Cloud-node state shared by the router.
pub struct CloudNode {
    enabled: bool,
    claim: RwLock<Option<Claim>>,
    sealed: RwLock<HashSet<AgentId>>,
    workspace: Option<PathBuf>,
    sampler: Arc<Mutex<HostSampler>>,
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
            claim: RwLock::new(None),
            sealed: RwLock::new(HashSet::new()),
            workspace,
            sampler: Arc::new(Mutex::new(HostSampler::new(disk_path, manifest))),
        })
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
        *self.claim.write().unwrap_or_else(PoisonError::into_inner) = Some(Claim {
            node_id: claim.node_id,
            node_token: Zeroizing::new(claim.node_token),
            mesh_token: Zeroizing::new(claim.mesh_token),
        });
        tracing::info!(node = %node_id, sealed, "cloud node claimed");
        NodeClaimed {
            node_id,
            port,
            sealed,
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
        if crate::auth::check_bearer(headers, &claim.node_token).is_ok() {
            return Ok(Caller::Hub);
        }
        if path.starts_with("/v1/mesh")
            && crate::auth::check_bearer(headers, &claim.mesh_token).is_ok()
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn node() -> Arc<CloudNode> {
        CloudNode::new(&NodeConfig {
            cloud_node: true,
            ..NodeConfig::default()
        })
    }

    fn bearer(token: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            HeaderValue::from_str(&format!("Bearer {token}")).expect("header"),
        );
        headers
    }

    fn claim(seal: Vec<String>) -> NodeClaim {
        NodeClaim {
            node_id: "aura-re36-3f9c".into(),
            node_token: "node-token-0123456789".into(),
            mesh_token: "mesh-token-0123456789".into(),
            seal,
        }
    }

    #[test]
    fn an_unclaimed_node_refuses_everything() {
        let node = node();
        assert_eq!(
            node.authorize(&bearer("anything"), "/v1/host"),
            Err(StatusCode::SERVICE_UNAVAILABLE)
        );
    }

    #[test]
    fn a_claimed_node_takes_its_tokens_only() {
        let node = node();
        let agent = AgentId::new([9; 32]);
        let answer = node.apply_claim(claim(vec![agent.to_hex(), "zz".into()]), 8080);
        assert_eq!(answer.sealed, 1);
        assert_eq!(node.node_id().as_deref(), Some("aura-re36-3f9c"));
        assert!(node.is_sealed(&agent));
        assert_eq!(
            node.authorize(&bearer("node-token-0123456789"), "/v1/host"),
            Ok(Caller::Hub)
        );
        assert_eq!(
            node.authorize(&bearer("mesh-token-0123456789"), "/v1/mesh"),
            Ok(Caller::Peer)
        );
        assert_eq!(
            node.authorize(&bearer("mesh-token-0123456789"), "/v1/host"),
            Err(StatusCode::UNAUTHORIZED),
            "peers only reach the mesh"
        );
        assert_eq!(
            node.authorize(&HeaderMap::new(), "/v1/host"),
            Err(StatusCode::UNAUTHORIZED)
        );
    }

    #[test]
    fn a_new_claim_replaces_the_old_tokens() {
        let node = node();
        node.apply_claim(claim(Vec::new()), 8080);
        let mut second = claim(Vec::new());
        second.node_token = "rotated-token-0123456789".into();
        node.apply_claim(second, 8080);
        assert_eq!(
            node.authorize(&bearer("node-token-0123456789"), "/v1/host"),
            Err(StatusCode::UNAUTHORIZED)
        );
        assert_eq!(
            node.authorize(&bearer("rotated-token-0123456789"), "/v1/host"),
            Ok(Caller::Hub)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn claims_travel_over_the_socket() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = claim_socket_path(dir.path());
        let node = node();
        let server = tokio::spawn(serve_claims(Arc::clone(&node), path.clone(), 8080));
        for _ in 0..50 {
            if path.exists() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let bytes = serde_json::to_vec(&claim(Vec::new())).expect("json");
        let answer = send_claim(&path, &bytes).await.expect("claimed");
        let claimed: NodeClaimed = serde_json::from_slice(&answer).expect("answer");
        assert_eq!(claimed.node_id, "aura-re36-3f9c");
        assert_eq!(claimed.port, 8080);
        assert_eq!(node.node_id().as_deref(), Some("aura-re36-3f9c"));
        let err = send_claim(&path, b"not json").await.expect_err("bad claim");
        assert!(err.to_string().contains("bad claim"), "{err}");
        server.abort();
    }
}
