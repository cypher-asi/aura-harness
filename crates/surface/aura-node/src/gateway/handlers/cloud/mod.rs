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

mod mesh;
mod patch;
mod sync;
#[cfg(test)]
mod tests;

pub(in crate::gateway) use mesh::{mesh_mail_handler, mesh_ws_handler};
pub(in crate::gateway) use patch::workspace_patch_handler;
pub(in crate::gateway) use sync::{
    host_handler, sync_import_handler, sync_seal_handler, sync_traces_handler,
};

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
