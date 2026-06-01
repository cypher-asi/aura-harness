//! Cross-crate seam that lets a child (subagent) run reuse the
//! session-scoped executor router instead of the scheduler's bare
//! node-level resolver.
//!
//! ## Why a trait lives here
//!
//! The session's subagent-dispatch hook and spawn hooks
//! (`AuraServerSpawnHook` / `AuraServerAgentHook` / the streaming
//! observability wrapper) live in `aura-runtime` (the upper gateway
//! layer). Child runs, however, are launched from this crate's
//! [`crate::child_runner::RuntimeChildRunner`] driving
//! [`crate::scheduler::Scheduler`] (a lower layer). Wiring the rich
//! session resolver onto a child run from down here would require an
//! upward dependency on `aura-runtime`, and the resolver itself needs a
//! [`aura_fleet_spawn::ChildRunner`] for the child's own (grand)children
//! — a construction cycle.
//!
//! Defining the factory trait in this low layer and implementing it in
//! `aura-runtime` breaks the cycle: the engine holds an
//! `Arc<dyn ChildKernelFactory>` it can call without knowing how the
//! session resolver is assembled, and the runtime implementation
//! re-injects itself into each child runner it builds so nesting works
//! to arbitrary depth.

use aura_agent_kernel::ExecutorRouter;
use aura_core_types::{AgentId, AgentPermissions, AgentToolPermissions, UserToolDefaults};

/// Inputs the [`ChildKernelFactory`] needs to build a child run's
/// session-equivalent [`ExecutorRouter`]. Everything that is constant
/// for the session (catalog, tool config, domain executor, installed
/// tools, spawn-hook wiring, store/scheduler handles, workspace root)
/// is captured by the factory implementation itself; only the
/// per-child, run-specific surface is passed in here.
#[derive(Debug, Clone)]
pub struct ChildKernelRequest {
    /// Freshly-minted child agent id for this run.
    pub child_agent_id: AgentId,
    /// Narrowed capabilities/scope the child runs under (mirrors the
    /// kernel policy the scheduler is handed for the same run).
    pub permissions: AgentPermissions,
    /// Narrowed per-tool permission overrides for the child.
    pub tool_permissions: AgentToolPermissions,
    /// User-level tool defaults resolved for this child run.
    pub user_tool_defaults: UserToolDefaults,
    /// Child's ancestor chain (`[parent, grandparent, ...]`). Threaded
    /// onto the child resolver so the `task`/`spawn_agent` tools enforce
    /// the depth + ancestor-cycle guards on nested spawns.
    pub parent_chain: Vec<AgentId>,
    /// Originating end-user id propagated for delegate audit attribution.
    pub originating_user_id: Option<String>,
    /// Child's resolved model id (forwarded to cross-agent tools).
    pub model_id: String,
}

/// Builds a session-equivalent [`ExecutorRouter`] for a child subagent
/// run. Implemented in `aura-runtime` over the session's catalog +
/// hooks; consumed by [`crate::child_runner::RuntimeChildRunner`] via the
/// scheduler so child runs get the same subagent dispatch, spawn hooks,
/// permissions, and parent-chain as a real top-level turn.
pub trait ChildKernelFactory: Send + Sync {
    /// Assemble the child run's executor router.
    fn build_child_router(&self, request: ChildKernelRequest) -> ExecutorRouter;
}
