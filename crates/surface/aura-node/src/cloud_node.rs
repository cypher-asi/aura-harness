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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, RwLock};

use aura_agent::checkpoint::Checkpoints;
use aura_core_protocol::cloud::{GitClaim, HostStats, NodeClaim, NodeClaimed};
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
    /// Where this node's model client gets its owner's plan credentials;
    /// filled at each claim.
    plans: RwLock<Option<Arc<PlanResolver>>>,
    claim: RwLock<Option<Claim>>,
    /// The claim's git settings, without the placeholder.
    git: RwLock<Option<GitClaim>>,
    sealed: RwLock<HashSet<AgentId>>,
    workspace: Option<PathBuf>,
    /// Where git and `gh` look for their settings; `None` off the cloud.
    home: Option<PathBuf>,
    sampler: Arc<Mutex<HostSampler>>,
    sessions: Checkpoints,
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
            workspace,
            home: config.cloud_node.then(dirs::home_dir).flatten(),
            sampler: Arc::new(Mutex::new(HostSampler::new(disk_path, manifest))),
            sessions: Checkpoints::new(&config.data_dir),
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

/// The plan credentials of the cloud node's owner, as its claim gave
/// them: each bearer is a boxd `bxds_` placeholder, which boxd's egress
/// proxy swaps for the owner's real token on the way to the provider.
/// Before a claim, and for plans without a placeholder, it answers
/// "not linked".
#[derive(Default)]
pub struct PlanResolver {
    inner: RwLock<(Option<String>, Vec<aura_core_protocol::cloud::PlanTemplate>)>,
}

impl std::fmt::Debug for PlanResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let inner = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        f.debug_struct("PlanResolver")
            .field("owner", &inner.0)
            .field("plans", &inner.1.len())
            .finish()
    }
}

impl PlanResolver {
    /// Use `plans` of `owner` from now on.
    pub fn set(&self, owner: Option<String>, plans: &[aura_core_protocol::cloud::PlanTemplate]) {
        let usable: Vec<_> = plans
            .iter()
            .filter(|plan| {
                plan.placeholder
                    .as_deref()
                    .is_some_and(|p| p.starts_with(aura_core_protocol::cloud::PLACEHOLDER_PREFIX))
            })
            .cloned()
            .collect();
        tracing::info!(
            owner = owner.as_deref().unwrap_or("-"),
            plans = usable.len(),
            "cloud node plans set"
        );
        *self.inner.write().unwrap_or_else(PoisonError::into_inner) = (owner, usable);
    }

    /// The owner tag the plans belong to.
    #[must_use]
    pub fn owner(&self) -> Option<String> {
        self.inner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .clone()
    }
}

#[async_trait::async_trait]
impl aura_core_auth::CredentialResolver for PlanResolver {
    async fn inference_credential(
        &self,
        provider: aura_core_auth::ProviderKind,
    ) -> Result<aura_core_auth::InferenceCredential, aura_core_auth::AuthError> {
        let inner = self.inner.read().unwrap_or_else(PoisonError::into_inner);
        let plan = inner
            .1
            .iter()
            .find(|plan| plan.provider == provider.id())
            .ok_or(aura_core_auth::AuthError::NotLinked(provider))?;
        let bearer = plan
            .placeholder
            .clone()
            .ok_or(aura_core_auth::AuthError::NotLinked(provider))?;
        Ok(aura_core_auth::InferenceCredential {
            provider,
            bearer,
            headers: plan.headers.clone(),
            system_preamble: plan.system_preamble.clone(),
        })
    }
}

/// Fill each plan template's placeholder in `claim` from `lookup` (the
/// machine's environment): boxd puts a `bxds_` placeholder for each of
/// the owner's host-bound secrets there. A plan whose variable is unset
/// stays without one (its requests then fail as not linked).
///
/// # Errors
///
/// When a template names a secret that is not the claim owner's plan
/// secret, or a variable holds anything but a `bxds_` placeholder: a real
/// token must never end up on a machine.
pub fn fill_placeholders(
    claim: &mut NodeClaim,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<usize, String> {
    let owner = claim.owner.clone().unwrap_or_default();
    let mut filled = 0;
    for plan in &mut claim.plans {
        if !aura_engine_cloud::names::is_plan_secret_of(&plan.env, &owner) {
            return Err(format!(
                "plan {} names {}, which is not this machine owner's secret",
                plan.provider, plan.env
            ));
        }
        let Some(value) = lookup(&plan.env).filter(|v| !v.trim().is_empty()) else {
            plan.placeholder = None;
            continue;
        };
        let value = value.trim().to_string();
        if !value.starts_with(aura_core_protocol::cloud::PLACEHOLDER_PREFIX) {
            return Err(format!(
                "{} holds a value that is not a boxd placeholder; refusing to keep a plan token on this machine",
                plan.env
            ));
        }
        plan.placeholder = Some(value);
        filled += 1;
    }
    if let Some(git) = claim.git.as_mut() {
        git.placeholder = None;
        if let Some(env) = git.env.as_deref() {
            if !aura_engine_cloud::names::is_repo_secret(env) {
                return Err(format!("git names {env}, which is not a repository secret"));
            }
            if let Some(value) = lookup(env).filter(|v| !v.trim().is_empty()) {
                let value = value.trim().to_string();
                if !value.starts_with(aura_core_protocol::cloud::PLACEHOLDER_PREFIX) {
                    return Err(format!(
                        "{env} holds a value that is not a boxd placeholder; refusing to keep a GitHub token on this machine"
                    ));
                }
                git.placeholder = Some(value);
            }
        }
    }
    Ok(filled)
}

/// Whether `value` is a boxd placeholder that can go in a file line.
fn is_placeholder(value: &str) -> bool {
    value.len() > aura_core_protocol::cloud::PLACEHOLDER_PREFIX.len()
        && value.starts_with(aura_core_protocol::cloud::PLACEHOLDER_PREFIX)
        && value.chars().all(|c| c.is_ascii_graphic())
}

/// `value` as it may sit in a URL's user info.
fn url_encoded(value: &str) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// `value` inside a double-quoted YAML string.
fn yaml_quoted(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Where git and `gh` read the settings [`write_git_files`] writes,
/// under a home folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitFiles {
    /// git's own settings (`~/.config/git/config`).
    pub config: PathBuf,
    /// The credential store git's helper reads.
    pub credentials: PathBuf,
    /// `gh`'s hosts file.
    pub gh_hosts: PathBuf,
}

impl GitFiles {
    /// The files under `home`.
    #[must_use]
    pub fn under(home: &Path) -> Self {
        let config = home.join(".config");
        Self {
            config: config.join("git").join("config"),
            credentials: config.join("aura").join("git-credentials"),
            gh_hosts: config.join("gh").join("hosts.yml"),
        }
    }
}

/// Whether `value` can sit in a git config value unquoted.
fn plain_config_value(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| !c.is_control() && !matches!(c, '"' | '\\' | '#' | ';'))
}

/// Write a file only the machine's user can read, replacing it whole.
fn write_private(path: &Path, contents: &str) -> std::io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| std::io::Error::other(format!("{} has no folder", path.display())))?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    let tmp = path.with_extension("aura-tmp");
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    {
        use std::io::Write;
        let mut file = options.open(&tmp)?;
        file.write_all(contents.as_bytes())?;
        file.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// Set up git and `gh` for `git` (a claim's, its placeholder filled):
/// the bot as commit author, `git push` pushing the current branch, and,
/// when the claim has a token, the placeholder as the password git sends
/// to `github.com` and the token `gh` sends to its API. Each run replaces
/// the files whole; a claim without a token removes the token files.
/// Blocking IO; spawns nothing.
///
/// # Errors
///
/// When a file can't be written, or the placeholder is not one.
pub fn write_git_files(home: &Path, git: &GitClaim) -> std::io::Result<()> {
    let files = GitFiles::under(home);
    let name = git
        .author_name
        .as_deref()
        .filter(|name| plain_config_value(name))
        .unwrap_or("aura-agent");
    let email = git
        .author_email
        .as_deref()
        .filter(|email| plain_config_value(email))
        .unwrap_or("aura-agent@localhost");
    let mut config =
        format!("[user]\n\tname = {name}\n\temail = {email}\n[push]\n\tdefault = current\n");
    match git.placeholder.as_deref() {
        Some(placeholder) => {
            if !is_placeholder(placeholder) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "the git token is not a boxd placeholder",
                ));
            }
            let store = files.credentials.to_string_lossy().into_owned();
            if !store
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'))
            {
                return Err(std::io::Error::other(format!(
                    "{store} can't be named in a credential helper"
                )));
            }
            write_private(
                &files.credentials,
                &format!(
                    "https://x-access-token:{}@github.com\n",
                    url_encoded(placeholder)
                ),
            )?;
            write_private(
                &files.gh_hosts,
                &format!(
                    "github.com:\n    oauth_token: {}\n    user: {}\n    git_protocol: https\n",
                    yaml_quoted(placeholder),
                    yaml_quoted(name)
                ),
            )?;
            // The empty helper drops any helper set elsewhere for GitHub.
            config.push_str(&format!(
                "[credential \"https://github.com\"]\n\thelper =\n\thelper = store --file {store}\n"
            ));
        }
        None => {
            for stale in [&files.credentials, &files.gh_hosts] {
                match std::fs::remove_file(stale) {
                    Ok(()) => {}
                    Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                    Err(err) => return Err(err),
                }
            }
        }
    }
    write_private(&files.config, &config)
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
            owner: None,
            plans: Vec::new(),
            git: None,
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

    fn claim_with(owner: &str, env: &str) -> NodeClaim {
        NodeClaim {
            node_id: "aura-x".into(),
            node_token: "t".into(),
            mesh_token: "m".into(),
            seal: Vec::new(),
            owner: Some(owner.into()),
            plans: vec![aura_core_protocol::cloud::PlanTemplate {
                provider: "claude".into(),
                env: env.into(),
                headers: Vec::new(),
                system_preamble: None,
                placeholder: None,
            }],
            git: None,
        }
    }

    const REPO_SECRET: &str = "AURA_GH_INST01_0A1B2C3D";

    fn git_claim(placeholder: Option<&str>) -> GitClaim {
        GitClaim {
            branch: "aura/reviewer-re36".into(),
            remote_url: Some("https://github.com/acme/app.git".into()),
            env: Some(REPO_SECRET.into()),
            placeholder: placeholder.map(str::to_string),
            author_name: Some("aura-agents[bot]".into()),
            author_email: Some("4242+aura-agents[bot]@users.noreply.github.com".into()),
        }
    }

    #[test]
    fn the_git_placeholder_is_filled_like_the_plans() {
        let mut claim = claim(Vec::new());
        claim.git = Some(git_claim(Some("bxds_sent-by-the-hub")));
        fill_placeholders(&mut claim, |name| {
            (name == REPO_SECRET).then(|| "bxds_gh123".to_string())
        })
        .expect("filled");
        assert_eq!(
            claim.git.as_ref().and_then(|g| g.placeholder.as_deref()),
            Some("bxds_gh123")
        );

        let mut unset = claim.clone();
        fill_placeholders(&mut unset, |_| None).expect("unset");
        assert_eq!(unset.git.and_then(|g| g.placeholder), None);

        let mut leaked = claim.clone();
        let err = fill_placeholders(&mut leaked, |_| Some("ghs_realtoken".into()))
            .expect_err("real token");
        assert!(
            err.contains("not a boxd placeholder") && !err.contains("ghs_"),
            "{err}"
        );

        let mut foreign = claim.clone();
        if let Some(git) = foreign.git.as_mut() {
            git.env = Some("AURA_A11CE000_INST01_CLAUDE".into());
        }
        let err = fill_placeholders(&mut foreign, |_| Some("bxds_x".into())).expect_err("plan");
        assert!(err.contains("not a repository secret"), "{err}");
    }

    #[test]
    fn git_files_hold_only_the_placeholder_and_only_the_user_reads_them() {
        let home = tempfile::tempdir().expect("tempdir");
        write_git_files(home.path(), &git_claim(Some("bxds_gh123"))).expect("written");
        let files = GitFiles::under(home.path());
        let config = std::fs::read_to_string(&files.config).expect("config");
        assert!(config.contains("name = aura-agents[bot]"), "{config}");
        assert!(config.contains("email = 4242+aura-agents[bot]@users.noreply.github.com"));
        assert!(config.contains("default = current"));
        assert!(config.contains(&format!(
            "helper = store --file {}",
            files.credentials.display()
        )));
        assert!(!config.contains("bxds_"), "no token in git's own settings");
        assert_eq!(
            std::fs::read_to_string(&files.credentials).expect("store"),
            "https://x-access-token:bxds_gh123@github.com\n"
        );
        let hosts = std::fs::read_to_string(&files.gh_hosts).expect("hosts");
        assert!(hosts.contains("oauth_token: \"bxds_gh123\""), "{hosts}");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for file in [&files.credentials, &files.gh_hosts] {
                let mode = std::fs::metadata(file).expect("stat").permissions().mode();
                assert_eq!(mode & 0o777, 0o600, "{}", file.display());
            }
        }

        // A re-claim without a token takes the token files away.
        write_git_files(home.path(), &git_claim(None)).expect("rewritten");
        assert!(!files.credentials.exists() && !files.gh_hosts.exists());
        let config = std::fs::read_to_string(&files.config).expect("config");
        assert!(!config.contains("credential"), "{config}");

        let err = write_git_files(home.path(), &git_claim(Some("ghs_real"))).expect_err("real");
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(!files.credentials.exists());
    }

    #[test]
    fn the_claimed_branch_is_kept_without_the_placeholder() {
        let node = node();
        assert_eq!(node.git_branch(), None);
        let mut claimed = claim(Vec::new());
        claimed.git = Some(git_claim(Some("bxds_gh123")));
        node.apply_claim(claimed, 8080);
        assert_eq!(node.git_branch().as_deref(), Some("aura/reviewer-re36"));
        let kept = node.git.read().expect("lock").clone().expect("git");
        assert_eq!(kept.placeholder, None);
    }

    #[test]
    fn placeholders_are_filled_and_real_tokens_refused() {
        let env = "AURA_A11CE000_INST01_CLAUDE";
        let mut claim = claim_with("a11ce000", env);
        let filled = fill_placeholders(&mut claim, |name| {
            (name == env).then(|| "bxds_abc123".to_string())
        })
        .expect("filled");
        assert_eq!(filled, 1);
        assert_eq!(claim.plans[0].placeholder.as_deref(), Some("bxds_abc123"));

        let mut unset = claim_with("a11ce000", env);
        assert_eq!(fill_placeholders(&mut unset, |_| None).expect("unset"), 0);
        assert!(unset.plans[0].placeholder.is_none());

        let mut leaked = claim_with("a11ce000", env);
        let err = fill_placeholders(&mut leaked, |_| Some("sk-ant-oat01-real".into()))
            .expect_err("real token");
        assert!(err.contains("not a boxd placeholder"), "{err}");
        assert!(!err.contains("sk-ant"), "the value is never echoed");

        let mut foreign = claim_with("a11ce000", "AURA_B0B00000_INST01_CLAUDE");
        let err = fill_placeholders(&mut foreign, |_| Some("bxds_x".into())).expect_err("foreign");
        assert!(err.contains("not this machine owner's"), "{err}");
    }

    #[tokio::test]
    async fn the_plan_resolver_answers_from_the_claim_only() {
        use aura_core_auth::{AuthError, CredentialResolver, ProviderKind};
        let node = CloudNode::new(&NodeConfig {
            cloud_node: true,
            ..NodeConfig::default()
        });
        let plans = Arc::new(PlanResolver::default());
        node.attach_plans(Arc::clone(&plans));
        let err = plans
            .inference_credential(ProviderKind::Claude)
            .await
            .expect_err("before the claim");
        assert!(matches!(err, AuthError::NotLinked(ProviderKind::Claude)));

        let mut claim = claim_with("a11ce000", "AURA_A11CE000_INST01_CLAUDE");
        claim.plans[0].headers = vec![("anthropic-beta".into(), "oauth-2025-04-20".into())];
        claim.plans[0].system_preamble = Some("You are Claude Code".into());
        claim.plans[0].placeholder = Some("bxds_abc".into());
        node.apply_claim(claim, 8080);
        let credential = plans
            .inference_credential(ProviderKind::Claude)
            .await
            .expect("claude");
        assert_eq!(credential.bearer, "bxds_abc");
        assert_eq!(credential.headers[0].0, "anthropic-beta");
        assert_eq!(
            credential.system_preamble.as_deref(),
            Some("You are Claude Code")
        );
        assert_eq!(plans.owner().as_deref(), Some("a11ce000"));
        assert!(plans
            .inference_credential(ProviderKind::Chatgpt)
            .await
            .is_err());
    }
}
