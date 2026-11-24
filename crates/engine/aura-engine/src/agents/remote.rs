//! Agents on other nodes.
//!
//! A host only runs the agents it holds. When the hub runs some agents
//! on cloud machines, mail between agents can cross nodes: a
//! [`RemoteRouter`] says which agents another node holds and carries
//! mail to them; mail arriving from another node comes in through
//! [`super::AgentHost::deliver_remote`].
//!
//! ## Invariants
//!
//! - A message to an agent no node holds still fails with
//!   "unknown agent", as before.
//! - Replies keep their kind: a [`MessageKind::Reply`] crossing nodes is
//!   never answered automatically, so two agents on different nodes
//!   can't ping-pong forever either.
//! - Threads keep their id across nodes; ids minted here get this
//!   node's prefix when they leave (see the router implementation).

use aura_core_types::AgentId;

use super::inbox::MessageKind;
use super::mail::ThreadId;

/// One message between an agent here and an agent on another node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteMail {
    /// Sender.
    pub from: AgentId,
    /// Sender's display name.
    pub from_name: String,
    /// Recipient.
    pub to: AgentId,
    /// Request or reply.
    pub kind: MessageKind,
    /// Thread it belongs to.
    pub thread: ThreadId,
    /// Thread subject.
    pub subject: String,
    /// Body.
    pub text: String,
}

/// Carries mail to agents held by other nodes. Implementations must not
/// block: queue the message and send it from a task of their own.
pub trait RemoteRouter: Send + Sync {
    /// The display name of `agent` when another node holds it.
    fn name_of(&self, agent: AgentId) -> Option<String>;

    /// Send `mail` toward the node holding `mail.to`.
    ///
    /// # Errors
    ///
    /// When no node holds the recipient or the mail can't be queued.
    fn route(&self, mail: RemoteMail) -> Result<(), String>;

    /// Agents on other nodes that agents under `root` may reach, in the
    /// shape `list_agents` returns.
    fn peers(&self, root: AgentId) -> Vec<serde_json::Value> {
        let _ = root;
        Vec::new()
    }
}
