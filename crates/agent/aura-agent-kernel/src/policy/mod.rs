//! Policy engine for authorizing proposals and tool usage.
//!
//! ## Tool States
//!
//! Per-tool enablement resolves through
//! [`aura_core_types::resolve_effective_permission`] into
//! [`aura_core_types::ToolState`] (`on` / `off` / `ask`). Capability, scope,
//! integration, and execution guardrails remain separate policy layers.
//!
//! The module is split into:
//! - [`config`] — shape types (`PolicyConfig`).
//! - [`check`] — the [`Policy`] engine itself plus its authorization
//!   pipeline (`check`, `check_with_runtime_capabilities`,
//!   agent-permission + runtime-capability checks).
//!
//! The public API is re-exported from both submodules so downstream
//! crates still import via `aura_agent_kernel::policy::{Policy, PolicyConfig,
//! PolicyResult}`.

mod check;
mod config;

pub(crate) use check::tool_state_label;
pub use check::{
    gated_action, tool_mode_class, Policy, PolicyEvaluation, PolicyResult, PolicyVerdict,
    ToolModeClass,
};
pub use config::PolicyConfig;
