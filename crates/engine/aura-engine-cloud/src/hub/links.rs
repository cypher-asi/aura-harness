//! Mesh mail: the sockets to the nodes and the router the local host
//! sends cloud agents' mail through.

use std::sync::{Arc, Weak};
use std::time::Duration;

use crate::mesh::{global_thread, HUB_NODE};
use crate::names;
use aura_core_protocol::cloud::{MeshFrame, MeshMail, MeshMailKind, MESH_PROTOCOL};
use aura_core_types::time::now_ms;
use aura_core_types::AgentId;
use aura_engine::{AgentHost, MessageKind, RemoteMail, RemoteRouter, ThreadId};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::node_client::NodeClient;

use super::turn::{ws_request, ws_url};
use super::{CloudHub, Link};

impl CloudHub {
    /// Hand mail from nodes to `host`'s agents, and send mail for cloud
    /// agents out through this hub.
    pub fn attach_host(self: &Arc<Self>, host: &AgentHost) {
        *self.host.lock() = Some(host.clone());
        host.set_remote_router(Arc::new(HubRouter(Arc::downgrade(self))));
    }

    /// Every agent of `project` and the node holding it, for list
    /// markers.
    pub(super) fn send_directories(&self, project: &str) {
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

    pub(super) fn open_link(self: &Arc<Self>, hex: &str) {
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
    pub(super) fn take_mail(&self, mail: &MeshMail) -> MeshFrame {
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
pub(super) struct HubRouter(pub(super) Weak<CloudHub>);

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
