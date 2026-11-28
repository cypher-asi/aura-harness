//! Routes a cloud node serves for its hub and its peers.
//!
//! | Route | Caller | Purpose |
//! |---|---|---|
//! | `GET /v1/host` | hub | this machine's [`HostStats`] |
//! | `POST /v1/sync/import` | hub | seed an agent's record before it runs here |
//! | `POST /v1/sync/seal/:agent_id` | hub | stop writing for an agent, report its final head |
//! | `PUT /v1/sync/session/:agent_id` | hub | the conversation the agent's first run here continues |
//! | `GET /v1/sync/session/:agent_id` | hub | the agent's current session checkpoint, to keep a copy |
//! | `POST /v1/sync/session/:agent_id/new` | hub | start a new session (`/new`) |
//! | `GET /v1/sync/traces?agent=` | hub | the agent's newest trace runs, whole |
//! | `GET /v1/workspace/patch` | hub | the agent's work as a patch |
//! | `POST /v1/mesh/mail` | hub, peers | one message for an agent here |
//! | `WS /v1/mesh` | hub | mail both ways, directory, host stats, keep-alive |
//!
//! Every route sits behind the cloud-node token check
//! ([`crate::cloud_node::CloudNode::authorize`]) and answers `404` on a
//! node that is not a cloud node.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use std::time::{Duration, Instant};

use aura_agent::checkpoint::{SessionCheckpoint, CHECKPOINT_VERSION};
use aura_agent_kernel::{ExecutorRouter, Kernel, KernelConfig};
use aura_core_protocol::cloud::{
    MeshFrame, MeshMail, MeshMailKind, MeshPeer, MeshProcess, SealResponse, SessionSaved,
    SessionStarted, SyncImport, SyncImported, TracePage, WorkspacePatch, MESH_PROCESS_AGENT,
    MESH_PROTOCOL,
};
use aura_core_types::{AgentId, Transaction};
use aura_engine::agents::{AgentInfo, AgentState, MessageKind, RemoteMail, RemoteRouter, ThreadId};
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

mod keepalive;
mod mesh;
mod patch;
mod sync;
#[cfg(test)]
mod tests;
mod watch;

pub(in crate::gateway) use keepalive::keep_awake;
pub(in crate::gateway) use mesh::{mesh_mail_handler, mesh_ws_handler};
pub(in crate::gateway) use patch::workspace_patch_handler;
pub(in crate::gateway) use sync::{
    host_handler, sync_import_handler, sync_seal_handler, sync_session_get_handler,
    sync_session_new_handler, sync_session_put_handler, sync_traces_handler,
};

/// Largest patch served.
const MAX_PATCH_BYTES: usize = 64 * 1024 * 1024;
/// Limit on the git commands behind the patch.
const GIT_TIMEOUT: Duration = Duration::from_secs(60);
/// Longest a seal waits for the agent's writes in flight to end.
const SEAL_DRAIN: std::time::Duration = std::time::Duration::from_secs(30);

/// How often host stats go up the hub socket.
const HOST_STATS_EVERY: Duration = Duration::from_secs(5);
/// How often the hosted agents' heads are compared with what the hub
/// was last told.
const HEAD_WATCH_EVERY: Duration = Duration::from_secs(1);
/// How often the hosted agents' turns and spawned agents are compared
/// with what the hub was last told.
const PROCESS_WATCH_EVERY: Duration = Duration::from_secs(1);
/// Limit on a direct send to a peer node.
const PEER_TIMEOUT: Duration = Duration::from_secs(15);
/// Runs of an agent served per traces call: its open run and the ones
/// before it, which may have changed since the hub's last copy.
const TRACE_RUNS_SERVED: usize = 3;
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

fn sealed(agent: AgentId) -> (StatusCode, String) {
    (
        StatusCode::CONFLICT,
        format!("agent {agent} is sealed on this node"),
    )
}

fn storage(e: &aura_store_db::StoreError) -> (StatusCode, String) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        format!("storage error: {e}"),
    )
}

/// Stop every chat run and wait (up to [`SEAL_DRAIN`]) for them to end. A
/// cloud node only runs its hub's agent.
async fn end_chat_runs(state: &RouterState) {
    for run in state.chat_runs.iter() {
        run.shutdown.cancel();
    }
    let deadline = tokio::time::Instant::now() + SEAL_DRAIN;
    while !state.chat_runs.is_empty() && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Wait (up to [`SEAL_DRAIN`]) for a turn the scheduler runs for `agent`
/// (mail) to end, and hold its claim so none starts.
async fn hold_claim(
    state: &RouterState,
    agent: AgentId,
) -> ApiResult<aura_engine::scheduler::ProcessingClaim> {
    tokio::time::timeout(SEAL_DRAIN, state.scheduler.processing_claim(agent))
        .await
        .map_err(|_| {
            (
                StatusCode::CONFLICT,
                format!("agent {agent} is still writing; try again"),
            )
        })?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("claim: {e}")))
}

/// The node's half of the mesh: the hub socket, the peer directory and
/// mail ids. Also this node's [`RemoteRouter`].
pub struct MeshNode {
    node: Arc<CloudNode>,
    directory: RwLock<HashMap<AgentId, MeshPeer>>,
    uplink: Mutex<Option<mpsc::UnboundedSender<MeshFrame>>>,
    pending: Mutex<Vec<MeshFrame>>,
    /// Mail for the hub, by id, until the hub acks it: a socket can close
    /// with mail on its way (its hub quit or crashed), so each hub socket
    /// is sent all of it, and the hub drops a copy it took already.
    unacked: Mutex<BTreeMap<u64, MeshMail>>,
    /// When a hub socket opened or closed last, or the node started.
    hub_seen: Mutex<Instant>,
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
            unacked: Mutex::new(BTreeMap::new()),
            hub_seen: Mutex::new(Instant::now()),
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

    /// Send `frame` to the hub, or keep it until the hub reconnects. Mail
    /// is kept until the hub acks it ([`Self::unacked_mail`]).
    pub fn to_hub(&self, frame: MeshFrame) {
        if let MeshFrame::Mail(mail) = &frame {
            let mut unacked = self.unacked.lock();
            unacked.insert(mail.id, mail.clone());
            while unacked.len() > MAX_PENDING {
                unacked.pop_first();
            }
        }
        let sender = self.uplink.lock().clone();
        if let Some(sender) = sender {
            if sender.send(frame.clone()).is_ok() {
                return;
            }
        }
        if matches!(frame, MeshFrame::Mail(_)) {
            return;
        }
        let mut pending = self.pending.lock();
        if pending.len() >= MAX_PENDING {
            pending.remove(0);
        }
        pending.push(frame);
    }

    /// How long no hub socket has been open: zero while one is.
    pub(super) fn hub_away(&self) -> Duration {
        if self.uplink.lock().is_some() {
            return Duration::ZERO;
        }
        self.hub_seen.lock().elapsed()
    }

    /// Note a hub socket opening or closing.
    fn saw_hub(&self) {
        *self.hub_seen.lock() = Instant::now();
    }

    /// The mail the hub has not acked, oldest first.
    pub(super) fn unacked_mail(&self) -> Vec<MeshMail> {
        self.unacked.lock().values().cloned().collect()
    }

    /// The hub took mail `id` that `from_node` sent, delivered or not.
    pub(super) fn acked(&self, from_node: &str, id: u64, error: Option<&str>) {
        if from_node != self.node_id() || self.unacked.lock().remove(&id).is_none() {
            return;
        }
        if let Some(error) = error {
            tracing::info!(
                mail = id,
                error,
                "the hub could not deliver mail from this node"
            );
        }
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
                // The code agents address each other by, as the hub's
                // own `list_agents` shows it.
                serde_json::json!({
                    "agent_id": peer.agent_id,
                    "code": aura_engine::code_in_name(&peer.name),
                    "name": peer.name,
                    "node": peer.node_id,
                    "remote": true,
                })
            })
            .collect()
    }
}
