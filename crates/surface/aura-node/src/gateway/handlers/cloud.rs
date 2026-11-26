//! Routes a cloud node serves for its hub and its peers.
//!
//! | Route | Caller | Purpose |
//! |---|---|---|
//! | `GET /v1/host` | hub | this machine's [`HostStats`] |
//! | `POST /v1/sync/import` | hub | seed an agent's record before it runs here |
//! | `POST /v1/sync/seal/:agent_id` | hub | stop writing for an agent, report its final head |
//! | `GET /v1/sync/traces` | hub | trace runs since a cursor |
//! | `GET /v1/workspace/patch` | hub | the agent's work as a patch |
//! | `POST /v1/mesh/mail` | hub, peers | one message for an agent here |
//! | `WS /v1/mesh` | hub | mail both ways, directory, host stats, keep-alive |
//!
//! Every route sits behind the cloud-node token check
//! ([`crate::cloud_node::CloudNode::authorize`]) and answers `404` on a
//! node that is not a cloud node.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::Duration;

use aura_core_protocol::cloud::{
    MeshFrame, MeshMail, MeshMailKind, MeshPeer, SealResponse, SyncImport, SyncImported, TracePage,
    WorkspacePatch, MESH_PROTOCOL,
};
use aura_core_types::AgentId;
use aura_engine::agents::{MessageKind, RemoteMail, RemoteRouter, ThreadId};
use aura_engine_cloud::mesh::{global_thread, MailBook};
use aura_engine_cloud::replica::verify_page;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Path as AxumPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::Deserialize;
use tokio::sync::mpsc;

use crate::cloud_node::CloudNode;
use crate::gateway::RouterState;

/// Largest patch served.
const MAX_PATCH_BYTES: usize = 64 * 1024 * 1024;
/// Limit on the git commands behind the patch.
const GIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest a seal waits for the agent's writes in flight to end.
const SEAL_DRAIN: std::time::Duration = std::time::Duration::from_secs(30);

/// How often host stats go up the hub socket.
const HOST_STATS_EVERY: Duration = Duration::from_secs(5);
/// Limit on a direct send to a peer node.
const PEER_TIMEOUT: Duration = Duration::from_secs(15);
/// Most frames kept for the hub while its socket is down.
const MAX_PENDING: usize = 1024;

type ApiResult<T> = Result<T, (StatusCode, String)>;

fn not_cloud() -> (StatusCode, String) {
    (StatusCode::NOT_FOUND, "not a cloud node".to_string())
}

fn bad(message: impl Into<String>) -> (StatusCode, String) {
    (StatusCode::BAD_REQUEST, message.into())
}

fn parse_agent(hex: &str) -> ApiResult<AgentId> {
    AgentId::from_hex(hex.trim()).map_err(|err| bad(format!("invalid agent id: {err}")))
}

/// The node's half of the mesh: the hub socket, the peer directory and
/// mail ids. Also this node's [`RemoteRouter`].
pub struct MeshNode {
    node: Arc<CloudNode>,
    directory: RwLock<HashMap<AgentId, MeshPeer>>,
    uplink: Mutex<Option<mpsc::UnboundedSender<MeshFrame>>>,
    pending: Mutex<Vec<MeshFrame>>,
    next_id: AtomicU64,
    seen: MailBook,
    http: reqwest::Client,
}

impl std::fmt::Debug for MeshNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MeshNode").finish_non_exhaustive()
    }
}

impl MeshNode {
    /// The mesh state of `node`.
    #[must_use]
    pub fn new(node: Arc<CloudNode>) -> Arc<Self> {
        Arc::new(Self {
            node,
            directory: RwLock::new(HashMap::new()),
            uplink: Mutex::new(None),
            pending: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(aura_engine_cloud::mesh::first_mail_id()),
            seen: MailBook::new(),
            http: reqwest::Client::builder()
                .timeout(PEER_TIMEOUT)
                .build()
                .unwrap_or_default(),
        })
    }

    fn node_id(&self) -> String {
        self.node
            .node_id()
            .unwrap_or_else(|| "unclaimed".to_string())
    }

    /// Send `frame` to the hub, or keep it until the hub reconnects.
    pub fn to_hub(&self, frame: MeshFrame) {
        let sender = self.uplink.lock().clone();
        if let Some(sender) = sender {
            if sender.send(frame.clone()).is_ok() {
                return;
            }
        }
        let mut pending = self.pending.lock();
        if pending.len() >= MAX_PENDING {
            pending.remove(0);
        }
        pending.push(frame);
    }

    fn mail_frame(&self, mail: &RemoteMail) -> MeshMail {
        let node = self.node_id();
        MeshMail {
            id: self.next_id.fetch_add(1, Ordering::Relaxed),
            from_node: node.clone(),
            thread: global_thread(&node, &mail.thread.0),
            subject: mail.subject.clone(),
            from: mail.from.to_hex(),
            from_name: mail.from_name.clone(),
            to: mail.to.to_hex(),
            kind: match mail.kind {
                MessageKind::Request => MeshMailKind::Request,
                MessageKind::Reply => MeshMailKind::Reply,
            },
            text: mail.text.clone(),
            sent_ms: u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0),
        }
    }
}

/// Turn a mesh message into mail for this node's host.
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

/// [`RemoteRouter`] for a cloud node: direct to a peer's domain when the
/// directory has one, through the hub otherwise. The hub always gets a
/// copy for its Inbox and its mirror.
pub struct MeshRouter(pub Arc<MeshNode>);

impl RemoteRouter for MeshRouter {
    fn name_of(&self, agent: AgentId) -> Option<String> {
        self.0
            .directory
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&agent)
            .map(|peer| peer.name.clone())
    }

    fn route(&self, mail: RemoteMail) -> Result<(), String> {
        let peer = self
            .0
            .directory
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&mail.to)
            .cloned()
            .ok_or_else(|| format!("unknown agent {}", mail.to))?;
        let frame = self.0.mail_frame(&mail);
        let token = self.0.node.mesh_token();
        match (peer.url, token) {
            (Some(url), Some(token)) if peer.node_id != aura_engine_cloud::mesh::HUB_NODE => {
                let mesh = Arc::clone(&self.0);
                tokio::spawn(async move {
                    let sent = mesh
                        .http
                        .post(format!("{}/v1/mesh/mail", url.trim_end_matches('/')))
                        .bearer_auth(token.as_str())
                        .json(&frame)
                        .send()
                        .await
                        .and_then(reqwest::Response::error_for_status);
                    match sent {
                        // The hub still gets a copy, marked delivered.
                        Ok(_) => mesh.to_hub(MeshFrame::Mail(frame)),
                        Err(err) => {
                            tracing::info!(peer = %url, error = %err, "direct mesh send failed; relaying through the hub");
                            mesh.to_hub(MeshFrame::Mail(frame));
                        }
                    }
                });
            }
            _ => self.0.to_hub(MeshFrame::Mail(frame)),
        }
        Ok(())
    }

    fn peers(&self, _root: AgentId) -> Vec<serde_json::Value> {
        self.0
            .directory
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(|peer| {
                serde_json::json!({
                    "agent_id": peer.agent_id,
                    "name": peer.name,
                    "node": peer.node_id,
                    "remote": true,
                })
            })
            .collect()
    }
}

/// `GET /v1/host`.
pub(in crate::gateway) async fn host_handler(
    State(state): State<RouterState>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    Ok(Json(state.cloud.host_stats().await))
}

/// `POST /v1/sync/import`: append verified entries verbatim (the node
/// side of the record hand-off; a declared exception to kernel-only
/// appends, see `docs/invariants.md`).
pub(in crate::gateway) async fn sync_import_handler(
    State(state): State<RouterState>,
    Json(import): Json<SyncImport>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    let agent = parse_agent(&import.agent_id)?;
    if state.cloud.is_sealed(&agent) {
        return Err((
            StatusCode::CONFLICT,
            format!("agent {agent} is sealed on this node"),
        ));
    }
    let storage = |e: aura_store_db::StoreError| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("storage error: {e}"),
        )
    };
    let head = state.store.get_head_seq(agent).map_err(storage)?;
    let head_hash = if head == 0 {
        None
    } else {
        Some(
            state
                .store
                .get_record_entry(agent, head)
                .map_err(storage)?
                .tx
                .hash,
        )
    };
    let fresh = verify_page(agent, head, head_hash, &import.entries)
        .map_err(|err| (StatusCode::CONFLICT, err.to_string()))?;
    if !fresh.is_empty() {
        state
            .store
            .append_replica_batch(agent, head + 1, fresh)
            .map_err(storage)?;
    }
    let head_seq = state.store.get_head_seq(agent).map_err(storage)?;
    Ok(Json(SyncImported { head_seq }))
}

/// `POST /v1/sync/seal/:agent_id`.
pub(in crate::gateway) async fn sync_seal_handler(
    State(state): State<RouterState>,
    AxumPath(agent_hex): AxumPath<String>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    let agent = parse_agent(&agent_hex)?;
    state.cloud.seal(agent);
    // New turns are refused from now on by the sealed check on `/v1/run`
    // and `/tx`. Stop what is already running: the agent's spawned
    // children, and every chat run (a cloud node only runs its hub's
    // agent), then wait for them to end so the head is final.
    let _ = state
        .agent_host
        .lifecycle(agent, aura_engine::agents::LifecycleAction::Stop);
    for run in state.chat_runs.iter() {
        run.shutdown.cancel();
    }
    let deadline = tokio::time::Instant::now() + SEAL_DRAIN;
    while !state.chat_runs.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    // A turn the scheduler runs (mail) holds the agent's claim; waiting
    // for it means no write is in flight when the head is read.
    let claim = tokio::time::timeout(SEAL_DRAIN, state.scheduler.processing_claim(agent))
        .await
        .map_err(|_| {
            (
                StatusCode::CONFLICT,
                format!("agent {agent} is still writing; try the seal again"),
            )
        })?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("claim: {e}")))?;
    let head_seq = state.store.get_head_seq(agent).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("storage error: {e}"),
        )
    })?;
    drop(claim);
    Ok(Json(SealResponse { head_seq }))
}

#[derive(Debug, Deserialize)]
pub(in crate::gateway) struct SinceQuery {
    #[serde(default)]
    since: u64,
}

/// `GET /v1/sync/traces?since=`: trace runs are mirrored by the hub from
/// the agents' records today, so this page is empty; the cursor is kept
/// so the hub's loop needs no change when runs are served here.
pub(in crate::gateway) async fn sync_traces_handler(
    State(state): State<RouterState>,
    Query(query): Query<SinceQuery>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    Ok(Json(TracePage {
        runs: Vec::new(),
        next_since: query.since,
    }))
}

async fn git(dir: &std::path::Path, args: &[&str]) -> ApiResult<String> {
    git_with_index(dir, args, None).await
}

/// `git args` in `dir`, with `index` as the index file when given.
async fn git_with_index(
    dir: &std::path::Path,
    args: &[&str],
    index: Option<&std::path::Path>,
) -> ApiResult<String> {
    let mut command = tokio::process::Command::new("git");
    command.arg("-C").arg(dir).args(args);
    if let Some(index) = index {
        command.env("GIT_INDEX_FILE", index);
    }
    let output = tokio::time::timeout(GIT_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            (
                StatusCode::GATEWAY_TIMEOUT,
                format!("git {} timed out", args.join(" ")),
            )
        })?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("git: {e}")))?;
    if !output.status.success() {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!(
                "git {}: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Paths a patch never carries.
const PATCH_EXCLUDES: &[&str] = &[
    ":(exclude,glob)**/.env",
    ":(exclude,glob)**/.env.*",
    ":(exclude,glob).env",
    ":(exclude,glob).env.*",
];

/// The working tree (tracked changes and the untracked files
/// `.gitignore` lets through, never `.env` files) written as a tree
/// object from a scratch copy of the index, so the agent's own index is
/// never touched.
async fn worktree_tree(dir: &std::path::Path) -> ApiResult<String> {
    let git_dir = git(dir, &["rev-parse", "--absolute-git-dir"]).await?;
    let git_dir = std::path::PathBuf::from(git_dir.trim());
    let scratch = git_dir.join(format!(
        "aura-patch-{}.index",
        uuid::Uuid::new_v4().simple()
    ));
    match tokio::fs::copy(git_dir.join("index"), &scratch).await {
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("copy the index: {err}"),
            ))
        }
    }
    let mut add_args = vec!["add", "-A", "--", "."];
    add_args.extend(PATCH_EXCLUDES);
    let tree = match git_with_index(dir, &add_args, Some(&scratch)).await {
        Ok(_) => git_with_index(dir, &["write-tree"], Some(&scratch)).await,
        Err(err) => Err(err),
    };
    let _ = tokio::fs::remove_file(&scratch).await;
    Ok(tree?.trim().to_string())
}

/// `GET /v1/workspace/patch`: the working tree's changes (tracked and
/// untracked) since what the hub last had of it
/// ([`aura_engine_cloud::workspace::BASE_REF`]; `HEAD` on a workspace
/// without one), and the tree they were taken from.
pub(in crate::gateway) async fn workspace_patch_handler(
    State(state): State<RouterState>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    let dir = state
        .cloud
        .workspace()
        .ok_or_else(|| {
            (
                StatusCode::CONFLICT,
                "AURA_WORKSPACE is not set".to_string(),
            )
        })?
        .to_path_buf();
    let base_ref = format!("{}^{{commit}}", aura_engine_cloud::workspace::BASE_REF);
    let base = match git(&dir, &["rev-parse", "-q", "--verify", &base_ref]).await {
        Ok(commit) => commit.trim().to_string(),
        Err(_) => git(&dir, &["rev-parse", "HEAD"]).await?.trim().to_string(),
    };
    let tree = worktree_tree(&dir).await?;
    let mut diff_args = vec!["diff", "--binary", base.as_str(), tree.as_str(), "--", "."];
    diff_args.extend(PATCH_EXCLUDES);
    let patch = git(&dir, &diff_args).await?;
    if patch.len() > MAX_PATCH_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            format!("patch is {} bytes, above {MAX_PATCH_BYTES}", patch.len()),
        ));
    }
    let mut name_args = vec![
        "diff",
        "--name-only",
        base.as_str(),
        tree.as_str(),
        "--",
        ".",
    ];
    name_args.extend(PATCH_EXCLUDES);
    let files = git(&dir, &name_args)
        .await?
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect();
    Ok(Json(WorkspacePatch {
        base_commit: base,
        patch,
        files,
        tree: Some(tree),
    }))
}

/// Deliver one mesh message to the agent here it is for.
fn accept_mail(state: &RouterState, mesh: &MeshNode, mail: &MeshMail) -> MeshFrame {
    let ack = |delivered: bool, error: Option<String>| MeshFrame::MailAck {
        from_node: mail.from_node.clone(),
        id: mail.id,
        delivered,
        error,
    };
    if !mesh.seen.first_sight(mail) {
        return ack(true, None);
    }
    let delivered = remote_mail(mail).and_then(|remote| {
        if state.cloud.is_sealed(&remote.to) {
            return Err(format!("agent {} is sealed on this node", remote.to));
        }
        state.agent_host.deliver_remote(remote)
    });
    match delivered {
        Ok(()) => ack(true, None),
        Err(err) => {
            mesh.seen.forget(mail);
            ack(false, Some(err))
        }
    }
}

/// `POST /v1/mesh/mail`.
pub(in crate::gateway) async fn mesh_mail_handler(
    State(state): State<RouterState>,
    Json(mail): Json<MeshMail>,
) -> ApiResult<impl IntoResponse> {
    if !state.cloud.enabled() {
        return Err(not_cloud());
    }
    Ok(Json(accept_mail(&state, &state.mesh, &mail)))
}

/// `WS /v1/mesh`: the hub's socket.
pub(in crate::gateway) async fn mesh_ws_handler(
    State(state): State<RouterState>,
    ws: WebSocketUpgrade,
) -> Response {
    if !state.cloud.enabled() {
        return not_cloud().into_response();
    }
    ws.on_upgrade(move |socket| run_mesh_socket(state, socket))
}

async fn run_mesh_socket(state: RouterState, socket: WebSocket) {
    let mesh = Arc::clone(&state.mesh);
    let (tx, mut rx) = mpsc::unbounded_channel::<MeshFrame>();
    let pending: Vec<MeshFrame> = std::mem::take(&mut *mesh.pending.lock());
    *mesh.uplink.lock() = Some(tx.clone());
    let (mut sink, mut stream) = socket.split();

    let _ = tx.send(MeshFrame::Hello {
        node_id: mesh.node_id(),
        protocol: MESH_PROTOCOL,
    });
    for frame in pending {
        let _ = tx.send(frame);
    }

    let writer = tokio::spawn(async move {
        while let Some(frame) = rx.recv().await {
            if sink.send(WsMessage::Text(frame.to_text())).await.is_err() {
                break;
            }
        }
    });
    let stats = {
        let tx = tx.clone();
        let node = Arc::clone(&state.cloud);
        tokio::spawn(async move {
            loop {
                if tx
                    .send(MeshFrame::HostStats(node.host_stats().await))
                    .is_err()
                {
                    break;
                }
                tokio::time::sleep(HOST_STATS_EVERY).await;
            }
        })
    };

    while let Some(Ok(message)) = stream.next().await {
        let WsMessage::Text(text) = message else {
            if matches!(message, WsMessage::Close(_)) {
                break;
            }
            continue;
        };
        let Ok(frame) = MeshFrame::from_text(&text) else {
            tracing::debug!("skipping an unreadable mesh frame");
            continue;
        };
        match frame {
            MeshFrame::Directory { peers } => {
                let map = peers
                    .into_iter()
                    .filter_map(|peer| AgentId::from_hex(&peer.agent_id).ok().map(|id| (id, peer)))
                    .collect();
                *mesh
                    .directory
                    .write()
                    .unwrap_or_else(PoisonError::into_inner) = map;
            }
            MeshFrame::Mail(mail) => {
                let _ = tx.send(accept_mail(&state, &mesh, &mail));
            }
            MeshFrame::Ping { ms } => {
                let _ = tx.send(MeshFrame::Pong { ms });
            }
            _ => {}
        }
    }
    stats.abort();
    writer.abort();
    let mut uplink = mesh.uplink.lock();
    if uplink
        .as_ref()
        .is_some_and(|current| current.same_channel(&tx))
    {
        *uplink = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::NodeConfig;
    use crate::gateway::{create_router, RouterStateConfig};
    use aura_core_protocol::cloud::NodeClaim;
    use aura_core_types::{RecordEntry, Transaction};
    use aura_exec_tools::{ToolCatalog, ToolConfig};
    use aura_model::{MockProvider, ModelProvider};
    use aura_store_db::{RocksStore, Store};
    use axum::body::Body;
    use axum::http::Request;
    use tower::util::ServiceExt;

    const NODE_TOKEN: &str = "node-token-for-tests-0123";

    fn cloud_state(dir: &std::path::Path) -> RouterState {
        cloud_state_in(dir, None)
    }

    fn cloud_state_in(dir: &std::path::Path, workspace: Option<std::path::PathBuf>) -> RouterState {
        let store: Arc<dyn Store> =
            Arc::new(RocksStore::open(dir.join("db"), false).expect("store"));
        let provider: Arc<dyn ModelProvider + Send + Sync> =
            Arc::new(MockProvider::simple_response("mock"));
        let scheduler = Arc::new(aura_engine::scheduler::Scheduler::new(
            store.clone(),
            provider.clone(),
            vec![],
            vec![],
            dir.join("ws"),
            None,
        ));
        RouterState::new(RouterStateConfig {
            store,
            scheduler,
            config: NodeConfig {
                cloud_node: true,
                data_dir: dir.to_path_buf(),
                workspace,
                ..NodeConfig::default()
            },
            provider,
            tool_config: ToolConfig::default(),
            catalog: Arc::new(ToolCatalog::new()),
            domain_api: None,
            automaton_controller: None,
            automaton_bridge: None,
            memory_manager: None,
            skill_manager: None,
            secrets_vault: None,
            process_store: None,
            trigger_registrar: None,
            router_url: None,
        })
    }

    fn claim(state: &RouterState) {
        state.cloud.apply_claim(
            NodeClaim {
                node_id: "aura-re36-3f9c".into(),
                node_token: NODE_TOKEN.into(),
                mesh_token: "mesh-token-for-tests-0123".into(),
                seal: Vec::new(),
                owner: None,
                plans: Vec::new(),
                git: None,
            },
            8080,
        );
    }

    fn git_in(dir: &std::path::Path, args: &[&str]) -> String {
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

    #[tokio::test]
    async fn the_patch_holds_only_work_past_the_base_and_leaves_the_index_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ws = dir.path().join("workspace");
        std::fs::create_dir(&ws).expect("mkdir");
        git_in(&ws, &["init", "-q"]);
        std::fs::write(ws.join("a.rs"), "one").expect("write");
        git_in(&ws, &["add", "-A"]);
        git_in(
            &ws,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit",
                "-q",
                "-m",
                "c",
            ],
        );
        // What the hub sent: an edit, left uncommitted, marked as the base.
        std::fs::write(ws.join("a.rs"), "one, from the hub").expect("write");
        let tree = {
            git_in(&ws, &["add", "-A"]);
            let tree = git_in(&ws, &["write-tree"]);
            git_in(&ws, &["reset", "-q"]);
            tree
        };
        let base = git_in(
            &ws,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "commit-tree",
                &tree,
                "-p",
                "HEAD",
                "-m",
                "base",
            ],
        );
        git_in(
            &ws,
            &["update-ref", aura_engine_cloud::workspace::BASE_REF, &base],
        );
        // The agent's work: an untracked file and a secret it must not send.
        std::fs::write(ws.join("new.rs"), "agent").expect("write");
        std::fs::write(ws.join(".env"), "TOKEN=1").expect("write");
        let index_before = std::fs::read(ws.join(".git/index")).expect("index");

        let state = cloud_state_in(dir.path(), Some(ws.clone()));
        claim(&state);
        let (status, body) =
            call(&state, "GET", "/v1/workspace/patch", Some(NODE_TOKEN), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["files"], serde_json::json!(["new.rs"]), "{body}");
        assert_eq!(body["base_commit"], base.as_str());
        let patch = body["patch"].as_str().unwrap_or_default();
        assert!(
            !patch.contains("from the hub") && !patch.contains("TOKEN"),
            "{patch}"
        );
        let tree = body["tree"].as_str().expect("tree").to_string();
        assert_eq!(git_in(&ws, &["cat-file", "-t", &tree]), "tree");
        assert_eq!(
            std::fs::read(ws.join(".git/index")).expect("index"),
            index_before,
            "the agent's index is untouched"
        );
        assert!(
            std::fs::read_dir(ws.join(".git"))
                .expect("git dir")
                .filter_map(Result::ok)
                .all(|e| !e.file_name().to_string_lossy().starts_with("aura-patch-")),
            "the scratch index is gone"
        );
    }

    async fn call(
        state: &RouterState,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let body = match body {
            Some(json) => {
                request = request.header("content-type", "application/json");
                Body::from(json.to_string())
            }
            None => Body::empty(),
        };
        let response = create_router(state.clone())
            .oneshot(request.body(body).expect("request"))
            .await
            .expect("response");
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body");
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null),
        )
    }

    #[tokio::test]
    async fn an_unclaimed_node_refuses_and_a_claimed_one_serves_its_hub() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = cloud_state(dir.path());
        let (status, _) = call(&state, "GET", "/v1/host", Some(NODE_TOKEN), None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        // The node's own startup token is never accepted on a cloud node.
        let (status, _) = call(&state, "GET", "/v1/host", Some("test"), None).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);

        claim(&state);
        let (status, _) = call(&state, "GET", "/v1/host", Some("test"), None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        let (status, body) = call(&state, "GET", "/v1/host", Some(NODE_TOKEN), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert!(body["cpu_count"].as_u64().unwrap_or(0) > 0, "{body}");
        let (status, _) = call(&state, "GET", "/health", None, None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn import_appends_verified_entries_and_seal_stops_further_writes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = cloud_state(dir.path());
        claim(&state);
        let agent = AgentId::new([5; 32]);
        let entries: Vec<RecordEntry> = (1..=3)
            .map(|seq| {
                RecordEntry::builder(seq, Transaction::user_prompt(agent, format!("p{seq}")))
                    .build()
            })
            .collect();
        let import = serde_json::json!({
            "agent_id": agent.to_hex(),
            "name": "re36",
            "entries": entries,
            "done": true,
        });
        let (status, body) = call(
            &state,
            "POST",
            "/v1/sync/import",
            Some(NODE_TOKEN),
            Some(import.clone()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["head_seq"], 3);
        // Importing the same page again changes nothing.
        let (status, body) = call(
            &state,
            "POST",
            "/v1/sync/import",
            Some(NODE_TOKEN),
            Some(import.clone()),
        )
        .await;
        assert_eq!(
            (status, body["head_seq"].as_u64()),
            (StatusCode::OK, Some(3))
        );

        let (status, body) = call(
            &state,
            "POST",
            &format!("/v1/sync/seal/{}", agent.to_hex()),
            Some(NODE_TOKEN),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["head_seq"], 3);
        let (status, _) = call(
            &state,
            "POST",
            "/v1/sync/import",
            Some(NODE_TOKEN),
            Some(import),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT);
        let tx = serde_json::json!({"agent_id": agent.to_hex(), "kind": "user_prompt", "payload": "aGk="});
        let (status, _) = call(&state, "POST", "/tx", Some(NODE_TOKEN), Some(tx)).await;
        assert_eq!(status, StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn a_gap_in_an_import_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = cloud_state(dir.path());
        claim(&state);
        let agent = AgentId::new([6; 32]);
        let entry = RecordEntry::builder(2, Transaction::user_prompt(agent, "p2")).build();
        let import = serde_json::json!({"agent_id": agent.to_hex(), "name": "x", "entries": [entry], "done": true});
        let (status, body) = call(
            &state,
            "POST",
            "/v1/sync/import",
            Some(NODE_TOKEN),
            Some(import),
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
    }

    #[tokio::test]
    async fn mail_from_a_peer_reaches_nobody_unknown() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = cloud_state(dir.path());
        claim(&state);
        let mail = serde_json::json!({
            "id": 1, "from_node": "aura-ex2-1111", "thread": "aura-ex2-1111:t1", "subject": "s",
            "from": AgentId::new([1; 32]).to_hex(), "from_name": "peer", "to": AgentId::new([2; 32]).to_hex(),
            "kind": "request", "text": "hi", "sent_ms": 0
        });
        let (status, body) = call(
            &state,
            "POST",
            "/v1/mesh/mail",
            Some("mesh-token-for-tests-0123"),
            Some(mail),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["type"], "mail_ack");
        assert_eq!(body["delivered"], false);
        assert!(
            body["error"]
                .as_str()
                .unwrap_or_default()
                .contains("unknown agent"),
            "{body}"
        );
    }

    #[tokio::test]
    async fn other_nodes_do_not_serve_cloud_routes() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut state = cloud_state(dir.path());
        state.cloud = crate::cloud_node::CloudNode::new(&NodeConfig::default());
        state.config.require_auth = false;
        state.config.cloud_node = false;
        let (status, _) = call(&state, "GET", "/v1/host", None, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[test]
    fn mesh_mail_maps_to_host_mail() {
        let mail = MeshMail {
            id: 3,
            from_node: "hub".into(),
            thread: "hub:t1".into(),
            subject: "Review".into(),
            from: AgentId::new([1; 32]).to_hex(),
            from_name: "lead".into(),
            to: AgentId::new([2; 32]).to_hex(),
            kind: MeshMailKind::Reply,
            text: "done".into(),
            sent_ms: 0,
        };
        let remote = remote_mail(&mail).expect("maps");
        assert_eq!(remote.kind, MessageKind::Reply);
        assert_eq!(remote.thread, ThreadId("hub:t1".into()));
        assert!(remote_mail(&MeshMail {
            from: "zz".into(),
            ..mail
        })
        .is_err());
    }
}
