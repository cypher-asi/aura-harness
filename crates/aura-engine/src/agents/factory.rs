//! Seam that lets the surface that owns a root agent decide which tools
//! that agent's children get.
//!
//! The tool surface (catalog, domain executor, installed tools, automaton
//! tools) is built by the gateway or terminal, which sit above this crate.
//! They implement [`ChildKernelFactory`] once per root; the host calls it
//! for every child in that root's tree and then attaches its own hooks, so
//! nesting works to any depth without an upward dependency.

use aura_core_types::{AgentId, AgentPermissions, AgentToolPermissions, UserToolDefaults};
use aura_model_reasoner::ToolDefinition;
use aura_tools::ToolResolver;

/// Per-child inputs for [`ChildKernelFactory::build_child`].
#[derive(Debug, Clone)]
pub struct ChildKernelRequest {
    /// Id of the child being built.
    pub child_agent_id: AgentId,
    /// Capabilities and scope the child runs under.
    pub permissions: AgentPermissions,
    /// Per-tool overrides for the child.
    pub tool_permissions: AgentToolPermissions,
    /// User-level tool defaults for the child's tree.
    pub user_tool_defaults: UserToolDefaults,
    /// Child's ancestors, nearest first.
    pub parent_chain: Vec<AgentId>,
    /// End user at the root of the tree.
    pub originating_user_id: Option<String>,
    /// Child's model id.
    pub model_id: String,
}

/// Tool surface for one child: the resolver that executes its tool calls
/// and the definitions its model sees.
pub struct ChildKernel {
    /// Resolver without agent hooks; the host attaches those.
    pub resolver: ToolResolver,
    /// Tool definitions offered to the child's model.
    pub tools: Vec<ToolDefinition>,
}

/// Builds the tool surface for children in one root agent's tree.
pub trait ChildKernelFactory: Send + Sync {
    /// Build the resolver and tool list for `request`.
    fn build_child(&self, request: &ChildKernelRequest) -> ChildKernel;
}
