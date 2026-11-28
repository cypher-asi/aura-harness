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
use tokio_tungstenite::tungstenite::Message;
use tokio_util::sync::CancellationToken;

use super::node_client::NodeClient;
use crate::fleet::VmFleet;

use super::turn::{ws_request, ws_url, NodeSink, NodeSocket, NodeStream};
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
            let frame = self.directory_frame(&node, project);
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

    /// [`Self::open_link`], unless one is open already: a link gives up
    /// while its machine sleeps or is unclaimed, and a wake or claim of
    /// the hub's own opens none.
    pub(super) fn ensure_link(self: &Arc<Self>, hex: &str) {
        let open = self
            .links
            .lock()
            .get(hex)
            .is_some_and(|link| !link.frames.is_closed());
        if !open {
            self.open_link(hex);
        }
    }

    /// Mail from a node, for an agent here or on another node, or for the
    /// app's desk ([`Self::desk_for`]).
    pub(super) async fn take_mail(&self, mail: &MeshMail) -> MeshFrame {
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
            let why = format!("agent {} belongs to another user", mail.to);
            self.tell_refused(mail, &why);
            return ack(false, Some(why));
        }
        let filing = self.desk_for(mail);
        let elsewhere = self
            .directory
            .holder(&mail.to)
            .filter(|holder| holder.node != HUB_NODE && filing.is_none());
        if let Some(holder) = elsewhere {
            // For another node: forward it; that node drops it if its
            // sender already reached it directly.
            if holder.node != mail.from_node {
                self.forward(&mail.to, MeshFrame::Mail(mail.clone()));
            }
            return ack(true, None);
        }
        let delivered = match (filing, remote_mail(mail)) {
            (_, Err(err)) => Err(err),
            (Some(filing), Ok(remote)) => filing.file(remote).await,
            (None, Ok(remote)) => match self.host.lock().clone() {
                Some(host) => host.deliver_remote(remote),
                None => Err("the hub has no agents".to_string()),
            },
        };
        match delivered {
            Ok(()) => ack(true, None),
            Err(err) => {
                // A resend may succeed (the agent restarted).
                self.mail.forget(mail);
                self.tell_refused(mail, &err);
                ack(false, Some(err))
            }
        }
    }

    /// Send `frame` toward the node holding `agent`: its socket, or a
    /// direct call when the socket is down. A request sent on the socket
    /// is remembered until its node acks it ([`Self::mail_acked`]).
    fn forward(&self, agent: &str, frame: MeshFrame) {
        if let MeshFrame::Mail(mail) = &frame {
            if mail.kind == MeshMailKind::Request {
                self.mail.sent_on(mail);
            }
        }
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
        // Answered by the call itself, not by an ack on the socket.
        self.mail.acked(&mail.from_node, mail.id);
        self.send_direct(agent, mail);
    }

    /// The node holding the recipient acked mail the hub sent on: when it
    /// refused a request (its agent is sealed there, say), tell the
    /// request's sender, as a refusal here is told.
    pub(super) fn mail_acked(&self, from_node: &str, id: u64, delivered: bool, why: Option<&str>) {
        let Some(mail) = self.mail.acked(from_node, id) else {
            return;
        };
        if !delivered {
            self.tell_refused(&mail, why.unwrap_or(REFUSED_THERE));
        }
    }

    /// Wake the machine of `agent` and hand it `mail` over HTTPS. When
    /// that fails, or the machine refuses it, the sender of a request is
    /// told ([`Self::tell_refused`]).
    fn send_direct(&self, agent: &str, mail: MeshMail) {
        let fleet = Arc::clone(&self.fleet);
        let agent = agent.to_string();
        let teller = self.refusal_teller(&mail);
        tokio::spawn(async move {
            let why = match deliver_direct(&fleet, &agent, &mail).await {
                Ok(()) => return,
                Err(why) => why,
            };
            tracing::warn!(agent = %agent, error = %why, "could not deliver mail to a cloud agent");
            if let Some(teller) = teller {
                teller.tell(&why);
            }
        });
    }
}

/// Why the node holding an agent refused mail, when its ack says no more.
const REFUSED_THERE: &str = "its machine refused it";

/// Hand `mail` to the node of `agent` over HTTPS, waking its machine
/// first. `Err` is why it was not delivered.
async fn deliver_direct(fleet: &VmFleet, agent: &str, mail: &MeshMail) -> Result<(), String> {
    fleet
        .ensure_awake(agent)
        .await
        .map_err(|err| format!("its machine could not be woken: {err}"))?;
    let (Some(view), Some(token)) = (fleet.view(agent), fleet.node_token(agent)) else {
        return Err("its machine is gone".to_string());
    };
    let node = NodeClient::new(view.machine.name.clone(), view.machine.url(), token);
    let answer = node
        .mail(mail)
        .await
        .map_err(|err| format!("its machine could not be reached: {err}"))?;
    match serde_json::from_value::<MeshFrame>(answer) {
        Ok(MeshFrame::MailAck {
            delivered: false,
            error,
            ..
        }) => Err(error.unwrap_or_else(|| REFUSED_THERE.to_string())),
        _ => Ok(()),
    }
}

pub(super) fn remote_mail(mail: &MeshMail) -> Result<RemoteMail, String> {
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
        hub.forget_processes(&hex);
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
    let mut backoff = Duration::from_secs(1);
    loop {
        if stop.is_cancelled() {
            return;
        }
        let Some((node, project)) = link_target(hub, hex) else {
            return;
        };
        let Some(socket) = connect_link(&node).await else {
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(backoff) => {}
            }
            backoff = (backoff * 2).min(Duration::from_secs(60));
            continue;
        };
        backoff = Duration::from_secs(1);
        let (mut sink, mut stream) = socket.split();
        greet(&mut sink, hub, &node, project.as_deref()).await;
        // What the node wrote while no socket was open (a wake, a
        // reconnect) has no hint coming.
        if let (Some(hub), Ok(agent)) = (hub.upgrade(), AgentId::from_hex(hex)) {
            hub.note_awake(hex);
            hub.copy_soon(agent);
        }
        let link = LinkSocket {
            hub,
            hex,
            sink: &mut sink,
            stream: &mut stream,
        };
        if link.serve(outbox, stop).await == LinkEnd::Stop {
            return;
        }
        // The next socket's first frame says what runs now.
        if let Some(hub) = hub.upgrade() {
            hub.forget_processes(hex);
        }
    }
}

/// `hex`'s node and project, when its machine is awake and claimed.
fn link_target(hub: &Weak<CloudHub>, hex: &str) -> Option<(NodeClient, Option<String>)> {
    let strong = hub.upgrade()?;
    let asleep = strong.fleet.view(hex).is_none_or(|v| v.state.is_asleep());
    let node = strong.node(hex);
    let project = strong.directory.holder(hex).map(|h| h.project);
    drop(strong);
    match node {
        Ok(node) if !asleep => Some((node, project)),
        // Asleep or not claimed yet: a wake reopens the link.
        _ => None,
    }
}

async fn connect_link(node: &NodeClient) -> Option<NodeSocket> {
    let url = ws_url(node.base(), "/v1/mesh");
    let connected = match ws_request(&url, node.token()) {
        Ok(request) => tokio_tungstenite::connect_async(request).await.ok(),
        Err(_) => None,
    };
    connected.map(|(socket, _)| socket)
}

/// Say hello and send the project's directory.
async fn greet(
    sink: &mut NodeSink,
    hub: &Weak<CloudHub>,
    node: &NodeClient,
    project: Option<&str>,
) {
    let hello = MeshFrame::Hello {
        node_id: HUB_NODE.into(),
        protocol: MESH_PROTOCOL,
    };
    let _ = sink.send(Message::Text(hello.to_text().into())).await;
    if let (Some(hub), Some(project)) = (hub.upgrade(), project) {
        let frame = hub.directory_frame(node.node(), project);
        let _ = sink.send(Message::Text(frame.to_text().into())).await;
    }
}

/// How a socket's session ended.
#[derive(Debug, PartialEq, Eq)]
enum LinkEnd {
    /// The link is done.
    Stop,
    /// The socket broke; connect again.
    Reconnect,
}

/// One connected socket to `hex`'s node.
struct LinkSocket<'a> {
    hub: &'a Weak<CloudHub>,
    hex: &'a str,
    sink: &'a mut NodeSink,
    stream: &'a mut NodeStream,
}

impl LinkSocket<'_> {
    /// Carry frames both ways until the socket breaks or the link stops.
    async fn serve(
        mut self,
        outbox: &mut mpsc::UnboundedReceiver<MeshFrame>,
        stop: &CancellationToken,
    ) -> LinkEnd {
        loop {
            tokio::select! {
                () = stop.cancelled() => return LinkEnd::Stop,
                frame = outbox.recv() => {
                    let Some(frame) = frame else { return LinkEnd::Stop };
                    if self.sink.send(Message::Text(frame.to_text().into())).await.is_err() {
                        return LinkEnd::Reconnect;
                    }
                }
                message = self.stream.next() => {
                    let Some(Ok(message)) = message else { return LinkEnd::Reconnect };
                    let Message::Text(text) = message else { continue };
                    let Ok(frame) = MeshFrame::from_text(&text) else { continue };
                    let Some(hub) = self.hub.upgrade() else { return LinkEnd::Stop };
                    self.take_frame(&hub, frame).await;
                }
            }
        }
    }

    /// Act on one frame from the node.
    async fn take_frame(&mut self, hub: &Arc<CloudHub>, frame: MeshFrame) {
        let hex = self.hex;
        match frame {
            MeshFrame::Mail(mail) => {
                hub.fleet.touch(hex);
                let ack = hub.take_mail(&mail).await;
                let _ = self.sink.send(Message::Text(ack.to_text().into())).await;
            }
            MeshFrame::MailAck {
                from_node,
                id,
                delivered,
                error,
            } => hub.mail_acked(&from_node, id, delivered, error.as_deref()),
            MeshFrame::HostStats(stats) => hub.track(hex, stats),
            // Only the link's agent, and the agents it spawned
            // there, are copied from its node.
            MeshFrame::RecordCommitted { agent_id, head_seq } => {
                let Ok(agent) = AgentId::from_hex(hex) else {
                    return;
                };
                if agent_id == hex {
                    hub.note_head(agent, head_seq);
                    hub.copy_soon(agent);
                } else if hub
                    .children_of(hex)
                    .iter()
                    .any(|child| child.to_hex() == agent_id)
                {
                    hub.copy_soon(agent);
                }
            }
            MeshFrame::Processes { processes } => hub.note_processes(hex, processes),
            MeshFrame::Ping { ms } => {
                let _ = self
                    .sink
                    .send(Message::Text(MeshFrame::Pong { ms }.to_text().into()))
                    .await;
            }
            _ => {}
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
