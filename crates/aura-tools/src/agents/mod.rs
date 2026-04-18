//! Cross-agent tools.
//!
//! All five tools (`spawn_agent`, `send_to_agent`, `agent_lifecycle`,
//! `get_agent_state`, `delegate_task`) are always compiled. Their
//! registration in `ToolCatalog` is unconditional; what gates them at the
//! surface is [`crate::ToolCatalog::visible_tools_with_permissions`] — a
//! caller that lacks the matching `Capability` never sees the tool names
//! in its prompt.
//!
//! The kernel additionally enforces the capability at proposal time via
//! [`aura_kernel::PolicyConfig::tool_capability_requirements`]. The
//! policy gate is always on; there is no feature flag or opt-out.
//!
//! Production side-effects for `send_to_agent` / `agent_lifecycle` /
//! `delegate_task` flow through [`crate::AgentControlHook`]; production
//! reads for `get_agent_state` flow through [`crate::AgentReadHook`]. When
//! the hooks are absent the tools still run the permission gate and emit a
//! descriptive outcome payload so the runtime effect can be filled in later
//! without changing the gate contract.

pub mod agent_lifecycle;
pub mod delegate_task;
pub mod get_agent_state;
pub mod send_to_agent;
pub mod spawn_agent;

pub use agent_lifecycle::{AgentLifecycleInput, AgentLifecycleTool};
pub use delegate_task::{DelegateTaskInput, DelegateTaskTool};
pub use get_agent_state::{GetAgentStateInput, GetAgentStateTool};
pub use send_to_agent::{SendToAgentInput, SendToAgentTool};
pub use spawn_agent::{SpawnAgentInput, SpawnAgentOutcome, SpawnAgentTool};

use crate::tool::Tool;
use aura_core::{Capability, ToolDefinition};

/// Static catalog metadata for the five cross-agent tools. Consumed by
/// [`crate::ToolCatalog::new`] to register every cross-agent tool
/// unconditionally with its declared `required_capabilities`.
#[must_use]
pub fn cross_agent_catalog_entries() -> Vec<(Box<dyn Tool>, ToolDefinition, Vec<Capability>)> {
    vec![
        (
            Box::new(SpawnAgentTool) as Box<dyn Tool>,
            SpawnAgentTool::definition(),
            vec![Capability::SpawnAgent],
        ),
        (
            Box::new(SendToAgentTool),
            SendToAgentTool::definition(),
            vec![Capability::ControlAgent],
        ),
        (
            Box::new(AgentLifecycleTool),
            AgentLifecycleTool::definition(),
            vec![Capability::ControlAgent],
        ),
        (
            Box::new(GetAgentStateTool),
            GetAgentStateTool::definition(),
            vec![Capability::ReadAgent],
        ),
        (
            Box::new(DelegateTaskTool),
            DelegateTaskTool::definition(),
            vec![Capability::ControlAgent],
        ),
    ]
}
