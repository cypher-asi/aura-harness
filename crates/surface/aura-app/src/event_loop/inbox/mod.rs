//! Agents talking to each other through their inboxes.
//!
//! A top-level agent reaches its peers with `send_to_agent`, `list_agents`
//! and `get_agent_state` ([`PeerHooks`]), and a channel's coordinator
//! hands them work with `assign` ([`AssignTool`]). An agent's peers:
//!
//! - a coordinator's are the other top-level members of its channel;
//! - a channel member's are the other top-level members of its channel,
//!   the coordinator among them, and the top-level agents of its project;
//! - any other top-level agent's are the top-level agents of its project.
//!
//! Subagents have no peers: like any target that isn't a peer, theirs
//! are reached through the background agents' hooks when those run.
//!
//! A message reaches its agent like a harness notice: into its running
//! turn, else as the prompt of a turn that carries on with its latest
//! task. An assignment is a task of its own instead, and waits while the
//! agent is busy. A message's first line says who sent it and what about
//! and its second names it ([`aura_ui::inbox`]), so chats fold
//! it to one line. Every message also goes to the INBOX tab.
//!
//! A request or an assignment is answered on its own: when the turn that
//! saw it ends, the agent's last words go back as a reply, which closes
//! the todo it was for. Coordinators answer nobody on their own, as what
//! they write goes to their channel, and replies are never answered. One
//! thread between two agents carries at most [`THREAD_REQUESTS`]
//! requests, so two agents can't keep each other busy forever.
//!
//! - `peers`: who an agent is and the peers it can reach
//! - `messages`: a message sent, and given to its agent
//! - `answers`: what an agent was asked, answered when its turn ends
//! - `hooks`: `send_to_agent`, `list_agents` and `get_agent_state`
//! - `assignments`: `assign`
//!
//! # Invariants
//!
//! * The channel store is read off the async runtime.
//! * Locks are never held across an `.await`.
//!
//! # Failure modes
//!
//! A channel that cannot be read leaves its agent without channel peers
//! (logged): messages to them fail as to an unknown agent. A message sent
//! as the terminal shuts down is lost.

use super::agent_events::AgentUi;
use super::channels;
use super::handlers::system_message;
use super::mail::{next_mail_id, subject_of};
use super::notices::Notice;
use super::steering::SteerSource;
use super::subagents::{assign_tool_definition, lock, AgentHub};
use super::turns::{Prompt, Request, Work};
use super::{submit, LoopState};
use crate::agent_definitions::COORDINATOR_DEFINITION;
use crate::agent_registry::{AgentRegistry, RegisteredAgent};
use async_trait::async_trait;
use aura_core_types::time::now_ms;
use aura_core_types::{AgentId, ToolDefinition, ToolResult};
use aura_engine::AgentHooks;
use aura_exec_tools::todo::{task_report_line, ASSIGN_TOOL_NAME};
use aura_exec_tools::{AgentControlHook, AgentReadHook, Tool, ToolContext, ToolError};
use aura_model::{Message, Role};
use aura_store_db::{ChannelRecord, MemberRecord, MemberRole};
use aura_ui::events::MailView;
use aura_ui::inbox::{self, InboxKind};
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use tracing::{info, warn};

mod answers;
mod assignments;
mod hooks;
mod messages;
mod peers;
#[cfg(test)]
mod tests;

pub(super) use answers::{answer, give_up};
pub use assignments::AssignTool;
#[cfg(test)]
pub(crate) use assignments::{assign, AssignArgs};
pub use hooks::PeerHooks;
pub(super) use messages::deliver;
use messages::{mail_view, new_id, send, shown};
use peers::{circle, Circle, Named, Peer, Relation};

/// Requests one thread between two agents carries; then they have to
/// settle it some other way.
pub(crate) const THREAD_REQUESTS: u32 = 8;

/// What the inboxes keep on the hub.
#[derive(Default)]
pub(crate) struct Inbox {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// The requests each agent owes an answer to, oldest first
    owed: HashMap<AgentId, Vec<Owed>>,
    /// Requests each thread has carried
    requests: HashMap<String, u32>,
    /// The subject each two agents last wrote about, whichever wrote
    /// ([`messages::pair`])
    subjects: HashMap<(AgentId, AgentId), String>,
    /// Each agent's last reply: whether it finished, and what it said
    reports: HashMap<AgentId, (bool, String)>,
}

/// A request or assignment an agent owes an answer to.
#[derive(Debug, Clone)]
struct Owed {
    id: String,
    kind: InboxKind,
    from: AgentId,
    thread: String,
    subject: String,
    /// The sender's todo it is for
    todo: Option<String>,
}

#[cfg(test)]
impl Inbox {
    /// How many answers `agent` owes.
    pub(crate) fn owed_by(&self, agent: AgentId) -> usize {
        lock(&self.state).owed.get(&agent).map_or(0, Vec::len)
    }
}

/// The hub, for the tools of kernels built before it exists; set once it
/// does ([`Self::set`]).
#[derive(Clone, Default)]
pub struct HubLink(Arc<OnceLock<Weak<AgentHub>>>);

impl HubLink {
    /// Point the link at `hub`. Only the first call counts.
    pub fn set(&self, hub: &Arc<AgentHub>) {
        let _ = self.0.set(Arc::downgrade(hub));
    }

    /// The hub, unless it isn't set yet or is gone.
    pub(crate) fn hub(&self) -> Option<Arc<AgentHub>> {
        self.0.get()?.upgrade()
    }
}
