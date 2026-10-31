//! Recursive agents.
//!
//! An agent that holds `SpawnAgent` can start child agents, and children
//! are full agents: they get the same tools and the same host, so they can
//! start children of their own, down to [`AgentHostConfig::max_depth`].
//! Two tools sit on top of the one mechanism:
//!
//! - `task` starts a child of a bundled kind (`explore`,
//!   `general_purpose`, ...) to get a job done and returns its result. In
//!   detached mode it returns at once and the result arrives later as a
//!   message.
//! - `spawn_agent` starts a long-lived child with a name and role. It
//!   handles messages until `agent_lifecycle` stops it.
//!
//! Agents talk with `send_to_agent` / `delegate_task`. A hosted agent's
//! final answer to a request goes back to whoever sent it; replies are
//! never answered automatically, so two agents cannot ping-pong forever.
//! A child's own results flow up to its parent, and roots (driven by a
//! chat run or the terminal) receive them through a [`ParentInbox`].
//!
//! Children may only narrow what their parent has: mode, permissions and
//! tools are derived through `aura-agent-subagent`.

mod factory;
mod hooks;
mod host;
mod inbox;
mod setup;

#[cfg(test)]
mod tests;

pub use factory::{ChildKernel, ChildKernelFactory, ChildKernelRequest};
pub use hooks::AgentHooks;
pub use host::{
    AgentEnv, AgentHost, AgentHostConfig, AgentInfo, AgentState, LifecycleAction, RootAgent,
};
pub use inbox::{InboxMessage, MessageKind, ParentInbox};
