//! [`ChildRunner`] trait — pluggable child-execution surface.
//!
//! Decouples [`crate::FleetSpawner`] from the runtime-side scheduler
//! / identity registry / agent-loop wiring that today lives in
//! `aura-runtime`. Fleet-spawn invokes the runner once the lease,
//! quota, audit write, and registry slot are in place.
//!
//! The trait deliberately speaks only in [`aura_core`],
//! [`aura_core_permissions`], and [`aura_agent_subagent`] types so
//! the fleet layer stays free of upward dependencies on agent /
//! runtime crates.

use async_trait::async_trait;
use aura_agent_subagent::SubagentSpec;
use aura_core::{
    AgentId, AgentPermissions, AgentToolPermissions, SubagentResult, UserToolDefaults,
};
use thiserror::Error;

/// Errors a [`ChildRunner`] may surface.
#[derive(Debug, Error)]
pub enum ChildRunError {
    /// Runner-internal failure (scheduler error, identity
    /// registration failure, kernel error). String-typed so the
    /// concrete runtime can map any error variant in.
    #[error("child runner error: {0}")]
    Internal(String),
}

/// Phase 7a-only carrier for the per-call SubagentDispatch fields
/// the legacy runtime path needs but the layered
/// [`SubagentSpec`] does not yet model.
///
/// **Scope**: this struct is a compatibility shim that keeps the
/// task-tool [`SubagentResult`] byte-identical across the Phase 7a
/// refactor. Phase 7b extends
/// [`aura_agent_subagent::SubagentOverrides`] to absorb
/// `subagent_type`, `system_prompt_addendum`, the explicit
/// `parent_tool_permissions`, and `user_tool_defaults` as first-
/// class override fields; once that lands this carrier is removed.
#[derive(Debug, Clone)]
pub struct TaskCompatContext {
    /// Bundled subagent-kind identifier (e.g. `"explore"`,
    /// `"general_purpose"`); the runner uses this to look up the
    /// kind's allowed-capabilities / tools / system prompt.
    pub subagent_type: String,
    /// Free-form addendum appended to the kind's system prompt.
    pub system_prompt_addendum: Option<String>,
    /// Untouched parent permissions; the runner intersects these
    /// with the kind's `allowed_capabilities` to produce the child
    /// `Permissions`. `SubagentSpec::permissions` does NOT yet do
    /// the kind-driven narrowing.
    pub parent_permissions: AgentPermissions,
    /// Parent's per-tool override map (used by the legacy
    /// per-tool policy resolver).
    pub parent_tool_permissions: Option<AgentToolPermissions>,
    /// User-level default tool policy applied to children.
    pub user_tool_defaults: UserToolDefaults,
    /// Optional model override the parent specified explicitly.
    pub model_override: Option<String>,
    /// Parent agent id, propagated so the runner can register the
    /// child's identity in the scheduler.
    pub parent_agent_id: AgentId,
    /// Parent's `parent_chain` snapshot. Used to forward audit
    /// attribution into the child's identity record.
    pub parent_chain: Vec<AgentId>,
}

/// Bundle of data the [`ChildRunner`] receives per call. Wrapping
/// the args in a struct keeps the trait signature stable as Phase
/// 7b grows the carrier surface (and lets the runtime adapter ship
/// the optional [`TaskCompatContext`] without rewriting the trait).
#[derive(Debug)]
pub struct ChildRunContext {
    /// Derived spec from `aura-agent-subagent`.
    pub spec: SubagentSpec,
    /// Initial prompt for the child.
    pub prompt: String,
    /// Originating user id for audit attribution.
    pub originating_user_id: Option<String>,
    /// Phase 7a compatibility carrier. Phase 7b retires.
    pub task_compat: Option<TaskCompatContext>,
}

/// Run a derived [`SubagentSpec`] to completion and return a
/// [`SubagentResult`] for the parent's tool call to consume.
///
/// Implementations are expected to:
///
/// - Look up the bundled subagent kind (or other registry) the
///   spec references via `ctx.spec.kind`.
/// - Register the child's identity with the scheduler.
/// - Enqueue the child's initial prompt as a transaction.
/// - Run the child agent loop to completion (with the spec's
///   timeout).
/// - Translate the agent-loop result into a [`SubagentResult`] with
///   the exact same field semantics as today's
///   `RuntimeSubagentDispatch::dispatch` so the task tool's
///   surface remains byte-for-byte stable.
#[async_trait]
pub trait ChildRunner: Send + Sync {
    /// Run the child loop and return its terminal
    /// [`SubagentResult`].
    ///
    /// # Errors
    ///
    /// Returns [`ChildRunError`] if the runner could not start, the
    /// scheduler errored, or the loop produced no result. A child
    /// timeout / failure is returned INSIDE a successful
    /// [`SubagentResult`] (`exit: SubagentExit::Timeout` /
    /// `Failed`) — only infrastructure failures bubble up as an
    /// error.
    async fn run(&self, ctx: ChildRunContext) -> Result<SubagentResult, ChildRunError>;
}
