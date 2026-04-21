//! Policy engine for authorizing proposals and tool usage.
//!
//! ## Permission Levels
//!
//! Tools have different permission levels:
//! - `AlwaysAllow`: Safe read-only operations
//! - `AskOnce`: Requires approval once per session
//! - `AlwaysAsk`: Requires approval for each use
//! - `Deny`: Never allowed
//!
//! The module is split into:
//! - [`config`] — shape types (`PermissionLevel`, `PolicyConfig`) and
//!   the `default_tool_permission` preset.
//! - [`check`] — the [`Policy`] engine itself plus its authorization
//!   pipeline (`check`, `check_with_runtime_capabilities`,
//!   agent-permission + runtime-capability checks).
//!
//! The public API is re-exported from both submodules so downstream
//! crates still import via `aura_kernel::policy::{Policy, PolicyConfig,
//! PermissionLevel, PolicyResult, default_tool_permission}`.

mod check;
mod config;

pub use check::{Policy, PolicyResult};
pub use config::{default_tool_permission, PermissionLevel, PolicyConfig};

#[cfg(test)]
mod tests;
