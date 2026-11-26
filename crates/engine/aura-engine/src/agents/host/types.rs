//! Public configuration and view types of the [`AgentHost`](super::AgentHost).

use std::path::PathBuf;
use std::sync::Arc;

use aura_agent_kernel::SpawnHook;
use aura_core_types::{
    AgentId, AgentMode, AgentPermissions, AgentToolPermissions, UserToolDefaults,
};
use aura_exec_tools::{AgentControlHook, AgentReadHook};

use crate::agents::factory::ChildKernelFactory;
use crate::agents::inbox::ParentInbox;

/// Limits for an [`AgentHost`](super::AgentHost).
#[derive(Debug, Clone)]
pub struct AgentHostConfig {
    /// Longest allowed chain of spawns below a root agent.
    pub max_depth: u32,
    /// Most child agents that may be running a turn at once. Spawns past
    /// this are rejected; messages to existing agents just queue.
    pub max_running: usize,
}

impl Default for AgentHostConfig {
    fn default() -> Self {
        Self {
            max_depth: 8,
            max_running: 32,
        }
    }
}

/// What every agent in one tree shares. Set when the root registers and
/// inherited by each child at spawn time.
#[derive(Clone)]
pub struct AgentEnv {
    /// Builds each child's tools.
    pub factory: Arc<dyn ChildKernelFactory>,
    /// Workspace root for child runs as `(workspace_base,
    /// use_workspace_base_as_root)`. `None` gives each child the
    /// scheduler's per-agent directory.
    pub workspace: Option<(PathBuf, bool)>,
    /// User-level tool defaults for the tree.
    pub user_tool_defaults: UserToolDefaults,
    /// End user at the root of the tree.
    pub originating_user_id: Option<String>,
    /// Replaces the local `spawn_agent` behaviour, e.g. to create agents
    /// in aura-os instead.
    pub spawn_override: Option<Arc<dyn SpawnHook>>,
    /// Handles messaging and lifecycle calls for agents outside the host.
    pub remote_control: Option<Arc<dyn AgentControlHook>>,
    /// Handles reads for agents outside the host.
    pub remote_read: Option<Arc<dyn AgentReadHook>>,
}

impl AgentEnv {
    /// An environment with no workspace override and no remote hooks.
    #[must_use]
    pub fn new(factory: Arc<dyn ChildKernelFactory>) -> Self {
        Self {
            factory,
            workspace: None,
            user_tool_defaults: UserToolDefaults::full_access(),
            originating_user_id: None,
            spawn_override: None,
            remote_control: None,
            remote_read: None,
        }
    }
}

/// A top-level agent whose turns a surface (chat run, terminal) drives.
pub struct RootAgent {
    pub agent_id: AgentId,
    pub name: String,
    pub permissions: AgentPermissions,
    pub tool_permissions: Option<AgentToolPermissions>,
    pub mode: AgentMode,
    /// Model children inherit when nothing else picks one.
    pub model: String,
    pub env: AgentEnv,
    /// Where replies to the root go. `None` appends them to its record.
    pub inbox: Option<Arc<dyn ParentInbox>>,
}

/// Lifecycle state of an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// Driven by a surface, not the host.
    Root,
    /// Waiting for a message.
    Idle,
    /// Running a turn.
    Running,
    /// Holding messages until resumed.
    Paused,
}

impl AgentState {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Idle => "idle",
            Self::Running => "running",
            Self::Paused => "paused",
        }
    }
}

/// Read-only view of one agent.
#[derive(Debug, Clone)]
pub struct AgentInfo {
    pub agent_id: AgentId,
    pub name: String,
    pub role: String,
    pub parent: Option<AgentId>,
    /// Number of ancestors.
    pub depth: usize,
    pub state: AgentState,
    pub model: String,
    /// Finished turns.
    pub turns: u32,
    /// Messages waiting to be handled.
    pub queued: usize,
    pub last_reply: Option<String>,
}

impl AgentInfo {
    #[must_use]
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "agent_id": self.agent_id.to_hex(),
            "name": self.name,
            "role": self.role,
            "parent_agent_id": self.parent.map(|p| p.to_hex()),
            "depth": self.depth,
            "state": self.state.as_str(),
            "model": self.model,
            "turns": self.turns,
            "queued": self.queued,
            "last_reply": self.last_reply,
        })
    }
}

/// Lifecycle actions accepted by [`AgentHost::lifecycle`](super::AgentHost::lifecycle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LifecycleAction {
    Pause,
    Resume,
    Stop,
    /// Clear the conversation and wake the agent up again.
    Restart,
}

impl LifecycleAction {
    /// Parse the `agent_lifecycle` tool's action string.
    #[must_use]
    pub fn parse(action: &str) -> Option<Self> {
        match action {
            "pause" => Some(Self::Pause),
            "resume" => Some(Self::Resume),
            "stop" => Some(Self::Stop),
            "restart" => Some(Self::Restart),
            _ => None,
        }
    }
}
