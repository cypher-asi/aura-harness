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
//! - An agent spawned on a placed agent's machine reaches whom that
//!   placed agent reaches: its requests are filed as its root's, naming
//!   it in the text, so they never bypass the desk for the recipient's
//!   machine, where nothing here would see them.
//!
//! # Failure modes
//!
//! A request that isn't delivered (the desk refuses it, an agent it
//! doesn't know, a full thread, another user's agent) is acked as
//! undelivered, and the desk is told ([`MailDesk::refused`]) so the
//! sender hears of it: nodes don't act on acks.

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

    /// `mail`, a request from a placed agent (or, for one spawned on its
    /// machine, filed as that placed agent's), was not delivered, `why`:
    /// tell its sender. Nothing by default.
    fn refused(&self, mail: &RemoteMail, why: &str) {
        let _ = (mail, why);
    }
}

/// Where a request a placed agent's machine sent is filed: the desk, and
/// the placed agent it counts as from when an agent spawned on that
/// machine sent it.
pub(super) struct Filing {
    desk: Arc<dyn MailDesk>,
    root: Option<AgentId>,
}

impl Filing {
    /// File `mail` at the desk, as from its root when it has one.
    pub(super) async fn file(&self, mail: RemoteMail) -> Result<(), String> {
        let mail = match self.root {
            Some(root) => on_behalf_of(mail, root),
            None => mail,
        };
        self.desk.file(mail).await
    }
}

/// `mail`, sent by an agent spawned on `root`'s machine, as from `root`,
/// its text saying who wrote it.
fn on_behalf_of(mut mail: RemoteMail, root: AgentId) -> RemoteMail {
    if mail.from != root {
        mail.text = format!(
            "Sent by {}, an agent spawned on its machine:\n\n{}",
            mail.from_name, mail.text
        );
        mail.from = root;
    }
    mail
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

    /// Where to file `mail`: when it is a request a placed agent, or an
    /// agent spawned on its machine, sent an agent the placed agent
    /// reaches.
    pub(super) fn desk_for(&self, mail: &MeshMail) -> Option<Filing> {
        if mail.kind != MeshMailKind::Request {
            return None;
        }
        let (sender, root) = self.sender_of(mail);
        let reached = self
            .reach
            .lock()
            .get(&sender)
            .is_some_and(|peers| peers.iter().any(|peer| peer.agent_id.to_hex() == mail.to));
        if !reached {
            return None;
        }
        let desk = self.desk.lock().clone()?;
        Some(Filing { desk, root })
    }

    /// The placed agent (hex) `mail` counts as from: its sender when the
    /// directory holds it, else the one placed agent of the machine that
    /// sent it, which spawned the sender there (also returned as the
    /// root to file it as).
    fn sender_of(&self, mail: &MeshMail) -> (String, Option<AgentId>) {
        if self.directory.holder(&mail.from).is_some() {
            return (mail.from.clone(), None);
        }
        match self.directory.held_by(&mail.from_node).as_slice() {
            [root] => (root.clone(), AgentId::from_hex(root).ok()),
            _ => (mail.from.clone(), None),
        }
    }

    /// Tell the desk that `mail` was not delivered, `why`, when it is a
    /// request, so its sender (or the placed agent that spawned it)
    /// hears of it: its machine drops the ack.
    pub(super) fn tell_refused(&self, mail: &MeshMail, why: &str) {
        if mail.kind != MeshMailKind::Request {
            return;
        }
        let (Some(desk), Ok(remote)) = (self.desk.lock().clone(), super::links::remote_mail(mail))
        else {
            return;
        };
        let remote = match self.sender_of(mail).1 {
            Some(root) => on_behalf_of(remote, root),
            None => remote,
        };
        tracing::info!(from = %mail.from, to = %mail.to, %why, "a cloud agent's request was not delivered; telling it");
        desk.refused(&remote, why);
    }
}
