//! Who a placed agent reaches through the hub, and the requests it sends
//! them.
//!
//! The app names the agents a placed agent reaches as it would reach them
//! here ([`CloudHub::set_reach`]): for a channel member, its channel's and
//! project's peers. Its machine's directory lists them as held by the
//! hub, even one another machine holds, so their mail comes here, and the
//! hub files each request at the app's [`MailDesk`] instead of an agent's
//! mailbox. It then counts as a request the agent sent here: an inbox
//! entry, an answer owed, a request on its thread's cap.
//!
//! # Invariants
//!
//! - Only requests are filed. A reply goes on to the mailbox of the
//!   request it answers, which came by mesh mail: what the desk files is
//!   answered through the app's inbox.
//! - A reach only adds to its machine's directory: agents without one
//!   list and mail their project's agents as before.
//!
//! # Failure modes
//!
//! A request the desk refuses (an agent it doesn't know, a full thread)
//! is acked as undelivered. Nodes don't act on acks, so the sending agent
//! isn't told; the desk logs it.

use std::sync::Arc;

use async_trait::async_trait;
use aura_core_protocol::cloud::{MeshFrame, MeshMail, MeshMailKind, MeshPeer};
use aura_core_types::AgentId;
use aura_engine::RemoteMail;

use crate::mesh::HUB_NODE;

use super::CloudHub;

/// An agent a placed agent reaches through the hub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    /// The agent reached.
    pub agent_id: AgentId,
    /// What the placed agent's machine calls it.
    pub name: String,
}

/// Where the requests placed agents send the agents they reach go.
#[async_trait]
pub trait MailDesk: Send + Sync {
    /// File `mail`, a request from a placed agent to an agent it reaches.
    ///
    /// # Errors
    ///
    /// Why it wasn't filed: the sender or the recipient is unknown here,
    /// or their thread carries all the requests it may.
    async fn file(&self, mail: RemoteMail) -> Result<(), String>;
}

impl CloudHub {
    /// File the requests placed agents send the agents they reach at
    /// `desk`.
    pub fn attach_desk(&self, desk: Arc<dyn MailDesk>) {
        *self.desk.lock() = Some(desk);
    }

    /// Have placed `agent` reach `peers` through the hub from now on. Its
    /// machine gets its new directory when they changed.
    pub fn set_reach(&self, agent: AgentId, peers: Vec<Reach>) {
        let hex = agent.to_hex();
        {
            let mut reach = self.reach.lock();
            if reach.get(&hex) == Some(&peers) {
                return;
            }
            reach.insert(hex.clone(), peers);
        }
        if let Some(holder) = self.directory.holder(&hex) {
            self.send_directories(&holder.project);
        }
    }

    /// The directory frame for `node`: the agents of `project` it may
    /// mail ([`crate::mesh::Directory::peers_for`]), with the agents its
    /// own agents reach listed as held by the hub.
    pub(super) fn directory_frame(&self, node: &str, project: &str) -> MeshFrame {
        let mut peers = self.directory.peers_for(node, project);
        let held = self.directory.held_by(node);
        let reach = self.reach.lock();
        for reached in held.iter().filter_map(|hex| reach.get(hex)).flatten() {
            let id = reached.agent_id.to_hex();
            if held.contains(&id) {
                continue;
            }
            peers.retain(|peer| peer.agent_id != id);
            peers.push(MeshPeer {
                agent_id: id,
                name: reached.name.clone(),
                node_id: HUB_NODE.into(),
                url: None,
            });
        }
        MeshFrame::Directory { peers }
    }

    /// The desk to file `mail` at: when it is a request a placed agent
    /// sent an agent it reaches.
    pub(super) fn desk_for(&self, mail: &MeshMail) -> Option<Arc<dyn MailDesk>> {
        if mail.kind != MeshMailKind::Request {
            return None;
        }
        let reached = self
            .reach
            .lock()
            .get(&mail.from)
            .is_some_and(|peers| peers.iter().any(|peer| peer.agent_id.to_hex() == mail.to));
        if !reached {
            return None;
        }
        self.desk.lock().clone()
    }
}
