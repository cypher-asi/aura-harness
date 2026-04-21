//! Policy engine for authorizing proposals and tool usage.
//!
//! ## Permission Levels
//!
//! Tools have different permission levels:
//! - `AlwaysAllow`: Safe read-only operations
//! - `AskOnce`: Requires approval once per session
//! - `RequireApproval`: Denied unless an explicit single-use approval
//!   was registered for the exact `(agent_id, tool, args_hash)` triple
//!   via [`crate::Kernel::grant_approval`]. Renamed from `AlwaysAsk` in
//!   Phase 6 (security audit) because the old name implied an
//!   interactive prompt the kernel never surfaced.
//! - `Deny`: Never allowed
//!
//! The module is split into:
//! - [`config`] — shape types (`PermissionLevel`, `PolicyConfig`) and
//!   the `default_tool_permission` preset.
//! - [`check`] — the [`Policy`] engine itself plus its authorization
//!   pipeline (`check`, `check_with_runtime_capabilities`,
//!   agent-permission + runtime-capability checks).
//! - [`approvals`] — [`ApprovalRegistry`] / [`ApprovalKey`] backing the
//!   pre-approval path consulted when `PolicyVerdict::RequireApproval`
//!   is raised.
//!
//! The public API is re-exported from both submodules so downstream
//! crates still import via `aura_kernel::policy::{Policy, PolicyConfig,
//! PermissionLevel, PolicyResult, default_tool_permission}`.

mod approvals;
mod check;
mod config;

pub use approvals::{ApprovalKey, ApprovalRegistry};
pub use check::{Policy, PolicyResult, PolicyVerdict};
pub use config::{default_tool_permission, PermissionLevel, PolicyConfig};

#[cfg(test)]
mod tests;
